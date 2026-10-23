"""Hoisting + naming + printing of the decompiled AST."""
import sys, re
from .gen import *
from .post import Paren

EVENT_PARAMS = {
    'InputBegan': ['input', 'gameProcessed'], 'InputChanged': ['input', 'gameProcessed'], 'InputEnded': ['input', 'gameProcessed'],
    'Touched': ['hit'], 'TouchEnded': ['hit'], 'Heartbeat': ['deltaTime'], 'RenderStepped': ['deltaTime'], 'Stepped': ['time', 'deltaTime'],
    'PlayerAdded': ['player'], 'PlayerRemoving': ['player'], 'CharacterAdded': ['character'], 'ChildAdded': ['child'],
    'ChildRemoved': ['child'], 'DescendantAdded': ['descendant'], 'FocusLost': ['enterPressed', 'inputThatCausedFocusLoss'],
    'MouseMoved': ['x', 'y'], 'Changed': ['value'], 'OnClientEvent': [], 'Died': [],
}

PREC = {'or': 1, 'and': 2, '==': 3, '~=': 3, '<': 3, '>': 3, '<=': 3, '>=': 3, '..': 4, '+': 5, '-': 5, '*': 6, '/': 6, '%': 6, '//': 6, '^': 8}


def flat(e, op):
    out = []

    def rec(x):
        if isinstance(x, Bin) and x.op == op:
            rec(x.l)
            rec(x.r)
        else:
            out.append(x)
    rec(e)
    return out


def chain(op, ops):
    r = ops[-1]
    for o in reversed(ops[:-1]):
        r = Bin(op, o, r)
    return r


def group_or(ops):
    if len(ops) >= 2 and all(isinstance(o, Not) for o in ops):
        return [Not(chain('and', [o.e for o in ops]))]
    out, i = [], 0
    while i < len(ops):
        if isinstance(ops[i], Not):
            j = i
            while j < len(ops) and isinstance(ops[j], Not):
                j += 1
            if j - i >= 3:
                out.append(Not(chain('and', [o.e for o in ops[i:j]])))
                i = j
                continue
        out.append(ops[i])
        i += 1
    return out


def nots(e):
    if isinstance(e, Not):
        compound = isinstance(e.e, Bin) and e.e.op in ('and', 'or')
        return (3 if compound else 1) + nots(e.e)
    if isinstance(e, Bin) and e.op in ('and', 'or'):
        ops = flat(e, e.op)
        if e.op == 'or':
            ops = group_or(ops)
        return sum(nots(o) for o in ops)
    return 0


def negate_full(e):
    if isinstance(e, Not):
        return tru(e.e)
    if isinstance(e, Bin) and e.op in ('and', 'or'):
        return Bin('or' if e.op == 'and' else 'and', negate_full(e.l), negate_full(e.r))
    if isinstance(e, Bin) and e.op == '==':
        return Bin('~=', e.l, e.r)
    if isinstance(e, Bin) and e.op == '~=':
        return Bin('==', e.l, e.r)
    return Not(e)


def tru(e):
    """rewrite an expression that is only used for its truthiness (if/while conditions)"""
    if isinstance(e, Not):
        x = e.e
        if isinstance(x, Not):
            return tru(x.e)
        if isinstance(x, Bin) and x.op in ('and', 'or'):
            full = negate_full(x)
            base = Not(tru(x))
            return full if nots(full) < nots(base) else base
        if isinstance(x, Bin) and x.op == '==':
            return Bin('~=', x.l, x.r)
        if isinstance(x, Bin) and x.op == '~=':
            return Bin('==', x.l, x.r)
        return Not(tru(x))
    if isinstance(e, Bin) and e.op in ('and', 'or'):
        return Bin(e.op, tru(e.l), tru(e.r))
    return e


def wkey(w):
    fn = w.fn
    pid = fn.pid if fn is not None else -1
    if w.kind == 'param':
        idx = fn.params.index(w) + 1 if (fn is not None and w in fn.params) else 0
        return '%d:p%d' % (pid, idx)
    d = w.defs[0][0] if w.defs else 0
    return '%d:%s' % (pid, d)


class Scope:
    def __init__(self, parent):
        self.parent = parent
        self.used = set()
        self.counter = collections.Counter()

    def visible(self, name):
        s = self
        while s is not None:
            if name in s.used:
                return True
            s = s.parent
        return False


class Printer:
    def __init__(self, gen, reserved, namemap=None, annotate=False):
        self.gen = gen
        self.namemap = namemap or {}
        self.annotate = annotate
        self.used_keys = set()
        self.fn_text = {}
        self.dec = gen.dec
        self.reserved = set(reserved) | KEYWORDS | ROBLOX_GLOBALS
        self.loopnames = ['i', 'j', 'k', 'l', 'm', 'n', 'o', 'p']

    # ---------------------------------------------------------------- naming
    def name_of(self, w, scope):
        if w.name:
            return w.name
        key = wkey(w)
        mapped = self.namemap.get(key)
        if mapped:
            self.used_keys.add(key)
        base = mapped or w.hint
        if isinstance(base, tuple) and base[0] == 'clone':
            base = self.name_of(base[1], scope) + 'Clone'
        if w.kind == 'loop' and not mapped:
            depth = 0
            s = scope
            while s is not None:
                depth += s.counter['loop_active']
                s = s.parent
            base = self.loopnames[min(depth, len(self.loopnames) - 1)]
        if not base:
            if w.kind == 'param':
                idx = w.fn.params.index(w) + 1 if w in w.fn.params else 0
                base = 'p%d' % idx if idx else 'p'
                full = base
            else:
                root = scope
                while root.parent is not None:
                    root = root.parent
                root.counter['v'] += 1
                base = 'v%d' % root.counter['v']
        base = re.sub(r'[^A-Za-z0-9_]', '', base) or 'v'
        if base[0].isdigit():
            base = '_' + base
        cand = base
        i = 2
        while cand in self.reserved or scope.visible(cand):
            cand = '%s%d' % (base, i)
            i += 1
        scope.used.add(cand)
        w.name = cand
        return cand

    def dn(self, w, scope):
        nm = self.name_of(w, scope)
        if self.annotate:
            return nm + '<@' + wkey(w) + '>'
        return nm

    # ---------------------------------------------------------------- expressions
    def key_str(self, k):
        if isinstance(k, str) and IDENT.match(k) and k not in KEYWORDS:
            return '.' + k
        return '[' + lua_str(k.encode('latin1')) + ']'

    def atom(self, e, scope, ind):
        if isinstance(e, (Bin, Not, Len, Neg)):
            return '(' + self.ex(e, scope, ind, 0) + ')'
        s = self.ex(e, scope, ind, 100)
        if isinstance(e, (Const, Func, Tbl)) or (isinstance(e, Multi)):
            if isinstance(e, Const) and e.kind == 'z':
                return '(nil)'
            return '(' + s + ')'
        return s

    def ex(self, e, scope, ind, prec=0):
        if isinstance(e, Const):
            if e.kind == 's':
                return lua_str(e.v)
            if e.kind == 'n':
                s = fmt_num(e.v)
                return '(' + s + ')' if (s.startswith('-') and prec > 5) else s
            if e.kind == 'b':
                return 'true' if e.v else 'false'
            return 'nil'
        if isinstance(e, Imp):
            out = e.path[0]
            for p in e.path[1:]:
                out += self.key_str(p)
            return out
        if isinstance(e, Var):
            return self.name_of(e.web, scope)
        if isinstance(e, Idx):
            if isinstance(e.key, str):
                return self.atom(e.obj, scope, ind) + self.key_str(e.key)
            if isinstance(e.key, Const) and e.key.kind == 's':
                return self.atom(e.obj, scope, ind) + self.key_str(e.key.v.decode('latin1'))
            return self.atom(e.obj, scope, ind) + '[' + self.ex(e.key, scope, ind) + ']'
        if isinstance(e, Multi):
            return self.ex(e.e, scope, ind, prec)
        if isinstance(e, Paren):
            return '(' + self.ex(e.e, scope, ind) + ')'
        if isinstance(e, Call):
            args = []
            for i, a in enumerate(e.args):
                if isinstance(a, Func) and e.method in ('Connect', 'Once', 'connect') and i == len(e.args) - 1:
                    key = None
                    if isinstance(e.fn, Idx) and isinstance(e.fn.key, str):
                        key = e.fn.key
                    args.append(self.func(a, scope, ind, EVENT_PARAMS.get(key)))
                else:
                    args.append(self.ex(a, scope, ind))
            if e.method:
                if IDENT.match(e.method) and e.method not in KEYWORDS:
                    return self.atom(e.fn, scope, ind) + ':' + e.method + '(' + ', '.join(args) + ')'
                o = self.atom(e.fn, scope, ind)
                return o + '[' + lua_str(e.method.encode('latin1')) + '](' + ', '.join([o] + args) + ')'
            return self.atom(e.fn, scope, ind) + '(' + ', '.join(args) + ')'
        if isinstance(e, Bin) and e.op in ('<', '<=') and isinstance(e.l, Const) and e.l.kind == 'n' and not isinstance(e.r, Const):
            e = Bin('>' if e.op == '<' else '>=', e.r, e.l)
        if isinstance(e, Bin) and e.op == 'or':
            ops = flat(e, 'or')
            g = group_or(ops)
            if len(g) < len(ops):
                return self.ex(chain('or', g), scope, ind, prec)
        if isinstance(e, Bin):
            p = PREC[e.op]
            if e.op in ('^', '..'):
                s = self.ex(e.l, scope, ind, p + 1) + ' ' + e.op + ' ' + self.ex(e.r, scope, ind, p)
            elif e.op in ('and', 'or') and isinstance(e.r, Bin) and e.r.op == e.op:
                s = self.ex(e.l, scope, ind, p) + ' ' + e.op + ' ' + self.ex(e.r, scope, ind, p)
            else:
                s = self.ex(e.l, scope, ind, p) + ' ' + e.op + ' ' + self.ex(e.r, scope, ind, p + 1)
            return '(' + s + ')' if p < prec else s
        if isinstance(e, Len):
            return '#' + self.ex(e.e, scope, ind, 7)
        if isinstance(e, Not):
            return 'not ' + self.ex(e.e, scope, ind, 7)
        if isinstance(e, Neg):
            inner = self.ex(e.e, scope, ind, 7)
            if inner.startswith('-'):
                inner = '(' + inner + ')'
            return '-' + inner
        if isinstance(e, Tbl):
            parts = []
            ind2 = ind + '\t'
            for k, v in e.fields:
                if v is None:
                    continue
                if isinstance(k, str) and IDENT.match(k) and k not in KEYWORDS:
                    parts.append(k + ' = ' + self.ex(v, scope, ind2))
                else:
                    parts.append('[' + lua_str(k.encode('latin1')) + '] = ' + self.ex(v, scope, ind2))
            parts += [self.ex(x, scope, ind2) for x in e.items]
            if not parts:
                return '{}'
            one = '{' + ', '.join(parts) + '}'
            if len(one) > 110 and len(parts) > 1 and all('\n' not in x for x in parts):
                return '{\n' + ',\n'.join(ind + '\t' + x for x in parts) + '\n' + ind + '}'
            if len(one) > 110 and len(parts) > 1:
                return '{\n' + ',\n'.join(ind + '\t' + x for x in parts) + '\n' + ind + '}'
            return one
        if isinstance(e, Func):
            return self.func(e, scope, ind, None)
        raise Exception('cannot print ' + type(e).__name__)

    # ---------------------------------------------------------------- functions
    def make_child(self, f):
        child = Fn(self.dec, f.pid, list(f.caps))
        for w in child.upvals:
            pass
        self.gen.run(child)
        hoist(child)
        return child

    def func(self, f, scope, ind, hints, name_for=None):
        child = self.child_cache(f)
        return self.render_fn(child, scope, ind, hints)

    def render_fn(self, child, scope, ind, hints, prefix='function'):
        sc = Scope(scope)
        names = []
        for i, w in enumerate(child.params):
            if hints and i < len(hints) and not w.name:
                w.hint = hints[i]
            names.append(self.dn(w, sc))
        if child.P['va']:
            names.append('...')
        head = prefix + '(' + ', '.join(names) + ')'
        body = self.block(child.body, sc, ind + '\t')
        if not body:
            res = head + ' end'
        else:
            res = head + '\n' + '\n'.join(body) + '\n' + ind + 'end'
        if self.annotate:
            self.fn_text[child.pid] = res
        return res

    def forin_hint(self, exprs, i):
        e = exprs[0] if exprs else None
        if isinstance(e, Multi):
            e = e.e
        name = None
        if isinstance(e, Call) and isinstance(e.fn, Imp) and len(e.fn.path) == 1:
            name = e.fn.path[0]
        if isinstance(e, Imp) and len(e.path) == 1:
            name = e.path[0]
        if name == 'ipairs':
            return ['index', 'value'][i] if i < 2 else None
        if name in ('pairs', 'next'):
            return ['key', 'value'][i] if i < 2 else None
        return None

    # ---------------------------------------------------------------- statements
    def block(self, stmts, scope, ind):
        out = []
        i = 0
        while i < len(stmts):
            s = stmts[i]
            if s[0] == 'localn':
                m = self.multi_assign(stmts, i)
                if m is not None:
                    targets, cnt = m
                    out.append(ind + ', '.join(self.ex(t, scope, ind) for t in targets) + ' = ' + self.ex(s[2], scope, ind))
                    i += 1 + cnt
                    continue
            out.extend(self.stmt(s, scope, ind, stmts[i + 1:]))
            i += 1
        return out

    def multi_assign(self, stmts, i):
        # `local a, b = f()` immediately followed by `x = a`, `y = b` (each temp read only there) is the source `x, y = f()`
        ws = stmts[i][1]
        if len(ws) < 2 or any(w is None for w in ws):
            return None
        targets = []
        for k, w in enumerate(ws):
            j = i + 1 + k
            if j >= len(stmts):
                return None
            t = stmts[j]
            if t[0] != 'assign' or not isinstance(t[1], Var) or not isinstance(t[2], Var) or t[2].web is not w:
                return None
            if len(w.uses) != 1 or w.captured or t[1].web is w:
                return None
            targets.append(t[1])
        return targets, len(ws)

    def assign_hint(self, w, stmts):
        # look for a later assignment to this variable (also inside nested closures) to derive a name
        def scan(stmts):
            for s in stmts:
                k = s[0]
                if k == 'assign' and isinstance(s[1], Var) and s[1].web is w:
                    h = suggest(s[2])
                    if h and not isinstance(h, tuple):
                        return h
                exprs = []
                if k == 'local' and s[2] is not None:
                    exprs.append(s[2])
                elif k in ('assign',):
                    exprs.append(s[2])
                elif k in ('call', 'call_cond'):
                    exprs.append(s[1])
                elif k == 'if':
                    exprs.append(s[1])
                for e in exprs:
                    r = scan_expr(e)
                    if r:
                        return r
                if k == 'if':
                    r = scan(s[2]) or scan(s[3])
                    if r:
                        return r
                if k == 'for':
                    r = scan(s[5])
                    if r:
                        return r
                if k == 'forin':
                    r = scan(s[3])
                    if r:
                        return r
                if k == 'while':
                    r = scan(s[2])
                    if r:
                        return r
            return None

        def scan_expr(e):
            if isinstance(e, Func):
                if w in e.caps:
                    child = self.child_cache(e)
                    return scan(child.body)
                return None
            if isinstance(e, E):
                for v in vars(e).values():
                    if isinstance(v, E):
                        r = scan_expr(v)
                        if r:
                            return r
                    elif isinstance(v, list):
                        for x in v:
                            if isinstance(x, E):
                                r = scan_expr(x)
                                if r:
                                    return r
            return None
        return scan(stmts)

    def child_cache(self, f):
        c = getattr(f, 'child', None)
        if c is None:
            c = self.make_child(f)
            f.child = c
        return c

    def stmt(self, s, scope, ind, rest=()):
        k = s[0]
        if k == 'local':
            w, e = s[1], s[2]
            if w.hint is None and e is not None and not (isinstance(e, Const) and e.kind == 'z'):
                w.hint = suggest(e)
            if w.hint is None:
                h = self.assign_hint(w, rest)
                if h:
                    w.hint = h
            nm = self.dn(w, scope)
            if e is None or (isinstance(e, Const) and e.kind == 'z'):
                return [ind + 'local ' + nm]
            if isinstance(e, Func):
                child = self.child_cache(e)
                txt = self.render_fn(child, scope, ind, None, prefix='local function ' + nm)
                return [ind + txt]
            return [ind + 'local ' + nm + ' = ' + self.ex(e, scope, ind)]
        if k == 'assign':
            tgt = self.ex(s[1], scope, ind)
            return [ind + tgt + ' = ' + self.ex(s[2], scope, ind)]
        if k == 'call':
            txt = self.ex(s[1], scope, ind)
            if txt.startswith('('):
                txt = ';' + txt
            return [ind + txt]
        if k == 'call_cond':
            return [ind + 'local _ = ' + self.ex(s[1], scope, ind)]
        if k == 'return':
            if not s[1]:
                return [ind + 'return']
            return [ind + 'return ' + ', '.join(self.ex(x, scope, ind) for x in s[1])]
        if k == 'if':
            out = []
            c, th, el = s[1], s[2], s[3]
            c = tru(c)
            out.append(ind + 'if ' + self.ex(c, scope, ind) + ' then')
            out.extend(self.block(th, Scope(scope), ind + '\t'))
            while len(el) == 1 and el[0][0] == 'if':
                _, c2, th2, el2 = el[0]
                c2 = tru(c2)
                out.append(ind + 'elseif ' + self.ex(c2, scope, ind) + ' then')
                out.extend(self.block(th2, Scope(scope), ind + '\t'))
                el = el2
            if el:
                out.append(ind + 'else')
                out.extend(self.block(el, Scope(scope), ind + '\t'))
            out.append(ind + 'end')
            return out
        if k == 'break':
            return [ind + 'break']
        if k == 'continue':
            return [ind + 'continue']
        if k == 'while':
            out = [ind + 'while ' + self.ex(tru(s[1]), scope, ind) + ' do']
            out.extend(self.block(s[2], Scope(scope), ind + '\t'))
            out.append(ind + 'end')
            return out
        if k == 'localn':
            ws, e = s[1], s[2]
            names = []
            for w in ws:
                if w is None:
                    names.append('_')
                else:
                    names.append(self.dn(w, scope))
            return [ind + 'local ' + ', '.join(names) + ' = ' + self.ex(e, scope, ind)]
        if k == 'forin':
            _, ws, exprs, body = s
            sc = Scope(scope)
            names = []
            for i, w in enumerate(ws):
                if not w.uses:
                    names.append('_')
                else:
                    if w.hint is None:
                        w.hint = self.forin_hint(exprs, i)
                    names.append(self.dn(w, sc))
            sc.counter['loop_active'] = 1
            ex = [self.ex(x, scope, ind) for x in exprs]
            if len(exprs) == 3 and isinstance(exprs[2], Const) and exprs[2].kind == 'z':
                ex = ex[:2]
            out = [ind + 'for ' + ', '.join(names) + ' in ' + ', '.join(ex) + ' do']
            out.extend(self.block(body, sc, ind + '\t'))
            out.append(ind + 'end')
            return out
        if k == 'for':
            _, w, ini, lim, stp, body = s
            sc = Scope(scope)
            nm = self.dn(w, sc)
            sc.counter['loop_active'] = 1
            hdr = 'for ' + nm + ' = ' + self.ex(ini, scope, ind) + ', ' + self.ex(lim, scope, ind)
            if not (isinstance(stp, Const) and stp.kind == 'n' and stp.v == 1):
                hdr += ', ' + self.ex(stp, scope, ind)
            out = [ind + hdr + ' do']
            out.extend(self.block(body, sc, ind + '\t'))
            out.append(ind + 'end')
            return out
        raise Exception('unknown stmt ' + k)


# ---------------------------------------------------------------------------------------------------------------
# hoisting of declarations whose variable is used outside the declaring block
# ---------------------------------------------------------------------------------------------------------------
def expr_vars(e, acc):
    if isinstance(e, Var):
        acc.append(e.web)
    elif isinstance(e, E):
        for v in vars(e).values():
            if isinstance(v, E):
                expr_vars(v, acc)
            elif isinstance(v, list):
                for x in v:
                    if isinstance(x, E):
                        expr_vars(x, acc)


def hoist(fn):
    return fn

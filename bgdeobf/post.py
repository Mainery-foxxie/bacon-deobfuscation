"""AST post passes: and/or value folding, boolean folding, single-use local inlining, table constructor folding,
declaration hoisting."""
import sys, copy
from .gen import *


class Paren(E):
    def __init__(s, e):
        s.e = e


# ------------------------------------------------------------------------------------------------ generic helpers
def children(e):
    for v in vars(e).values():
        if isinstance(v, E):
            yield v
        elif isinstance(v, (list, tuple)):
            for x in v:
                if isinstance(x, E):
                    yield x
                elif isinstance(x, (list, tuple)):
                    for y in x:
                        if isinstance(y, E):
                            yield y


def sig(x):
    if x is None or isinstance(x, (bool, int, float, str, bytes)):
        return x
    if isinstance(x, (list, tuple)):
        return tuple(sig(y) for y in x)
    if isinstance(x, Web):
        return ('W', x.uid)
    if isinstance(x, E):
        return (type(x).__name__,) + tuple((k, sig(v)) for k, v in sorted(vars(x).items()) if k != 'child')
    return ('?', id(x))


def refs(e, web):
    if isinstance(e, Var):
        return e.web is web
    if isinstance(e, Func):
        return web in e.caps
    if isinstance(e, E):
        return any(refs(c, web) for c in children(e))
    return False


def map_val(v, f):
    if isinstance(v, E):
        return map_expr(v, f)
    if isinstance(v, list):
        return [map_val(x, f) for x in v]
    if isinstance(v, tuple):
        return tuple(map_val(x, f) for x in v)
    return v


def map_expr(e, f):
    r = f(e)
    if r is not None:
        return r
    if isinstance(e, (Const, Imp, Var, Upv, Func)):
        return e
    n = copy.copy(e)
    for k, v in list(vars(e).items()):
        setattr(n, k, map_val(v, f))
    return n


def is_truthy_const(e):
    return isinstance(e, Const) and (e.kind in ('s', 'n') or (e.kind == 'b' and e.v is True))


def is_boolish(e):
    if isinstance(e, Not):
        return True
    if isinstance(e, Bin):
        if e.op in ('==', '~=', '<', '<=', '>', '>='):
            return True
        if e.op in ('and', 'or'):
            return is_boolish(e.l) and is_boolish(e.r)
    if isinstance(e, Const) and e.kind == 'b':
        return True
    return False


def tgt_web(s):
    if s[0] == 'local' and s[2] is not None:
        return s[1]
    if s[0] == 'assign' and isinstance(s[1], Var):
        return s[1].web
    return None


def mk_set(web, e, declare):
    return ('local', web, e) if declare else ('assign', web.var, e)


def stable_simple(e):
    """expression that can be evaluated twice with the same result (no calls, plain reads)"""
    if isinstance(e, (Const, Imp)):
        return True
    if isinstance(e, Var):
        return True
    if isinstance(e, Idx):
        return (isinstance(e.key, str) or isinstance(e.key, Const)) and stable_simple(e.obj)
    return False


# ------------------------------------------------------------------------------------------------ and/or folding
def try_and_or(s1, s2):
    X = tgt_web(s1)
    if X is None or s2[0] != 'if':
        return None
    _, c, th, el = s2
    if el or len(th) != 1:
        return None
    s3 = th[0]
    if s3[0] != 'assign' or not isinstance(s3[1], Var) or s3[1].web is not X:
        return None
    if isinstance(c, Var) and c.web is X:
        op = 'and'
    elif isinstance(c, Not) and isinstance(c.e, Var) and c.e.web is X:
        op = 'or'
    else:
        return None
    a, b = s1[2], s3[2]
    if refs(b, X):
        if not stable_simple(a) or refs(a, X):
            return None
        b = map_expr(b, lambda x: a if isinstance(x, Var) and x.web is X else None)
    return mk_set(X, Bin(op, a, b), s1[0] == 'local')


def try_bool_acc(s1, s2):
    X = tgt_web(s1)
    if X is None or s2[0] != 'if':
        return None
    a = s1[2]
    if not (isinstance(a, Const) and a.kind == 'b'):
        return None
    _, c, th, el = s2
    if el:
        return None
    ins = single_set(th, X)
    if not ins or refs(ins[1], X) or refs(c, X):
        return None
    d = ins[1]
    if a.v is False:
        if not is_boolish(c):
            return None
        return mk_set(X, Bin('and', c, d), s1[0] == 'local')
    nc = negate(c)
    if not is_boolish(nc):
        return None
    return mk_set(X, Bin('or', nc, d), s1[0] == 'local')


def single_set(stmts, X=None):
    """stmts == [one plain assignment/declaration of a web] -> (web, expr, is_local)"""
    if len(stmts) != 1:
        return None
    s = stmts[0]
    w = tgt_web(s)
    if w is None or (X is not None and w is not X):
        return None
    return w, s[2], s[0] == 'local'


def flat_or(e):
    if isinstance(e, Bin) and e.op == 'or':
        return flat_or(e.l) + flat_or(e.r)
    return [e]


def chain_or(xs):
    r = xs[0]
    for x in xs[1:]:
        r = Bin('or', r, x)
    return r


def try_ternary(s):
    if s[0] != 'if':
        return None
    _, c, th, el = s
    for flip in (False, True):
        a_side, b_side = (th, el) if not flip else (el, th)
        cond = c if not flip else negate(c)
        # c and A or B :  A-side = [X = a ; if not X then X = b end],  B-side = [X = b]
        if len(a_side) == 2 and a_side[1][0] == 'if':
            A = a_side[0]
            X = tgt_web(A)
            ib = a_side[1]
            if X is not None and isinstance(ib[1], Not) and isinstance(ib[1].e, Var) and ib[1].e.web is X and not ib[3]:
                in1 = single_set(ib[2], X)
                in2 = single_set(b_side, X)
                if in1 and in2 and sig(in1[1]) == sig(in2[1]) and not refs(in1[1], X):
                    return mk_set(X, Bin('or', Bin('and', cond, A[2]), in1[1]), A[0] == 'local' or in2[2])
        # if c then X = a or b else X = b  ->  X = c and a or b
        if len(a_side) == 1 and len(b_side) == 1:
            sa = single_set(a_side)
            sb = single_set(b_side, sa[0] if sa else None)
            if sa and sb and isinstance(sa[1], Bin) and sa[1].op == 'or' and sig(sa[1].r) == sig(sb[1]):
                X = sa[0]
                if not refs(sb[1], X) and not refs(cond, X) and not refs(sa[1].l, X):
                    return mk_set(X, Bin('or', Bin('and', cond, sa[1].l), sb[1]), sa[2] or sb[2])
        # if c then X = a1 or .. or an or T else X = T   ->   X = c and (a1 or .. or an) or T   (T shared tail)
        if len(a_side) == 1 and len(b_side) == 1:
            sa = single_set(a_side)
            sb = single_set(b_side, sa[0] if sa else None)
            if sa and sb:
                X = sa[0]
                fa, fb = flat_or(sa[1]), flat_or(sb[1])
                k = len(fb)
                if len(fa) > k and [sig(x) for x in fa[-k:]] == [sig(x) for x in fb] \
                        and not any(refs(x, X) for x in fa + fb) and not refs(cond, X):
                    return mk_set(X, chain_or([Bin('and', cond, chain_or(fa[:-k]))] + fb), sa[2] or sb[2])
        # if c then X = a else X = b  with truthy constant a
        if len(a_side) == 1 and len(b_side) == 1:
            sa = single_set(a_side)
            sb = single_set(b_side, sa[0] if sa else None)
            if sa and sb:
                X = sa[0]
                a, b = sa[1], sb[1]
                decl = sa[2] or sb[2]
                if is_truthy_const(a) and not refs(b, X) and not refs(cond, X):
                    return mk_set(X, Bin('or', Bin('and', cond, a), b), decl)
                if isinstance(a, Const) and isinstance(b, Const) and a.kind == 'b' and b.kind == 'b' and a.v != b.v \
                        and is_boolish(cond) and not refs(cond, X):
                    return mk_set(X, cond if a.v else negate(cond), decl)
    return None


def fold_values(stmts):
    def rec(s):
        k = s[0]
        if k == 'if':
            return ('if', s[1], fold_values(s[2]), fold_values(s[3]))
        if k == 'while':
            return ('while', s[1], fold_values(s[2]))
        if k == 'for':
            return ('for', s[1], s[2], s[3], s[4], fold_values(s[5]))
        if k == 'forin':
            return ('forin', s[1], s[2], fold_values(s[3]))
        return s
    stmts = [rec(s) for s in stmts]
    changed = True
    while changed:
        changed = False
        out = []
        i = 0
        while i < len(stmts):
            if i + 1 < len(stmts):
                r = try_and_or(stmts[i], stmts[i + 1])
                if r is None:
                    r = try_bool_acc(stmts[i], stmts[i + 1])
                if r is not None:
                    out.append(r)
                    i += 2
                    changed = True
                    continue
            r = try_ternary(stmts[i])
            if r is not None:
                out.append(r)
                i += 1
                changed = True
                continue
            out.append(stmts[i])
            i += 1
        stmts = out
    return stmts


# ------------------------------------------------------------------------------------------------ single-use inlining
def count_uses(stmts, cnt):
    """number of occurrences of every web in expressions / assignment targets of the whole body (closures count twice)"""
    def ex(e):
        if isinstance(e, Var):
            cnt[e.web] = cnt.get(e.web, 0) + 1
        elif isinstance(e, Func):
            for w in e.caps:
                if isinstance(w, Web):
                    cnt[w] = cnt.get(w, 0) + 2
        elif isinstance(e, E):
            for c in children(e):
                ex(c)
    for s in stmts:
        k = s[0]
        if k == 'local':
            if s[2] is not None:
                ex(s[2])
        elif k == 'localn':
            ex(s[2])
        elif k == 'assign':
            ex(s[1])
            ex(s[2])
        elif k in ('call', 'call_cond'):
            ex(s[1])
        elif k == 'return':
            for x in s[1]:
                ex(x)
        elif k == 'if':
            ex(s[1])
            count_uses(s[2], cnt)
            count_uses(s[3], cnt)
        elif k == 'while':
            ex(s[1])
            count_uses(s[2], cnt)
        elif k == 'for':
            ex(s[2]); ex(s[3]); ex(s[4])
            count_uses(s[5], cnt)
        elif k == 'forin':
            for x in s[2]:
                ex(x)
            count_uses(s[3], cnt)


def head_exprs(s):
    k = s[0]
    if k == 'assign':
        t = s[1]
        if isinstance(t, Idx):
            ops = [t.obj] + ([t.key] if isinstance(t.key, E) else [])
            return ops + [s[2]]
        return [s[2]]
    if k in ('call', 'call_cond'):
        return [s[1]]
    if k == 'return':
        return list(s[1])
    if k == 'if':
        return [s[1]]
    if k == 'local':
        return [s[2]] if s[2] is not None else []
    if k == 'localn':
        return [s[2]]
    if k == 'forin':
        return list(s[2])
    if k == 'for':
        return [s[2], s[3], s[4]]
    return None


def rebuild_head(s, heads):
    k = s[0]
    if k == 'assign':
        t = s[1]
        if isinstance(t, Idx):
            n = 1 + (1 if isinstance(t.key, E) else 0)
            nt = Idx(heads[0], heads[1] if isinstance(t.key, E) else t.key)
            return ('assign', nt, heads[n])
        return ('assign', s[1], heads[0])
    if k in ('call', 'call_cond'):
        return (k, heads[0])
    if k == 'return':
        return ('return', list(heads))
    if k == 'if':
        return ('if', heads[0], s[2], s[3])
    if k == 'local':
        return ('local', s[1], heads[0] if heads else None)
    if k == 'localn':
        return ('localn', s[1], heads[0])
    if k == 'forin':
        return ('forin', s[1], list(heads), s[3])
    if k == 'for':
        return ('for', s[1], heads[0], heads[1], heads[2], s[5])
    return s


def inline_use(heads, web, repl, multi_last=False):
    """substitute Var(web) by repl inside the head expression list; None when the substitution would reorder
    side effects.  Evaluation order: left to right, operands before the operation."""
    st = {'eff': False, 'reads': False, 'done': False, 'bad': False}
    rc = has_call(repl)
    rsimple = isinstance(repl, (Const, Imp, Var))

    def visit(e, multi=False, cond=False):
        if isinstance(e, Var) and e.web is web:
            if (st['eff'] and not rsimple) or (rc and st['reads']) or (cond and not rsimple):
                st['bad'] = True
            st['done'] = True
            if multi and isinstance(repl, (Call, Multi)):
                return Paren(repl)
            return repl
        if isinstance(e, (Const, Imp, Var, Upv, Func)):
            return e
        n = copy.copy(e)
        if isinstance(e, Call):
            n.fn = visit(e.fn, False, cond)
            n.args = [visit(a, i == len(e.args) - 1, cond) for i, a in enumerate(e.args)]
            st['eff'] = True
            return n
        if isinstance(e, Idx):
            n.obj = visit(e.obj, False, cond)
            if isinstance(e.key, E):
                n.key = visit(e.key, False, cond)
            st['reads'] = True
            return n
        if isinstance(e, Bin) and e.op in ('and', 'or'):
            n.l = visit(e.l, False, cond)
            n.r = visit(e.r, False, True)
            return n
        if isinstance(e, Tbl):
            n.fields = [(k, visit(v, False, cond) if v is not None else None) for k, v in e.fields]
            n.items = [visit(x, i == len(e.items) - 1, cond) for i, x in enumerate(e.items)]
            return n
        for k, v in list(vars(e).items()):
            if isinstance(v, E):
                setattr(n, k, visit(v, False, cond))
            elif isinstance(v, list):
                setattr(n, k, [visit(x, False, cond) if isinstance(x, E) else x for x in v])
        return n

    out = []
    for i, h in enumerate(heads):
        out.append(visit(h, multi_last and i == len(heads) - 1))
    if st['bad'] or not st['done']:
        return None
    return out


def inline_single_use(stmts, cnt):
    stmts = list(stmts)
    # recurse first
    for i, s in enumerate(stmts):
        k = s[0]
        if k == 'if':
            stmts[i] = ('if', s[1], inline_single_use(s[2], cnt), inline_single_use(s[3], cnt))
        elif k == 'while':
            stmts[i] = ('while', s[1], inline_single_use(s[2], cnt))
        elif k == 'for':
            stmts[i] = ('for', s[1], s[2], s[3], s[4], inline_single_use(s[5], cnt))
        elif k == 'forin':
            stmts[i] = ('forin', s[1], s[2], inline_single_use(s[3], cnt))
    i = 0
    while i + 1 < len(stmts):
        s = stmts[i]
        if s[0] == 'local' and s[2] is not None and s[1].kind == 'var' and cnt.get(s[1], 0) == 1 \
                and (not getattr(s[1], 'keep', False) or (isinstance(s[2], Var) and CUR_FN[0] is not None and s[2].web.fn is not CUR_FN[0])) \
                and not isinstance(s[2], Func) and not (isinstance(s[2], Const) and s[2].kind == 'z'):
            w = s[1]
            nxt = stmts[i + 1]
            heads = head_exprs(nxt)
            if heads is not None and any(refs(h, w) for h in heads):
                e = s[2]
                # a table constructor must stay a statement when it is built field by field later
                new_heads = inline_use(heads, w, e, nxt[0] in ('return', 'forin'))
                if new_heads is not None:
                    stmts[i + 1] = rebuild_head(nxt, new_heads)
                    del stmts[i]
                    cnt[w] = 0
                    if i > 0:
                        i -= 1
                    continue
        i += 1
    return stmts


# ------------------------------------------------------------------------------------------------ table constructors
def fold_tables(stmts):
    stmts = list(stmts)
    for i, s in enumerate(stmts):
        k = s[0]
        if k == 'if':
            stmts[i] = ('if', s[1], fold_tables(s[2]), fold_tables(s[3]))
        elif k == 'while':
            stmts[i] = ('while', s[1], fold_tables(s[2]))
        elif k == 'for':
            stmts[i] = ('for', s[1], s[2], s[3], s[4], fold_tables(s[5]))
        elif k == 'forin':
            stmts[i] = ('forin', s[1], s[2], fold_tables(s[3]))
    out = []
    i = 0
    while i < len(stmts):
        s = stmts[i]
        if s[0] == 'local' and isinstance(s[2], Tbl) and not s[2].items and s[1].kind == 'var':
            w = s[1]
            tbl = s[2]
            fields = list(tbl.fields)
            pending = list(tbl.pending)
            idx = {k_: n for n, (k_, v) in enumerate(fields)}
            j = i + 1
            added = []
            while j < len(stmts):
                t = stmts[j]
                if t[0] != 'assign' or not isinstance(t[1], Idx) or not isinstance(t[1].obj, Var) or t[1].obj.web is not w:
                    break
                key = t[1].key
                if isinstance(key, Const) and key.kind == 's':
                    key = key.v.decode('latin1')
                if not isinstance(key, str):
                    break
                if refs(t[2], w):
                    break
                if key in idx and fields[idx[key]][1] is not None:
                    break
                if key in [a for a, _ in added]:
                    break
                added.append((key, t[2]))
                j += 1
            if added:
                # fill template slots in assignment order, append unknown keys
                newfields = []
                tmpl_vals = {}
                for key, v in added:
                    tmpl_vals[key] = v
                placed = set()
                for k_, v in fields:
                    if v is not None:
                        newfields.append((k_, v))
                    elif k_ in tmpl_vals:
                        pass
                for key, v in added:
                    newfields.append((key, v))
                tb = Tbl([], newfields, [])
                out.append(('local', w, tb))
                i = j
                continue
        out.append(s)
        i += 1
    return out


# ------------------------------------------------------------------------------------------------ hoisting
def find_list(stmts, lid):
    if id(stmts) == lid:
        return stmts
    for s in stmts:
        k = s[0]
        subs = []
        if k == 'if':
            subs = [s[2], s[3]]
        elif k == 'while':
            subs = [s[2]]
        elif k == 'for':
            subs = [s[5]]
        elif k == 'forin':
            subs = [s[3]]
        for sub in subs:
            r = find_list(sub, lid)
            if r is not None:
                return r
    return None


def hoist_decls(body):
    """a web declared inside a nested block but used outside of it: declare it before the first statement that
    contains the declaration or any use, in the common ancestor block"""
    def collect(stmts, chain, occ):
        for i, s in enumerate(stmts):
            here = chain + ((id(stmts), i),)
            k = s[0]
            ws = []

            def ex(e):
                if isinstance(e, Var):
                    ws.append(e.web)
                elif isinstance(e, Func):
                    ws.extend(w for w in e.caps if isinstance(w, Web))
                elif isinstance(e, E):
                    for c in children(e):
                        ex(c)
            decl = None
            if k == 'local':
                decl = [s[1]]
                if s[2] is not None:
                    ex(s[2])
            elif k == 'localn':
                decl = [w for w in s[1] if w is not None]
                ex(s[2])
            elif k == 'assign':
                ex(s[1])
                ex(s[2])
            elif k in ('call', 'call_cond'):
                ex(s[1])
            elif k == 'return':
                for x in s[1]:
                    ex(x)
            elif k == 'if':
                ex(s[1])
            elif k == 'while':
                ex(s[1])
            elif k == 'for':
                ex(s[2])
                ex(s[3])
                ex(s[4])
            elif k == 'forin':
                for x in s[2]:
                    ex(x)
            if decl:
                for w in decl:
                    occ.setdefault(w, {'decl': None, 'uses': []})['decl'] = (stmts, i, here)
            for w in ws:
                occ.setdefault(w, {'decl': None, 'uses': []})['uses'].append((stmts, i, here))
            if k == 'if':
                collect(s[2], here, occ)
                collect(s[3], here, occ)
            elif k == 'while':
                collect(s[2], here, occ)
            elif k == 'for':
                collect(s[5], here, occ)
            elif k == 'forin':
                collect(s[3], here, occ)

    for _round in range(500):
        occ = {}
        collect(body, (), occ)
        fixed = False
        for w, o in occ.items():
            if o['decl'] is None:
                continue
            dl, di, dchain = o['decl']
            bad = False
            for (ul, ui, uchain) in o['uses']:
                ok = False
                for item in uchain:
                    if item[0] == id(dl):
                        ok = item[1] > di
                        break
                if not ok:
                    bad = True
                    break
            if not bad:
                continue
            chains = [dchain] + [u[2] for u in o['uses']]
            depth = 0
            while all(len(c) > depth for c in chains) and all(c[depth][0] == chains[0][depth][0] for c in chains):
                depth += 1
            if depth == 0:
                raise Exception('hoist: no common block')
            lid = chains[0][depth - 1][0]
            pos = min(c[depth - 1][1] for c in chains)
            lst = find_list(body, lid)
            ds = dl[di]
            if ds[0] != 'local':
                raise Exception('hoist of multi-assignment not supported')
            if ds[2] is not None:
                dl[di] = ('assign', ds[1].var, ds[2])
            else:
                del dl[di]
                if dl is lst and di < pos:
                    pos -= 1
            lst.insert(pos, ('local', w, None))
            fixed = True
            break
        if not fixed:
            break
    return body


NO_ALIAS = False
CUR_FN = [None]


def inline_aliases(body, cnt):
    """local a = b  (b a plain variable that is never reassigned): use b instead of a"""
    assigned = set()

    def scan(stmts):
        for s in stmts:
            k = s[0]
            if k == 'assign' and isinstance(s[1], Var):
                assigned.add(s[1].web)
            elif k == 'if':
                scan(s[2]); scan(s[3])
            elif k == 'while':
                scan(s[2])
            elif k == 'for':
                scan(s[5])
            elif k == 'forin':
                scan(s[3])
    scan(body)
    capset = set()

    def capscan(stmts):
        def ex(e):
            if isinstance(e, Func):
                capset.update(w for w in e.caps if isinstance(w, Web))
            elif isinstance(e, E):
                for c in children(e):
                    ex(c)
        for s in stmts:
            k = s[0]
            if k == 'local':
                if s[2] is not None:
                    ex(s[2])
            elif k == 'localn':
                ex(s[2])
            elif k == 'assign':
                ex(s[1]); ex(s[2])
            elif k in ('call', 'call_cond'):
                ex(s[1])
            elif k == 'return':
                for x in s[1]:
                    ex(x)
            elif k == 'if':
                ex(s[1]); capscan(s[2]); capscan(s[3])
            elif k == 'while':
                ex(s[1]); capscan(s[2])
            elif k == 'for':
                ex(s[2]); ex(s[3]); ex(s[4]); capscan(s[5])
            elif k == 'forin':
                for x in s[2]:
                    ex(x)
                capscan(s[3])
    capscan(body)
    subst = {}

    def collect(stmts):
        for s in stmts:
            k = s[0]
            if k == 'local' and isinstance(s[2], Var) and s[1].kind == 'var' and s[1] not in assigned:
                a, b = s[1], s[2].web
                if b not in assigned and len(b.defs) <= 1 and b.kind in ('var', 'param') and a not in capset \
                        and (CUR_FN[0] is None or b.fn is CUR_FN[0]):
                    subst[a] = b
            if k == 'if':
                collect(s[2]); collect(s[3])
            elif k == 'while':
                collect(s[2])
            elif k == 'for':
                collect(s[5])
            elif k == 'forin':
                collect(s[3])
    collect(body)
    if NO_ALIAS:
        return {}
    return subst


def apply_subst(stmts, subst):
    def f(e):
        if isinstance(e, Var) and e.web in subst:
            w = subst[e.web]
            while w in subst:
                w = subst[w]
            return w.var
        return None

    def me(e):
        return map_expr(e, f) if e is not None else None
    out = []
    for s in stmts:
        k = s[0]
        if k == 'local':
            if s[1] in subst and isinstance(s[2], Var):
                continue
            out.append(('local', s[1], me(s[2])))
        elif k == 'localn':
            out.append(('localn', s[1], me(s[2])))
        elif k == 'assign':
            out.append(('assign', me(s[1]), me(s[2])))
        elif k in ('call', 'call_cond'):
            out.append((k, me(s[1])))
        elif k == 'return':
            out.append(('return', [me(x) for x in s[1]]))
        elif k == 'if':
            out.append(('if', me(s[1]), apply_subst(s[2], subst), apply_subst(s[3], subst)))
        elif k == 'while':
            out.append(('while', me(s[1]), apply_subst(s[2], subst)))
        elif k == 'for':
            out.append(('for', s[1], me(s[2]), me(s[3]), me(s[4]), apply_subst(s[5], subst)))
        elif k == 'forin':
            out.append(('forin', s[1], [me(x) for x in s[2]], apply_subst(s[3], subst)))
        else:
            out.append(s)
    return out


def post_passes(body, fn=None):
    CUR_FN[0] = fn
    for _ in range(6):
        before = sig(body)
        body = fold_values(body)
        body = fold_tables(body)
        cnt = {}
        count_uses(body, cnt)
        sub = inline_aliases(body, cnt)
        if sub:
            body = apply_subst(body, sub)
            cnt = {}
            count_uses(body, cnt)
        body = inline_single_use(body, cnt)
        body = fold_tables(body)
        if sig(body) == before:
            break
    body = fold_tables(body)
    body = hoist_decls(body)
    return body

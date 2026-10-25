"""Behavioural classification of the handlers of a Bacon Guard build.

The loader (loader.py) tells where every handler lives; this module runs each handler in the AST interpreter on many
pseudo-random machine states and finds the reference operation (refspec.py) that behaves identically.
"""
import time

from .astutil import walk, lkey, unparen
from .loader import LayoutError, loc_of, is_in
from .luavm import (Interp, Func, HostFunc, LuaTable, LuaError, StepLimit, Unsupported, _Return, _Env, LOG, num)
from .refspec import State, SPECS

TRIALS = 24
HF_SWEEP = range(0, 64)
SOFT_ERRORS = (LuaError, TypeError, AttributeError, ValueError, OverflowError, ZeroDivisionError)
FIELDS = ('A', 'B', 'C', 'RF', 'HF', 'AUX')
RAWF = ('op', 'ef', 'A', 'B', 'C', 'RF', 'HF', 'AUX', 'NXT', 'ALT')


class ClassifyError(Exception):
    pass


def _noop(*a):
    return []


def _snap(st, spec):
    return st.snapshot(spec.observe)


def collect_domains(L, dump):
    """field values really used by the program for every handler (fallback for specialised handlers)"""
    dom = {}

    def add(key, fields):
        d = dict((k, fields[k]) for k in FIELDS if fields.get(k) is not None)
        if d:
            dom.setdefault(key, {})[tuple(sorted(d.items()))] = d

    for idx, P in dump.protos.items():
        df = P['df']
        perm, fperm = L.perm[df], L.fused_perm[df]
        for rid, rec in P['recs'].items():
            sem, T = rec['sem'], rec['T']
            parts = [dict(zip(FIELDS, [pr[i - 1] for i in fperm])) for pr in rec['parts']]
            if rec['ded'] != '-':
                ti = int(rec['ded'])
                key = sem + idx * 256 if ti == 6 else sem
                for k, f in enumerate(parts):
                    add(('ded', ti, key, k), f)
                continue
            if rec['pat'] is not None:
                for i, b in enumerate(rec['pat']):
                    if i < len(parts):
                        add(('op', b ^ dump.xorkey), parts[i])
                continue
            fl = dict(zip(RAWF, [T[i - 1] for i in perm]))
            add(('op', fl['op']), fl)
    return dict((k, list(v.values())) for k, v in dom.items())


class Mapping(object):
    """result of the classification (plain data, JSON friendly)"""

    def __init__(self):
        self.simple = {}      # op -> name | ('HF', {hf: name})
        self.hot = {}         # op -> name
        self.ctrl = {}        # op -> name
        self.ded = {}         # (ti, key) -> [(name, part, fixed, is_ctrl)]
        self.junk = None
        self.unknown = []     # (where, op/key, why)

    def to_json(self):
        return dict(
            simple={str(k): v for k, v in self.simple.items()},
            hot={str(k): v for k, v in self.hot.items()},
            ctrl={str(k): v for k, v in self.ctrl.items()},
            ded={'%d:%d' % k: v for k, v in self.ded.items()},
            junk=self.junk,
            unknown=self.unknown)


def _strip_fixed(name):
    """spec name -> (IR name, fixed field overrides)"""
    if name.startswith('CALL_'):
        _, b, c = name.split('_')
        return 'CALL', {'B': int(b), 'C': int(c)}
    return name, {}


class Classifier(object):
    def __init__(self, loader, domains=None, trials=TRIALS, deadline=None):
        self.deadline = deadline
        self.L = loader
        self.domains = domains or {}
        self.trials = trials
        self.interp = Interp(max_steps=60000)
        self.env_table = LuaTable()
        self.loc = {}
        for role, local in loader.role_local.items():
            self.loc[role] = local['location']
        self.nil_locs = self._nil_locals()
        self.mapping = Mapping()
        self.mapping.junk = loader.junk_op
        self.kget_loc = self.loc.get('KGET')

    # ------------------------------------------------------------------ runtime seeding
    def _nil_locals(self):
        out = []
        for s in self.L.frame_body:
            if s['type'] == 'AstStatLocal' and not s['values']:
                for v in s['vars']:
                    out.append(v['location'])
        return out

    def seed(self, st):
        L = self.L
        f = st.f
        self.interp.unbound = lambda local, st=st: st.tag
        vals = {
            'REGS': st.R, 'KGET': HostFunc(st.kget, 'kget'), 'IMP': HostFunc(st.imp, 'imp'),
            'FUSED': HostFunc(st.fused, 'fused'), 'UP': st.UP, 'ENV': self.env_table,
            'OP': f['op'], 'EF': f['ef'], 'A': f['A'], 'B': f['B'], 'C': f['C'], 'RF': f['RF'], 'HF': f['HF'],
            'AUX': f['AUX'], 'NXT': st.nxt, 'ALT': st.alt, 'TOP': st.top, 'REC': st.rec, 'CLR': st.clr,
            'PROF': st.prof, 'VARARGS': st.varargs, 'NVAR': st.nvar, 'PC': 1,
        }
        cells = {}
        for k in self.nil_locs:
            cells[k] = [None]
        for role, v in vals.items():
            loc = self.loc.get(role)
            if loc is not None:
                cells[loc] = [v]
        for key, role in L.roles.items():
            if role == 'MKFN':
                cells[key] = [HostFunc(_noop, 'mkfn')]
        for local, items in L.simple_table_items:
            tab = LuaTable()
            for key, expr in items:
                v = self._table_value(expr, cells)
                if v is not None:
                    tab.set(key, v)
            cells[local['location']] = [tab]
        return cells

    def _table_value(self, expr, cells):
        expr = unparen(expr)
        t = expr['type']
        if t == 'AstExprConstantNumber':
            return num(expr['value'])
        if t == 'AstExprConstantBool':
            return bool(expr['value'])
        if t == 'AstExprConstantString':
            return expr['value']
        fn = self._func_node(expr)
        if fn is not None:
            return Func(fn, self.interp, cells)
        return None

    def _func_node(self, expr):
        expr = unparen(expr)
        if expr['type'] == 'AstExprFunction':
            return expr
        if expr['type'] == 'AstExprLocal':
            return self.L.funcs.get(lkey(expr['local']))
        return None

    def args_for(self, roles, st, op=None):
        f = st.f
        m = {'REC': st.rec, 'REGS': st.R, 'KGET': HostFunc(st.kget, 'kget'), 'IMP': HostFunc(st.imp, 'imp'),
             'FUSED': HostFunc(st.fused, 'fused'), 'ENV': self.env_table, 'UP': st.UP, 'TOP': st.top, 'NXT': st.nxt,
             'ALT': st.alt, 'CLR': st.clr, 'VARARGS': st.varargs, 'NVAR': st.nvar, 'PROF': st.prof,
             'OP': op if op is not None else f['op'], 'EF': f['ef'], 'A': f['A'], 'B': f['B'], 'C': f['C'],
             'RF': f['RF'], 'HF': f['HF'], 'AUX': f['AUX']}
        return [m.get(r) for r in roles]

    # ------------------------------------------------------------------ runners (state -> effects)
    def run_simple(self, op):
        L = self.L

        def run(st):
            cells = self.seed(st)
            fn = Func(L.simple_fn, self.interp, cells)
            f = st.f
            base = self.args_for(L.call_roles['SIMPLE'][:5], st)
            self.interp.call(fn, base + [op, f['A'], f['B'], f['C'], f['RF'], f['HF'], f['AUX']])
        return run

    def run_ctrl(self, op):
        L = self.L

        def run(st):
            cells = self.seed(st)
            fn = Func(L.ctrl_fn, self.interp, cells)
            res = self.interp.call(fn, self.args_for(L.call_roles['CTRL'], st, op))
            if len(res) > 0:
                st.nxt = res[0]
            if len(res) > 1:
                st.top = res[1]
            if len(res) > 2 and res[2] is not None:
                st.ret = [res[2].get(i + 1) for i in range(res[2].get('n') or 0)] if isinstance(res[2], LuaTable) else res[2]
        return run

    def run_block(self, block):
        def run(st):
            cells = self.seed(st)
            env = _Env(cells, [])
            ret = None
            try:
                self.interp.exec_block(block, env)
            except _Return as r:
                ret = r.values
            st.nxt = cells[self.loc['NXT']][0]
            st.top = cells[self.loc['TOP']][0]
            if ret is not None:
                st.ret = list(ret)
        return run

    def run_whole(self, ti, fn):
        roles = self.L.call_roles['D%d' % ti]

        def run(st):
            cells = self.seed(st)
            res = self.interp.call(Func(fn, self.interp, cells), self.args_for(roles, st))
            if ti == 1:
                st.ret = list(res)
        return run

    def run_micro(self, ti, fn, stmts):
        roles = self.L.call_roles['D%d' % (5 if ti == 6 else ti)]
        params = fn['args']

        def run(st):
            cells = self.seed(st)
            local = dict(cells)
            args = self.args_for(roles, st)
            for i, p in enumerate(params):
                local[p['location']] = [args[i] if i < len(args) else None]
            env = _Env(local, [])
            ret = None
            try:
                for s in stmts:
                    self.interp.exec_stat(s, env)
            except _Return as r:
                ret = r.values
            for r_, attr in (('TOP', 'top'), ('NXT', 'nxt')):
                if r_ in roles:
                    setattr(st, attr, local[params[roles.index(r_)]['location']][0])
            if ret is not None:
                if ti == 4:
                    st.nxt, st.top = ret[0], ret[1]
                elif ti in (2, 3):
                    st.top = ret[0]
        return run

    def run_group(self, ti, fn, stmts, mode):
        """statements of a fused-loop handler run once.  mode: 'plain' (a data operation); 'jump' (the operation ends by
        entering the loop, i.e. it behaves like a jump); 'loop' (the loop control: leaving the loop is a `return`, falling
        through goes round again, i.e. it behaves like a conditional jump back)"""
        roles = self.L.call_roles['D%d' % (5 if ti == 6 else ti)]
        params = fn['args']

        def run(st):
            cells = self.seed(st)
            local = dict(cells)
            args = self.args_for(roles, st)
            for i, p in enumerate(params):
                local[p['location']] = [args[i] if i < len(args) else None]
            env = _Env(local, [])
            exited = False
            try:
                for s in stmts:
                    self.interp.exec_stat(s, env)
            except _Return:
                exited = True
            for r_, attr in (('TOP', 'top'), ('NXT', 'nxt')):
                if r_ in roles:
                    setattr(st, attr, local[params[roles.index(r_)]['location']][0])
            if mode == 'jump' or (mode == 'loop' and not exited):
                st.nxt = st.alt
        return run

    # ------------------------------------------------------------------ fused loops
    def classify_loop_ded(self, ti, key, fn):
        """A handler that runs a whole loop inside one record: the record parts are read first, then
        `while true do <micro-operations, one of them the loop control> end`.  The result is
        {'loop': {'pre': [...], 'head': [...], 'body': [...], 'tail': [...]}} with micro-operations as in other handlers;
        the loop control is a FORGLOOP at the head (the iterator is called before each pass of the body) or a FORNLOOP
        at the tail."""
        where = 'ded%d' % ti
        body = fn['body']['body']
        w = body[-1] if body else None
        if w is None or w['type'] != 'AstStatWhile' or sum(1 for s_ in body if s_['type'] == 'AstStatWhile') != 1:
            self.mapping.unknown.append((where, key, 'loop handler shape not recognised'))
            return
        cond = unparen(w['condition'])
        if cond['type'] != 'AstExprConstantBool' or not cond['value']:
            self.mapping.unknown.append((where, key, 'loop handler shape not recognised'))
            return
        rec_loc = fn['args'][0]['location']
        var_part, part_of, reads, others = {}, {}, {}, []
        for s_ in body[:-1]:
            part = self._part_read(s_, rec_loc)
            if part is not None:
                var_part[lkey(s_['vars'][0])] = part
                reads.setdefault(part, []).append(s_)
                continue
            fp = self._fused_read(s_, var_part)
            if fp is not None:
                for v in s_['vars']:
                    part_of[lkey(v)] = fp
                reads[fp].append(s_)
                continue
            others.append(s_)

        def parts_in(s_):
            ps = set()
            for n, p in walk(s_):
                if n['type'] == 'AstExprLocal' and lkey(n['local']) in part_of:
                    ps.add(part_of[lkey(n['local'])])
            return ps

        def groups_of(stmts):
            gs = []
            for s_ in stmts:
                ps = parts_in(s_)
                if len(ps) > 1:
                    return None
                if not ps:
                    if not gs:
                        return None
                    gs[-1][1].append(s_)
                    continue
                part = next(iter(ps))
                if gs and gs[-1][0] == part:
                    gs[-1][1].append(s_)
                else:
                    gs.append([part, [s_]])
            return gs

        pre_groups = groups_of(others)
        loop_groups = groups_of(w['body']['body'])
        if pre_groups is None or loop_groups is None or not loop_groups:
            self.mapping.unknown.append((where, key, 'loop handler: micro-operations not separable'))
            return
        used = [g[0] for g in pre_groups + loop_groups]
        if len(set(used)) != len(used) or any(pt not in reads for pt in used):
            self.mapping.unknown.append((where, key, 'loop handler: a record part is used twice'))
            return
        ctl = [i for i, g in enumerate(loop_groups)
               if any(n['type'] == 'AstStatReturn' for s_ in g[1] for n, p in walk(s_))]
        if len(ctl) != 1:
            self.mapping.unknown.append((where, key, 'loop handler: loop control not found'))
            return
        ci = ctl[0]
        if ci == 0 and len(loop_groups) > 1:
            shape = 'head'
        elif ci == len(loop_groups) - 1:
            shape = 'tail'
        else:
            self.mapping.unknown.append((where, key, 'loop handler: loop control in the middle of the body'))
            return
        res = {'pre': [], 'head': [], 'body': [], 'tail': []}

        def one(group, mode, section, label):
            part, stmts = group
            # the iterator check in front of a loop only looks like a no-op until a non-function is tried, so a group that
            # behaves like a jump is tried first
            modes = [mode] if mode != 'pre' else ['jump', 'plain']
            for md in modes:
                runner = self.run_group(ti, fn, reads[part] + stmts, md)
                names = self.find(runner, domain_key=('ded', ti, key, part), skip=self.SKIP)
                if names:
                    break
            nm = self.pick(names, '%s/%d.%s' % (where, key, label), key)
            if nm is None:
                return False
            ir, fixed = _strip_fixed(nm)
            res[section].append((ir, part, fixed, SPECS[nm].kind == 'ctrl'))
            return True

        for gi, g in enumerate(pre_groups):
            if not one(g, 'pre', 'pre', 'pre%d' % gi):
                return
        for gi, g in enumerate(loop_groups):
            if gi == ci:
                if not one(g, 'loop', shape, 'loop'):
                    return
            elif not one(g, 'plain', 'body', 'body%d' % gi):
                return
        ctrl_name = res[shape][0][0]
        if ctrl_name != ('FORGLOOP' if shape == 'head' else 'FORNLOOP'):
            self.mapping.unknown.append((where, key, 'loop handler: unexpected loop control %s' % ctrl_name))
            return
        # a preparation that ends in a jump (the iterator check) is only possible in front of a head-controlled loop
        for m in res['pre'][:-1] + (res['pre'] if shape == 'tail' else []):
            if m[3]:
                self.mapping.unknown.append((where, key, 'loop handler: unexpected jump before the loop'))
                return
        self.mapping.ded[(ti, key)] = {'loop': res}

    @staticmethod
    def _part_read(s_, rec_loc):
        """`local V = REC[11 + k]` -> k"""
        if s_['type'] != 'AstStatLocal' or len(s_['vars']) != 1 or len(s_['values']) != 1:
            return None
        v = unparen(s_['values'][0])
        if v['type'] != 'AstExprIndexExpr':
            return None
        o, i = loc_of(v['expr']), unparen(v['index'])
        if o is None or o['location'] != rec_loc or i['type'] != 'AstExprConstantNumber' or i['value'] < 11:
            return None
        return int(i['value']) - 11

    @staticmethod
    def _fused_read(s_, var_part):
        """`local a, b, ... = FUSED(V)` with V a part read -> the part"""
        if s_['type'] != 'AstStatLocal' or len(s_['values']) != 1:
            return None
        v = unparen(s_['values'][0])
        if v['type'] != 'AstExprCall' or len(v['args']) != 1:
            return None
        a = loc_of(v['args'][0])
        if a is None:
            return None
        return var_part.get(lkey(a))

    # ------------------------------------------------------------------ matching
    def match(self, runner, spec, domain=None, variant=None, strict=False):
        if self.deadline is not None and time.time() > self.deadline:
            raise ClassifyError('classification took too long')
        for t in range(self.trials):
            seed = 7919 * t + 13
            ref = State(spec, seed, domain, variant, t)
            got = State(spec, seed, domain, variant, t)
            LOG.clear()
            try:
                spec.run(ref)
                if spec.normalize and not strict:
                    spec.normalize(ref)
                a = ('ok', _snap(ref, spec))
            except SOFT_ERRORS:
                a = ('err',)
            LOG.clear()
            self.interp.steps = 0
            try:
                runner(got)
                if spec.normalize and not strict:
                    spec.normalize(got)
                b = ('ok', _snap(got, spec))
            except SOFT_ERRORS:
                b = ('err',)
            except (StepLimit, Unsupported, KeyError, IndexError):
                return False
            if a != b:
                return False
        return True

    SKIP = ('RETURN', 'TAILCALL')

    def candidates(self, kinds=None, skip=()):
        for name in sorted(SPECS):
            if name in skip:
                continue
            yield name, SPECS[name]

    def find(self, runner, domain_key=None, skip=(), hf=None):
        """names of all reference ops whose behaviour equals the runner's.

        Passes, most demanding first: exact on any input; exact modulo dead registers; the same two restricted to the
        field values the program really uses (for handlers specialised to the program).  The first pass with a hit wins.
        """
        variant = {'HF': hf} if hf is not None else None
        dom = self.domains.get(domain_key) if domain_key is not None else None
        passes = [(False, False), (False, True)]
        if dom:
            passes += [(True, False), (True, True)]
        for restricted, lenient in passes:
            out = []
            for name, spec in self.candidates(skip=skip):
                if lenient and spec.normalize is None:
                    continue
                if restricted:
                    if not spec.domain_fields:
                        continue
                    dd = [dict((k, d[k]) for k in spec.domain_fields if k in d) for d in dom]
                    dd = [d for d in dd if d]
                    if not dd:
                        continue
                else:
                    dd = None
                if self.match(runner, spec, dd, variant, strict=not lenient):
                    out.append(name)
            if out:
                return out
        return []

    # ------------------------------------------------------------------ structural rules (closure related ops)
    def structural(self, body):
        L = self.L
        stmts = body['body']
        if len(stmts) == 1 and stmts[0]['type'] == 'AstStatExpr':
            e = unparen(stmts[0]['expr'])
            if e['type'] == 'AstExprCall' and unparen(e['func'])['type'] == 'AstExprGlobal' and unparen(e['func'])['global'] == 'error':
                return 'CAPTURE'
        has_nup = False
        has_children = False
        has_kget = False
        has_forin = False
        for n, p in walk(body):
            if is_in(n, 'numUpvalues'):
                has_nup = True
            if is_in(n, 'children'):
                has_children = True
            if n['type'] == 'AstExprCall' and loc_of(n['func']) is not None and self.kget_loc is not None and loc_of(n['func'])['location'] == self.kget_loc:
                has_kget = True
            if n['type'] == 'AstStatForIn':
                has_forin = True
        if has_nup and has_children:
            return 'NEWCLOSURE'
        if has_nup and has_kget:
            return 'DUPCLOSURE'
        if has_forin and not has_nup:
            return 'CLOSEUPVALS'
        return None

    # ------------------------------------------------------------------ the classification passes
    def pick(self, names, where, key):
        names = [n for n in names]
        if not names:
            self.mapping.unknown.append((where, key, 'no reference operation matches'))
            return None
        uniq = sorted(set(_strip_fixed(n)[0] for n in names))
        if len(uniq) > 1:
            # prefer the most specific description; several equivalent names are fine when they decode identically
            prefer = [n for n in uniq if n not in ('NOP',)]
            if len(prefer) != 1:
                self.mapping.unknown.append((where, key, 'ambiguous: ' + ','.join(sorted(names))))
                return None
            uniq = prefer
        return sorted(n for n in names if _strip_fixed(n)[0] == uniq[0])[0]

    def classify_simple(self, ops):
        for op in sorted(ops):
            names = self.find(self.run_simple(op), domain_key=('op', op), skip=self.SKIP)
            if names:
                nm = self.pick(names, 'simple', op)
                if nm is not None:
                    self.mapping.simple[op] = nm
                continue
            hfmap = {}
            for h in HF_SWEEP:
                names = self.find(self.run_simple(op), domain_key=('op', op), skip=self.SKIP, hf=h)
                if names:
                    nm = self.pick(names, 'simple/hf%d' % h, op)
                    if nm is not None:
                        hfmap[h] = nm
            if hfmap:
                self.mapping.simple[op] = ('HF', hfmap)
            else:
                self.mapping.unknown.append(('simple', op, 'handler could not be classified'))

    def classify_hot(self):
        for op, body in self.L.hot_chain:
            names = self.find(self.run_block(body), domain_key=('op', op), skip=())
            nm = self.pick(names, 'hot', op)
            if nm is not None:
                self.mapping.hot[op] = nm

    def classify_ctrl(self):
        for op, body in self.L.ctrl_chain:
            st = self.structural(body)
            if st is not None:
                self.mapping.ctrl[op] = st
                continue
            names = self.find(self.run_ctrl(op), domain_key=('op', op), skip=self.SKIP)
            nm = self.pick(names, 'ctrl', op)
            if nm is not None:
                self.mapping.ctrl[op] = nm

    def classify_ded(self):
        L = self.L
        for ti in range(1, 7):
            for key, fn in L.ded_decl[ti]:
                self.classify_one_ded(ti, key, fn)

    def classify_one_ded(self, ti, key, fn):
        body = fn['body']['body']
        if ti == 1:
            names = self.find(self.run_whole(1, fn), skip=())
            nm = self.pick(names, 'ded%d' % ti, key)
            if nm is not None:
                self.mapping.ded[(ti, key)] = [(nm, 0, {}, False)]
            return
        if any(s['type'] == 'AstStatWhile' for s in body):
            self.classify_loop_ded(ti, key, fn)
            return
        blocks = [s for s in body if s['type'] == 'AstStatBlock']
        tail = [s for s in body if s['type'] != 'AstStatBlock']
        if not blocks:
            self.mapping.unknown.append(('ded%d' % ti, key, 'no micro-operations'))
            return
        if any(s['type'] not in ('AstStatReturn',) for s in tail):
            self.mapping.unknown.append(('ded%d' % ti, key, 'unexpected statements in handler body'))
            return
        micro = []
        for k, blk in enumerate(blocks):
            part = None
            for n, p in walk(blk):
                if n['type'] == 'AstExprIndexExpr' and unparen(n['index'])['type'] == 'AstExprConstantNumber':
                    v = unparen(n['index'])['value']
                    if v >= 11 and loc_of(n['expr']) is not None and loc_of(n['expr'])['location'] == fn['args'][0]['location']:
                        part = int(v) - 11
            if part is None:
                self.mapping.unknown.append(('ded%d' % ti, key, 'micro-operation %d reads no record part' % k))
                return
            last = (k == len(blocks) - 1)
            stmts = [blk] + (tail if last else [])
            runner = self.run_micro(ti, fn, stmts)
            names = self.find(runner, domain_key=('ded', ti, key, part), skip=self.SKIP)
            nm = self.pick(names, 'ded%d/%d.%d' % (ti, key, k), key)
            if nm is None:
                return
            ir, fixed = _strip_fixed(nm)
            micro.append((ir, part, fixed, SPECS[nm].kind == 'ctrl'))
        self.mapping.ded[(ti, key)] = micro

    def run(self, simple_ops):
        self.classify_simple(simple_ops)
        self.classify_hot()
        self.classify_ctrl()
        self.classify_ded()
        return self.mapping

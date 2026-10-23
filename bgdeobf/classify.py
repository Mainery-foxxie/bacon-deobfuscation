"""Behavioural classification of the handlers of a Bacon Guard build.

The loader (loader.py) tells where every handler lives; this module runs each handler in the AST interpreter on many
pseudo-random machine states and finds the reference operation (refspec.py) that behaves identically.
"""
import time

from .astutil import walk, lkey, unparen
from .loader import LayoutError, loc_of, is_in
from .luavm import (Interp, Func, HostFunc, LuaTable, LuaError, StepLimit, Unsupported, _Return, _Env, LOG)
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
            'PROF': st.prof, 'VARARGS': LuaTable(), 'NVAR': 0, 'PC': 1,
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
        wt = LuaTable()
        for op, expr in L.wtab_items:
            fn = self._func_node(expr)
            if fn is not None:
                wt.set(op, Func(fn, self.interp, cells))
        wl = self.loc.get('WTAB')
        if wl is not None:
            cells[wl] = [wt]
        return cells

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
             'ALT': st.alt, 'CLR': st.clr, 'VARARGS': LuaTable(), 'NVAR': 0, 'PROF': st.prof,
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

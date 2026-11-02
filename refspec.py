"""Reference semantics of the VM operations (Luau bytecode meaning) used to recognise handler code by behaviour.

A handler is accepted as implementing a reference operation when, on many pseudo-random machine states, both produce
the same observable result (registers, upvalue boxes, effects on opaque values, next-record choice, returned values).
"""
import random

from .luavm import LuaTable, U, mkU, LOG, kof, arith, compare, truthy, lua_eq, lua_type, HostFunc, num, LuaError

NXT0, ALT0 = 1000, 2000
NREG = 24
PLAIN = [None, False, True, 0, 1, 2, 'x']


class State(object):
    def __init__(self, spec, seed, domain=None, variant=None, trial=0):
        rng = random.Random(seed)
        self.rng = rng
        self.trial = trial
        self.spec = spec
        self.R = LuaTable()
        for i in range(NREG):
            self.R.set(i, mkU(('r', i)))
        self.UP = LuaTable()
        for i in range(12):
            cont = LuaTable()
            cont.set(100 + i, mkU(('uv', i)))
            box = LuaTable()
            box.set(1, cont)
            box.set(2, 100 + i)
            self.UP.set(i, box)
        regs = rng.sample(range(0, 12), 3)
        self.f = dict(op=0, ef=rng.randrange(0, 256), A=regs[0], B=regs[1], C=regs[2], RF=rng.randrange(0, 40),
                      HF=rng.randrange(0, 2 ** 32), AUX=rng.randrange(0, 40))
        self.top = rng.randrange(3, 12)
        self.tag = mkU(('tmpl-tag',))
        self.nxt, self.alt = NXT0, ALT0
        self.rec = LuaTable()
        self.clr = bool(rng.randrange(0, 2))
        self.prof = LuaTable()
        for i in (1, 2, 3):
            self.prof.set(i, rng.randrange(0, 4))
        self.tmpl = None
        self.ret = None
        # the function's extra arguments (a separate generator: the draws above must not change)
        vr = random.Random(seed ^ 0x5BD1E995)
        self.nvar = vr.randrange(0, 5)
        self.varargs = LuaTable()
        for i in range(self.nvar):
            self.varargs.set(i + 1, mkU(('va', i)))
        self.iter_nil = False
        self.fixed = set()
        if domain:
            d = rng.choice(domain)
            for k, v in d.items():
                self.f[k] = v
                self.fixed.add(k)
        if variant:
            for k, v in variant.items():
                self.f[k] = v
                self.fixed.add(k)
        if spec is not None and spec.prep is not None:
            spec.prep(self)
        self.fill_rec()

    def cyc(self, pool, salt=0):
        return pool[(self.trial + salt) % len(pool)]

    def setf(self, k, v):
        if k not in self.fixed:
            self.f[k] = v

    def fill_rec(self):
        for i in range(8):
            part = LuaTable()
            vals = [0, self.f['ef'], self.f['A'], self.f['B'], self.f['C'], self.f['RF'], self.f['HF'], self.f['AUX'], NXT0, ALT0]
            for j, v in enumerate(vals):
                part.set(j + 1, v)
            self.rec.set(11 + i, part)

    def kget(self, i):
        if self.tmpl is not None and i == self.f['RF'] and self.spec is not None and self.spec.name in ('DUPTABLE', 'NEWTABLE'):
            return [self.tmpl]
        return [mkU(('K', i))]

    def imp(self, i):
        return [mkU(('imp', i))]

    def fused(self, part):
        if not isinstance(part, LuaTable):
            raise LuaError('bad part')
        return [part.get(k) for k in (3, 4, 5, 6, 7, 8)]

    def snapshot(self, observe):
        out = []
        if 'R' in observe:
            out.append(kof(self.R))
        if 'up' in observe:
            out.append(kof(self.UP))
        if 'top' in observe:
            out.append(self.top)
        if 'nxt' in observe:
            out.append(self.nxt)
        if 'ret' in observe:
            out.append(None if self.ret is None else tuple(kof(v) for v in self.ret))
        if 'rec' in observe:
            out.append(kof(self.rec))
        if 'log' in observe:
            out.append(tuple(LOG))
        return tuple(out)


class Spec(object):
    def __init__(self, name, run, prep=None, observe=('R', 'up', 'log', 'nxt'), kind='data', rule=None, top=False,
                 domain_fields=(), normalize=None):
        self.normalize = normalize
        self.name = name
        self.run = run
        self.prep = prep
        self.observe = tuple(observe) + (('top',) if top else ())
        self.kind = kind
        self.rule = rule
        self.domain_fields = domain_fields


SPECS = {}


def spec(name, **kw):
    def deco(fn):
        prep = kw.pop('prep', None)
        SPECS[name] = Spec(name, fn, prep=prep, **kw)
        return fn
    return deco


def R(st, i):
    return st.R.get(i)


def setR(st, i, v):
    st.R.set(i, v)


def F(st, k):
    return st.f[k]


# ---------------------------------------------------------------------------------------------------- data ops
@spec('NOP')
def _nop(st):
    pass


@spec('MOVE', prep=lambda st: _truthy_prep('B')(st))
def _move(st):
    setR(st, F(st, 'A'), R(st, F(st, 'B')))


@spec('LOADNIL')
def _loadnil(st):
    setR(st, F(st, 'A'), None)


@spec('LOADN')
def _loadn(st):
    setR(st, F(st, 'A'), F(st, 'RF'))


@spec('LOADK')
def _loadk(st):
    setR(st, F(st, 'A'), st.kget(F(st, 'RF'))[0])


@spec('GETIMPORT')
def _getimport(st):
    setR(st, F(st, 'A'), st.imp(F(st, 'AUX'))[0])


@spec('GETUPVAL')
def _getupval(st):
    box = st.UP.get(F(st, 'B'))
    setR(st, F(st, 'A'), box.get(1).get(box.get(2)))


@spec('SETUPVAL')
def _setupval(st):
    box = st.UP.get(F(st, 'B'))
    box.get(1).set(box.get(2), R(st, F(st, 'A')))


def _idx(o, k):
    if isinstance(o, U):
        return mkU(('idx', o.key, kof(k)))
    return o.get(k)


def _setidx(o, k, v):
    if isinstance(o, U):
        LOG.append(('set', o.key, kof(k), kof(v)))
    else:
        o.set(k, v)


@spec('GETTABLEKS')
def _gtks(st):
    setR(st, F(st, 'A'), _idx(R(st, F(st, 'B')), st.kget(F(st, 'AUX'))[0]))


@spec('SETTABLEKS')
def _stks(st):
    _setidx(R(st, F(st, 'B')), st.kget(F(st, 'AUX'))[0], R(st, F(st, 'A')))


@spec('GETTABLE')
def _gt(st):
    setR(st, F(st, 'A'), _idx(R(st, F(st, 'B')), R(st, F(st, 'C'))))


@spec('SETTABLE')
def _st(st):
    _setidx(R(st, F(st, 'B')), R(st, F(st, 'C')), R(st, F(st, 'A')))


@spec('GETTABLEN')
def _gtn(st):
    setR(st, F(st, 'A'), _idx(R(st, F(st, 'B')), F(st, 'C') + 1))


@spec('SETTABLEN')
def _stn(st):
    _setidx(R(st, F(st, 'B')), F(st, 'C') + 1, R(st, F(st, 'A')))


@spec('SELF')
def _self(st):
    o = R(st, F(st, 'B'))
    setR(st, F(st, 'A') + 1, o)
    setR(st, F(st, 'A'), _idx(o, st.kget(F(st, 'AUX'))[0]))


def _mk_arith(name, op):
    def run(st):
        setR(st, F(st, 'A'), arith(op, R(st, F(st, 'B')), R(st, F(st, 'C'))))
    SPECS[name] = Spec(name, run)


for _n, _o in (('ADD', 'Add'), ('SUB', 'Sub'), ('MUL', 'Mul'), ('DIV', 'Div'), ('MOD', 'Mod'), ('POW', 'Pow'), ('IDIV', 'FloorDiv')):
    _mk_arith(_n, _o)


def _mk_arithk(name, op):
    def run(st):
        setR(st, F(st, 'A'), arith(op, R(st, F(st, 'B')), st.kget(F(st, 'C'))[0]))
    SPECS[name] = Spec(name, run)


for _n, _o in (('ADDK', 'Add'), ('SUBK', 'Sub'), ('MULK', 'Mul'), ('DIVK', 'Div'), ('MODK', 'Mod'), ('POWK', 'Pow'), ('IDIVK', 'FloorDiv')):
    _mk_arithk(_n, _o)


def _mk_arithrk(name, op):
    def run(st):
        setR(st, F(st, 'A'), arith(op, st.kget(F(st, 'B'))[0], R(st, F(st, 'C'))))
    SPECS[name] = Spec(name, run)


for _n, _o in (('SUBRK', 'Sub'), ('DIVRK', 'Div')):
    _mk_arithrk(_n, _o)


def _truthy_prep(*slots):
    def prep(st):
        pool = [None, False, True, 0, 1, 'x', mkU(('t', 1)), mkU(('t', 2))]
        for i, s in enumerate(slots):
            st.R.set(st.f[s], st.cyc(pool, 3 * i))
    return prep


def _mk_logic(name, is_or, const):
    def run(st):
        b = R(st, F(st, 'B'))
        other = st.kget(F(st, 'C'))[0] if const else R(st, F(st, 'C'))
        if is_or:
            setR(st, F(st, 'A'), b if truthy(b) else other)
        else:
            setR(st, F(st, 'A'), other if truthy(b) else b)
    SPECS[name] = Spec(name, run, prep=_truthy_prep('B'))


for _n, _a, _b in (('OR', True, False), ('ORK', True, True), ('AND', False, False), ('ANDK', False, True)):
    _mk_logic(_n, _a, _b)


@spec('NOT', prep=_truthy_prep('B'))
def _not(st):
    setR(st, F(st, 'A'), not truthy(R(st, F(st, 'B'))))


@spec('MINUS')
def _minus(st):
    v = R(st, F(st, 'B'))
    if isinstance(v, bool):
        raise LuaError('attempt to perform arithmetic on a boolean value')
    setR(st, F(st, 'A'), mkU(('neg', v.key)) if isinstance(v, U) else -v)


@spec('LENGTH')
def _len(st):
    v = R(st, F(st, 'B'))
    setR(st, F(st, 'A'), mkU(('len', v.key)))


def _concat_prep(st):
    f = st.f
    st.setf('B', st.rng.randrange(0, 6))
    st.setf('C', f['B'] + st.rng.randrange(1, 5))
    st.setf('A', st.rng.randrange(0, 20))
    if st.rng.randrange(0, 2):
        for i in range(f['B'], f['C'] + 1):
            st.R.set(i, st.rng.choice(['a', 'bc', 7, 'd']))


@spec('CONCAT', prep=_concat_prep)
def _concat(st):
    vals = [R(st, i) for i in range(F(st, 'B'), F(st, 'C') + 1)]
    if all(isinstance(v, (str, int, float)) and not isinstance(v, bool) for v in vals):
        from .luavm import tostr
        setR(st, F(st, 'A'), ''.join(tostr(v) for v in vals))
    else:
        acc = vals[0]
        for v in vals[1:]:
            acc = mkU(('concat', kof(acc), kof(v)))
        setR(st, F(st, 'A'), acc)


@spec('NEWTABLE', prep=lambda st: _duptable_prep(st))
def _newtable(st):
    setR(st, F(st, 'A'), LuaTable())


def _duptable_prep(st):
    tag = st.tag
    items = LuaTable()
    n = st.rng.randrange(1, 4)
    for i in range(n):
        pair = LuaTable()
        pair.set(1, 200 + i)
        pair.set(2, 300 + i if st.rng.randrange(0, 3) else -1)
        items.set(i + 1, pair)
    t = LuaTable()
    t.set(1, tag)
    t.set(2, items)
    st.tmpl = t


@spec('DUPTABLE', prep=_duptable_prep)
def _duptable(st):
    t = LuaTable()
    for k, pair in st.tmpl.get(2).items():
        if pair.get(2) >= 0:
            t.set(st.kget(pair.get(1))[0], st.kget(pair.get(2))[0])
    setR(st, F(st, 'A'), t)


# ---------------------------------------------------------------------------------------------------- control ops
def _jump(st):
    st.nxt = st.alt


def _jump_prep(st):
    pool = [None, False, True, 0, 1, 5, 'x', mkU(('j', 1)), mkU(('j', 2))]
    for i, r in enumerate((st.f['A'], st.f['B'], st.f['C'], st.f['AUX'])):
        st.R.set(r, st.cyc(pool, 2 * i))
    if st.trial % 3 == 0:
        st.R.set(st.f['AUX'], st.R.get(st.f['A']))


@spec('JUMP', prep=_jump_prep, observe=('R', 'up', 'log', 'nxt'), kind='ctrl')
def _j_(st):
    _jump(st)


def _cond_prep(st):
    st.R.set(st.f['A'], st.cyc([None, False, True, 0, 1, 'x', mkU(('t', 1))]))


@spec('JUMPIF', prep=_cond_prep, kind='ctrl')
def _jumpif(st):
    if truthy(R(st, F(st, 'A'))):
        _jump(st)


@spec('JUMPIFNOT', prep=_cond_prep, kind='ctrl')
def _jumpifnot(st):
    if not truthy(R(st, F(st, 'A'))):
        _jump(st)


def _cmp_prep(numeric):
    def prep(st):
        st.setf('AUX', st.rng.randrange(12, 20))
        a = st.f['A']
        b = st.f['AUX']
        if numeric:
            st.R.set(a, st.trial % 4)
            st.R.set(b, (st.trial // 4 + st.trial // 2) % 4)
        else:
            pool = [None, False, 1, 2, 'x', mkU(('t', 1)), mkU(('t', 2))]
            st.R.set(a, st.cyc(pool))
            st.R.set(b, st.cyc(pool, 3) if st.trial % 2 else st.R.get(a))
    return prep


def _mk_cmp(name, op, neg, numeric):
    def run(st):
        a, b = R(st, F(st, 'A')), R(st, F(st, 'AUX'))
        r = compare(op, a, b)
        if neg:
            r = not r
        if r:
            _jump(st)
    SPECS[name] = Spec(name, run, prep=_cmp_prep(numeric), kind='ctrl')


for _n, _op, _neg, _num in (('JUMPIFEQ', 'CompareEq', False, False), ('JUMPIFNOTEQ', 'CompareNe', False, False),
                            ('JUMPIFLE', 'CompareLe', False, True), ('JUMPIFLT', 'CompareLt', False, True),
                            ('JUMPIFNOTLT', 'CompareLt', True, True), ('JUMPIFNOTLE', 'CompareLe', True, True)):
    _mk_cmp(_n, _op, _neg, _num)


def _xeqk_prep(st):
    idx = st.rng.randrange(0, 30)
    flag = st.trial % 2
    st.setf('AUX', (flag << 31) | idx)
    pool = [None, False, True, 1, 2, 'x', mkU(('K', idx)), mkU(('K', idx))]
    st.R.set(st.f['A'], st.cyc(pool, st.trial // 2))


@spec('JUMPXEQK', prep=_xeqk_prep, kind='ctrl')
def _jxeqk(st):
    k = st.kget(F(st, 'AUX') & 0xFFFFFF)[0]
    r = lua_eq(R(st, F(st, 'A')), k) if not isinstance(R(st, F(st, 'A')), U) else (R(st, F(st, 'A')) is k)
    if r != bool((F(st, 'AUX') >> 31) & 1):
        _jump(st)


def _xeqkb_prep(st):
    st.setf('AUX', ((st.trial % 2) << 31) | ((st.trial // 2) % 2))
    st.R.set(st.f['A'], st.cyc([None, False, True, 1, mkU(('t', 1))], st.trial // 2))


@spec('JUMPXEQKB', prep=_xeqkb_prep, kind='ctrl')
def _jxeqkb(st):
    v = R(st, F(st, 'A'))
    r = isinstance(v, bool) and v == bool(F(st, 'AUX') & 1)
    if r != bool((F(st, 'AUX') >> 31) & 1):
        _jump(st)


def _xeqknil_prep(st):
    st.setf('AUX', (st.trial % 2) << 31)
    st.R.set(st.f['A'], st.cyc([None, False, True, 1, mkU(('t', 1))], st.trial // 2))


@spec('JUMPXEQKNIL', prep=_xeqknil_prep, kind='ctrl')
def _jxeqknil(st):
    r = R(st, F(st, 'A')) is None
    if r != bool((F(st, 'AUX') >> 31) & 1):
        _jump(st)


def _loadb_prep(st):
    st.setf('B', st.rng.randrange(0, 2))
    st.setf('C', st.rng.choice([0, 0, st.rng.randrange(1, 9)]))


@spec('LOADB', prep=_loadb_prep, kind='ctrl')
def _loadb(st):
    setR(st, F(st, 'A'), F(st, 'B') == 1)
    if F(st, 'C') != 0:
        _jump(st)


def _forn_prep(st):
    st.setf('A', st.rng.randrange(0, 10))
    A = st.f['A']
    lim = st.rng.randrange(0, 8)
    step = st.rng.choice([1, 1, 2, -1, -2, 0.5, 3])
    idx = st.rng.randrange(-3, 9)
    st.R.set(A, lim)
    st.R.set(A + 1, step)
    st.R.set(A + 2, idx)


@spec('FORNPREP', prep=_forn_prep, kind='ctrl')
def _fornprep(st):
    A = F(st, 'A')
    lim, step, idx = R(st, A), R(st, A + 1), R(st, A + 2)
    if (step > 0 and idx > lim) or (step <= 0 and idx < lim):
        _jump(st)


@spec('FORNLOOP', prep=_forn_prep, kind='ctrl')
def _fornloop(st):
    A = F(st, 'A')
    lim, step = R(st, A), R(st, A + 1)
    idx = num(R(st, A + 2) + step)
    setR(st, A + 2, idx)
    if (step > 0 and idx <= lim) or (step <= 0 and idx >= lim):
        _jump(st)


def _forgprep_prep(st):
    st.bad_iter = st.rng.randrange(0, 4) == 0
    st.R.set(st.f['A'], 5 if st.bad_iter else mkU(('iterfn',)))


@spec('FORGPREP', prep=_forgprep_prep, kind='ctrl')
def _forgprep(st):
    if st.bad_iter:
        raise LuaError('invalid iterator')
    _jump(st)


def _forgloop_prep(st):
    st.setf('A', st.rng.randrange(0, 12))
    st.setf('AUX', st.rng.choice([1, 2, 3, 2, 1]))
    st.iter_nil = st.rng.randrange(0, 3) == 0
    A = st.f['A']
    st.R.set(A, HostFunc((lambda s, c, st=st: [None] if st.iter_nil else [mkU(('it', 1)), mkU(('it', 2)), mkU(('it', 3))]), 'iter'))


@spec('FORGLOOP', prep=_forgloop_prep, kind='ctrl', observe=('R', 'up', 'log', 'nxt'))
def _forgloop(st):
    A = F(st, 'A')
    n = F(st, 'AUX') & 255
    res = [None] if st.iter_nil else [mkU(('it', 1)), mkU(('it', 2)), mkU(('it', 3))]
    for i in range(n):
        setR(st, A + 3 + i, res[i] if i < len(res) else None)
    if R(st, A + 3) is not None:
        setR(st, A + 2, R(st, A + 3))
        _jump(st)


def _call_prep(st):
    st.setf('A', st.rng.randrange(0, 8))
    A = st.f['A']
    st.setf('B', st.rng.randrange(0, 6))
    st.setf('C', st.rng.randrange(0, 5))
    st.R.set(A, mkU(('fn', A)))
    if st.f['B'] == 0:
        st.top = A + st.rng.randrange(0, 5)
    else:
        st.top = st.rng.randrange(0, 12)


def _call_norm(st):
    A, C = st.f['A'], st.f['C']
    if C != 0:
        for i in range(A + C - 1, A + 8):
            st.R.set(i, None)
        st.top = 0


@spec('CALL', prep=_call_prep, kind='data', top=True, domain_fields=('B', 'C'), normalize=_call_norm)
def _call(st):
    A, B, C = F(st, 'A'), F(st, 'B'), F(st, 'C')
    nargs = (st.top - A) if B == 0 else B - 1
    args = [R(st, A + 1 + i) for i in range(nargs)]
    f = R(st, A)
    LOG.append(('call', f.key, tuple(kof(a) for a in args)))
    res = [mkU(('call', f.key, tuple(kof(a) for a in args), i)) for i in range(4)]
    if C == 0:
        for i, v in enumerate(res):
            setR(st, A + i, v)
        st.top = A + len(res) - 1
    else:
        for i in range(C - 1):
            setR(st, A + i, res[i] if i < len(res) else None)


def _mk_callbc(b, c):
    def prep(st):
        st.setf('A', st.rng.randrange(0, 8))
        st.R.set(st.f['A'], mkU(('fn', st.f['A'])))

    def run(st):
        A = F(st, 'A')
        args = [R(st, A + 1 + i) for i in range(b - 1)]
        f = R(st, A)
        LOG.append(('call', f.key, tuple(kof(a) for a in args)))
        res = [mkU(('call', f.key, tuple(kof(a) for a in args), i)) for i in range(4)]
        for i in range(c - 1):
            setR(st, A + i, res[i])
    def norm(st, c=c):
        A = st.f['A']
        for i in range(A + c - 1, A + 8):
            st.R.set(i, None)
    SPECS['CALL_%d_%d' % (b, c)] = Spec('CALL_%d_%d' % (b, c), run, prep=prep, normalize=norm)


for _b in range(1, 7):
    for _c in range(1, 5):
        _mk_callbc(_b, _c)


def _return_prep(st):
    st.setf('A', st.rng.randrange(0, 8))
    A = st.f['A']
    st.setf('B', st.rng.randrange(0, 6))
    st.top = A + st.rng.randrange(0, 5) if st.f['B'] == 0 else st.rng.randrange(0, 12)


@spec('RETURN', prep=_return_prep, kind='ctrl', observe=('R', 'up', 'log', 'nxt', 'ret', 'rec'), domain_fields=('B',))
def _return(st):
    A, B = F(st, 'A'), F(st, 'B')
    last = st.top if B == 0 else A + B - 2
    st.ret = [R(st, i) for i in range(A, last + 1)]
    if st.clr:
        st.rec.d.clear()


def _setlist_prep(st):
    st.setf('A', st.rng.randrange(0, 6))
    A = st.f['A']
    st.setf('B', st.rng.randrange(6, 12))
    st.setf('C', st.rng.randrange(0, 5))
    st.setf('AUX', st.rng.randrange(1, 5))
    st.R.set(A, LuaTable())
    st.top = st.f['B'] + st.rng.randrange(0, 4)


@spec('SETLIST', prep=_setlist_prep, kind='data', top=False)
def _setlist(st):
    A, B, C, aux = F(st, 'A'), F(st, 'B'), F(st, 'C'), F(st, 'AUX')
    n = C - 1
    if C == 0:
        n = st.top - B + 1
    for i in range(n):
        R(st, A).set(aux + i, R(st, B + i))


def _getvarargs_prep(st):
    st.setf('A', st.rng.randrange(0, 8))
    st.setf('B', st.rng.randrange(0, 5))
    st.top = st.rng.randrange(0, 12)


@spec('GETVARARGS', prep=_getvarargs_prep, kind='data', top=True)
def _getvarargs(st):
    A, B = F(st, 'A'), F(st, 'B')
    n = B - 1
    if B == 0:
        n = st.nvar
        st.top = A + n - 1
    for i in range(n):
        setR(st, A + i, st.varargs.get(i + 1))


def _tailcall_prep(st):
    _call_prep(st)


@spec('TAILCALL', prep=_call_prep, kind='ctrl', observe=('R', 'up', 'log', 'ret', 'rec'))
def _tailcall(st):
    A, B = F(st, 'A'), F(st, 'B')
    nargs = (st.top - A) if B == 0 else B - 1
    args = [R(st, A + 1 + i) for i in range(nargs)]
    f = R(st, A)
    LOG.append(('call', f.key, tuple(kof(a) for a in args)))
    st.ret = [mkU(('call', f.key, tuple(kof(a) for a in args), i)) for i in range(4)]
    if st.clr:
        st.rec.d.clear()

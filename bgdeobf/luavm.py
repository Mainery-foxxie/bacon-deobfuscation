"""A small interpreter for the subset of Luau used by the VM handlers (works on luau-ast JSON).

Values: None, bool, int/float, str (latin1), LuaTable, Func (AST closure), HostFunc / python callable, U (opaque symbolic value).
"""
import hashlib
import math


class LuaError(Exception):
    def __init__(self, value='error'):
        Exception.__init__(self, str(value))
        self.value = value


class StepLimit(Exception):
    pass


class Unsupported(Exception):
    pass


class _Return(Exception):
    def __init__(self, values):
        self.values = values


class _Break(Exception):
    pass


class _Continue(Exception):
    pass


_INTERN = {}
LOG = []


class U(object):
    """opaque value: result of operations on opaque values, structurally interned"""
    __slots__ = ('key',)

    def __init__(self, key):
        self.key = key

    def __repr__(self):
        return 'U%s' % (self.key,)


def mkU(key):
    u = _INTERN.get(key)
    if u is None:
        u = U(key)
        _INTERN[key] = u
    return u


def kof(v):
    """structural key of any value (used to build U keys and to snapshot state)"""
    if isinstance(v, U):
        return v.key
    if v is None or isinstance(v, (bool, int, float, str)):
        if isinstance(v, float) and v.is_integer() and abs(v) < 2 ** 53:
            v = int(v)
        return ('lit', type(v).__name__, v)
    if isinstance(v, LuaTable):
        return ('tbl', tuple(sorted(((kof(k), kof(x)) for k, x in v.d.items()), key=repr)))
    if isinstance(v, (Func, HostFunc)):
        return ('fn', getattr(v, 'name', id(v)))
    return ('?', repr(v))


def _bit(x):
    h = hashlib.md5(repr(x).encode('latin1', 'replace')).digest()
    return h[0] & 1


class LuaTable(object):
    __slots__ = ('d', 'mt')

    def __init__(self, d=None):
        self.d = d if d is not None else {}
        self.mt = None

    @staticmethod
    def nk(k):
        if isinstance(k, bool):
            return ('bool', k)
        if isinstance(k, float) and k.is_integer():
            return int(k)
        return k

    def get(self, k):
        return self.d.get(self.nk(k))

    def set(self, k, v):
        k = self.nk(k)
        if k is None:
            raise LuaError('index is nil')
        if v is None:
            self.d.pop(k, None)
        else:
            self.d[k] = v

    def length(self):
        n = 0
        while (n + 1) in self.d:
            n += 1
        return n

    def items(self):
        arr = []
        n = 1
        while n in self.d:
            arr.append((n, self.d[n]))
            n += 1
        rest = [(k, v) for k, v in self.d.items() if not (isinstance(k, int) and not isinstance(k, bool) and 1 <= k < n)]
        rest.sort(key=lambda kv: repr(kv[0]))
        return arr + [((k[1] if isinstance(k, tuple) and k and k[0] == 'bool' else k), v) for k, v in rest]


class HostFunc(object):
    def __init__(self, fn, name='host'):
        self.fn = fn
        self.name = name

    def __call__(self, *args):
        return self.fn(*args)


class Func(object):
    def __init__(self, node, interp, cells):
        self.node = node
        self.interp = interp
        self.cells = cells
        self.name = node['location']


def num(x):
    if isinstance(x, float) and x.is_integer() and abs(x) < 2 ** 53:
        return int(x)
    return x


def lua_type(v):
    if v is None:
        return 'nil'
    if isinstance(v, bool):
        return 'boolean'
    if isinstance(v, (int, float)):
        return 'number'
    if isinstance(v, str):
        return 'string'
    if isinstance(v, LuaTable):
        return 'table'
    if isinstance(v, U):
        return 'function'
    return 'function'


def truthy(v):
    return not (v is None or v is False)


def tostr(v):
    if isinstance(v, str):
        return v
    if isinstance(v, bool):
        return 'true' if v else 'false'
    if v is None:
        return 'nil'
    if isinstance(v, (int, float)):
        v = num(v)
        if isinstance(v, int):
            return str(v)
        return '%.14g' % v
    return '<%s>' % lua_type(v)


def arith(op, a, b):
    if isinstance(a, U) or isinstance(b, U):
        return mkU((op, kof(a), kof(b)))
    if isinstance(a, str):
        try:
            a = float(a)
        except ValueError:
            raise LuaError('attempt to perform arithmetic on a string value')
    if isinstance(b, str):
        try:
            b = float(b)
        except ValueError:
            raise LuaError('attempt to perform arithmetic on a string value')
    if isinstance(a, bool) or isinstance(b, bool) or not isinstance(a, (int, float)) or not isinstance(b, (int, float)):
        raise LuaError('attempt to perform arithmetic on a %s value' % lua_type(a if not isinstance(a, (int, float)) or isinstance(a, bool) else b))
    if op == 'Add':
        return num(a + b)
    if op == 'Sub':
        return num(a - b)
    if op == 'Mul':
        return num(a * b)
    if op == 'Div':
        if b == 0:
            return math.nan if a == 0 else (math.inf if (a > 0) == (math.copysign(1, b) > 0) else -math.inf)
        return num(a / b)
    if op == 'FloorDiv':
        if b == 0:
            return math.nan if a == 0 else (math.inf if (a > 0) == (math.copysign(1, b) > 0) else -math.inf)
        return num(math.floor(a / b)) if isinstance(a / b, float) and not (isinstance(a, int) and isinstance(b, int)) else num(a // b)
    if op == 'Mod':
        if b == 0:
            return math.nan
        return num(a - math.floor(a / b) * b) if not (isinstance(a, int) and isinstance(b, int)) else a % b
    if op == 'Pow':
        try:
            return num(float(a) ** float(b))
        except (OverflowError, ZeroDivisionError):
            return math.inf
    raise Unsupported(op)


def compare(op, a, b):
    if isinstance(a, U) or isinstance(b, U):
        if op == 'CompareEq':
            return a is b
        if op == 'CompareNe':
            return a is not b
        if op == 'CompareGt':
            op, a, b = 'CompareLt', b, a
        elif op == 'CompareGe':
            op, a, b = 'CompareLe', b, a
        return bool(_bit((op, kof(a), kof(b))))
    if op == 'CompareEq':
        return lua_eq(a, b)
    if op == 'CompareNe':
        return not lua_eq(a, b)
    if op == 'CompareGt':
        op, a, b = 'CompareLt', b, a
    elif op == 'CompareGe':
        op, a, b = 'CompareLe', b, a
    ok = (isinstance(a, (int, float)) and not isinstance(a, bool) and isinstance(b, (int, float)) and not isinstance(b, bool)) or (isinstance(a, str) and isinstance(b, str))
    if not ok:
        raise LuaError('attempt to compare %s with %s' % (lua_type(a), lua_type(b)))
    return a < b if op == 'CompareLt' else a <= b


def lua_eq(a, b):
    if a is None or b is None:
        return a is b
    if isinstance(a, bool) or isinstance(b, bool):
        return isinstance(a, bool) and isinstance(b, bool) and a == b
    if isinstance(a, (int, float)) and isinstance(b, (int, float)):
        return a == b
    if isinstance(a, str) and isinstance(b, str):
        return a == b
    return a is b


class Interp(object):
    def __init__(self, max_steps=200000):
        self.max_steps = max_steps
        self.steps = 0
        self.unbound = None
        self.globals = LuaTable()
        self._install_builtins()

    # ---------------------------------------------------------------- builtins
    def _install_builtins(self):
        G = self.globals

        def H(name, fn):
            return HostFunc(fn, name)

        def table_create(n=0, v=None):
            return [LuaTable()]

        def table_unpack(t, i=1, j=None):
            if not isinstance(t, LuaTable):
                raise LuaError('table expected')
            i = 1 if i is None else int(i)
            j = t.length() if j is None else int(j)
            if j - i >= 8000:
                raise LuaError('too many results to unpack')
            return [t.get(k) for k in range(i, j + 1)]

        def table_pack(*a):
            t = LuaTable()
            for i, v in enumerate(a):
                t.set(i + 1, v)
            t.set('n', len(a))
            return [t]

        def table_clear(t):
            t.d.clear()
            return []

        def table_move(a1, f, e, t, a2=None):
            if a2 is None:
                a2 = a1
            f, e, t = int(f), int(e), int(t)
            if e >= f:
                if t > e or t <= f or a1 is not a2:
                    for i in range(0, e - f + 1):
                        a2.set(t + i, a1.get(f + i))
                else:
                    for i in range(e - f, -1, -1):
                        a2.set(t + i, a1.get(f + i))
            return [a2]

        def table_concat(t, sep='', i=1, j=None):
            sep = '' if sep is None else sep
            i = 1 if i is None else int(i)
            j = t.length() if j is None else int(j)
            out = []
            for k in range(i, j + 1):
                v = t.get(k)
                if isinstance(v, bool) or not isinstance(v, (str, int, float)):
                    raise LuaError("invalid value (at index %d) in table for 'concat'" % k)
                out.append(tostr(v))
            return [sep.join(out)]

        def table_insert(t, a, b=None):
            if b is None:
                t.set(t.length() + 1, a)
            else:
                n = t.length()
                for k in range(n, int(a) - 1, -1):
                    t.set(k + 1, t.get(k))
                t.set(a, b)
            return []

        T = LuaTable()
        for k, f in (('create', table_create), ('unpack', table_unpack), ('pack', table_pack), ('clear', table_clear),
                     ('move', table_move), ('concat', table_concat), ('insert', table_insert)):
            T.set(k, H('table.' + k, f))
        G.set('table', T)
        G.set('unpack', T.get('unpack'))

        M32 = 0xFFFFFFFF

        def u32(x):
            return int(x) & M32

        def band(*a):
            r = M32
            for x in a:
                r &= u32(x)
            return [r]

        def bor(*a):
            r = 0
            for x in a:
                r |= u32(x)
            return [r]

        def bxor(*a):
            r = 0
            for x in a:
                r ^= u32(x)
            return [r]

        def bnot(x):
            return [(~u32(x)) & M32]

        def lshift(x, n):
            n = int(n)
            if n >= 32 or n <= -32:
                return [0]
            if n < 0:
                return [u32(x) >> (-n)]
            return [(u32(x) << n) & M32]

        def rshift(x, n):
            n = int(n)
            if n >= 32 or n <= -32:
                return [0]
            if n < 0:
                return [(u32(x) << (-n)) & M32]
            return [u32(x) >> n]

        def arshift(x, n):
            x = u32(x)
            n = int(n)
            if x & 0x80000000:
                x -= 1 << 32
            return [(x >> min(n, 31)) & M32]

        def lrotate(x, n):
            x = u32(x)
            n = int(n) & 31
            return [((x << n) | (x >> (32 - n))) & M32 if n else x]

        def rrotate(x, n):
            x = u32(x)
            n = int(n) & 31
            return [((x >> n) | (x << (32 - n))) & M32 if n else x]

        def extract(x, f, w=1):
            x, f, w = u32(x), int(f), int(w)
            if f < 0 or w <= 0 or f + w > 32:
                raise LuaError('invalid field/width')
            return [(x >> f) & ((1 << w) - 1)]

        B = LuaTable()
        for k, f in (('band', band), ('bor', bor), ('bxor', bxor), ('bnot', bnot), ('lshift', lshift), ('rshift', rshift),
                     ('arshift', arshift), ('lrotate', lrotate), ('rrotate', rrotate), ('extract', extract)):
            B.set(k, H('bit32.' + k, f))
        G.set('bit32', B)

        def s_byte(s, i=1, j=None):
            i = 1 if i is None else int(i)
            j = i if j is None else int(j)
            if i < 1:
                i = 1
            return [ord(s[k - 1]) for k in range(i, min(j, len(s)) + 1)]

        def s_char(*a):
            return [''.join(chr(int(x)) for x in a)]

        def s_sub(s, i, j=None):
            n = len(s)
            i = int(i)
            j = n if j is None else int(j)
            if i < 0:
                i = max(n + i + 1, 1)
            if j < 0:
                j = n + j + 1
            if i < 1:
                i = 1
            return [s[i - 1:j]]

        S = LuaTable()
        S.set('byte', H('string.byte', s_byte))
        S.set('char', H('string.char', s_char))
        S.set('sub', H('string.sub', s_sub))
        G.set('string', S)

        Mt = LuaTable()
        Mt.set('floor', H('math.floor', lambda x: [num(math.floor(x))]))
        Mt.set('ceil', H('math.ceil', lambda x: [num(math.ceil(x))]))
        Mt.set('max', H('math.max', lambda *a: [max(a)]))
        Mt.set('min', H('math.min', lambda *a: [min(a)]))
        Mt.set('abs', H('math.abs', lambda x: [abs(x)]))
        Mt.set('huge', math.inf)
        G.set('math', Mt)

        G.set('type', H('type', lambda v=None: [lua_type(v)]))

        def err(msg=None, level=None):
            raise LuaError(msg)

        G.set('error', H('error', err))

        def pcall(f, *a):
            try:
                return [True] + list(self.call(f, list(a)))
            except LuaError as e:
                return [False, e.value]
            except (TypeError, AttributeError, KeyError, ValueError) as e:
                return [False, str(e)]

        G.set('pcall', H('pcall', pcall))

        def select(n, *a):
            if n == '#':
                return [len(a)]
            n = int(n)
            if n < 0:
                n = len(a) + n + 1
            return list(a[n - 1:])

        G.set('select', H('select', select))

        def tonumber(v=None, base=None):
            if isinstance(v, bool):
                return [None]
            if isinstance(v, (int, float)):
                return [v]
            if isinstance(v, str):
                try:
                    return [num(float(v))]
                except ValueError:
                    return [None]
            return [None]

        G.set('tonumber', H('tonumber', tonumber))
        G.set('tostring', H('tostring', lambda v=None: [tostr(v)]))
        G.set('getmetatable', H('getmetatable', lambda v=None: [v.mt if isinstance(v, LuaTable) else None]))

        def setmt(t, m):
            t.mt = m
            return [t]

        G.set('setmetatable', H('setmetatable', setmt))

        def lnext(t, k=None):
            items = t.items()
            if k is None:
                return list(items[0]) if items else [None]
            for i, (kk, v) in enumerate(items):
                if lua_eq(kk, k):
                    return list(items[i + 1]) if i + 1 < len(items) else [None]
            return [None]

        G.set('next', H('next', lnext))
        G.set('rawget', H('rawget', lambda t, k: [t.get(k)]))
        G.set('rawequal', H('rawequal', lambda a, b: [a is b or lua_eq(a, b)]))

        def rawset(t, k, v):
            t.set(k, v)
            return [t]

        G.set('rawset', H('rawset', rawset))

    # ---------------------------------------------------------------- calls
    def call(self, f, args):
        if isinstance(f, Func):
            return self.call_func(f, args)
        if isinstance(f, HostFunc):
            r = f.fn(*args)
            return list(r) if r is not None else []
        if isinstance(f, U):
            LOG.append(('call', f.key, tuple(kof(a) for a in args)))
            return [mkU(('call', f.key, tuple(kof(a) for a in args), i)) for i in range(4)]
        if callable(f):
            r = f(*args)
            return list(r) if r is not None else []
        raise LuaError('attempt to call a %s value' % lua_type(f))

    def call_func(self, fn, args):
        node = fn.node
        cells = fn.cells
        local = dict(cells)
        params = node['args']
        for i, p in enumerate(params):
            local[p['location']] = [args[i] if i < len(args) else None]
        va = list(args[len(params):]) if node['vararg'] else []
        env = _Env(local, va)
        try:
            self.exec_block(node['body'], env)
        except _Return as r:
            return r.values
        return []

    # ---------------------------------------------------------------- statements
    def tick(self):
        self.steps += 1
        if self.steps > self.max_steps:
            raise StepLimit()

    def exec_block(self, blk, env):
        for s in blk['body']:
            self.exec_stat(s, env)

    def exec_stat(self, s, env):
        self.tick()
        t = s['type']
        if t == 'AstStatBlock':
            self.exec_block(s, env)
        elif t == 'AstStatLocal':
            vals = self.eval_list(s['values'], env, len(s['vars']))
            for i, v in enumerate(s['vars']):
                env.cells[v['location']] = [vals[i] if i < len(vals) else None]
        elif t == 'AstStatAssign':
            vals = self.eval_list(s['values'], env, len(s['vars']))
            for i, tgt in enumerate(s['vars']):
                self.assign(tgt, vals[i] if i < len(vals) else None, env)
        elif t == 'AstStatCompoundAssign':
            cur = self.eval(s['var'], env)
            val = self.binop(s['op'], cur, self.eval(s['value'], env))
            self.assign(s['var'], val, env)
        elif t == 'AstStatExpr':
            self.eval_multi(s['expr'], env)
        elif t == 'AstStatIf':
            if truthy(self.eval(s['condition'], env)):
                self.exec_block(s['thenbody'], env)
            elif s.get('elsebody') is not None:
                e = s['elsebody']
                if e['type'] == 'AstStatIf':
                    self.exec_stat(e, env)
                else:
                    self.exec_block(e, env)
        elif t == 'AstStatReturn':
            vals = self.eval_list(s['list'], env, None)
            raise _Return(vals)
        elif t == 'AstStatFor':
            a = self.eval(s['from'], env)
            b = self.eval(s['to'], env)
            st = self.eval(s['step'], env) if s.get('step') else 1
            for x in (a, b, st):
                if isinstance(x, bool) or not isinstance(x, (int, float)):
                    raise LuaError("'for' initial value must be a number")
            if st == 0:
                raise LuaError("'for' step is zero")
            i = a
            vloc = s['var']['location']
            while (st > 0 and i <= b) or (st < 0 and i >= b):
                self.tick()
                env.cells[vloc] = [i]
                try:
                    self.exec_block(s['body'], env)
                except _Break:
                    break
                except _Continue:
                    pass
                i = num(i + st)
        elif t == 'AstStatForIn':
            vals = list(self.eval_list(s['values'], env, 3))
            vals += [None] * (3 - len(vals))
            f, st, ctl = vals[0], vals[1], vals[2]
            if isinstance(f, LuaTable):
                items = f.items()
                for k, v in items:
                    self.tick()
                    for j, var in enumerate(s['vars']):
                        env.cells[var['location']] = [(k, v)[j] if j < 2 else None]
                    try:
                        self.exec_block(s['body'], env)
                    except _Break:
                        break
                    except _Continue:
                        pass
            else:
                while True:
                    self.tick()
                    rs = self.call(f, [st, ctl])
                    if not rs or rs[0] is None:
                        break
                    ctl = rs[0]
                    for j, var in enumerate(s['vars']):
                        env.cells[var['location']] = [rs[j] if j < len(rs) else None]
                    try:
                        self.exec_block(s['body'], env)
                    except _Break:
                        break
                    except _Continue:
                        pass
        elif t == 'AstStatWhile':
            while truthy(self.eval(s['condition'], env)):
                self.tick()
                try:
                    self.exec_block(s['body'], env)
                except _Break:
                    break
                except _Continue:
                    pass
        elif t == 'AstStatRepeat':
            while True:
                self.tick()
                try:
                    self.exec_block(s['body'], env)
                except _Break:
                    break
                except _Continue:
                    pass
                if truthy(self.eval(s['condition'], env)):
                    break
        elif t == 'AstStatBreak':
            raise _Break()
        elif t == 'AstStatContinue':
            raise _Continue()
        elif t == 'AstStatLocalFunction':
            cell = [None]
            env.cells[s['name']['location']] = cell
            cell[0] = Func(s['func'], self, env.cells)
        elif t == 'AstStatFunction':
            self.assign(s['name'], Func(s['func'], self, env.cells), env)
        elif t in ('AstStatTypeAlias', 'AstStatTypeFunction'):
            pass
        else:
            raise Unsupported(t)

    def assign(self, tgt, val, env):
        t = tgt['type']
        if t == 'AstExprLocal':
            loc = tgt['local']['location']
            c = env.cells.get(loc)
            if c is None:
                env.cells[loc] = [val]
            else:
                c[0] = val
        elif t == 'AstExprGlobal':
            self.globals.set(tgt['global'], val)
        elif t == 'AstExprIndexName':
            self.setindex(self.eval(tgt['expr'], env), tgt['index'], val)
        elif t == 'AstExprIndexExpr':
            o = self.eval(tgt['expr'], env)
            k = self.eval(tgt['index'], env)
            self.setindex(o, k, val)
        else:
            raise Unsupported('assign ' + t)

    def setindex(self, o, k, v):
        if isinstance(o, LuaTable):
            if isinstance(k, float) and k != k:
                raise LuaError('index is NaN')
            o.set(k, v)
        elif isinstance(o, U):
            LOG.append(('set', o.key, kof(k), kof(v)))
        else:
            raise LuaError('attempt to index a %s value' % lua_type(o))

    def index(self, o, k):
        if isinstance(o, LuaTable):
            return o.get(k)
        if isinstance(o, U):
            return mkU(('idx', o.key, kof(k)))
        if isinstance(o, str):
            S = self.globals.get('string')
            return S.get(k)
        raise LuaError('attempt to index a %s value' % lua_type(o))

    # ---------------------------------------------------------------- expressions
    def eval_list(self, exprs, env, want):
        out = []
        n = len(exprs)
        for i, e in enumerate(exprs):
            if i == n - 1:
                out.extend(self.eval_multi(e, env))
            else:
                out.append(self.eval(e, env))
        return out

    def eval_multi(self, e, env):
        t = e['type']
        if t == 'AstExprCall':
            return self.eval_call(e, env)
        if t == 'AstExprVarargs':
            return list(env.varargs)
        if t == 'AstExprGroup':
            return [self.eval(e['expr'], env)]
        return [self.eval(e, env)]

    def eval_call(self, e, env):
        self.tick()
        f = e['func']
        if e['self']:
            obj = self.eval(f['expr'], env)
            fn = self.index(obj, f['index'])
            args = [obj] + self.eval_list(e['args'], env, None)
        else:
            fn = self.eval(f, env)
            args = self.eval_list(e['args'], env, None)
        return self.call(fn, args)

    def eval(self, e, env):
        t = e['type']
        if t == 'AstExprLocal':
            c = env.cells.get(e['local']['location'])
            if c is None:
                if self.unbound is not None:
                    return self.unbound(e['local'])
                raise Unsupported('unbound local %s' % e['local']['name'])
            return c[0]
        if t == 'AstExprConstantNumber':
            return num(e['value'])
        if t == 'AstExprConstantString':
            return e['value']
        if t == 'AstExprConstantBool':
            return bool(e['value'])
        if t == 'AstExprConstantNil':
            return None
        if t == 'AstExprGroup':
            return self.eval(e['expr'], env)
        if t == 'AstExprIndexName':
            return self.index(self.eval(e['expr'], env), e['index'])
        if t == 'AstExprIndexExpr':
            return self.index(self.eval(e['expr'], env), self.eval(e['index'], env))
        if t == 'AstExprGlobal':
            return self.globals.get(e['global'])
        if t == 'AstExprCall':
            r = self.eval_call(e, env)
            return r[0] if r else None
        if t == 'AstExprVarargs':
            return env.varargs[0] if env.varargs else None
        if t == 'AstExprBinary':
            op = e['op']
            if op == 'And':
                l = self.eval(e['left'], env)
                return self.eval(e['right'], env) if truthy(l) else l
            if op == 'Or':
                l = self.eval(e['left'], env)
                return l if truthy(l) else self.eval(e['right'], env)
            return self.binop(op, self.eval(e['left'], env), self.eval(e['right'], env))
        if t == 'AstExprUnary':
            v = self.eval(e['expr'], env)
            op = e['op']
            if op == 'Not':
                return not truthy(v)
            if op == 'Minus':
                if isinstance(v, U):
                    return mkU(('neg', v.key))
                if isinstance(v, str):
                    v = float(v)
                if isinstance(v, bool) or not isinstance(v, (int, float)):
                    raise LuaError('attempt to perform arithmetic on a %s value' % lua_type(v))
                return num(-v)
            if op == 'Len':
                if isinstance(v, U):
                    return mkU(('len', v.key))
                if isinstance(v, str):
                    return len(v)
                if isinstance(v, LuaTable):
                    return v.length()
                raise LuaError('attempt to get length of a %s value' % lua_type(v))
            raise Unsupported(op)
        if t == 'AstExprFunction':
            return Func(e, self, env.cells)
        if t == 'AstExprTable':
            tb = LuaTable()
            items = e['items']
            n = 0
            for i, it in enumerate(items):
                k = it.get('key')
                if k is not None:
                    tb.set(self.eval(k, env), self.eval(it['value'], env))
                else:
                    if i == len(items) - 1:
                        for v in self.eval_multi(it['value'], env):
                            n += 1
                            tb.set(n, v)
                    else:
                        n += 1
                        tb.set(n, self.eval(it['value'], env))
            return tb
        if t == 'AstExprIfElse':
            if truthy(self.eval(e['condition'], env)):
                return self.eval(e['trueExpr'], env)
            return self.eval(e['falseExpr'], env)
        if t == 'AstExprTypeAssertion':
            return self.eval(e['expr'], env)
        raise Unsupported(t)

    def binop(self, op, a, b):
        if op in ('Add', 'Sub', 'Mul', 'Div', 'FloorDiv', 'Mod', 'Pow'):
            return arith(op, a, b)
        if op == 'Concat':
            if isinstance(a, U) or isinstance(b, U):
                return mkU(('concat', kof(a), kof(b)))
            for x in (a, b):
                if isinstance(x, bool) or not isinstance(x, (str, int, float)):
                    raise LuaError('attempt to concatenate a %s value' % lua_type(x))
            return tostr(a) + tostr(b)
        if op in ('CompareEq', 'CompareNe', 'CompareLt', 'CompareLe', 'CompareGt', 'CompareGe'):
            return compare(op, a, b)
        raise Unsupported(op)


class _Env(object):
    def __init__(self, cells, varargs):
        self.cells = cells
        self.varargs = varargs

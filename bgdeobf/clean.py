"""Source-level cleanup of a decompiled script.

The VM decompiler gives back exactly the program that ran inside the VM.  Bacon Guard can also scramble the *source* of a
script before the VM wraps it:

  * every string is a small function that builds it from encrypted bytes at run time
    (`(function() local fromstring = buffer.fromstring("...") ... return buffer.tostring(create) end)()`),
    some of them written as a state machine (`while n2 do if n2 == 7671419 then ...`);
  * numbers are written as bit32 puzzles (`bit32.bxor(bit32.bor(a, b), bit32.band(a, b))`);
  * statements are wrapped in checks that are always true (`if bit32.bor(math.floor(os.clock() * 50), 4294967295) ==
    4294967295 then ... elseif ... then error("unreachable", 0) end`) next to arms that can never run.

None of this changes what the script does, so it can be taken out without understanding the script: an expression that
only uses harmless library functions (`buffer`, `bit32`, `math`, `string`) and variables declared inside itself is run in
the Luau sandbox and replaced by its result; an `if` whose condition has the same value for every reading of the clock is
replaced by the arm that runs.  The rest of the text is left exactly as it was.

The edits are made on the text, at the positions luau-ast reports, so the layout of the script is not touched anywhere
else.  The result is checked by the caller (it has to compile)."""
import re
import sys

from .astutil import Src, parse_ast, parse_loc
from .decomp import lua_str
from .sandbox import SandboxError, run_luau

SAFE_FUNCS = frozenset(['type', 'tostring', 'tonumber', 'select', 'ipairs', 'pairs', 'next', 'rawequal', 'rawlen'])
SAFE_MEMBERS = {
    'buffer': frozenset(['create', 'fromstring', 'tostring', 'len', 'copy', 'fill', 'readstring', 'writestring',
                         'readu8', 'readu16', 'readu32', 'readi8', 'readi16', 'readi32', 'readf32', 'readf64',
                         'writeu8', 'writeu16', 'writeu32', 'writei8', 'writei16', 'writei32', 'writef32', 'writef64']),
    'bit32': frozenset(['band', 'bor', 'bxor', 'bnot', 'lshift', 'rshift', 'arshift', 'lrotate', 'rrotate', 'extract',
                        'replace', 'countlz', 'countrz', 'byteswap']),
    'math': frozenset(['floor', 'ceil', 'abs', 'max', 'min', 'sqrt', 'fmod', 'pow', 'sign', 'clamp', 'round', 'huge', 'pi',
                       'exp', 'log', 'log10', 'sin', 'cos', 'tan', 'modf']),
    'string': frozenset(['char', 'byte', 'sub', 'len', 'reverse', 'rep', 'lower', 'upper', 'format']),
    'table': frozenset(['concat', 'unpack', 'pack']),
    'os': frozenset(['clock']),
}

CLOCK_VALUE = (0.37, 54321.5)
CLOCK_PREDICATE = (0.0, 0.0004, 0.5, 1.0, 1.999, 7.77, 100.25, 4096.0, 12345.678, 999999.999, 3000000.5, 8000000.125)
BATCH = 40
MAX_CANDIDATES = 6000
MAX_ROUNDS = 12
MAX_STRING = 200000

VALUE_TYPES = ('AstExprCall', 'AstExprBinary', 'AstExprUnary', 'AstExprGroup', 'AstExprIfElse', 'AstExprIndexExpr')
LITERALS = ('AstExprConstantNil', 'AstExprConstantBool', 'AstExprConstantNumber', 'AstExprConstantString')


class CleanError(Exception):
    pass


class Info(object):
    """what a subtree is made of: ok = it only does harmless things, ops = how much real work it does,
    decls / refs = locals declared / used inside it (a subtree is self-contained when refs <= decls), obf = it uses what
    the protector's junk is made of (bit32 / buffer calls, or a function that is called where it is written): an
    expression like `math.pi / 2` or `tostring(5)` is harmless too, but a script may have written it itself, so it stays"""
    __slots__ = ('ok', 'clock', 'ops', 'decls', 'refs', 'obf')

    def __init__(self, ok=True, clock=False, ops=0, decls=frozenset(), refs=frozenset(), obf=False):
        self.ok, self.clock, self.ops, self.decls, self.refs, self.obf = ok, clock, ops, decls, refs, obf

    @property
    def closed(self):
        return self.ok and self.refs <= self.decls


_EMPTY = frozenset()
BAD = Info(ok=False)


def lkey(loc):
    return (loc['name'], loc['location'])


def join(infos, extra_ops=0, decls=None, obf=False):
    ok, clock, ops, d, r = True, False, extra_ops, set(), set()
    for i in infos:
        ok = ok and i.ok
        clock = clock or i.clock
        obf = obf or i.obf
        ops += i.ops
        if i.decls:
            d |= i.decls
        if i.refs:
            r |= i.refs
    if decls:
        d |= decls
    return Info(ok, clock, ops, frozenset(d), frozenset(r), obf)


class Analyzer(object):
    """bottom-up pass that fills `info[id(node)]` for every node"""

    def __init__(self):
        self.info = {}
        self.parent = {}

    def run(self, root):
        sys.setrecursionlimit(max(sys.getrecursionlimit(), 20000))
        self.visit(root, None)

    def visit(self, n, parent):
        i = self.compute(n, parent)
        self.info[id(n)] = i
        return i

    def sub(self, n, parent):
        if n is None:
            return Info()
        return self.visit(n, parent)

    def compute(self, n, parent):
        t = n['type']
        if t in LITERALS:
            return Info()
        if t == 'AstExprGroup':
            return self.sub(n['expr'], n)
        if t == 'AstExprLocal':
            return Info(refs=frozenset([lkey(n['local'])]))
        if t == 'AstExprGlobal':
            return BAD
        if t == 'AstExprVarargs':
            return BAD
        if t == 'AstExprIndexName':
            base = n['expr']
            if base['type'] == 'AstExprGlobal':
                self.info[id(base)] = BAD
                allowed = SAFE_MEMBERS.get(base['global'])
                if allowed is not None and n['index'] in allowed:
                    return Info(clock=(base['global'] == 'os'), ops=1, obf=base['global'] in ('bit32', 'buffer'))
                return BAD
            return join([self.sub(base, n)], 1)
        if t == 'AstExprIndexExpr':
            return join([self.sub(n['expr'], n), self.sub(n['index'], n)], 1)
        if t == 'AstExprCall':
            f = n['func']
            infos = []
            if f['type'] == 'AstExprGlobal':
                self.info[id(f)] = BAD
                if f['global'] not in SAFE_FUNCS:
                    infos.append(BAD)
            else:
                infos.append(self.sub(f, n))
            for a in n['args']:
                infos.append(self.sub(a, n))
            iife = f['type'] == 'AstExprFunction' or (f['type'] == 'AstExprGroup' and f['expr']['type'] == 'AstExprFunction')
            return join(infos, 1, obf=iife)
        if t == 'AstExprFunction':
            args = frozenset(lkey(a) for a in n['args'])
            body = self.sub(n['body'], n)
            return join([body], 1, args)
        if t == 'AstExprTable':
            infos = []
            for it in n['items']:
                self.info[id(it)] = Info()
                if it.get('key') is not None:
                    infos.append(self.sub(it['key'], n))
                infos.append(self.sub(it['value'], n))
            return join(infos)
        if t == 'AstExprUnary':
            e = self.sub(n['expr'], n)
            if n['op'] not in ('Not', 'Minus', 'Len'):
                return BAD
            lit = n['op'] == 'Minus' and n['expr']['type'] == 'AstExprConstantNumber'
            return join([e], 0 if lit else 1)
        if t == 'AstExprBinary':
            return join([self.sub(n['left'], n), self.sub(n['right'], n)], 1)
        if t == 'AstExprIfElse':
            return join([self.sub(n['condition'], n), self.sub(n['trueExpr'], n), self.sub(n.get('falseExpr'), n)], 1)
        if t == 'AstStatBlock':
            return join([self.sub(s, n) for s in n['body']])
        if t == 'AstStatLocal':
            return join([self.sub(v, n) for v in n['values']], 0, set(lkey(v) for v in n['vars']))
        if t == 'AstStatLocalFunction':
            return join([self.sub(n['func'], n)], 0, set([lkey(n['name'])]))
        if t in ('AstStatAssign', 'AstStatCompoundAssign'):
            targets = n['vars'] if t == 'AstStatAssign' else [n['var']]
            values = n['values'] if t == 'AstStatAssign' else [n['value']]
            infos = [self.sub(x, n) for x in targets] + [self.sub(v, n) for v in values]
            for x in targets:
                if self.root(x) != 'AstExprLocal':
                    infos.append(BAD)
            return join(infos, 1)
        if t == 'AstStatIf':
            return join([self.sub(n['condition'], n), self.sub(n['thenbody'], n), self.sub(n.get('elsebody'), n)])
        if t in ('AstStatWhile', 'AstStatRepeat'):
            return join([self.sub(n['condition'], n), self.sub(n['body'], n)])
        if t == 'AstStatFor':
            return join([self.sub(n['from'], n), self.sub(n['to'], n), self.sub(n.get('step'), n), self.sub(n['body'], n)],
                        0, set([lkey(n['var'])]))
        if t == 'AstStatForIn':
            return join([self.sub(v, n) for v in n['values']] + [self.sub(n['body'], n)], 0,
                        set(lkey(v) for v in n['vars']))
        if t == 'AstStatReturn':
            return join([self.sub(v, n) for v in n['list']])
        if t in ('AstStatBreak', 'AstStatContinue'):
            return Info()
        if t == 'AstStatExpr':
            return self.sub(n['expr'], n)
        for k in kids_of(n):
            self.visit(k, n)
        return BAD

    @staticmethod
    def root(x):
        while x['type'] in ('AstExprIndexName', 'AstExprIndexExpr', 'AstExprGroup'):
            x = x['expr']
        return x['type']


def kids_of(n):
    for k, v in n.items():
        if isinstance(v, dict) and 'type' in v and v['type'] != 'AstLocal':
            yield v
        elif isinstance(v, list):
            for x in v:
                if isinstance(x, dict) and 'type' in x and x['type'] != 'AstLocal':
                    yield x


HARNESS_HEAD = '''local CLK = %s
local os = {clock = function() return CLK end}
local W = {}
local function hx(s)
  local t = {}
  for i = 1, #s do t[i] = string.format("%%02x", string.byte(s, i)) end
  return table.concat(t)
end
local function put(i, ok, ...)
  local n = select("#", ...)
  local v = ...
  if not ok then W[#W + 1] = i .. "\\tE"
  elseif n ~= 1 then W[#W + 1] = i .. "\\tE"
  elseif type(v) == "string" then W[#W + 1] = i .. "\\tS\\t" .. hx(v)
  elseif type(v) == "number" then W[#W + 1] = i .. "\\tN\\t" .. string.format("%%.17g", v)
  elseif type(v) == "boolean" then W[#W + 1] = i .. "\\tB\\t" .. tostring(v)
  elseif v == nil then W[#W + 1] = i .. "\\tZ"
  else W[#W + 1] = i .. "\\tE" end
end
local function truth(i, ok, v)
  if not ok then W[#W + 1] = i .. "\\tE" else W[#W + 1] = i .. "\\t" .. ((v and "T") or "F") end
end
'''


def _fmt_clock(v):
    return repr(float(v))


def _vm_text(text):
    """The sandbox hands the program to the VM one byte per character (latin-1), but the script is UTF-8: an accented
    letter would reach the VM as the single code point U+00E9 instead of its two UTF-8 bytes, and an emoji (above
    U+00FF) would make the encode fail.  Rewriting every character as the latin-1 characters of its UTF-8 bytes makes
    the VM read exactly the bytes the source has."""
    return text.encode('utf-8').decode('latin1')


def run_batch(items, clock, timeout, mem_mb):
    """items: [(index, mode, text)] -> {index: result string or None}; raises SandboxError"""
    parts = [HARNESS_HEAD % _fmt_clock(clock)]
    for idx, mode, text in items:
        if mode == 'truth':
            parts.append('truth(%d, pcall(function() return (%s) end))\n' % (idx, text))
        else:
            parts.append('put(%d, pcall(function() return %s end))\n' % (idx, text))
    parts.append('print(table.concat(W, "\\n"))\n')
    out = run_luau(_vm_text(''.join(parts)), timeout=timeout, mem_mb=mem_mb).decode('latin1')
    res = {}
    for line in out.replace('\r', '').split('\n'):
        f = line.split('\t')
        if len(f) >= 2 and f[0].isdigit():
            res[int(f[0])] = '\t'.join(f[1:])
    return res


def decode_result(r):
    """'S\\t<hex>' -> ('S', bytes) ... ; None for an error or an odd value"""
    if r is None:
        return None
    f = r.split('\t')
    k = f[0]
    try:
        if k == 'S':
            b = bytes.fromhex(f[1]) if len(f) > 1 else b''
            return ('S', b) if len(b) <= MAX_STRING else None
        if k == 'N':
            v = float(f[1])
            return ('N', v) if v == v and v not in (float('inf'), float('-inf')) else None
        if k == 'B':
            return ('B', f[1] == 'true')
        if k == 'Z':
            return ('Z', None)
        if k in ('T', 'F'):
            return ('T', k == 'T')
    except (ValueError, IndexError):
        return None
    return None


class Evaluator(object):
    def __init__(self, timeout=20, mem_mb=512, budget=90):
        self.timeout = timeout
        self.mem_mb = mem_mb
        self.budget = budget
        self.spent = 0.0
        self.cache = {}

    def evaluate(self, wants):
        """wants: [(mode, text, samples)] -> list of decoded results (None = could not be decided)"""
        import time
        todo = {}
        for mode, text, samples in wants:
            key = (mode, text, samples)
            if key not in self.cache and key not in todo:
                todo[key] = len(todo)
        keys = sorted(todo, key=lambda k: todo[k])
        for sample_set in set(k[2] for k in keys):
            group = [k for k in keys if k[2] == sample_set]
            per_sample = []
            for clock in sample_set:
                got = {}
                for start in range(0, len(group), BATCH):
                    chunk = group[start:start + BATCH]
                    items = [(start + j, k[0], k[1]) for j, k in enumerate(chunk)]
                    t0 = time.time()
                    try:
                        if self.spent > self.budget:
                            raise SandboxError('timeout', 'cleanup budget used up')
                        res = run_batch(items, clock, self.timeout, self.mem_mb)
                    except SandboxError:
                        res = {}
                        for it in items:
                            if self.spent > self.budget:
                                break
                            try:
                                res.update(run_batch([it], clock, 5, self.mem_mb))
                            except SandboxError:
                                pass
                    self.spent += time.time() - t0
                    got.update(res)
                per_sample.append(got)
            for j, k in enumerate(group):
                vals = [decode_result(g.get(j)) for g in per_sample]
                if any(v is None for v in vals) or any(v != vals[0] for v in vals):
                    self.cache[k] = None
                else:
                    self.cache[k] = vals[0]
        return [self.cache.get((m, t, s)) for m, t, s in wants]


def literal_text(res):
    """source text of a decoded result"""
    kind, v = res
    if kind == 'S':
        return lua_str(v)
    if kind == 'N':
        if v == int(v) and abs(v) < 1e15:
            return str(int(v))
        return repr(v)
    if kind == 'B':
        return 'true' if v else 'false'
    return 'nil'


def wrap_literal(text, res, ctx):
    kind, v = res
    neg = text.startswith('-')
    if ctx == 'recv':
        return '(' + text + ')'
    if ctx in ('bin', 'un') and kind == 'N' and neg:
        return '(' + text + ')'
    if ctx == 'cat' and kind == 'N':
        return '(' + text + ')'
    return text


class Edit(object):
    __slots__ = ('start', 'end', 'build', 'kind')

    def __init__(self, start, end, build, kind):
        self.start, self.end, self.build, self.kind = start, end, build, kind


class Editor(object):
    """applies a set of edits that are nested or apart (never partly overlapping); a builder gets `render(a, b)`, the
    source between a and b with the edits inside it applied"""

    def __init__(self, text, edits):
        self.text = text
        self.edits = sorted(edits, key=lambda e: (e.start, -e.end))

    def render(self, a, b):
        out = []
        pos = a
        last_end = -1
        for e in self.edits:
            if e.start < a or e.end > b or e.start < last_end or e.start < pos:
                continue
            out.append(self.text[pos:e.start])
            out.append(e.build(self.render))
            pos = last_end = e.end
        out.append(self.text[pos:b])
        return ''.join(out)


class Cleaner(object):
    def __init__(self, text, timeout=20, mem_mb=512, budget=90):
        self.text = text
        self.ev = Evaluator(timeout, mem_mb, budget)
        self.stats = dict(strings=0, numbers=0, values=0, checks=0, arms=0, statements=0)

    def run(self):
        text = original = self.text
        for _ in range(MAX_ROUNDS):
            new = self.round(text)
            if new == text:
                break
            text = new
        if text == original:
            return text, self.stats
        return tidy(text), self.stats

    def round(self, text):
        root = parse_ast(text)
        self.src = Src(text)
        self.text = text
        an = Analyzer()
        an.run(root)
        self.info = an.info
        self.values = []
        self.chains = []
        self.suffix = {}
        self.walk_block(root, None)
        if len(self.values) > MAX_CANDIDATES:
            self.values = self.values[:MAX_CANDIDATES]

        wants = []
        for node, ctx in self.values:
            wants.append(('val', self.src.slice(node), CLOCK_VALUE))
        chain_wants = []
        for chain in self.chains:
            for arm in chain['arms']:
                if arm['static'] is None and arm['text'] is not None:
                    chain_wants.append((arm, ('truth', arm['text'], CLOCK_PREDICATE)))
        wants.extend(w for _, w in chain_wants)
        results = self.ev.evaluate(wants)

        edits = []
        n_val = len(self.values)
        picked = []
        for (node, ctx), res in zip(self.values, results[:n_val]):
            if res is not None and res[0] != 'T':
                picked.append((self.src.span(node), node, ctx, res))
        picked.sort(key=lambda p: (p[0][0], -p[0][1]))
        end = -1
        for (a, b), node, ctx, res in picked:
            if a < end:
                continue
            end = b
            lit = wrap_literal(literal_text(res), res, ctx)
            if self.text[a:b] == lit:
                continue
            edits.append(Edit(a, b, self.value_builder(lit, res, node), 'value'))
        for (arm, _), res in zip(chain_wants, results[n_val:]):
            if res is not None and res[0] == 'T':
                arm['static'] = res[1]
        for chain in self.chains:
            e = self.chain_edit(chain)
            if e is not None:
                edits.append(e)
        if not edits:
            return text
        ed = Editor(text, edits)
        return ed.render(0, len(text))

    def value_builder(self, lit, res, node):
        stats = self.stats
        key = 'strings' if res[0] == 'S' and node['type'] == 'AstExprCall' else ('numbers' if res[0] == 'N' else 'values')

        def build(render):
            stats[key] += 1
            return lit
        return build

    def walk_block(self, block, parent):
        body = block['body']
        for i, st in enumerate(body):
            self.walk_stmt(st, block, i)

    def walk_stmt(self, st, block, i):
        t = st['type']
        if t == 'AstStatIf':
            chain = self.make_chain(st, block, i)
            arm = st
            while True:
                self.walk_expr(arm['condition'], 'free', skip=chain is not None)
                self.walk_block(arm['thenbody'], None)
                e = arm.get('elsebody')
                if e is None:
                    break
                if e['type'] == 'AstStatIf':
                    arm = e
                    continue
                self.walk_block(e, None)
                break
            return
        if t == 'AstStatBlock':
            self.walk_block(st, None)
            return
        for k, v in st.items():
            if k in ('type', 'location'):
                continue
            ctx = 'free'
            if isinstance(v, dict) and 'type' in v:
                self.walk_any(v, ctx)
            elif isinstance(v, list):
                for x in v:
                    if isinstance(x, dict) and 'type' in x:
                        self.walk_any(x, ctx)

    def walk_any(self, n, ctx):
        t = n['type']
        if t == 'AstStatBlock':
            self.walk_block(n, None)
        elif t.startswith('AstStat'):
            self.walk_stmt(n, {'body': [n]}, 0)
        elif t == 'AstLocal':
            return
        else:
            self.walk_expr(n, ctx)

    def walk_expr(self, n, ctx, skip=False):
        t = n['type']
        info = self.info.get(id(n))
        if (not skip and t in VALUE_TYPES and info is not None and info.closed and info.ops > 0 and info.obf
                and not self.trivial_group(n, ctx)):
            self.values.append((n, ctx))
            if t == 'AstExprCall' and n['func']['type'] in ('AstExprFunction', 'AstExprGroup'):
                return
        elif skip and t == 'AstExprGroup' and False:
            pass
        if t == 'AstExprFunction':
            self.walk_block(n['body'], None)
            return
        if t == 'AstExprCall':
            f = n['func']
            self.walk_expr(f, 'recv')
            for a in n['args']:
                self.walk_expr(a, 'free')
            return
        if t == 'AstExprIndexName':
            self.walk_expr(n['expr'], 'recv')
            return
        if t == 'AstExprIndexExpr':
            self.walk_expr(n['expr'], 'recv')
            self.walk_expr(n['index'], 'free')
            return
        if t == 'AstExprBinary':
            c = 'cat' if n['op'] == 'Concat' else 'bin'
            self.walk_expr(n['left'], c)
            self.walk_expr(n['right'], c)
            return
        if t == 'AstExprUnary':
            self.walk_expr(n['expr'], 'un')
            return
        if t == 'AstExprGroup':
            self.walk_expr(n['expr'], ctx if ctx != 'recv' else 'recv')
            return
        if t == 'AstExprTable':
            for it in n['items']:
                if it.get('key') is not None:
                    self.walk_expr(it['key'], 'free')
                self.walk_expr(it['value'], 'free')
            return
        if t == 'AstExprIfElse':
            self.walk_expr(n['condition'], 'free')
            self.walk_expr(n['trueExpr'], 'free')
            if n.get('falseExpr') is not None:
                self.walk_expr(n['falseExpr'], 'free')
            return

    @staticmethod
    def trivial_group(n, ctx):
        """a group around a plain constant is only worth folding where its brackets can go"""
        if n['type'] != 'AstExprGroup':
            return False
        e = n['expr']
        while e['type'] == 'AstExprGroup':
            e = e['expr']
        return e['type'] in LITERALS and ctx in ('recv',)

    def make_chain(self, st, block, index):
        arms = []
        else_body = None
        cur = st
        while True:
            cond = cur['condition']
            static = None
            text = None
            ct = cond['type']
            if ct == 'AstExprConstantBool':
                static = bool(cond['value'])
            elif ct == 'AstExprConstantNil':
                static = False
            elif ct in ('AstExprConstantNumber', 'AstExprConstantString'):
                static = True
            else:
                info = self.info.get(id(cond))
                if info is not None and info.closed and (info.clock or info.obf):
                    text = self.src.slice(cond)
            arms.append(dict(cond=cond, body=cur['thenbody'], static=static, text=text))
            e = cur.get('elsebody')
            if e is None:
                break
            if e['type'] == 'AstStatIf':
                cur = e
                continue
            else_body = e
            break
        if all(a['static'] is None and a['text'] is None for a in arms):
            return None
        chain = dict(node=st, arms=arms, else_body=else_body, block=block, index=index)
        self.chains.append(chain)
        return chain

    def body_lines(self, body, header_end_line, closer_line):
        """(first offset, last offset) of the whole lines that hold the statements of a block, or None when a statement
        shares a line with a keyword (the layout is not the printer's)"""
        sts = body['body']
        if not sts:
            return ()
        l1 = parse_loc(sts[0]['location'])[0]
        l2 = parse_loc(sts[-1]['location'])[2]
        if l1 <= header_end_line or l2 >= closer_line:
            return None
        a = self.src.starts[l1]
        b = self.src.starts[l2 + 1] - 1 if l2 + 1 < len(self.src.starts) else len(self.text)
        return (a, b)

    def chain_edit(self, chain):
        arms = chain['arms']
        if all(a['static'] is None for a in arms):
            return None
        node = chain['node']
        l1, c1, l2, c2 = parse_loc(node['location'])
        start, end = self.src.off(l1, c1), self.src.off(l2, c2)
        line_start = self.src.starts[l1]
        indent = self.text[line_start:start]
        if indent.strip() != '':
            return None
        eol = self.text.find('\n', end)
        tail = self.text[end:eol if eol >= 0 else len(self.text)]
        if tail.strip() != '':
            return None

        keep = []
        final = chain['else_body']
        always = None
        for a in arms:
            if a['static'] is False:
                continue
            if a['static'] is True:
                always = a
                break
            keep.append(a)
        if always is not None:
            final = always['body']
        if len(keep) == len(arms) and always is None:
            return None

        def bounds(body, cond_node, nxt):
            hl = parse_loc(cond_node['location'])[2] if cond_node is not None else parse_loc(body['location'])[0]
            cl = parse_loc(body['location'])[2]
            return self.body_lines(body, hl, cl)

        spans = {}
        for a in keep + ([always] if always is not None else []):
            b = bounds(a['body'], a['cond'], None)
            if b is None:
                return None
            spans[id(a)] = b
        fb = None
        if always is None and final is not None:
            fb = bounds(final, None, None)
            if fb is None:
                return None
        elif always is not None:
            fb = spans[id(always)]

        names = self.declared_names(final) if final is not None and not keep else set()
        wrap_do = bool(names) and not self.safe_to_unwrap(chain, names)
        stats = self.stats
        dropped = len(arms) - len(keep) - (1 if always is not None else 0)
        if always is not None and chain['else_body'] is not None:
            dropped += 1

        if not keep and not wrap_do and final is not None and final['body'] and chain['block'] is not None \
                and final['body'][-1]['type'] in ('AstStatReturn', 'AstStatBreak', 'AstStatContinue'):
            rest = chain['block']['body'][chain['index'] + 1:]
            if rest:
                l3, c3, l4, c4 = parse_loc(rest[-1]['location'])
                end = self.src.off(l4, c4)
                eol = self.text.find('\n', end)
                if self.text[end:eol if eol >= 0 else len(self.text)].strip() != '':
                    return None

        def build(render):
            stats['checks'] += 1
            stats['arms'] += dropped
            def lines(span):
                if not span:
                    return None
                return render(span[0], span[1])

            body_text = lines(fb) if fb is not None else None
            if not keep:
                if body_text is None:
                    stats['statements'] += 1
                    return ''
                if wrap_do:
                    return 'do\n' + body_text + '\n' + indent + 'end'
                return unindent(body_text, indent)
            out = []
            for i, a in enumerate(keep):
                cond = render(*self.src.span(a['cond']))
                out.append(('if ' if i == 0 else indent + 'elseif ') + cond + ' then')
                bt = lines(bounds(a['body'], a['cond'], None) or ())
                if bt:
                    out.append(bt)
            if final is not None and body_text is not None:
                out.append(indent + 'else')
                out.append(body_text)
            out.append(indent + 'end')
            return '\n'.join(out)

        return Edit(start, end, build, 'chain')

    def declared_names(self, block):
        names = set()
        for st in block['body']:
            t = st['type']
            if t == 'AstStatLocal':
                names.update(v['name'] for v in st['vars'])
            elif t == 'AstStatLocalFunction':
                names.add(st['name']['name'])
        return names

    def safe_to_unwrap(self, chain, names):
        """taking the statements out of their block makes the locals they declare visible to the code after the block;
        that is only the same program when that code never mentions those names"""
        block, idx = chain['block'], chain['index']
        if block is None:
            return False
        suf = self.suffix.get(id(block))
        if suf is None:
            sets = [self.names_in(s) for s in block['body']]
            suf = [set() for _ in range(len(sets) + 1)]
            for j in range(len(sets) - 1, -1, -1):
                suf[j] = suf[j + 1] | sets[j]
            self.suffix[id(block)] = suf
        return not (suf[idx + 1] & names)

    def names_in(self, n):
        out = set()
        stack = [n]
        while stack:
            x = stack.pop()
            if isinstance(x, dict):
                t = x.get('type')
                if t == 'AstExprGlobal':
                    out.add(x['global'])
                elif t == 'AstExprLocal':
                    out.add(x['local']['name'])
                elif t == 'AstLocal':
                    out.add(x['name'])
                    continue
                stack.extend(x.values())
            elif isinstance(x, list):
                stack.extend(x)
        return out


def unindent(text, indent):
    """one level less indentation for a block that is no longer inside an `if`; the first line starts where the old
    statement started, so it carries no indentation of its own"""
    lines = text.split('\n')
    first = None
    for l in lines:
        if l.strip():
            first = l[:len(l) - len(l.lstrip())]
            break
    if first is None:
        return ''
    unit = first[len(indent):] if first.startswith(indent) else '\t'
    out = []
    for l in lines:
        out.append(l[len(unit):] if l.startswith(first) else l)
    s = '\n'.join(out)
    return s.lstrip(' \t') if s.startswith(indent) else s


_BLANKS = re.compile(r'\n[ \t]*\n(?:[ \t]*\n)+')
_OPENER = re.compile(r'(?:\b(?:then|do|else|repeat)|\bfunction\b[^\n]*\))$')
_CLOSER = re.compile(r'[ \t]*(?:end|else|elseif|until)\b')


def opens_block(line):
    """the line ends with a keyword (or a function head) that starts a block: nothing needs a blank line after it"""
    line = line.rstrip()
    m = _OPENER.search(line)
    if m is None:
        return False
    tail = line[line.rfind('function'):] if 'function' in line else ''
    return not (tail and re.search(r'\bend\b', tail))


_BLOCK_START = re.compile(r'(?:if|for|while|repeat|do|function|local\s+function|end|else|elseif|until|return|break|continue)\b')


def plain_statement(line):
    """a statement that fits on its line: not a block keyword, brackets closed on the same line"""
    st = line.strip()
    if not st or st.startswith('--') or _BLOCK_START.match(st) or opens_block(st):
        return False
    depth = 0
    quote = None
    i = 0
    while i < len(st):
        c = st[i]
        if quote:
            if c == '\\':
                i += 1
            elif c == quote:
                quote = None
        elif c in '"\' ':
            quote = c
        elif c in '([{':
            depth += 1
        elif c in ')]}':
            depth -= 1
        i += 1
    return depth == 0 and quote is None and not st.endswith((',', '=', 'and', 'or', '..', '+', '-', '*', '/'))


def tidy(text):
    """the blank lines that removed statements leave behind: runs of them, and the ones right after the line that opens a
    block or right before the line that closes one"""
    if '[[' in text or '[=' in text:
        return text
    text = re.sub(r'[ \t]+\n', '\n', text)
    text = _BLANKS.sub('\n\n', text)
    lines = text.split('\n')
    out = []
    for i, l in enumerate(lines):
        if not l.strip():
            prev = out[-1] if out else ''
            nxt = next((x for x in lines[i + 1:] if x.strip()), '')
            if (prev.strip() and opens_block(prev)) or (nxt and _CLOSER.match(nxt)):
                continue
            if (plain_statement(prev) and plain_statement(nxt) and lines[i + 1].strip() == nxt.strip()
                    and indent_of(prev) == indent_of(nxt)):
                continue
        out.append(l)
    return '\n'.join(out)


def indent_of(line):
    return line[:len(line) - len(line.lstrip())]


def clean_source(text, timeout=20, mem_mb=512, budget=90):
    """-> (cleaned text, stats).  Raises CleanError / LuauError when the text cannot be processed."""
    c = Cleaner(text, timeout, mem_mb, budget)
    return c.run()

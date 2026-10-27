"""Decompiler for the dumped Bacon Guard VM code (see tdec.py for the instruction decoder).

Pipeline per proto:  records -> nodes -> basic blocks -> reaching definitions / webs ->
symbolic evaluation of each block (inlining single-use temporaries while keeping evaluation order)
-> structuring (if / for) -> AST -> Luau text.
"""
import sys, re, collections, math

KEYWORDS = {'and', 'break', 'do', 'else', 'elseif', 'end', 'false', 'for', 'function', 'if', 'in', 'local', 'nil', 'not',
            'or', 'repeat', 'return', 'then', 'true', 'until', 'while', 'continue'}
IDENT = re.compile(r'^[A-Za-z_][A-Za-z0-9_]*$')


# ----------------------------------------------------------------------------------------------------------------
# expressions
# ----------------------------------------------------------------------------------------------------------------
class E:
    pass


class Const(E):
    def __init__(s, kind, v):  # kind: 's' bytes, 'n' number, 'b' bool, 'z' nil
        s.kind, s.v = kind, v


class Imp(E):
    def __init__(s, path):
        s.path = path


class Var(E):
    def __init__(s, web):
        s.web = web


class Upv(E):
    def __init__(s, idx):
        s.idx = idx


class Idx(E):
    def __init__(s, obj, key):  # key: python str or E
        s.obj, s.key = obj, key


class Call(E):
    def __init__(s, fn, args, method=None):
        s.fn, s.args, s.method = fn, args, method  # method: str -> fn is the object


class Bin(E):
    def __init__(s, op, l, r):
        s.op, s.l, s.r = op, l, r


class Not(E):
    def __init__(s, e):
        s.e = e


class Tbl(E):
    def __init__(s, items, fields=None, pending=None, fromnew=False):
        s.items = items
        s.fields = fields or []       # [(key str, value E)] from a DUPTABLE template (constant values)
        s.pending = pending or []     # template keys still to be assigned (in template order)
        s.fromnew = fromnew           # made by NEWTABLE: the fields assigned right after were not written as a constructor


class Func(E):
    def __init__(s, pid, caps):
        s.pid, s.caps = pid, caps


class Multi(E):  # wrapper marking a multi-value (call) expression used as last argument
    def __init__(s, e):
        s.e = e


class Vararg(E):  # `...`; `single` marks a value that was cut down to one (a plain GETVARARGS, printed `(...)`)
    def __init__(s, single=False):
        s.single = single


class Len(E):
    def __init__(s, e):
        s.e = e


class Neg(E):
    def __init__(s, e):
        s.e = e


def kconst(c):
    if c is None:
        raise Exception('missing constant')
    if c[0] == 's':
        return Const('s', c[1])
    if c[0] == 'n':
        return Const('n', c[1])
    if c[0] == 'b':
        return Const('b', c[1])
    return Const('z', None)


def fmt_num(v):
    if isinstance(v, int):
        return str(v)
    if v != v:
        return '(0/0)'
    if v in (math.inf, -math.inf):
        return 'math.huge' if v > 0 else '-math.huge'
    if v == int(v) and abs(v) < 2 ** 53:
        return str(int(v))
    return repr(v)


def lua_str(b):
    try:
        s = b.decode('utf-8')
    except UnicodeDecodeError:
        s = None
    out = ['"']
    if s is None:
        for c in b:
            if c == 34: out.append('\\"')
            elif c == 92: out.append('\\\\')
            elif c == 10: out.append('\\n')
            elif c == 13: out.append('\\r')
            elif c == 9: out.append('\\t')
            elif 32 <= c < 127: out.append(chr(c))
            else: out.append('\\%03d' % c)
    else:
        for ch in s:
            c = ord(ch)
            if ch == '"': out.append('\\"')
            elif ch == '\\': out.append('\\\\')
            elif ch == '\n': out.append('\\n')
            elif ch == '\r': out.append('\\r')
            elif ch == '\t': out.append('\\t')
            elif c < 32 or c == 127: out.append('\\%03d' % c)
            else: out.append(ch)
    out.append('"')
    return ''.join(out)


# ----------------------------------------------------------------------------------------------------------------
# AST statements: tuples
#   ('local', web, expr|None)  ('assign', target, expr)  ('call', expr)  ('if', cond, then, else)
#   ('for', web, init, limit, step, body)  ('return', [exprs])  ('do', body)
# ----------------------------------------------------------------------------------------------------------------

class Web:
    _n = 0

    def __init__(s, kind='var'):
        Web._n += 1
        s.uid = Web._n
        s.kind = kind          # 'var', 'param', 'loop'
        s.defs = []
        s.uses = []
        s.captured = False
        s.hint = None
        s.name = None
        s.declared = False
        s.var = Var(s)
        s.fn = None            # owning Fn (set by Fn)


class Node:
    __slots__ = ('id', 'kind', 'a', 'rid', 'succ', 'reads', 'writes', 'blk', 'idx', 'preds', 'topbase')

    def __init__(s, kind, a, rid):
        s.kind, s.a, s.rid = kind, a, rid
        s.succ = []
        s.reads = []
        s.writes = []
        s.preds = []
        s.blk = None
        s.topbase = None


class Block:
    def __init__(s, bid):
        s.id = bid
        s.nodes = []
        s.succ = []   # block ids
        s.pred = []
        s.stmts = None
        s.term = None


COND_KINDS = ('JUMPIFNOT', 'JUMPIF', 'JUMPIFEQ', 'JUMPIFNOTEQ', 'JUMPIFLE', 'JUMPIFLT', 'JUMPIFNOTLT', 'JUMPIFNOTLE',
              'JUMPXEQK', 'JUMPXEQKB', 'JUMPXEQKNIL')
TERM_KINDS = ('RETURN', 'FORNPREP', 'FORNLOOP', 'FORGPREP', 'FORGLOOP') + COND_KINDS
ARITH_RR = {'ADD': '+', 'SUB': '-', 'MUL': '*', 'DIV': '/', 'MOD': '%', 'POW': '^', 'IDIV': '//'}
ARITH_RK = ('SUBRK', 'DIVRK')
ARITH_K = ('ADDK', 'SUBK', 'MULK', 'DIVK', 'MODK', 'POWK', 'IDIVK')


class Fn:
    """one decompiled function instance"""

    def __init__(s, dec, pid, upvals):
        s.dec = dec
        s.pid = pid
        s.P = dec.protos[pid]
        s.upvals = upvals       # list of Web (captured variables, owned by ancestors)
        s.body = None
        s.params = []
        s.webs = []
        s.children = []
        s.tmpcount = 0


class Decompiler:
    def __init__(self, decoder):
        self.decoder = decoder
        self.protos = decoder.protos
        self.fns = []

    # ------------------------------------------------------------------ node building
    def build(self, fn):
        P, protos = fn.P, self.protos
        pid = fn.pid
        D = self.decoder.decode_all(pid)
        fn.D = D
        consumed = set()
        recnodes = {}      # rid -> list of Nodes (ins..., terminator?)
        # first pass: create nodes
        for rid, d in D.items():
            nodes = []
            if d['kind'] == 'CAPTURE':
                continue
            for name, a in d['ins']:
                if name == 'LOADB' and d['ctrl'] and d['ctrl'][0] == 'LOADBSKIP':
                    assert a['C'] == 0, 'LOADB with skip'
                nodes.append(Node(name, a, rid))
            if d['ctrl'] and d['ctrl'][0] != 'LOADBSKIP':
                nm, a = d['ctrl']
                if nm in ('JUMP',):
                    pass
                else:
                    nodes.append(Node(nm, a, rid))
            recnodes[rid] = nodes

        def first(rid, seen=None):
            seen = seen or set()
            while True:
                if rid in seen:
                    raise Exception('empty cycle')
                seen.add(rid)
                d = D[rid]
                nodes = recnodes.get(rid)
                if nodes:
                    return nodes[0]
                if d['ctrl'] and d['ctrl'][0] == 'JUMP':
                    rid = d['alt']
                    continue
                rid = d['nxt']

        # closures: absorb the CAPTURE records following them
        for rid, nodes in recnodes.items():
            for n in nodes:
                if n.kind in ('NEWCLOSURE', 'DUPCLOSURE'):
                    a = n.a
                    if n.kind == 'NEWCLOSURE':
                        child = P['kids'][a['rf']]
                    else:
                        ck = P['consts'][a['rf']]
                        child = int(ck[1])
                    nup = protos[child]['nup']
                    caps = []
                    cur = D[rid]['nxt']
                    for _ in range(nup):
                        cd = D[cur]
                        assert cd['kind'] == 'CAPTURE', (pid, rid, cd['kind'])
                        f = cd['ins'][0][1]
                        caps.append((f['A'], f['B']))
                        consumed.add(cur)
                        last = cd
                        cur = cd['nxt']
                    n.a = dict(a, child=child, caps=caps)
                    D[rid] = dict(D[rid], nxt=cur)
        # successors
        for rid, nodes in recnodes.items():
            d = D[rid]
            for i, n in enumerate(nodes):
                last = (i == len(nodes) - 1)
                if not last:
                    n.succ = [nodes[i + 1]]
                    continue
                if n.kind == 'RETURN':
                    n.succ = []
                elif n.kind in COND_KINDS or n.kind in ('FORNPREP', 'FORNLOOP', 'FORGLOOP'):
                    n.succ = [first(d['nxt']), first(d['alt'])]
                else:
                    if d['ctrl'] and d['ctrl'][0] == 'JUMP':
                        n.succ = [first(d['alt'])]
                    else:
                        n.succ = [first(d['nxt'])]
        entry = first(P['entry'])
        # collect reachable nodes in DFS order
        order = []
        seen = set()
        st = [entry]
        while st:
            n = st.pop()
            if id(n) in seen:
                continue
            seen.add(id(n))
            order.append(n)
            for s in reversed(n.succ):
                st.append(s)
        for i, n in enumerate(order):
            n.id = i
        for n in order:
            for s in n.succ:
                s.preds.append(n)
        fn.nodes = order
        fn.entry = entry

    # ------------------------------------------------------------------ reads / writes
    def annotate(self, fn):
        # multret producer -> consumer linkage: scan each straight-line chain
        for n in fn.nodes:
            a = n.a
            k = n.kind
            n.reads, n.writes = [], []
            if k in ('LOADK', 'LOADN', 'LOADB', 'LOADNIL', 'GETIMPORT', 'GETUPVAL', 'NEWTABLE'):
                n.writes = [a['A']]
            elif k == 'MOVE':
                n.reads, n.writes = [a['B']], [a['A']]
            elif k == 'GETTABLEKS':
                n.reads, n.writes = [a['B']], [a['A']]
            elif k == 'SETTABLEKS':
                n.reads = [a['A'], a['B']]
            elif k == 'SELF':
                n.reads, n.writes = [a['B']], [a['A'] + 1, a['A']]
            elif k == 'SETUPVAL':
                n.reads = [a['A']]
            elif k in ARITH_RR or k in ('OR', 'AND', 'GETTABLE'):
                n.reads, n.writes = [a['B'], a['C']], [a['A']]
            elif k in ('LENGTH', 'NOT', 'MINUS', 'GETTABLEN', 'ORK', 'ANDK'):
                n.reads, n.writes = [a['B']], [a['A']]
            elif k == 'GETVARARGS':
                # B == 0 produces as many values as the call received, handed to the next consumer (like a CALL with C == 0)
                n.writes = [a['A'] + i for i in range(a['B'] - 1)] if a['B'] > 0 else []
            elif k == 'CONCAT':
                n.reads, n.writes = list(range(a['B'], a['C'] + 1)), [a['A']]
            elif k == 'DUPTABLE':
                n.writes = [a['A']]
            elif k == 'SETTABLE':
                n.reads = [a['A'], a['B'], a['C']]
            elif k in ARITH_K:
                n.reads, n.writes = [a['B']], [a['A']]
            elif k in ARITH_RK:
                n.reads, n.writes = [a['C']], [a['A']]
            elif k == 'FORGPREP':
                n.reads = [a['A'], a['A'] + 1, a['A'] + 2]
            elif k == 'FORGLOOP':
                n.reads = []
                n.writes = [a['A'] + 3 + i for i in range(a['aux'] & 255)]
            elif k == 'SETLIST':
                cnt = a['C'] - 1
                if cnt < 0:
                    cnt = None
                n.reads = [a['A']]
                n.writes = [a['A']]
            elif k in ('NEWCLOSURE', 'DUPCLOSURE'):
                # the closure is stored in its register before its captures are taken, so a capture of that register
                # is the closure itself (a recursive `local function`), not a read of the register's previous value
                n.reads = [idx for (t, idx) in a['caps'] if t in (0, 1) and idx != a['A']]
                n.writes = [a['A']]
            elif k == 'CALL':
                pass
            elif k == 'RETURN':
                pass
            elif k in ('JUMPIFNOT', 'JUMPIF'):
                n.reads = [a['A']]
            elif k in ('JUMPIFEQ', 'JUMPIFNOTEQ', 'JUMPIFLE', 'JUMPIFLT', 'JUMPIFNOTLT', 'JUMPIFNOTLE'):
                n.reads = [a['A'], a['aux']]
            elif k in ('JUMPXEQK', 'JUMPXEQKB', 'JUMPXEQKNIL'):
                n.reads = [a['A']]
            elif k == 'FORNPREP':
                n.reads = [a['A'], a['A'] + 1, a['A'] + 2]
                n.writes = [a['A'] + 2]
            elif k == 'FORNLOOP':
                n.reads = []
                n.writes = [a['A'] + 2]
            elif k == 'CLOSEUPVALS':
                pass
            else:
                raise Exception('annotate: unknown node kind ' + k)
        # CALL / SETLIST / RETURN with variable arity need the stack-top base from a preceding multret call
        for n in fn.nodes:
            if n.kind == 'CALL' and n.a['C'] == 0:
                pass
        for n in fn.nodes:
            a = n.a
            if n.kind in ('CALL', 'RETURN', 'SETLIST'):
                var = False
                if n.kind == 'CALL':
                    var = a['B'] == 0
                elif n.kind == 'RETURN':
                    var = a['B'] == 0
                else:
                    var = a['C'] == 0
                top = None
                if var:
                    # nearest preceding node (single-pred chain) that is a multret CALL
                    cur = n
                    while True:
                        if len(cur.preds) != 1:
                            raise Exception('multret consumer without single pred chain')
                        cur = cur.preds[0]
                        if (cur.kind == 'CALL' and cur.a['C'] == 0) or (cur.kind == 'GETVARARGS' and cur.a['B'] == 0):
                            top = cur.a['A']
                            break
                        if cur.kind in TERM_KINDS:
                            raise Exception('multret producer not found')
                n.topbase = top
                if n.kind == 'CALL':
                    base = a['A']
                    cnt = (a['B'] - 1) if a['B'] > 0 else (top - base - 1)
                    n.reads = [base + i for i in range(0, cnt + 1)]
                    if a['C'] >= 2:
                        n.writes = [base + i for i in range(a['C'] - 1)]
                elif n.kind == 'RETURN':
                    base = a['A']
                    cnt = (a['B'] - 1) if a['B'] > 0 else (top - base)
                    n.reads = [base + i for i in range(cnt)]
                else:
                    base = a['B']
                    cnt = (a['C'] - 1) if a['C'] > 0 else (top - base)
                    n.reads = [a['A']] + [base + i for i in range(cnt)]
                    n.writes = [a['A']]

    # ------------------------------------------------------------------ blocks
    def make_blocks(self, fn):
        nodes = fn.nodes
        leaders = {fn.entry.id}
        for n in nodes:
            if n.kind in TERM_KINDS:
                for s in n.succ:
                    leaders.add(s.id)
            elif len(n.succ) == 1 and len(n.succ[0].preds) > 1:
                leaders.add(n.succ[0].id)
        for n in nodes:
            if len(n.preds) > 1:
                leaders.add(n.id)
        blocks = []
        bymap = {}
        cur = None
        for n in nodes:
            pass
        # build by walking from leaders
        order = sorted(leaders)
        for lid in order:
            b = Block(len(blocks))
            blocks.append(b)
            n = nodes[lid]
            while True:
                b.nodes.append(n)
                n.blk = b
                if n.kind in TERM_KINDS or len(n.succ) != 1 or n.succ[0].id in leaders:
                    break
                n = n.succ[0]
        for b in blocks:
            last = b.nodes[-1]
            b.succ = [s.blk.id for s in last.succ]
        for b in blocks:
            for s in b.succ:
                blocks[s].pred.append(b.id)
        # entry block must be first
        fn.blocks = blocks
        fn.entry_block = fn.entry.blk

    # ------------------------------------------------------------------ reaching definitions + webs
    def webs(self, fn):
        P = fn.P
        nodes = fn.nodes
        # def ids
        defs = []   # (node or None, reg)
        node_defs = {}
        for r in range(P['np']):
            defs.append((None, r))
        for n in nodes:
            for r in n.writes:
                node_defs[(n.id, r)] = len(defs)
                defs.append((n, r))
        regdefs = collections.defaultdict(set)
        for i, (n, r) in enumerate(defs):
            regdefs[r].add(i)
        blocks = fn.blocks
        # per block gen/kill (by register -> last def)
        gen = []
        for b in blocks:
            g = {}
            for n in b.nodes:
                for r in n.writes:
                    g[r] = node_defs[(n.id, r)]
            gen.append(g)
        IN = [dict() for _ in blocks]   # reg -> frozenset(defs)
        OUT = [dict() for _ in blocks]
        entry_b = fn.entry_block.id
        init = {r: frozenset([i]) for i, (n, r) in enumerate(defs) if n is None}
        work = list(range(len(blocks)))
        inwork = set(work)
        IN[entry_b] = dict(init)
        while work:
            bi = work.pop(0)
            inwork.discard(bi)
            b = blocks[bi]
            # IN = union of OUT of preds (+init for entry)
            cur = {}
            srcs = [OUT[p] for p in b.pred]
            if bi == entry_b:
                srcs = srcs + [init]
            for src in srcs:
                for r, s in src.items():
                    if r in cur:
                        cur[r] = cur[r] | s
                    else:
                        cur[r] = s
            IN[bi] = cur
            out = dict(cur)
            for r, d in gen[bi].items():
                out[r] = frozenset([d])
            if out != OUT[bi]:
                OUT[bi] = out
                for s in b.succ:
                    if s not in inwork:
                        work.append(s)
                        inwork.add(s)
        # walk each block, record reaching defs for each use
        uf = list(range(len(defs)))

        def find(x):
            while uf[x] != x:
                uf[x] = uf[uf[x]]
                x = uf[x]
            return x

        def union(a, b):
            ra, rb = find(a), find(b)
            if ra != rb:
                uf[rb] = ra

        # Inside a for loop every temporary lives above the loop's own registers, so a write to a register below them
        # (`flag = true` for a `local flag` declared before the loop) is an assignment to that variable, even when
        # the new value never meets the old one at a read (`flag = true return flag`)
        loop_base = {}
        for n in nodes:
            if n.kind not in ('FORNPREP', 'FORGPREP') or not n.succ:
                continue
            if n.kind == 'FORNPREP':
                body, leave = n.succ[0], n.succ[1]
            else:
                lp = n.succ[0]
                if lp.kind != 'FORGLOOP' or len(lp.succ) != 2:
                    continue
                leave, body = lp.succ
            todo = [body]
            seen = set()
            while todo:
                m = todo.pop()
                if m is leave or m.id in seen:
                    continue
                seen.add(m.id)
                todo.extend(m.succ)
            for i in seen:
                loop_base[i] = max(loop_base.get(i, 0), n.a['A'])
        usedefs = {}   # (node id, reg) -> set of def ids
        movers = []    # (MOVE node into a lower register, defs of that register reaching it)
        assigns = []   # (node, register, defs reaching it): writes to a variable declared before the enclosing loop
        for b in blocks:
            cur = dict(IN[b.id])
            for n in b.nodes:
                for r in n.reads:
                    s = cur.get(r)
                    if not s:
                        raise Exception('read of undefined register R%d at node %d (rec %s, kind %s) in proto %d' % (r, n.id, n.rid, n.kind, fn.pid))
                    usedefs[(n.id, r)] = s
                    lst = list(s)
                    for x in lst[1:]:
                        union(lst[0], x)
                if n.kind == 'MOVE' and n.a['A'] < n.a['B']:
                    # a copy from a higher (temporary) register into a lower one is an assignment to the existing local
                    # (applied below, once every reader of the copy is known)
                    prev = cur.get(n.a['A'])
                    if prev:
                        movers.append((n, prev))
                if n.id in loop_base:
                    for r in n.writes:
                        if r < loop_base[n.id] and cur.get(r):
                            assigns.append((n, r, cur[r]))
                for r in n.writes:
                    cur[r] = frozenset([node_defs[(n.id, r)]])
                if n.kind in ('NEWCLOSURE', 'DUPCLOSURE'):
                    for (t, idx) in n.a['caps']:
                        if t in (0, 1) and idx == n.a['A']:
                            usedefs[(n.id, idx)] = frozenset([node_defs[(n.id, idx)]])
        if movers:
            readers = collections.defaultdict(list)
            for (nid, r), s in usedefs.items():
                for d in s:
                    readers[d].append(nodes[nid])
            for n, prev in movers:
                d_new = node_defs[(n.id, n.a['A'])]
                rd = readers[d_new]
                # not an assignment: the call result is moved into a slot the compiler reserved ahead for an operand
                # of `a .. f() .. b` (the registers of a concat are consecutive temporaries, never variables)
                if rd and all(k.kind == 'CONCAT' and k.a['B'] <= n.a['A'] <= k.a['C'] for k in rd):
                    continue
                for x in prev:
                    union(d_new, x)
        for n, r, prev in assigns:
            for x in prev:
                union(node_defs[(n.id, r)], x)
        # by-ref captures: later redefinitions of the register belong to the same variable
        for n in nodes:
            if n.kind in ('NEWCLOSURE', 'DUPCLOSURE'):
                for (t, idx) in n.a['caps']:
                    if t == 1:
                        s = usedefs[(n.id, idx)]
                        d0 = next(iter(s))
                        # forward walk
                        seen = set()
                        st = list(n.succ)
                        while st:
                            m = st.pop()
                            if m.id in seen:
                                continue
                            seen.add(m.id)
                            if m.kind == 'CLOSEUPVALS' and m.a['A'] <= idx:
                                continue
                            if idx in m.writes:
                                union(d0, node_defs[(m.id, idx)])
                                # redefinition keeps the same variable alive
                            for x in m.succ:
                                st.append(x)
        # a parameter register always belongs to the parameter variable: later writes are assignments to it
        for r in range(P['np']):
            for d_i in regdefs[r]:
                union(r, d_i)
        # build webs
        webs = {}
        for i, (n, r) in enumerate(defs):
            root = find(i)
            if root not in webs:
                webs[root] = Web()
                webs[root].fn = fn
                fn.webs.append(webs[root])
            w = webs[root]
            w.defs.append((i, n, r))
        defweb = {}
        for i in range(len(defs)):
            defweb[i] = webs[find(i)]
        for (nid, r), s in usedefs.items():
            w = defweb[next(iter(s))]
            w.uses.append((nid, r))
        use_web = {k: defweb[next(iter(s))] for k, s in usedefs.items()}
        node_def_web = {k: defweb[v] for k, v in node_defs.items()}
        fn.use_web = use_web
        fn.def_web = node_def_web
        fn.defs = defs
        fn.defweb = defweb
        # params
        for i in range(P['np']):
            w = defweb[i]
            w.kind = 'param'
            fn.params.append(w)
        for n in nodes:
            if n.kind in ('NEWCLOSURE', 'DUPCLOSURE'):
                for (t, idx) in n.a['caps']:
                    if t in (0, 1):
                        use_web[(n.id, idx)].captured = True
            if n.kind == 'FORNPREP':
                node_def_web[(n.id, n.a['A'] + 2)].kind = 'loop'
            if n.kind == 'FORGLOOP':
                for i in range(n.a['aux'] & 255):
                    node_def_web[(n.id, n.a['A'] + 3 + i)].kind = 'loop'
            if n.kind == 'FORNLOOP':
                w = node_def_web[(n.id, n.a['A'] + 2)]
                # make sure it is the same web as the FORNPREP one: they are merged when body uses read R[A+2]
                w.kind = 'loop'
        # webs that mix FORNPREP/FORNLOOP defs: if some loop has no body reads, they stay apart; fine

    def prepare(self, fn):
        self.build(fn)
        self.annotate(fn)
        self.make_blocks(fn)
        self.webs(fn)


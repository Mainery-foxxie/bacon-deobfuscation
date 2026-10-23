"""Compare the control-flow/semantic skeleton of the original (decoded VM) bytecode with the bytecode luau-compile
produces for the decompiled source.  Usage: bc_compare.py dump.txt recompiled.bc.txt [pid ...]"""
import sys, re, collections, difflib
from .decomp import Fn, Web, Decompiler

NONE_ID = 4294967295


def num(v):
    try:
        f = float(v)
    except Exception:
        return str(v)
    return '%.12g' % f


def sstr(b):
    if isinstance(b, bytes):
        b = b.decode('latin1')
    return b[:24]


# ----------------------------------------------------------------------------------------------------------- original
def kconst(P, i):
    c = P['consts'].get(i)
    if c is None:
        return '?'
    if c[0] == 's':
        return 'S' + sstr(c[1])
    if c[0] == 'n':
        return 'N' + num(c[1])
    if c[0] == 'b':
        return 'B' + str(c[1])
    return 'Z'


def kname(P, i):
    c = P['consts'].get(i)
    if c is not None and c[0] == 's':
        return sstr(c[1])
    return '?'


def import_path(P, aux):
    n = aux >> 30
    ks = [(aux >> 20) & 1023, (aux >> 10) & 1023, aux & 1023]
    return '.'.join(kname(P, k) for k in ks[:n])


ARITH = {'ADD': '+', 'SUB': '-', 'MUL': '*', 'DIV': '/', 'MOD': '%', 'POW': '^', 'IDIV': '//'}


def otok(P, name, a):
    A, B, C = a.get('A', 0), a.get('B', 0), a.get('C', 0)
    rf, aux = a.get('rf', 0), a.get('aux', 0)
    if name in ('MOVE', 'CLOSEUPVALS', 'NOP', 'JUNK'):
        return None
    if name == 'GETIMPORT':
        ip = import_path(P, aux)
        return None if ip == 'pcall' else 'IMP:' + ip
    if name == 'GETTABLEKS':
        return 'GT:' + kname(P, aux)
    if name == 'SETTABLEKS':
        return 'ST:' + kname(P, aux)
    if name == 'SELF':
        return 'NC:' + kname(P, aux)
    if name == 'GETTABLE':
        return 'GT[]'
    if name == 'SETTABLE':
        return 'ST[]'
    if name == 'GETTABLEN':
        return 'GTN:%d' % (C + 1)
    if name == 'SETTABLEN':
        return 'STN:%d' % (C + 1)
    if name == 'LOADK':
        return 'K:' + kconst(P, rf)[0:1].replace('S', 'S').replace('N', 'N') + kconst(P, rf)[1:]
    if name == 'LOADN':
        return 'K:N' + num(rf)
    if name == 'LOADB':
        return 'LB:%d' % B
    if name == 'LOADNIL':
        return 'LN'
    if name == 'GETUPVAL':
        return 'GU'
    if name == 'SETUPVAL':
        return 'SU'
    if name == 'CALL':
        return 'CALL:%d:%d' % (B - 1, C - 1)
    if name in ARITH:
        return 'OP:' + ARITH[name]
    if name.endswith('K') and name[:-1] in ARITH:
        return 'OP:%s:K:%s' % (ARITH[name[:-1]], kconst(P, C))
    if name.endswith('RK') and name[:-2] in ARITH:
        return 'OPR:%s:K:%s' % (ARITH[name[:-2]], kconst(P, B))
    if name == 'CONCAT':
        return 'CONCAT:%d' % (C - B + 1)
    if name == 'NOT':
        return 'NOT'
    if name == 'MINUS':
        return 'UNM'
    if name == 'LENGTH':
        return 'LEN'
    if name == 'OR':
        return 'OR'
    if name == 'AND':
        return 'AND'
    if name == 'ORK':
        return 'ORK:' + kconst(P, C)
    if name == 'ANDK':
        return 'ANDK:' + kconst(P, C)
    if name == 'NEWTABLE':
        return 'NEWT'
    if name == 'DUPTABLE':
        return 'DUPT'
    if name == 'SETLIST':
        return 'SETLIST:%d' % (C - 1)
    if name == 'NEWCLOSURE':
        return 'NCLO'
    if name == 'DUPCLOSURE':
        return 'DCLO'
    if name == 'CAPTURE':
        return 'CAP:%d' % A
    if name == 'FORGPREP':
        return 'FORGPREP'
    return 'UNK:' + name


def octrl_tok(P, name, a):
    A, B, C = a.get('A', 0), a.get('B', 0), a.get('C', 0)
    aux = a.get('aux', 0)
    if name == 'RETURN':
        return 'RET:%d' % (B - 1)
    if name == 'JUMPIF':
        return 'JIF'
    if name == 'JUMPIFNOT':
        return 'JIFN'
    if name == 'JUMPIFEQ':
        return 'JEQ'
    if name == 'JUMPIFNOTEQ':
        return 'JNEQ'
    if name == 'JUMPIFLE':
        return 'JLE'
    if name == 'JUMPIFNOTLE':
        return 'JNLE'
    if name == 'JUMPIFLT':
        return 'JLT'
    if name == 'JUMPIFNOTLT':
        return 'JNLT'
    if name == 'JUMPXEQK':
        c = P['consts'].get(aux & 0xFFFFFF)
        k = kconst(P, aux & 0xFFFFFF)
        return 'JXK:%s%s' % (k, '!' if (aux >> 31) & 1 else '')
    if name == 'JUMPXEQKB':
        return 'JXB:%d%s' % (aux & 1, '!' if (aux >> 31) & 1 else '')
    if name == 'JUMPXEQKNIL':
        return 'JXNIL%s' % ('!' if (aux >> 31) & 1 else '')
    if name == 'FORNPREP':
        return 'FORNPREP'
    if name == 'FORNLOOP':
        return 'FORNLOOP'
    if name == 'FORGLOOP':
        return 'FORGLOOP:%d' % (aux & 255)
    return 'UNKCTRL:' + name


COND = {'JIF', 'JIFN', 'JEQ', 'JNEQ', 'JLE', 'JNLE', 'JLT', 'JNLT', 'FORNPREP', 'FORNLOOP'}


def dead_loads(dec, pid):
    """(record id, instruction index) of constant/boolean/nil loads whose value is never read (obfuscator chaff or
    declarations that are overwritten before use)"""
    fn = Fn(dec, pid, [Web() for _ in range(dec.protos[pid]['nup'])])
    dec.prepare(fn)
    byrid = collections.defaultdict(list)
    for n in sorted(fn.nodes, key=lambda n_: n_.id):
        byrid[n.rid].append(n)
    dead = set()
    for rid, lst in byrid.items():
        for i, n in enumerate(lst):
            if n.kind in ('LOADK', 'LOADN', 'LOADB', 'LOADNIL'):
                w = fn.def_web.get((n.id, n.a['A']))
                if w is not None and not w.uses:
                    dead.add((rid, i))
    return dead


def orig_nodes(decoder, p, dead=None):
    P = decoder.protos[p]
    dead = dead or set()
    D = {rid: decoder.decode(p, rid) for rid in P['recs']}
    nodes = {}

    def first(rid):
        if rid is None or rid == NONE_ID or rid not in D:
            return None
        return (rid, 0)

    for rid, d in D.items():
        toks = []
        for ii, (name, a) in enumerate(d['ins']):
            if (rid, ii) in dead:
                continue
            t = otok(P, name, a)
            if t is not None:
                toks.append(t)
        ctrl = d['ctrl']
        kind_ctrl = None
        if ctrl:
            cn = ctrl[0]
            if cn == 'JUMP':
                kind_ctrl = 'jump'
            elif cn == 'RETURN':
                toks.append(octrl_tok(P, cn, ctrl[1]))
                kind_ctrl = 'ret'
            else:
                toks.append(octrl_tok(P, cn, ctrl[1]))
                kind_ctrl = 'cond'
        nxt_f, alt_f = first(d['nxt']), first(d['alt'])
        if not toks:
            if kind_ctrl == 'jump':
                nodes[(rid, 0)] = dict(tok=None, nxt=alt_f, alt=None)
            else:
                nodes[(rid, 0)] = dict(tok=None, nxt=nxt_f, alt=None)
            continue
        for i, t in enumerate(toks):
            last = i == len(toks) - 1
            nid = (rid, i)
            if not last:
                nodes[nid] = dict(tok=t, nxt=(rid, i + 1), alt=None)
            else:
                if kind_ctrl == 'jump':
                    nodes[nid] = dict(tok=t, nxt=alt_f, alt=None)
                elif kind_ctrl == 'ret':
                    nodes[nid] = dict(tok=t, nxt=None, alt=None)
                elif kind_ctrl == 'cond':
                    nodes[nid] = dict(tok=t, nxt=nxt_f, alt=alt_f)
                else:
                    nodes[nid] = dict(tok=t, nxt=nxt_f, alt=None)
    return nodes, first(P['entry'])


# ----------------------------------------------------------------------------------------------------------- luau text
def parse_luau(path):
    funcs = {}
    cur = None
    consts = None
    for ln in open(path, encoding='latin1'):
        ln = ln.rstrip('\n')
        m = re.match(r'^Function (\d+) ', ln)
        if m:
            cur = int(m.group(1))
            funcs[cur] = []
            continue
        if cur is None or not ln or ln.startswith(('REMARK', 'slot ')) or re.match(r'^K\d+:', ln):
            continue
        funcs[cur].append(ln)
    return funcs


def ann(ln):
    m = re.search(r'\[(.*)\]\s*$', ln)
    if m is None or re.match(r'^(?:L\d+: )?(SUBRK|DIVRK)\b', ln):
        m2 = re.search(r'\[(.*?)\]', ln)
        if m2:
            return m2.group(1)
    return m.group(1) if m else None


def unq(s):
    if s is None:
        return '?'
    m = re.match(r"^'(.*?)'(\.\.\.)?$", s, re.S)
    if m:
        return m.group(1)[:24]
    return s


def luau_nodes(lines):
    insts = []
    labels = {}
    for ln in lines:
        m = re.match(r'^(?:(L\d+): )?([A-Z_0-9]+)\b ?(.*)$', ln)
        if not m:
            raise Exception('unparsed ' + ln)
        lab, op, rest = m.groups()
        idx = len(insts)
        if lab:
            labels[lab] = idx
        insts.append((op, rest, ln))
    nodes = {}
    n = len(insts)
    for i, (op, rest, ln) in enumerate(insts):
        a = ann(ln)
        toks = rest.split()
        lbl = next((t for t in toks if re.fullmatch(r'L\d+', t)), None)
        tgt = labels.get(lbl) if lbl else None
        nxt = i + 1 if i + 1 < n else None
        tok = None
        edge_kind = 'seq'
        if op in ('MOVE', 'NOP', 'CLOSEUPVALS', 'PREPVARARGS', 'COVERAGE', 'FASTCALL', 'FASTCALL1', 'FASTCALL2', 'FASTCALL2K', 'FASTCALL3',
                  'FASTPCALL', 'DEP_JUMPIFEQK', 'DEP_JUMPIFNOTEQK'):
            tok = None
        elif op == 'GETIMPORT':
            tok = None if a == 'pcall' else 'IMP:' + (a or '?')
        elif op == 'GETTABLEKS':
            tok = 'GT:' + unq(a)
        elif op == 'SETTABLEKS':
            tok = 'ST:' + unq(a)
        elif op == 'NAMECALL':
            tok = 'NC:' + unq(a)
        elif op == 'GETTABLE':
            tok = 'GT[]'
        elif op == 'SETTABLE':
            tok = 'ST[]'
        elif op == 'GETTABLEN':
            tok = 'GTN:' + toks[2]
        elif op == 'SETTABLEN':
            tok = 'STN:' + toks[2]
        elif op == 'LOADK':
            v = a
            if v is not None and v.startswith("'"):
                tok = 'K:S' + unq(v)
            else:
                tok = 'K:N' + num(v)
        elif op == 'LOADN':
            tok = 'K:N' + num(toks[1])
        elif op == 'LOADB':
            tok = 'LB:' + toks[1]
            if len(toks) > 2 and re.fullmatch(r'\+\d+', toks[2]):
                tgt = i + 1 + int(toks[2][1:])
                edge_kind = 'jump'
        elif op == 'LOADNIL':
            tok = 'LN'
        elif op == 'GETUPVAL':
            tok = 'GU'
        elif op == 'SETUPVAL':
            tok = 'SU'
        elif op in ('CALL', 'CALLFB'):
            tok = 'CALL:%s:%s' % (toks[1], toks[2])
        elif op in ('ADD', 'SUB', 'MUL', 'DIV', 'MOD', 'POW', 'IDIV'):
            tok = 'OP:' + ARITH[op]
        elif op in ('ADDK', 'SUBK', 'MULK', 'DIVK', 'MODK', 'POWK', 'IDIVK'):
            tok = 'OP:%s:K:%s' % (ARITH[op[:-1]], ('S' + unq(a)) if (a or '').startswith("'") else 'N' + num(a))
        elif op in ('SUBRK', 'DIVRK'):
            tok = 'OPR:%s:K:%s' % (ARITH[op[:-2]], ('S' + unq(a)) if (a or '').startswith("'") else 'N' + num(a))
        elif op == 'CONCAT':
            rb, rc = int(toks[1][1:]), int(toks[2][1:])
            tok = 'CONCAT:%d' % (rc - rb + 1)
        elif op == 'NOT':
            tok = 'NOT'
        elif op == 'MINUS':
            tok = 'UNM'
        elif op == 'LENGTH':
            tok = 'LEN'
        elif op == 'OR':
            tok = 'OR'
        elif op == 'AND':
            tok = 'AND'
        elif op == 'ORK':
            tok = 'ORK:' + (('S' + unq(a)) if (a or '').startswith("'") else 'N' + num(a))
        elif op == 'ANDK':
            tok = 'ANDK:' + (('S' + unq(a)) if (a or '').startswith("'") else 'N' + num(a))
        elif op == 'NEWTABLE':
            tok = 'NEWT'
        elif op == 'DUPTABLE':
            tok = 'DUPT'
        elif op == 'SETLIST':
            tok = 'SETLIST:' + toks[2]
        elif op == 'NEWCLOSURE':
            tok = 'NCLO'
        elif op == 'DUPCLOSURE':
            tok = 'DCLO'
        elif op == 'CAPTURE':
            tok = 'CAP:%d' % {'VAL': 0, 'REF': 1, 'UPVAL': 2}[toks[0]]
        elif op == 'RETURN':
            tok = 'RET:' + toks[1]
            edge_kind = 'ret'
        elif op in ('JUMP', 'JUMPBACK', 'JUMPX'):
            tok = None
            edge_kind = 'jump'
        elif op == 'JUMPIF':
            tok, edge_kind = 'JIF', 'cond'
        elif op == 'JUMPIFNOT':
            tok, edge_kind = 'JIFN', 'cond'
        elif op == 'JUMPIFEQ':
            tok, edge_kind = 'JEQ', 'cond'
        elif op == 'JUMPIFNOTEQ':
            tok, edge_kind = 'JNEQ', 'cond'
        elif op == 'JUMPIFLE':
            tok, edge_kind = 'JLE', 'cond'
        elif op == 'JUMPIFNOTLE':
            tok, edge_kind = 'JNLE', 'cond'
        elif op == 'JUMPIFLT':
            tok, edge_kind = 'JLT', 'cond'
        elif op == 'JUMPIFNOTLT':
            tok, edge_kind = 'JNLT', 'cond'
        elif op in ('JUMPXEQKS', 'JUMPXEQKN'):
            kk = ('S' + unq(a)) if op == 'JUMPXEQKS' else 'N' + num(a)
            tok, edge_kind = 'JXK:%s%s' % (kk, '!' if 'NOT' in toks else ''), 'cond'
        elif op == 'JUMPXEQKB':
            tok, edge_kind = 'JXB:%s%s' % (toks[1], '!' if 'NOT' in toks else ''), 'cond'
        elif op == 'JUMPXEQKNIL':
            tok, edge_kind = 'JXNIL%s' % ('!' if 'NOT' in toks else ''), 'cond'
        elif op == 'FORNPREP':
            tok, edge_kind = 'FORNPREP', 'cond'
        elif op == 'FORNLOOP':
            tok, edge_kind = 'FORNLOOP', 'cond'
        elif op == 'FORGLOOP':
            tok, edge_kind = 'FORGLOOP:' + toks[2], 'cond'
        elif op in ('FORGPREP', 'FORGPREP_INEXT', 'FORGPREP_NEXT'):
            tok, edge_kind = 'FORGPREP', 'jump'
        else:
            tok = 'UNK:' + op
        if edge_kind == 'seq':
            nodes[i] = dict(tok=tok, nxt=nxt, alt=None)
        elif edge_kind == 'jump':
            nodes[i] = dict(tok=tok, nxt=tgt, alt=None)
        elif edge_kind == 'ret':
            nodes[i] = dict(tok=tok, nxt=None, alt=None)
        else:
            nodes[i] = dict(tok=tok, nxt=nxt, alt=tgt)
    return nodes, (0 if n else None)


# ----------------------------------------------------------------------------------------------------------- canonical form
CONDMAP = {'JIF': ('JIF', True), 'JIFN': ('JIF', False), 'JEQ': ('JEQ', True), 'JNEQ': ('JEQ', False),
           'JLE': ('JLE', True), 'JNLE': ('JLE', False), 'JLT': ('JLT', True), 'JNLT': ('JLT', False)}


def cond_norm(tok):
    """return (normalized token, true_is_alt) for orientation-insensitive conditional jumps, or None"""
    if tok in CONDMAP:
        return CONDMAP[tok]
    if tok.startswith(('JXK:', 'JXB:')) or tok.startswith('JXNIL'):
        if tok.endswith('!'):
            return tok[:-1], False
        return tok, True
    return None


def expand_or(nodes):
    """expand OR/ORK/AND/ANDK value operators into the equivalent jump form (compile-shape independent)"""
    for nid in list(nodes.keys()):
        n = nodes[nid]
        t = n['tok']
        if t is None:
            continue
        if t in ('OR', 'AND') or t.startswith(('ORK:', 'ANDK:')):
            is_or = t.startswith('OR')
            after = n['nxt']
            if t.startswith(('ORK:', 'ANDK:')):
                kid = (nid, 'k')
                nodes[kid] = dict(tok='K:' + t.split(':', 1)[1], nxt=after, alt=None)
                n['nxt'] = kid
            n['tok'] = 'JIF' if is_or else 'JIFN'
            n['alt'] = after
    return nodes


def canon(nodes, entry, relax=None):
    """canonical labelled-graph sequence.  relax: set of token prefixes treated as transparent"""
    nodes = {k: dict(v) for k, v in nodes.items()}
    expand_or(nodes)
    if relax:
        for n in nodes.values():
            if n['tok'] is not None and n['tok'].startswith(relax):
                n['tok'] = None
    # orientation normalisation
    for n in nodes.values():
        if n['tok'] is None:
            continue
        cn = cond_norm(n['tok'])
        if cn is not None:
            base, true_is_alt = cn
            if true_is_alt:
                n['t'], n['f'] = n['alt'], n['nxt']
            else:
                n['t'], n['f'] = n['nxt'], n['alt']
            n['tok'] = base
            n['cond'] = True
    def res(i):
        seen = set()
        while i is not None and nodes[i]['tok'] is None:
            if i in seen:
                return None
            seen.add(i)
            i = nodes[i]['nxt']
        return i

    # a conditional jump whose two outcomes lead to the same place has no effect (degenerate branch)
    changed = True
    while changed:
        changed = False
        for k_, d_ in nodes.items():
            if d_['tok'] is not None and d_.get('cond'):
                a_, b_ = res(d_['t']), res(d_['f'])
                if a_ == b_ and a_ != k_:
                    d_['tok'] = None
                    d_['nxt'] = a_
                    d_['alt'] = None
                    d_['cond'] = False
                    changed = True

    def succs(n):
        d = nodes[n]
        if d.get('cond'):
            return [res(d['t']), res(d['f'])]
        return [res(d['nxt']), res(d['alt'])]

    order, idx = [], {}
    start = res(entry)
    if start is None:
        return []
    idx[start] = 0
    order.append(start)
    stack = [(start, 0)]
    while stack:
        n, st = stack[-1]
        if st >= 2:
            stack.pop()
            continue
        stack[-1] = (n, st + 1)
        s = succs(n)[st]
        if s is not None and s not in idx:
            idx[s] = len(order)
            order.append(s)
            stack.append((s, 0))
    out = []
    for i, n in enumerate(order):
        a, b = succs(n)
        e1 = None if a is None else ('n' if idx[a] == i + 1 else idx[a])
        e2 = None if b is None else idx[b]
        out.append((nodes[n]['tok'], e1, e2))
    return out


PURE = ('GU', 'IMP:', 'K:', 'LB:', 'LN', 'GT:', 'GT[]', 'GTN:', 'OP:', 'OPR:', 'NOT', 'UNM', 'LEN', 'CONCAT:')


def pure_multiset(nodes):
    nodes = {k: dict(v) for k, v in nodes.items()}
    expand_or(nodes)
    c = collections.Counter()
    for n in nodes.values():
        t = n['tok']
        if t is not None and t.startswith(PURE):
            c[t] += 1
    return c


def similarity(a, b):
    sa = [str(x[0]) for x in a]
    sb = [str(x[0]) for x in b]
    return difflib.SequenceMatcher(None, sa, sb, autojunk=False).ratio()


def show_diff(a, b, out):
    ta = [x[0] for x in a]
    tb = [x[0] for x in b]
    sm = difflib.SequenceMatcher(None, ta, tb, autojunk=False)
    nedge = 0
    for tag, i1, i2, j1, j2 in sm.get_opcodes():
        if tag == 'equal':
            nedge += sum(1 for k in range(i2 - i1) if a[i1 + k][1:] != b[j1 + k][1:])
            continue
        out.append('   %s orig[%d:%d] %s' % (tag, i1, i2, ta[max(0, i1 - 3):i2 + 3]))
        out.append('      new[%d:%d]  %s' % (j1, j2, tb[max(0, j1 - 3):j2 + 3]))
    out.append('   (edge-only mismatches in matching tokens: %d)' % nedge)


def compare(decoder, bc_path, report_path=None):
    """compare the decoded program with the bytecode luau-compile makes of the decompiled source"""
    protos = decoder.protos
    dec = Decompiler(decoder)
    lf = parse_luau(bc_path)
    A, B, A2, B2, MA, MB = {}, {}, {}, {}, {}, {}
    ndead = 0
    for p_ in sorted(protos):
        dead = dead_loads(dec, p_)
        ndead += len(dead)
        on, oe = orig_nodes(decoder, p_, dead)
        A[p_] = canon(on, oe, relax=('LN',))
        A2[p_] = canon(on, oe, relax=PURE)
        MA[p_] = pure_multiset(on)
    for f_ in sorted(lf):
        ln_, le = luau_nodes(lf[f_])
        B[f_] = canon(ln_, le, relax=('LN',))
        B2[f_] = canon(ln_, le, relax=PURE)
        MB[f_] = pure_multiset(ln_)
    keyS = lambda s: repr(s)
    keyR = lambda s, m: repr(s) + repr(sorted(m.items()))
    match = {}
    level = {}
    used = set()
    for stage, (ka, kb) in enumerate([(lambda p_: keyS(A[p_]), lambda f_: keyS(B[f_])),
                                      (lambda p_: keyR(A2[p_], MA[p_]), lambda f_: keyR(B2[f_], MB[f_]))]):
        pool = collections.defaultdict(list)
        for f_ in B:
            if f_ not in used:
                pool[kb(f_)].append(f_)
        for p_ in sorted(A):
            if p_ in match:
                continue
            lst = pool.get(ka(p_))
            if lst:
                cand = p_ if p_ in lst else lst[0]
                lst.remove(cand)
                match[p_] = cand
                used.add(cand)
                level[p_] = stage
    rest_a = [p_ for p_ in A if p_ not in match]
    rest_b = [f_ for f_ in B if f_ not in used]
    sims = []
    for p_ in rest_a:
        for f_ in rest_b:
            la, lb = len(A[p_]), len(B[f_])
            if min(la, lb) * 2 < max(la, lb) - 6:
                continue
            sims.append((similarity(A[p_], B[f_]), p_, f_))
    sims.sort(reverse=True)
    done_a, done_b = set(), set()
    for s, p_, f_ in sims:
        if p_ in done_a or f_ in done_b:
            continue
        match[p_] = f_
        level[p_] = 2
        done_a.add(p_)
        done_b.add(f_)
    for p_ in rest_a:
        if p_ not in match:
            match[p_] = None
            level[p_] = 3
    n0 = sum(1 for p_ in A if level[p_] == 0)
    n1 = sum(1 for p_ in A if level[p_] == 1)
    out = []
    bad = []
    for p_ in sorted(A):
        f_ = match.get(p_)
        if level[p_] <= 1:
            continue
        bad.append(p_)
        if f_ is None:
            out.append('proto %d: NO MATCH' % p_)
            continue
        la = [t[0] for t in A[p_]]
        lb = [t[0] for t in B[f_]]
        sim = similarity(A[p_], B[f_])
        out.append('=== orig proto %d ~ new function %d  (nodes %d vs %d, token-seq %s, sim %.3f)' % (
            p_, f_, len(A[p_]), len(B[f_]), 'equal' if la == lb else 'differs', sim))
        show_diff(A[p_], B[f_], out)
        ma, mb = MA[p_], MB[f_]
        extra_a = ma - mb
        extra_b = mb - ma
        if extra_a or extra_b:
            out.append('   pure-op multiset: only-orig %s | only-new %s' % (dict(extra_a), dict(extra_b)))
    reord = [p_ for p_ in sorted(A) if level[p_] == 1]
    cosmetic = []
    for p_ in bad:
        f_ = match.get(p_)
        if f_ is None:
            continue
        ta = [t[0] for t in A[p_]]
        tb = [t[0] for t in B[f_]]
        if ta.count('RET:0') > tb.count('RET:0') >= 1 and \
                [t for t in ta if t != 'RET:0'] == [t for t in tb if t != 'RET:0']:
            cosmetic.append(p_)
    if report_path:
        with open(report_path, 'w') as f:
            f.write('\n'.join(out) + '\n')
    return dict(functions_orig=len(A), functions_new=len(B), identical=n0, reordered=n1, differing=bad,
                reordered_protos=reord, cosmetic=cosmetic, dead_loads=ndead, report=out, match=match)

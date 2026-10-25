"""Structural analysis of a Bacon Guard (FORGE) loader: finds the VM pieces independently of identifier names."""
from .astutil import (Src, normalize_source, parse_ast, walk, kids, lkey, unparen)


class LayoutError(Exception):
    pass


def is_in(e, name):
    return e is not None and e['type'] == 'AstExprIndexName' and e['index'] == name


def loc_of(e):
    e = unparen(e)
    if e['type'] == 'AstExprLocal':
        return e['local']
    return None


def const_num(e):
    e = unparen(e)
    if e['type'] == 'AstExprConstantNumber':
        return e['value']
    return None


def eq_info(c):
    """`local == number` (either side) -> (local, number); else None"""
    c = unparen(c)
    if c['type'] == 'AstExprBinary' and c['op'] == 'CompareEq':
        l, r = unparen(c['left']), unparen(c['right'])
        if l['type'] == 'AstExprLocal' and r['type'] == 'AstExprConstantNumber':
            return l['local'], r['value']
        if r['type'] == 'AstExprLocal' and l['type'] == 'AstExprConstantNumber':
            return r['local'], l['value']
    return None


def if_chain(n):
    """follow an if/elseif ladder -> ([(cond, body_block, node)], final_else_block_or_None)"""
    out = []
    cur = n
    final = None
    while cur is not None and cur['type'] == 'AstStatIf':
        out.append((cur['condition'], cur['thenbody'], cur))
        e = cur.get('elsebody')
        if e is None:
            break
        if e['type'] == 'AstStatIf':
            cur = e
        else:
            final = e
            break
    return out, final


def or_terms(c):
    c = unparen(c)
    if c['type'] == 'AstExprBinary' and c['op'] == 'Or':
        return or_terms(c['left']) + or_terms(c['right'])
    return [c]


class Loader(object):
    def __init__(self, data):
        self.text = normalize_source(data)
        self.src = Src(self.text)
        self.root = parse_ast(self.text)
        self.parent = {}
        for n, p in walk(self.root):
            self.parent[id(n)] = p
        self.funcs = {}        # local key -> AstExprFunction (local function f / local f = function)
        self.local_decl = {}   # local key -> (stmt node, kind)
        for n, p in walk(self.root):
            t = n['type']
            if t == 'AstStatLocalFunction':
                self.funcs[lkey(n['name'])] = n['func']
                self.local_decl[lkey(n['name'])] = (n, 'localfn')
            elif t == 'AstStatLocal':
                for i, v in enumerate(n['vars']):
                    self.local_decl[lkey(v)] = (n, 'local')
                    if i < len(n['values']) and unparen(n['values'][i])['type'] == 'AstExprFunction' and len(n['vars']) == len(n['values']):
                        self.funcs[lkey(v)] = unparen(n['values'][i])
        self.roles = {}        # local key -> role name
        self.role_local = {}   # role name -> local dict
        self.analyse()

    # ------------------------------------------------------------------ helpers
    def encl_fn(self, n):
        p = self.parent.get(id(n))
        while p is not None and p['type'] != 'AstExprFunction':
            p = self.parent.get(id(p))
        return p

    def set_role(self, local, name):
        if local is None:
            raise LayoutError('role %s: local not found' % name)
        k = lkey(local)
        self.roles[k] = name
        self.role_local[name] = local

    def role_of(self, e):
        e = unparen(e)
        if e['type'] == 'AstExprLocal':
            return self.roles.get(lkey(e['local']))
        return None

    # ------------------------------------------------------------------ main analysis
    def analyse(self):
        mx = [n for n, p in walk(self.root) if is_in(n, 'maxStack')]
        if not mx:
            raise LayoutError('not a Bacon Guard loader: no VM frame found')
        # the closure factory may build several kinds of closure (newer builds add a precompiled fast path before the
        # interpreter); the interpreter frame is the one that fetches instruction records
        frames = []
        for n in mx:
            f = self.encl_fn(n)
            if f is not None and f['vararg'] and not any(f is g for g in frames):
                frames.append(f)
        FRAME = None
        for f in frames:
            if any(is_in(n, 'fetchInstruction') for n, p in walk(f)):
                FRAME = f
                break
        if FRAME is None and frames:
            FRAME = frames[0]
        if FRAME is None or not FRAME['vararg']:
            raise LayoutError('unexpected frame function')
        MK = self.encl_fn(FRAME)
        MOD = self.encl_fn(MK)
        if MK is None or len(MK['args']) != 2 or MOD is None:
            raise LayoutError('unexpected proto-closure factory')
        self.FRAME, self.MK, self.MOD = FRAME, MK, MOD
        M, UP = MK['args'][0], MK['args'][1]
        self.set_role(M, 'PROTO')
        self.set_role(UP, 'UP')
        if len(MOD['args']) >= 2:
            self.set_role(MOD['args'][1], 'ENV')
        mk_local = None
        p = self.parent.get(id(MK))
        if p is not None and p['type'] == 'AstStatLocalFunction':
            mk_local = p['name']
        elif p is not None and p['type'] == 'AstStatLocal':
            for v, val in zip(p['vars'], p['values']):
                if unparen(val) is MK:
                    mk_local = v
        if mk_local is None:
            raise LayoutError('proto factory name not found')
        self.set_role(mk_local, 'MK')
        self.mk_stmt = p

        def based(e, name):
            return is_in(e, name) and loc_of(e['expr']) is not None and lkey(loc_of(e['expr'])) == lkey(M)

        # newer builds read the constant table in the factory, before the frame closures are made
        for s in MK['body']['body']:
            if s['type'] == 'AstStatLocal' and len(s['vars']) == 1 and len(s['values']) == 1 \
                    and based(unparen(s['values'][0]), 'constantReader'):
                self.set_role(s['vars'][0], 'KGET')
        body = FRAME['body']['body']
        self.frame_body = body
        idx = {}
        for i, s in enumerate(body):
            t = s['type']
            if t == 'AstStatLocal' and len(s['vars']) == 1 and len(s['values']) == 1:
                v = unparen(s['values'][0])
                var = s['vars'][0]
                if v['type'] == 'AstExprBinary' and v['op'] == 'Or' and unparen(v['right'])['type'] == 'AstExprCall':
                    cl = unparen(v['right'])
                    if cl['args'] and based(unparen(cl['args'][0]), 'maxStack') and loc_of(v['left']) is not None:
                        self.set_role(var, 'REGS')
                        self.set_role(loc_of(v['left']), 'RECYCLE')
                        idx['REGS'] = i
                elif based(v, 'instructionReader'):
                    self.set_role(var, 'READER')
                    idx['READER'] = i
                elif based(v, 'constantReader'):
                    self.set_role(var, 'KGET')
                    idx['KGET'] = i
                elif based(v, 'simpleFields'):
                    self.set_role(var, 'SIMPLEF')
                    idx['SIMPLEF'] = i
                elif based(v, 'entry'):
                    self.set_role(var, 'PC')
                elif based(v, 'fetchInstruction'):
                    self.set_role(var, 'FETCH')
                elif v['type'] == 'AstExprIndexExpr' and based(unparen(v['index']), 'index') and loc_of(v['expr']) is not None:
                    self.set_role(var, 'PROF')
                    self.set_role(loc_of(v['expr']), 'PROFTAB')
                    idx['PROF'] = i
                elif v['type'] == 'AstExprUnary' and v['op'] == 'Minus' and const_num(v['expr']) == 1:
                    self.set_role(var, 'TOP')
                elif based(v, 'numParams'):
                    idx['NUMPARAMS'] = i
            elif t == 'AstStatLocal' and len(s['vars']) == 1 and len(s['values']) == 0:
                idx.setdefault('EMPTY', []).append((i, s['vars'][0]))
        for n, p in walk(MK):
            if n['type'] == 'AstStatAssign' and len(n['vars']) == 1 and len(n['values']) == 1:
                tg = unparen(n['vars'][0])
                v = unparen(n['values'][0])
                if tg['type'] != 'AstExprLocal':
                    continue
                if based(v, 'importReader'):
                    self.set_role(tg['local'], 'IMP')
                elif based(v, 'fusedFields'):
                    self.set_role(tg['local'], 'FUSED')
                elif based(v, 'captureFields'):
                    self.set_role(tg['local'], 'CAPF')
                elif v['type'] == 'AstExprFunction' and len(v['args']) >= 10 and tg['local']['name'] and 'CTRL' not in self.role_local and self.encl_fn(n) is FRAME:
                    self.set_role(tg['local'], 'CTRL')
                    self.ctrl_fn = v
        for need in ('REGS', 'KGET', 'IMP', 'FUSED', 'SIMPLEF', 'PROF', 'CTRL', 'TOP', 'PC', 'FETCH', 'READER', 'CAPF'):
            if need not in self.role_local:
                raise LayoutError('loader role not found: ' + need)
        # statements around numParams: `local VARARGS = <module table>` then `local NVAR = 0`
        ip = idx.get('NUMPARAMS')
        if ip is None:
            raise LayoutError('loader role not found: NUMPARAMS')
        for j in range(ip + 1, min(ip + 10, len(body))):
            s = body[j]
            if s['type'] == 'AstStatLocal' and len(s['vars']) == 1 and len(s['values']) == 1:
                v = unparen(s['values'][0])
                if v['type'] == 'AstExprLocal' and 'VARARGS' not in self.role_local:
                    self.set_role(s['vars'][0], 'VARARGS')
                    self.set_role(v['local'], 'NOVARARGS')
                elif v['type'] == 'AstExprConstantNumber' and v['value'] == 0 and 'NVAR' not in self.role_local:
                    self.set_role(s['vars'][0], 'NVAR')
        # simple-op set: `local x = <module set>` right after the simpleFields statements
        isf = idx['SIMPLEF']
        for j in range(isf + 1, min(isf + 5, len(body))):
            s = body[j]
            if s['type'] == 'AstStatLocal' and len(s['vars']) == 1 and len(s['values']) == 1 and unparen(s['values'][0])['type'] == 'AstExprLocal':
                self.set_role(s['vars'][0], 'SIMPLESETV')
                self.set_role(unparen(s['values'][0])['local'], 'SIMPLESET')
                break
        if 'SIMPLESET' not in self.role_local:
            raise LayoutError('simple-op set not found')
        # the loop (newer builds wrap it in a `do ... end` block)
        def find_loops(stmts):
            out = []
            for s in stmts:
                if s['type'] in ('AstStatWhile', 'AstStatRepeat'):
                    out.append(s)
                elif s['type'] == 'AstStatBlock':
                    out.extend(find_loops(s['body']))
            return out

        loops = find_loops(body)
        if not loops:
            raise LayoutError('interpreter loop not found')
        self.loop = loops[-1]
        self.analyse_loop(M)
        self.analyse_module(MOD, M)
        self.analyse_fields(M)

    # ------------------------------------------------------------------ loop
    def analyse_loop(self, M):
        lb = self.loop['body']['body']
        i = 0
        # fetch
        while i < len(lb):
            s = lb[i]
            if s['type'] == 'AstStatLocal' and len(s['vars']) == 2 and len(s['values']) == 1 and unparen(s['values'][0])['type'] == 'AstExprCall':
                break
            i += 1
        else:
            raise LayoutError('loop fetch not found')
        s = lb[i]
        self.set_role(s['vars'][0], 'REC')
        self.set_role(s['vars'][1], 'CLR')
        s10 = lb[i + 1]
        if s10['type'] != 'AstStatLocal' or len(s10['vars']) != 10 or s10['values']:
            raise LayoutError('field locals not found')
        names = ['OP', 'EF', 'A', 'B', 'C', 'RF', 'HF', 'AUX', 'NXT', 'ALT']
        for v, nm in zip(s10['vars'], names):
            self.set_role(v, nm)
        # permutations
        permst = lb[i + 2]
        self.perm = self.dispatch_perm(permst, 10, assign=True)
        j = i + 3
        sem = lb[j]
        if not (sem['type'] == 'AstStatLocal' and unparen(sem['values'][0])['type'] == 'AstExprIndexExpr'):
            raise LayoutError('semantic key not found')
        semv = unparen(sem['values'][0])
        self.sem_index = const_num(semv['index'])
        if self.sem_index is None:
            raise LayoutError('semantic key index not constant')
        self.set_role(sem['vars'][0], 'SEM')
        # D1..D5 locals
        tabs = []
        for k in range(1, 6):
            st = lb[j + k]
            if st['type'] != 'AstStatLocal':
                raise LayoutError('dedicated table %d not found' % k)
            v = unparen(st['values'][0])
            tabs.append((st['vars'][0], loc_of(v['expr'])))
        ifd6 = lb[j + 6]
        if ifd6['type'] != 'AstStatIf':
            raise LayoutError('dedicated table 6 not found')
        asg = ifd6['thenbody']['body'][0]
        d6 = unparen(asg['values'][0])
        d6tab = loc_of(d6['expr'])
        p1st = lb[j + 7]
        p1v = unparen(p1st['values'][0])
        p1var, p1tab = p1st['vars'][0], loc_of(p1v['expr'])
        ifp2 = lb[j + 8]
        p2tab = loc_of(unparen(ifp2['thenbody']['body'][0]['values'][0])['expr'])
        for n, (var, tab) in enumerate(tabs):
            self.set_role(tab, 'D%d' % (n + 1))
            self.set_role(var, 'D%dV' % (n + 1))
        self.set_role(d6tab, 'D6')
        self.set_role(p1tab, 'P1')
        self.set_role(p2tab, 'P2')
        self.set_role(p1var, 'PATV')
        d5var = tabs[4][0]
        chain_stmt = lb[j + 9]
        if chain_stmt['type'] != 'AstStatIf':
            raise LayoutError('dispatch chain not found')
        self.dispatch(chain_stmt)

    def dispatch_perm(self, ifst, n, assign):
        """`if M.dispatchFormat == k then <assign/return indices>` ladder -> {k: [indices]}"""
        res = {}
        ch, fin = if_chain(ifst)
        for cond, body, node in ch:
            c = unparen(cond)
            if not (c['type'] == 'AstExprBinary' and c['op'] == 'CompareEq' and is_in(unparen(c['left']), 'dispatchFormat')):
                raise LayoutError('dispatchFormat ladder expected')
            k = const_num(c['right'])
            st = body['body'][0]
            if st['type'] == 'AstStatAssign':
                vals = st['values']
            elif st['type'] == 'AstStatReturn':
                vals = st['list']
                if len(vals) == 1 and unparen(vals[0])['type'] == 'AstExprTable':
                    vals = [it['value'] for it in unparen(vals[0])['items']]
            else:
                raise LayoutError('dispatchFormat body')
            ix = []
            for v in vals:
                v = unparen(v)
                if v['type'] != 'AstExprIndexExpr' or const_num(v['index']) is None:
                    raise LayoutError('dispatchFormat index')
                ix.append(int(const_num(v['index'])))
            res[int(k)] = ix
        return res

    def dispatch(self, st):
        ch, fin = if_chain(st)
        ctrl = []
        self.branches = {}
        for cond, body, node in ch:
            c = unparen(cond)
            # D tables
            if c['type'] == 'AstExprBinary' and c['op'] == 'CompareNe' and loc_of(c['left']) is not None:
                name = self.roles.get(lkey(loc_of(c['left'])))
                if name and name.startswith('D') and name.endswith('V'):
                    self.branches[name] = body
                    continue
                if name == 'PATV':
                    self.branches['PAT'] = body
                    continue
            if c['type'] == 'AstExprBinary' and c['op'] == 'CompareEq':
                e = eq_info(c)
                if e is not None and self.roles.get(lkey(e[0])) == 'OP':
                    self.branches['JUNK'] = (body, int(e[1]))
                    continue
            ts = or_terms(c)
            if len(ts) >= 2 and all(eq_info(t) is not None for t in ts):
                self.branches['HOT'] = (body, [int(eq_info(t)[1]) for t in ts])
                continue
            # `SIMPLESET[op] == true` (older builds) / `SIMPLESET[op] ~= nil` (newer builds, where the set maps an op to its group)
            if c['type'] == 'AstExprBinary' and c['op'] in ('CompareEq', 'CompareNe') \
                    and unparen(c['left'])['type'] == 'AstExprIndexExpr':
                self.branches['SIMPLE'] = body
                continue
            raise LayoutError('unrecognised dispatch branch at ' + node['location'])
        if fin is None:
            raise LayoutError('control fallback missing')
        self.branches['CTRLCALL'] = fin
        for need in ('D1V', 'D2V', 'D3V', 'D4V', 'D5V', 'PAT', 'JUNK', 'HOT', 'SIMPLE'):
            if need not in self.branches:
                raise LayoutError('dispatch branch missing: ' + need)

    # ------------------------------------------------------------------ module level
    def analyse_module(self, MOD, M):
        # roles used by the dumper: protos table, main index, ded/pattern tables, xorkey
        mb = MOD['body']['body']
        # the module function ends with: local main = PROTOS[MAINIDX]; if main == nil then error end; return MK(main, {})
        last = mb[-1]
        if not (last['type'] == 'AstStatReturn' and len(last['list']) == 1 and unparen(last['list'][0])['type'] == 'AstExprCall'):
            raise LayoutError('module tail not found')
        mkcall = unparen(last['list'][0])
        if loc_of(mkcall['func']) is None or lkey(loc_of(mkcall['func'])) != lkey(self.role_local['MK']):
            raise LayoutError('module tail does not call the proto factory')
        main_local = loc_of(mkcall['args'][0])
        self.set_role(main_local, 'MAIN')
        # `local main = PROTOS[MAINIDX]`; newer builds put other statements (precompiled handlers, prefetching) after it
        dl = self.local_decl.get(lkey(main_local))
        ms = dl[0] if dl is not None else None
        mv = None
        if ms is not None and ms['type'] == 'AstStatLocal':
            for v_, val_ in zip(ms['vars'], ms['values']):
                if lkey(v_) == lkey(main_local):
                    mv = unparen(val_)
        if mv is None or mv['type'] != 'AstExprIndexExpr' or not any(s is ms for s in mb):
            raise LayoutError('main proto statement not found')
        self.set_role(loc_of(mv['expr']), 'PROTOS')
        self.set_role(loc_of(mv['index']), 'MAINIDX')
        self.tail_stmt = last
        self.main_stmt = ms
        # pattern branch: for r = 1, #P1 do ... bxor(string.byte(P1, r), XORKEY) ... I(...) end
        pat = self.branches['PAT']['body'][0]
        if pat['type'] != 'AstStatFor':
            raise LayoutError('pattern loop not found')
        xork = None
        icall = None
        for n, p in walk(pat['body']):
            if n['type'] == 'AstExprCall' and unparen(n['func'])['type'] == 'AstExprIndexName' and unparen(n['func'])['index'] == 'bxor' and len(n['args']) == 2:
                a1 = unparen(n['args'][1])
                if a1['type'] == 'AstExprLocal':
                    xork = a1['local']
            if n['type'] == 'AstStatExpr' and unparen(n['expr'])['type'] == 'AstExprCall' and loc_of(unparen(n['expr'])['func']) is not None:
                icall = unparen(n['expr'])
        if xork is None or icall is None:
            raise LayoutError('pattern decoding not recognised')
        self.set_role(xork, 'XORKEY')
        self.set_role(loc_of(icall['func']), 'SIMPLEFN')
        self.pat_call = icall
        # junk op: NXT = bxor(NXT, MIX(A,B,C,RF,HF,AUX))
        jb, jop = self.branches['JUNK']
        self.junk_op = jop
        jst = jb['body'][0]
        jc = None
        for n, p in walk(jst):
            if n['type'] == 'AstExprCall' and loc_of(n['func']) is not None and len(n['args']) == 6:
                jc = n
        if jc is None:
            raise LayoutError('junk mix call not found')
        self.set_role(loc_of(jc['func']), 'MIX')
        # simple handlers: SIMPLEFN(...) body: ladder on its op parameter; else W[op](...)
        sf = self.funcs.get(lkey(self.role_local['SIMPLEFN']))
        if sf is None:
            raise LayoutError('simple dispatcher body not found')
        self.simple_fn = sf
        # roles of the simple dispatcher params from the call in the pattern branch
        args = icall['args']
        sroles = [self.role_of(a) for a in args]
        # args 6..11 are the fused-field results: A, B, C, RF, HF, AUX; arg 5 = op byte
        order = ['REGS', 'KGET', 'IMP', 'ENV', 'UP', 'X', 'A', 'B', 'C', 'RF', 'HF', 'AUX']
        if len(args) != 12:
            raise LayoutError('simple dispatcher call arity')
        self.simple_param_roles = order
        for prm, nm in zip(sf['args'], order):
            self.roles[lkey(prm)] = 'S_' + nm
        self.roles_param_names = dict((lkey(prm), nm) for prm, nm in zip(sf['args'], order))
        # the tables the dispatcher reads: `op -> handler function` (older builds: one table behind the ladder of common
        # ops; newer builds: a table of op groups and one handler table per group)
        self.simple_tables = self.free_tables(sf)
        if not any(self.table_has_functions(t) for loc, t in self.simple_tables):
            raise LayoutError('simple handler table not found')
        # simple ops' own `bxor` of the ladder variable
        # fused-field decode order for pattern parts
        self.xor_stmt = None

    def free_tables(self, fn):
        """[(local, table constructor)] for the module-level locals that `fn` reads and that are initialised with a table
        constructor, in order of first reference"""
        fa, fb = self.src.span(fn)
        out = []
        seen = set()
        for n, p in walk(fn):
            if n['type'] != 'AstExprLocal':
                continue
            loc = n['local']
            k = lkey(loc)
            if k in seen:
                continue
            seen.add(k)
            decl = self.local_decl.get(k)
            if decl is None or decl[1] != 'local':
                continue
            sa, sb = self.src.span(decl[0])
            if fa <= sa and sb <= fb:
                continue
            for v, val in zip(decl[0]['vars'], decl[0]['values']):
                if lkey(v) == k and unparen(val)['type'] == 'AstExprTable':
                    out.append((v, unparen(val)))
        return out

    @staticmethod
    def table_has_functions(tab):
        return any(it.get('key') is not None and unparen(it['value'])['type'] in ('AstExprFunction', 'AstExprLocal')
                   for it in tab['items'])

    def analyse_fields(self, M):
        """permutations for the fused/simple/capture field readers"""
        def fn_of(role):
            loc = self.role_local[role]
            k = lkey(loc)
            # assigned function expression
            for n, p in walk(self.FRAME):
                if n['type'] == 'AstStatAssign' and len(n['vars']) == 1:
                    tg = unparen(n['vars'][0])
                    if tg['type'] == 'AstExprLocal' and lkey(tg['local']) == k and unparen(n['values'][0])['type'] == 'AstExprFunction':
                        return unparen(n['values'][0])
            raise LayoutError('reader function for ' + role + ' not found')
        self.fused_perm = self.dispatch_perm(fn_of('FUSED')['body']['body'][0], 6, False)
        sfn = None
        # simpleFields: `local x = M.simpleFields; if x == nil then x = function ... end`
        for n, p in walk(self.FRAME):
            if n['type'] == 'AstStatAssign' and len(n['vars']) == 1:
                tg = unparen(n['vars'][0])
                v = unparen(n['values'][0])
                if tg['type'] == 'AstExprLocal' and self.roles.get(lkey(tg['local'])) == 'SIMPLEF' and v['type'] == 'AstExprFunction':
                    sfn = v
        if sfn is None:
            raise LayoutError('simpleFields reader not found')
        self.simple_perm = self.dispatch_perm(sfn['body']['body'][0], 7, False)
        self.cap_perm = self.dispatch_perm(fn_of('CAPF')['body']['body'][0], 4, False)

    # ------------------------------------------------------------------ source patching helpers
    def stmt_end(self, stmt):
        return self.src.span(stmt)[1]

    def stmt_start(self, stmt):
        return self.src.span(stmt)[0]


# ---------------------------------------------------------------------------------------------------- extra tables
def _table_items(tab_node):
    """AstExprTable with `[number] = value` items -> [(number, value_expr)]"""
    out = []
    for it in tab_node['items']:
        k = it.get('key')
        if k is None:
            continue
        kn = const_num(k)
        if kn is None:
            continue
        out.append((int(kn), unparen(it['value'])))
    return out


def _decl_table(loader, local):
    st, kind = loader.local_decl[lkey(local)]
    if kind != 'local':
        raise LayoutError('table declaration expected')
    for v, val in zip(st['vars'], st['values']):
        if lkey(v) == lkey(local):
            val = unparen(val)
            if val['type'] != 'AstExprTable':
                raise LayoutError('table constructor expected')
            return val
    raise LayoutError('table declaration not found')


def extend(loader):
    """derive handler lists for classification"""
    L = loader
    # tables read by the simple dispatcher: [(local, [(key, value expression)])]
    L.simple_table_items = [(loc, _table_items(tab)) for loc, tab in L.simple_tables]
    # dedicated tables: key -> function node (via the local function declarations)
    L.ded_decl = {}
    for ti in range(1, 7):
        role = 'D%d' % ti
        tab = _decl_table(L, L.role_local[role])
        items = []
        for key, val in _table_items(tab):
            if val['type'] == 'AstExprLocal':
                fn = L.funcs.get(lkey(val['local']))
            elif val['type'] == 'AstExprFunction':
                fn = val
            else:
                fn = None
            if fn is None:
                raise LayoutError('dedicated handler %d/%d not found' % (ti, key))
            items.append((key, fn))
        L.ded_decl[ti] = items
    # hot chain ops
    hb, hops = L.branches['HOT']
    inner = None
    for s in hb['body']:
        if s['type'] == 'AstStatIf':
            inner = s
            break
    if inner is None:
        raise LayoutError('hot chain not found')
    L.hot_chain = []
    ch, fin = if_chain(inner)
    for cond, body, node in ch:
        e = eq_info(cond)
        if e is None or L.roles.get(lkey(e[0])) != 'OP':
            raise LayoutError('hot chain branch not recognised')
        L.hot_chain.append((int(e[1]), body))
    # control function chain
    cf = L.ctrl_fn
    first_if = None
    for s in cf['body']['body']:
        if s['type'] == 'AstStatIf':
            first_if = s
            break
    if first_if is None:
        raise LayoutError('control chain not found')
    L.ctrl_chain = []
    ch, fin = if_chain(first_if)
    for cond, body, node in ch:
        e = eq_info(cond)
        if e is None:
            raise LayoutError('control chain branch not recognised')
        L.ctrl_chain.append((int(e[1]), body))
    # frame-level local functions of the closure factory (frame recycling helpers)
    for s in L.MK['body']['body']:
        if s['type'] == 'AstStatLocalFunction':
            L.roles[lkey(s['name'])] = 'MKFN'
    # call-site role lists
    def call_roles(branch_body, callee_role):
        for n, p in walk(branch_body):
            if n['type'] == 'AstExprCall' and L.role_of(n['func']) == callee_role:
                return [L.role_of(a) for a in n['args']]
        return None

    L.call_roles = {}
    for ti in range(1, 5):
        L.call_roles['D%d' % ti] = call_roles(L.branches['D%dV' % ti], 'D%dV' % ti)
    L.call_roles['D5'] = call_roles(L.branches['D5V'], 'D5V')
    L.call_roles['CTRL'] = call_roles(L.branches['CTRLCALL'], 'CTRL')
    L.call_roles['SIMPLE'] = [L.role_of(a) for a in L.pat_call['args']]
    return L

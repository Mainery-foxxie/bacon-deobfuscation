"""Second half of the decompiler: symbolic evaluation of blocks, structuring, post passes, printing."""
import sys, re, collections
from .decomp import *

EXIT = -1
ARITH_OPS = {'ADDK': '+', 'SUBK': '-', 'MULK': '*', 'DIVK': '/', 'MODK': '%', 'POWK': '^', 'IDIVK': '//', 'SUBRK': '-', 'DIVRK': '/'}

# Every global the Real executor defines, copied from its reference (https://docs.projectreal.gg/en, 24 categories,
# 232 function pages): the functions, their aliases and the library tables (`crypt`, `json`, `Drawing`, ...).  Methods
# such as `game:HttpGet` or `window:Button` are not globals and are not listed.
EXECUTOR_GLOBALS = set('''
    cache cloneref compareinstances
    crypt base64_decode base64_encode lz4compress lz4decompress
    appendfile delfile delfolder getcustomasset isfile isfolder listfiles loadfile makefolder openfiledialog
    openfilesdialog openfolderdialog readfile savefiledialog writefile
    json
    rconsoleclear rconsolecreate rconsoledestroy rconsoleerr rconsolehide rconsoleinfo rconsoleinput rconsolename
    rconsoleprint rconsoleshow rconsolewarn
    Drawing cleardrawcache getrenderproperty isrenderobj setrenderproperty
    imgui
    isrbxactive keyclick keypress keyrelease mouse1click mouse1press mouse1release mouse2click mouse2press
    mouse2release mousemoveabs mousemoverel mousescroll
    fireclickdetector fireproximityprompt firetouchinterest getcallbackvalue gethui getinstances getnilinstances
    isnetworkowner
    gethiddenproperty isscriptable sethiddenproperty setscriptable
    decompile dumpstring getcallingscript getinstancesbyclass getinstancesofclass getloadedmodules getmenv
    getreferencedby getrunningscripts getscriptbytecode getscriptclosure getscripthash getscripts getscriptthread
    getsenv loadstring
    cansignalreplicate firesignal getconnections getsignalarguments getsignalargumentsinfo getsignalwhitelist
    replicatesignal
    WebSocket
    create_comm_channel get_comm_channel getactorfromthread getactors getactorthreads getcurrentactor getdeletedactors
    isparallel checkparallel run_on_actor runonactor run_on_thread runonthread
    checkcaller clonefunction comparefunctions comparefunction getfunctionhash hookfunction hookmetamethod iscclosure
    isexecutorclosure isfunctionhooked ishiddenstack isstackhidden islclosure isluaclosure isnewcclosure newcclosure
    newlclosure restorefunction sethiddenstack setstackhidden
    getconstant getconstants getinfo getproto getprotos getreg getstack getupvalue getupvalues isvalidlevel validlevel
    setconstant setstack setupvalue
    filtergc getgc get_gc_objects getgenv getrenv getscriptglobals gettenv getstateenv setclipboard toclipboard
    setrbxclipboard
    getmetafield getnamecallmethod get_namecall_method getrawmetatable hasmetafield hasmetatable isreadonly
    makereadonly makewriteable setnamecallmethod set_namecall_method setrawmetatable setreadonly
    getscriptfromthread getthreadidentity isourthread is_our_thread setthreadidentity
    disassemble
    clear_teleport_queue getexecutorname getexecutorversion getfflag getfflagtype getfpscap gethwid identifyexecutor
    info messagebox queue_on_teleport request setfflag setfpscap
    RakNet
    Signal
    saveinstance
'''.split())

ROBLOX_GLOBALS = {'game', 'workspace', 'script', 'math', 'string', 'table', 'Instance', 'Enum', 'task', 'print', 'warn', 'error',
                  'pcall', 'xpcall', 'tostring', 'tonumber', 'type', 'typeof', 'pairs', 'ipairs', 'next', 'select', 'unpack',
                  'require', 'wait', 'spawn', 'delay', 'tick', 'time', 'os', 'utf8', 'bit32', 'coroutine', 'debug', 'buffer',
                  'Vector2', 'Vector3', 'CFrame', 'Color3', 'UDim2', 'UDim', 'BrickColor', 'TweenInfo', 'Ray', 'Rect',
                  'Random', 'getgenv', 'getfenv', 'setfenv', 'loadstring', 'rawget', 'rawset', 'rawequal', 'setmetatable',
                  'getmetatable', 'assert', 'shared', '_G', '_VERSION', 'Drawing', 'syn', 'request', 'http_request',
                  'isfile', 'readfile', 'writefile', 'identifyexecutor', 'gethui', 'hookfunction', 'newcclosure',
                  'clonefunction', 'getrawmetatable', 'setreadonly', 'islclosure', 'checkcaller', 'fireclickdetector',
                  'firetouchinterest', 'getconnections', 'setclipboard', 'queue_on_teleport'} | EXECUTOR_GLOBALS


# services a script usually fetches through its own small wrapper (`local function getService(name) ... end`)
SERVICE_NAMES = {'Players', 'ReplicatedStorage', 'ReplicatedFirst', 'Workspace', 'RunService', 'VirtualUser',
                 'VirtualInputManager', 'TweenService', 'UserInputService', 'CollectionService', 'BadgeService', 'CoreGui',
                 'HttpService', 'Lighting', 'StarterGui', 'StarterPack', 'StarterPlayer', 'TeleportService',
                 'MarketplaceService', 'SoundService', 'ContextActionService', 'GuiService', 'Debris', 'ServerStorage',
                 'ServerScriptService', 'TextService', 'TextChatService', 'PathfindingService', 'ProximityPromptService',
                 'Stats', 'GroupService', 'LocalizationService', 'MarketplaceService', 'PhysicsService', 'NetworkClient',
                 'ScriptContext', 'SocialService', 'UserService', 'VoiceChatService', 'AssetService', 'ContentProvider',
                 'InsertService', 'KeyframeSequenceProvider', 'LogService', 'MaterialService', 'Teams', 'TestService'}


class Pend:
    __slots__ = ('expr', 'web', 'leaves', 'kind', 'link', 'method')

    def __init__(s, expr, web, leaves, kind='val', link=None, method=None):
        s.expr, s.web, s.leaves, s.kind, s.link, s.method = expr, web, list(leaves), kind, link, method


class Link:
    def __init__(s, objexpr):
        s.objexpr = objexpr
        s.var = None
        s.m = None
        s.o = None


def has_call(e):
    if isinstance(e, Call):
        return True
    for v in vars(e).values() if hasattr(e, '__dict__') else ():
        if isinstance(v, E) and has_call(v):
            return True
        if isinstance(v, list):
            for x in v:
                if isinstance(x, E) and has_call(x):
                    return True
    return False


def negate(e):
    if isinstance(e, Not):
        return e.e
    if isinstance(e, Bin) and e.op == '==':
        return Bin('~=', e.l, e.r)
    if isinstance(e, Bin) and e.op == '~=':
        return Bin('==', e.l, e.r)
    return Not(e)


def camel(s):
    s = re.sub(r'[^A-Za-z0-9_]', '', s)
    if not s:
        return None
    if s[0].isdigit():
        s = '_' + s
    # lowercase leading capitals (UIListLayout -> uiListLayout)
    m = re.match(r'^([A-Z]+)([a-z0-9_].*)?$', s)
    if m:
        caps, rest = m.group(1), m.group(2) or ''
        if len(caps) == 1 or not rest:
            return caps.lower() + rest if rest or len(caps) == 1 else caps.lower()
        return caps[:-1].lower() + caps[-1] + rest
    return s


def suggest(e):
    try:
        if isinstance(e, Call):
            if e.method:
                arg0 = e.args[0] if e.args else None
                if e.method == 'GetService' and isinstance(arg0, Const) and arg0.kind == 's':
                    return re.sub(r'[^A-Za-z0-9_]', '', arg0.v.decode('latin1'))
                if e.method in ('WaitForChild', 'FindFirstChild', 'FindFirstChildOfClass', 'FindFirstChildWhichIsA',
                                'FindFirstAncestor') and isinstance(arg0, Const) and arg0.kind == 's':
                    return camel(arg0.v.decode('latin1'))
                if e.method == 'Clone':
                    if isinstance(e.fn, Var):
                        return ('clone', e.fn.web)
                    return 'clone'
                if e.method in ('GetChildren', 'GetDescendants'):
                    return 'children'
                if e.method == 'Connect':
                    return 'connection'
                return camel(e.method)
            if len(e.args) == 1 and isinstance(e.args[0], Const) and e.args[0].kind == 's' \
                    and e.args[0].v.decode('latin1') in SERVICE_NAMES and not isinstance(e.fn, Imp):
                return e.args[0].v.decode('latin1')
            if isinstance(e.fn, Imp):
                p = e.fn.path
                if p == ('Instance', 'new') and e.args and isinstance(e.args[0], Const) and e.args[0].kind == 's':
                    return camel(e.args[0].v.decode('latin1'))
                if len(p) >= 2:
                    return camel(p[0] if p[-1] in ('new', 'fromRGB', 'fromScale', 'fromOffset', 'fromHSV') else p[-1]) if p[0] not in ('math',) else camel(p[-1])
                return camel(p[0])
            return None
        if isinstance(e, Idx) and isinstance(e.key, str):
            return camel(e.key)
        if isinstance(e, Imp):
            return camel(e.path[-1])
        if isinstance(e, Func):
            return 'func'
        if isinstance(e, Tbl):
            return 'items'
        if isinstance(e, Const):
            if e.kind == 'b':
                return 'flag'
            if e.kind == 'n':
                return 'n'
            if e.kind == 's':
                return 'text'
    except Exception:
        return None
    return None


class Gen:
    def __init__(self, dec):
        self.dec = dec
        self.seq = 0
        self.log = []
        self.importroots = set()
        for P in dec.protos.values():
            for rid in P['recs']:
                pass

    def nseq(self):
        self.seq += 1
        return self.seq

    # ------------------------------------------------------------------ constants / imports
    def kbytes(self, fn, i):
        c = fn.P['consts'][i]
        assert c[0] == 's', 'expected string constant'
        return c[1].decode('latin1')

    def import_path(self, fn, aux):
        n = aux >> 30
        ks = [(aux >> 20) & 1023, (aux >> 10) & 1023, aux & 1023][:n]
        return tuple(self.kbytes(fn, k) for k in ks)

    # ------------------------------------------------------------------ block evaluation
    def eval_block(self, fn, blk):
        nodes = fn.nodes
        regs = {}
        stmts = []
        st = {'multi': None, 'multi3': None}
        gen = self

        def sens_var(w):
            return w.captured or len(w.defs) > 1

        def inline_ok(w):
            ok = (w.kind == 'var' and not w.captured and len(w.defs) == 1 and len(w.uses) == 1
                  and not getattr(w, 'noinline', False)
                  and w.defs[0][1] is not None and w.defs[0][1].blk is blk
                  and nodes[w.uses[0][0]].blk is blk
                  and not (nodes[w.uses[0][0]].kind == 'MOVE' and nodes[w.uses[0][0]].a['A'] > nodes[w.uses[0][0]].a['B']))
            if ok:
                u = nodes[w.uses[0][0]]
                if u.kind in ('SETTABLEKS', 'SETTABLE') and w.uses[0][1] == u.a['A'] and u.a['A'] < u.a['B']:
                    # the stored value sits below the table register: it is a local declared earlier, not a temporary
                    w.keep = True
                    return False
                if u.kind == 'SELF' and w.uses[0][1] == u.a['B'] and u.a['B'] != u.a['A']:
                    # a method call on an expression computes the object into the call's own register; one that is read
                    # from somewhere else is a local variable
                    w.keep = True
                    return False
            return ok

        def mark_materialized(w):
            # a value that was copied/materialised into a register and is then only tested or copied on:
            # the compiler never does that for a condition, so the source had a local variable here
            if (w.kind == 'var' and len(w.defs) >= 2 and len(w.uses) >= 1
                    and any(d_[1] is not None and d_[1].kind in ('LOADB', 'MOVE') for d_ in w.defs)
                    and all(nodes[u_[0]].kind in ('JUMPIF', 'JUMPIFNOT')
                            or (nodes[u_[0]].kind == 'MOVE' and nodes[u_[0]].a['A'] > nodes[u_[0]].a['B'])
                            for u_ in w.uses)):
                w.keep = True

        def rd(n, r):
            w = fn.use_web[(n.id, r)]
            p = regs.get(r)
            if p is not None and p.web is w:
                del regs[r]
                return p
            if inline_ok(w):
                raise Exception('inline candidate web not pending: proto %d rec %s node %s R%d' % (fn.pid, n.rid, n.kind, r))
            return Pend(w.var, None, [])

        def temp_web(hint=None):
            w = Web('tmp')
            w.fn = fn
            w.hint = hint
            return w

        def materialize(p):
            if p.kind in ('selfm', 'selfo'):
                lk = p.link
                if lk.var is None:
                    w = temp_web()
                    w.hint = suggest(lk.objexpr)
                    w.declared = True
                    stmts.append(('local', w, lk.objexpr))
                    lk.var = w.var
                lk.o.expr = lk.var
                lk.m.expr = Idx(lk.var, lk.m.method)
                lk.o.leaves = []
                lk.m.leaves = []
                return
            if p.kind in ('multi',):
                raise NotImplementedError('materialize ' + p.kind + ' in proto %d' % fn.pid)
            w = p.web if p.web is not None else temp_web()
            if w.hint is None:
                w.hint = suggest(p.expr)
            w.declared = True
            w.keep = True
            stmts.append(('local', w, p.expr))
            p.expr = w.var
            p.leaves = []

        def settle(ops, is_stmt, ctor=False):
            flush = {}
            if is_stmt:
                O = [p for p in regs.values() if p.leaves]
                if st['multi'] is not None and st['multi'] not in ops:
                    raise Exception('pending multi value interrupted by statement in proto %d' % fn.pid)
                if ctor:
                    # This statement builds a constructor that is folded back into one expression later, so an empty table
                    # created before it can stay pending: it will still be created before the constructor.  It has to be
                    # flushed when something that was evaluated after it is flushed, or the order would change.
                    fresh = [p for p in O if isinstance(p.expr, Tbl) and not p.expr.items]
                    rest = [p for p in O if not (isinstance(p.expr, Tbl) and not p.expr.items)]
                    lim = max([max(p.leaves) for p in rest] + [0])
                    O = rest + [p for p in fresh if min(p.leaves) < lim]
                if O:
                    mx = max(max(p.leaves) for p in O)
                    for p in O:
                        flush[id(p)] = p
                    for p in ops:
                        if p.leaves and min(p.leaves) < mx:
                            flush[id(p)] = p
            for p in sorted(flush.values(), key=lambda q: min(q.leaves)):
                materialize(p)
            cur = 0
            bad = False
            for p in ops:
                if p.leaves:
                    if min(p.leaves) < cur:
                        bad = True
                    cur = max(p.leaves)
            if bad:
                cand = sorted([p for p in ops if p.leaves], key=lambda q: min(q.leaves))
                k = 0
                while True:
                    rest = set(id(p) for p in cand[k:])
                    cur = 0
                    ok = True
                    for p in ops:
                        if id(p) in rest:
                            if min(p.leaves) < cur:
                                ok = False
                                break
                            cur = max(p.leaves)
                    if ok:
                        break
                    k += 1
                for p in cand[:k]:
                    materialize(p)

        def declare_assign(w, e):
            if w.hint is None:
                w.hint = suggest(e)
            if w.kind in ('param', 'loop') or w.declared:
                stmts.append(('assign', w.var, e))
            else:
                w.declared = True
                stmts.append(('local', w, e))

        def set_def(n, r, expr, leaves, kind='val', link=None, method=None, ctor=False):
            w = fn.def_web[(n.id, r)]
            if inline_ok(w):
                regs[r] = Pend(expr, w, leaves, kind, link, method)
                return
            p = Pend(expr, None, leaves)
            if not w.uses:
                # dead value: constants and plain copies vanish; anything that does work stays (a call, an index read
                # that can raise or run a metamethod, arithmetic, an upvalue read, a function), because the original
                # compiled it and removing it would change which upvalues a closure captures first
                if isinstance(expr, Call):
                    settle([p], True)
                    stmts.append(('call', expr))
                elif isinstance(expr, (Tbl, Func)):
                    # a table is built field by field later; a function nobody calls is still part of the program
                    settle([p], True)
                    declare_assign(w, p.expr)
                elif not isinstance(expr, (Const, Var)):
                    settle([p], True)
                    stmts.append(('call_cond', p.expr))
                else:
                    gen.log.append('dropped dead value in proto %d rec %s (%s)' % (fn.pid, n.rid, n.kind))
                return
            settle([p], True, ctor)
            declare_assign(w, p.expr)

        def fills_in(u_):
            # a store of a field into the table that is held in the register it reads as its object
            nu = nodes[u_[0]]
            return nu.kind == 'SETTABLEKS' and u_[1] == nu.a['B'] and nu.a['A'] != nu.a['B']

        P = fn.P
        pending_term = None
        for n in blk.nodes:
            k, a = n.kind, n.a
            if k == 'LOADK':
                set_def(n, a['A'], kconst(P['consts'][a['rf']]), [])
            elif k == 'LOADN':
                set_def(n, a['A'], Const('n', a['rf']), [])
            elif k == 'LOADB':
                mark_materialized(fn.def_web[(n.id, a['A'])])
                set_def(n, a['A'], Const('b', a['B'] == 1), [])
            elif k == 'LOADNIL':
                set_def(n, a['A'], Const('z', None), [])
            elif k == 'GETIMPORT':
                set_def(n, a['A'], Imp(self.import_path(fn, a['aux'])), [])
            elif k == 'MOVE':
                p = rd(n, a['B'])
                mark_materialized(fn.def_web[(n.id, a['A'])])
                if p.web is None and a['A'] > a['B']:
                    fn.use_web[(n.id, a['B'])].keep = True
                lv = p.leaves
                if p.web is None and not lv and sens_var(fn.use_web[(n.id, a['B'])]):
                    lv = [self.nseq()]
                set_def(n, a['A'], p.expr, lv)
            elif k == 'GETTABLEKS':
                o = rd(n, a['B'])
                set_def(n, a['A'], Idx(o.expr, self.kbytes(fn, a['aux'])), o.leaves + [self.nseq()])
            elif k in ARITH_RR:
                x = rd(n, a['B'])
                y = rd(n, a['C'])
                settle([x, y], False)
                set_def(n, a['A'], Bin(ARITH_RR[k], x.expr, y.expr), x.leaves + y.leaves + [self.nseq()])
            elif k in ('OR', 'AND'):
                x = rd(n, a['B'])
                y = rd(n, a['C'])
                settle([x, y], False)
                set_def(n, a['A'], Bin(k.lower(), x.expr, y.expr), x.leaves + y.leaves + [self.nseq()])
            elif k in ('ORK', 'ANDK'):
                x = rd(n, a['B'])
                kc = kconst(P['consts'][a['C']])
                settle([x], False)
                set_def(n, a['A'], Bin(k[:-1].lower(), x.expr, kc), x.leaves + [self.nseq()])
            elif k == 'GETTABLE':
                o = rd(n, a['B'])
                key = rd(n, a['C'])
                settle([o, key], False)
                set_def(n, a['A'], Idx(o.expr, key.expr), o.leaves + key.leaves + [self.nseq()])
            elif k == 'GETTABLEN':
                o = rd(n, a['B'])
                settle([o], False)
                set_def(n, a['A'], Idx(o.expr, Const('n', a['C'] + 1)), o.leaves + [self.nseq()])
            elif k == 'NOT':
                x = rd(n, a['B'])
                settle([x], False)
                set_def(n, a['A'], Not(x.expr), x.leaves + [self.nseq()])
            elif k == 'MINUS':
                x = rd(n, a['B'])
                settle([x], False)
                set_def(n, a['A'], Neg(x.expr), x.leaves + [self.nseq()])
            elif k == 'CONCAT':
                ops = [rd(n, r) for r in n.reads]
                settle(ops, False)
                e = ops[-1].expr
                for p_ in reversed(ops[:-1]):
                    e = Bin('..', p_.expr, e)
                set_def(n, a['A'], e, [l for p_ in ops for l in p_.leaves] + [self.nseq()])
            elif k == 'DUPTABLE':
                tc = P['consts'][a['rf']]
                assert tc[0] in ('t', 'i'), tc
                fields, pending = [], []
                shape = tc[1] if tc[0] == 't' else [(kid, -1) for kid in tc[1]]
                for (kid, vid) in shape:
                    key = self.kbytes(fn, kid)
                    if vid >= 0:
                        fields.append((key, kconst(P['consts'][vid])))
                    else:
                        pending.append(key)
                        fields.append((key, None))
                w = fn.def_web[(n.id, a['A'])]
                # fields that are not constants are stored after the table exists; when the table is then used once, the
                # stores and the table form one constructor expression (post.fold_tables) and are not statements of their own
                w.ctor = bool(pending) and sum(1 for u_ in w.uses if not fills_in(u_)) == 1
                set_def(n, a['A'], Tbl([], fields, pending), [self.nseq()], ctor=w.ctor)
            elif k == 'LENGTH':
                x = rd(n, a['B'])
                settle([x], False)
                set_def(n, a['A'], Len(x.expr), x.leaves + [self.nseq()])
            elif k in ARITH_K:
                x = rd(n, a['B'])
                kc = kconst(P['consts'][a['C']])
                settle([x], False)
                set_def(n, a['A'], Bin(ARITH_OPS[k], x.expr, kc), x.leaves + [self.nseq()])
            elif k in ARITH_RK:
                y = rd(n, a['C'])
                kc = kconst(P['consts'][a['B']])
                settle([y], False)
                set_def(n, a['A'], Bin(ARITH_OPS[k], kc, y.expr), y.leaves + [self.nseq()])
            elif k == 'SETTABLE':
                o = rd(n, a['B'])
                key = rd(n, a['C'])
                v = rd(n, a['A'])
                settle([o, key, v], True)
                stmts.append(('assign', Idx(o.expr, key.expr), v.expr))
            elif k == 'NEWTABLE':
                w = fn.def_web[(n.id, a['A'])]
                # A table that a SETLIST fills is a constructor in progress: whatever statements show up before the SETLIST
                # are fix-ups of the items inside it, so it has to stay pending.  Any other pending table is created where
                # it was created, so a statement that comes before its use flushes it (it was a local declared earlier).
                ctor = any(nodes[u_[0]].kind == 'SETLIST' and u_[1] == nodes[u_[0]].a['A'] for u_ in w.uses)
                set_def(n, a['A'], Tbl([], fromnew=True), [] if ctor else [self.nseq()])
            elif k == 'GETUPVAL':
                set_def(n, a['A'], fn.upvals[a['B']].var, [self.nseq()])
            elif k == 'SETUPVAL':
                v = rd(n, a['A'])
                settle([v], True)
                stmts.append(('assign', fn.upvals[a['B']].var, v.expr))
            elif k == 'SETTABLEKS':
                o = rd(n, a['B'])
                v = rd(n, a['A'])
                settle([o, v], True, isinstance(o.expr, Var) and getattr(o.expr.web, 'ctor', False))
                stmts.append(('assign', Idx(o.expr, self.kbytes(fn, a['aux'])), v.expr))
            elif k == 'SELF':
                o = rd(n, a['B'])
                key = self.kbytes(fn, a['aux'])
                A = a['A']
                wA = fn.def_web[(n.id, A)]
                wO = fn.def_web[(n.id, A + 1)]
                if not (inline_ok(wA) and inline_ok(wO)):
                    raise NotImplementedError('SELF with non-inline webs in proto %d' % fn.pid)
                lk = Link(o.expr)
                regs[A + 1] = Pend(o.expr, wO, o.leaves, 'selfo', lk)
                regs[A] = Pend(Idx(o.expr, key), wA, o.leaves, 'selfm', lk, key)
                lk.o = regs[A + 1]
                lk.m = regs[A]
            elif k == 'CALL':
                base, B, C = a['A'], a['B'], a['C']
                fnp = rd(n, base)
                args = [rd(n, r) for r in n.reads[1:]]
                mp = None
                if B == 0:
                    mp = st['multi']
                    if mp is None:
                        raise Exception('multret consumer without producer in proto %d' % fn.pid)
                    st['multi'] = None
                is_method = (fnp.kind == 'selfm' and args and args[0].kind == 'selfo' and args[0].link is fnp.link)
                if fnp.kind == 'selfm' and not is_method:
                    raise NotImplementedError('split method call in proto %d' % fn.pid)
                ops = [fnp] + (args[1:] if is_method else args) + ([mp] if mp else [])

                def build():
                    rest = (args[1:] if is_method else args)
                    aex = [p.expr for p in rest]
                    if mp is not None:
                        aex.append(Multi(mp.expr))
                    if is_method:
                        return Call(fnp.expr.obj, aex, method=fnp.method)
                    return Call(fnp.expr, aex)

                if C == 1:
                    settle(ops, True)
                    stmts.append(('call', build()))
                elif C == 2:
                    w = fn.def_web[(n.id, base)]
                    lv = [l for p in ops for l in p.leaves] + [self.nseq()]
                    if inline_ok(w):
                        settle(ops, False)
                        lv = [l for p in ops for l in p.leaves] + [self.nseq()]
                        regs[base] = Pend(build(), w, lv)
                    else:
                        settle(ops, True)
                        declare_assign(w, build())
                elif C == 0:
                    settle(ops, False)
                    lv = [l for p in ops for l in p.leaves] + [self.nseq()]
                    st['multi'] = Pend(build(), None, lv, 'multi')
                else:
                    rws = [fn.def_web[(n.id, base + i)] for i in range(C - 1)]
                    used = [i for i, w in enumerate(rws) if w.uses]
                    nx = n.succ[0] if len(n.succ) == 1 else None
                    if C == 4 and nx is not None and nx.kind == 'FORGPREP' and nx.a['A'] == base and nx.blk is blk \
                            and all(len(w.uses) == 1 and w.uses[0][0] == nx.id for w in rws):
                        settle(ops, False)
                        lv = [l for p in ops for l in p.leaves] + [self.nseq()]
                        st['multi3'] = Pend(build(), None, lv, 'multi3')
                    elif used == [0] and (rws[0].declared or rws[0].kind in ('param', 'loop')):
                        settle(ops, True)
                        declare_assign(rws[0], build())
                    else:
                        settle(ops, True)
                        for w in rws:
                            w.noinline = True
                        for w in rws:
                            if w.declared or w.kind in ('param', 'loop'):
                                raise NotImplementedError('multi-assign into declared variable in proto %d' % fn.pid)
                        for i in used:
                            if rws[i].hint is None:
                                rws[i].hint = None
                            rws[i].declared = True
                        stmts.append(('localn', [w if w.uses else None for w in rws], build()))
            elif k == 'GETVARARGS':
                A, B = a['A'], a['B']
                if B == 0:
                    st['multi'] = Pend(Vararg(), None, [], 'multi')
                elif B == 2:
                    set_def(n, A, Vararg(True), [])
                else:
                    rws = [fn.def_web[(n.id, A + i)] for i in range(B - 1)]
                    used = [i for i, w in enumerate(rws) if w.uses]
                    for w in rws:
                        w.noinline = True
                    for w in rws:
                        if w.declared or w.kind in ('param', 'loop'):
                            raise NotImplementedError('vararg read into a declared variable in proto %d' % fn.pid)
                    for i in used:
                        rws[i].declared = True
                    stmts.append(('localn', [w if w.uses else None for w in rws], Vararg()))
            elif k in ('NEWCLOSURE', 'DUPCLOSURE'):
                caps = []
                for (t, idx) in a['caps']:
                    if t in (0, 1):
                        caps.append(fn.use_web[(n.id, idx)])
                    else:
                        caps.append(fn.upvals[idx])
                set_def(n, a['A'], Func(a['child'], caps), [])
            elif k == 'CLOSEUPVALS':
                pass
            elif k == 'SETLIST':
                tbl = rd(n, a['A'])
                items = [rd(n, r) for r in n.reads[1:]]
                first = a['aux']
                mp = None
                if a['C'] == 0:
                    mp = st['multi']
                    if mp is None:
                        raise Exception('multret consumer without producer in proto %d' % fn.pid)
                    st['multi'] = None
                if not (isinstance(tbl.expr, Tbl) and tbl.web is not None and first == 1 + len(tbl.expr.items)):
                    raise NotImplementedError('SETLIST on a variable table in proto %d' % fn.pid)
                allp = [tbl] + items + ([mp] if mp is not None else [])
                settle(allp, False)
                lv = [l for p in allp for l in p.leaves]
                new_items = [p.expr for p in items] + ([Multi(mp.expr)] if mp is not None else [])
                set_def(n, a['A'], Tbl(tbl.expr.items + new_items), lv)
            elif k == 'RETURN':
                ops = [rd(n, r) for r in n.reads]
                if a['B'] == 0:
                    ops.append(st['multi'])
                    st['multi'] = None
                settle(ops, True)
                exprs = [p.expr for p in ops[:-1]] + ([Multi(ops[-1].expr)] if a['B'] == 0 else [])
                if a['B'] != 0:
                    exprs = [p.expr for p in ops]
                pending_term = ('ret', exprs)
            elif k in COND_KINDS:
                ops = []
                if k == 'JUMPIFNOT':
                    c = rd(n, a['A'])
                    ops = [c]
                    settle(ops, True)
                    cond = Not(c.expr)
                elif k == 'JUMPIF':
                    c = rd(n, a['A'])
                    ops = [c]
                    settle(ops, True)
                    cond = c.expr
                elif k in ('JUMPIFEQ', 'JUMPIFNOTEQ', 'JUMPIFLE', 'JUMPIFLT', 'JUMPIFNOTLT', 'JUMPIFNOTLE'):
                    x = rd(n, a['A'])
                    y = rd(n, a['aux'])
                    ops = [x, y]
                    settle(ops, True)
                    base_op = {'JUMPIFEQ': '==', 'JUMPIFNOTEQ': '~=', 'JUMPIFLE': '<=', 'JUMPIFLT': '<', 'JUMPIFNOTLT': '<',
                               'JUMPIFNOTLE': '<='}[k]
                    cond = Bin(base_op, x.expr, y.expr)
                    if k in ('JUMPIFNOTLT', 'JUMPIFNOTLE'):
                        cond = Not(cond)
                elif k == 'JUMPXEQKNIL':
                    x = rd(n, a['A'])
                    ops = [x]
                    settle(ops, True)
                    flag = (a['aux'] >> 31) & 1
                    cond = Bin('==' if flag == 0 else '~=', x.expr, Const('z', None))
                elif k == 'JUMPXEQK':
                    x = rd(n, a['A'])
                    ops = [x]
                    settle(ops, True)
                    flag = (a['aux'] >> 31) & 1
                    kc = kconst(P['consts'][a['aux'] & 0xFFFFFF])
                    cond = Bin('==' if flag == 0 else '~=', x.expr, kc)
                else:  # JUMPXEQKB
                    x = rd(n, a['A'])
                    ops = [x]
                    settle(ops, True)
                    flag = (a['aux'] >> 31) & 1
                    cond = Bin('==' if flag == 0 else '~=', x.expr, Const('b', bool(a['aux'] & 1)))
                nxt, alt = n.succ
                pending_term = ('cond', cond, nxt.blk.id, alt.blk.id)
            elif k == 'FORNPREP':
                A = a['A']
                ini = rd(n, A + 2)
                lim = rd(n, A)
                stp = rd(n, A + 1)
                ops = [ini, lim, stp]
                settle(ops, True)
                w = fn.def_web[(n.id, A + 2)]
                w.kind = 'loop'
                w.declared = True
                nxt, alt = n.succ
                pending_term = ('for', w, ini.expr, lim.expr, stp.expr, nxt.blk.id, alt.blk.id)
            elif k == 'FORNLOOP':
                nxt, alt = n.succ
                pending_term = ('fornloop', alt.blk.id, nxt.blk.id)
            elif k == 'FORGPREP':
                A = a['A']
                if st['multi3'] is not None:
                    mp = st['multi3']
                    st['multi3'] = None
                    settle([mp], True)
                    exprs = [Multi(mp.expr)]
                else:
                    ops = [rd(n, A), rd(n, A + 1), rd(n, A + 2)]
                    settle(ops, True)
                    exprs = [p.expr for p in ops]
                pending_term = ('forgprep', exprs, n.succ[0].blk.id)
            elif k == 'FORGLOOP':
                nxt, alt = n.succ
                ws = [fn.def_web[(n.id, a['A'] + 3 + i)] for i in range(a['aux'] & 255)]
                for w in ws:
                    w.kind = 'loop'
                    w.declared = True
                pending_term = ('forgloop', ws, alt.blk.id, nxt.blk.id)
            else:
                raise Exception('eval: unhandled node kind ' + k)
        # leftovers
        for r, p in sorted(regs.items(), key=lambda kv: (min(kv[1].leaves) if kv[1].leaves else 0)):
            gen.log.append('leftover pending value in proto %d block %d R%d' % (fn.pid, blk.id, r))
            materialize(p)
        regs.clear()
        if st['multi'] is not None or st['multi3'] is not None:
            raise Exception('unconsumed multret value in proto %d' % fn.pid)
        blk.stmts = stmts
        if pending_term is None:
            assert len(blk.succ) == 1, (fn.pid, blk.id, blk.succ)
            blk.term = ('goto', blk.succ[0])
        else:
            blk.term = pending_term

    # ------------------------------------------------------------------ short-circuit condition merging
    def merge_conds(self, fn):
        """b1: if p1 then T1 else E1 where one side is a statement-free, single-predecessor cond block c2
        -> fold both into one block with an and/or condition (what the compiler's jump chains encode)"""
        blocks = fn.blocks
        for b in blocks:
            b.dead = False
        changed = True
        while changed:
            changed = False
            for b1 in blocks:
                if b1.dead or b1.term[0] != 'cond':
                    continue
                _, c1, nxt1, alt1 = b1.term
                p1 = negate(c1)
                T1, E1 = nxt1, alt1
                for side in ('T', 'E'):
                    s = T1 if side == 'T' else E1
                    c2b = blocks[s]
                    if c2b is b1 or c2b.dead or c2b.term[0] != 'cond' or c2b.stmts or c2b.pred != [b1.id]:
                        continue
                    _, c2, nxt2, alt2 = c2b.term
                    p2 = negate(c2)
                    T2, E2 = nxt2, alt2
                    new = None
                    if side == 'T':
                        if T2 == E1:
                            new = (Bin('or', negate(p1), p2), E1, E2)
                        elif E2 == E1:
                            new = (Bin('and', p1, p2), T2, E1)
                    else:
                        if T2 == T1:
                            new = (Bin('or', p1, p2), T1, E2)
                        elif E2 == T1:
                            new = (Bin('or', p1, negate(p2)), T1, T2)
                    if new is None:
                        continue
                    np_, nT, nE = new
                    b1.term = ('cond', negate(np_), nT, nE)
                    c2b.dead = True
                    for x in (T2, E2):
                        lst = blocks[x].pred
                        if c2b.id in lst:
                            if b1.id in lst:
                                lst.remove(c2b.id)
                            else:
                                lst[lst.index(c2b.id)] = b1.id
                    changed = True
                    break

    # ------------------------------------------------------------------ post-dominators and structuring
    def block_succ(self, fn):
        succ = {}
        for b in fn.blocks:
            t = b.term
            if t[0] == 'ret':
                succ[b.id] = [EXIT]
            elif t[0] == 'goto':
                succ[b.id] = [t[1]]
            elif t[0] == 'cond':
                succ[b.id] = [t[2], t[3]]
            elif t[0] == 'for':
                succ[b.id] = [t[5], t[6]]
            elif t[0] == 'fornloop':
                succ[b.id] = [t[1], t[2]]
            elif t[0] == 'forgprep':
                succ[b.id] = [t[2]]
            elif t[0] == 'forgloop':
                succ[b.id] = [t[2], t[3]]
            else:
                raise Exception('unknown term ' + str(t[0]))
        return succ

    def find_loops(self, fn, succ):
        """natural loops formed by ordinary back edges (not FORNLOOP / FORGLOOP): header -> set of blocks"""
        blocks = fn.blocks
        state = {}
        back = []
        e0 = fn.entry_block.id
        state[e0] = 1
        stack = [(e0, iter([x for x in succ[e0] if x != EXIT]))]
        while stack:
            b, it = stack[-1]
            for x in it:
                if x not in state:
                    state[x] = 1
                    stack.append((x, iter([y for y in succ[x] if y != EXIT])))
                    break
                elif state[x] == 1:
                    back.append((b, x))
            else:
                state[b] = 2
                stack.pop()
        loops = {}
        for (b, h) in back:
            t = blocks[b].term
            # the edge a for loop takes to run its body again is the loop itself; the edge it takes when it is done can
            # still go back to the top of an enclosing loop (a for loop that ends the body of a while loop)
            if (t[0] == 'fornloop' and h == t[1]) or (t[0] == 'forgloop' and h == t[2]) \
                    or blocks[h].term[0] in ('fornloop', 'forgloop'):
                continue
            body = {h}
            work = [b]
            while work:
                x = work.pop()
                if x in body:
                    continue
                body.add(x)
                work.extend(blocks[x].pred)
            loops.setdefault(h, set()).update(body)
        return loops

    def ipdoms(self, fn, succ=None, loops=None, lexit=None):
        blocks = fn.blocks
        if succ is None:
            succ = self.block_succ(fn)
        succ = {b: list(v) for b, v in succ.items()}
        # loops without any exit would never reach EXIT: add a virtual edge from their back-edge sources
        for h, body in (loops or {}).items():
            if lexit.get(h) is None:
                for x in body:
                    if h in succ[x] and EXIT not in succ[x]:
                        succ[x].append(EXIT)
        allb = set(succ) | {EXIT}
        pd = {b: set(allb) for b in succ}
        pd[EXIT] = {EXIT}
        changed = True
        while changed:
            changed = False
            for b in succ:
                s = [pd[x] for x in succ[b]]
                new = set.intersection(*s) | {b}
                if new != pd[b]:
                    pd[b] = new
                    changed = True
        ip = {}
        for b in succ:
            cand = pd[b] - {b}
            best = None
            for c in cand:
                if pd[c] == cand:
                    best = c
            ip[b] = best
        fn.succ_blocks = succ
        return ip

    def structure(self, fn):
        blocks = fn.blocks
        gen = self
        succ0 = self.block_succ(fn)
        loops = self.find_loops(fn, succ0)
        lexit = {}
        for h, body in loops.items():
            tg = set()
            for x in body:
                for y in succ0[x]:
                    if y != EXIT and y not in body:
                        tg.add(y)
            if len(tg) > 1:
                X = None
                hb = blocks[h]
                if hb.term[0] == 'cond' and not hb.stmts:
                    outs = [y for y in (hb.term[2], hb.term[3]) if y not in body]
                    if len(outs) == 1 and outs[0] in tg:
                        X = outs[0]
                if X is None:
                    def reach_all(t):
                        seen, st = set(), [t]
                        while st:
                            x = st.pop()
                            if x in seen or x == EXIT:
                                continue
                            seen.add(x)
                            st.extend(succ0[x])
                        return seen
                    nt = []
                    for t in tg:
                        R = reach_all(t)
                        if (R & body) or any(u in R for u in tg if u != t):
                            nt.append(t)
                    if len(nt) == 1:
                        X = nt[0]
                    elif len(nt) > 1:
                        raise NotImplementedError('loop with several exits in proto %d' % fn.pid)
                lexit[h] = X
            else:
                lexit[h] = next(iter(tg)) if tg else None
        ip = self.ipdoms(fn, succ0, loops, lexit)
        emitted = collections.Counter()

        # acyclic copy of the graph (back edges removed) + reachability sets, used to find where two branches meet
        dag = {}
        post = []
        dstate = {}
        e0 = fn.entry_block.id

        def dsucc(b):
            t_ = blocks[b].term
            if t_[0] == 'fornloop':
                return [t_[2]]
            if t_[0] == 'forgloop':
                return [t_[3]]
            if t_[0] == 'forgprep':
                return [t_[2], blocks[t_[2]].term[2]]
            return [x for x in succ0[b] if x != EXIT]

        dstate[e0] = 1
        dstack = [(e0, iter(dsucc(e0)))]
        dag[e0] = []
        while dstack:
            b_, it_ = dstack[-1]
            for x in it_:
                if x not in dstate:
                    dstate[x] = 1
                    dag[x] = []
                    dag[b_].append(x)
                    dstack.append((x, iter(dsucc(x))))
                    break
                elif dstate[x] == 2:
                    dag[b_].append(x)
                # state 1: back edge, dropped
            else:
                dstate[b_] = 2
                post.append(b_)
                dstack.pop()
        rpo_index = {b: i for i, b in enumerate(reversed(post))}
        rset = {}
        for b in post:
            s_ = {b}
            for x in dag[b]:
                s_ |= rset[x]
            rset[b] = s_

        def dag_reach(start, avoid):
            seen = set()
            st_ = [start]
            while st_:
                x = st_.pop()
                if x in seen or x == avoid:
                    continue
                seen.add(x)
                st_.extend(dag[x])
            return seen

        def join_of(t_b, e_b, loop=None):
            hdr = loop.header if loop is not None else None
            rt = set() if t_b == hdr else rset[t_b]
            re_ = set() if e_b == hdr else rset[e_b]
            common = rt & re_
            if not common:
                return None
            return min(common, key=lambda q: rpo_index[q])

        class Ctx:
            def __init__(s, exit, body, header=None):
                s.exit, s.body, s.header = exit, body, header

        def reach(start, avoid):
            seen = set()
            st = [start]
            while st:
                x = st.pop()
                if x in seen or x == avoid or x == EXIT:
                    continue
                seen.add(x)
                st.extend(succ0[x])
            return seen

        def emit_loop(H, stop):
            body_set, X = loops[H], lexit[H]
            ctx = Ctx(X, body_set, H)
            hb = blocks[H]
            t = hb.term
            if t[0] == 'cond' and not hb.stmts:
                _, c, nxt_b, alt_b = t
                stay = None
                if nxt_b in body_set and alt_b not in body_set:
                    stay, cstay = nxt_b, negate(c)
                elif alt_b in body_set and nxt_b not in body_set:
                    stay, cstay = alt_b, c
                if stay is not None:
                    emitted[H] += 1
                    body = region(stay, set(stop) | {H}, ctx)
                    return ('while', cstay, gen.strip_cont(body))
            body = region(H, set(stop) | {H}, ctx, first=True)
            return ('while', Const('b', True), gen.strip_cont(body))

        def region(bid, stop, loop=None, first=False):
            out = []
            guard = 0
            while bid is not None:
                guard += 1
                if guard > 100000:
                    raise Exception('structuring loop')
                skip = first
                first = False
                if not skip:
                    if loop is not None and bid == loop.exit:
                        out.append(('break',))
                        return out
                    if loop is not None and loop.header is not None and bid == loop.header:
                        out.append(('continue',))
                        return out
                    if bid in stop:
                        return out
                    if bid in loops:
                        out.append(emit_loop(bid, stop))
                        bid = lexit[bid]
                        continue
                b = blocks[bid]
                emitted[bid] += 1
                out.extend(b.stmts)
                t = b.term
                k = t[0]
                if k == 'ret':
                    out.append(('return', t[1]))
                    return out
                if k == 'goto':
                    bid = t[1]
                    continue
                if k in ('fornloop', 'forgloop'):
                    return out
                if k == 'forgprep':
                    _, exprs, floop = t
                    ft = blocks[floop].term
                    body = region(ft[2], set(stop), Ctx(ft[3], reach(ft[2], ft[3])))
                    out.append(('forin', ft[1], exprs, body))
                    bid = ft[3]
                    continue
                if k == 'for':
                    _, w, ini, lim, stp, body_b, exit_b = t
                    body = region(body_b, set(stop), Ctx(exit_b, reach(body_b, exit_b)))
                    out.append(('for', w, ini, lim, stp, body))
                    bid = exit_b
                    continue
                if k == 'cond':
                    _, c, nxt_b, alt_b = t
                    j = join_of(nxt_b, alt_b, loop)
                    if j is not None:
                        later = rset[j] - {j}
                        bad_join = False
                        hdr_ = loop.header if loop is not None else None
                        for s0 in (nxt_b, alt_b):
                            if s0 != j and s0 != hdr_ and (dag_reach(s0, j) & later):
                                bad_join = True
                                break
                        if bad_join:
                            j2 = ip.get(bid)
                            j = None if (j2 is None or j2 == EXIT) else j2
                    outside = loop is not None and j is not None and j not in loop.body
                    if j is None or outside:
                        th = region(nxt_b, stop, loop)
                        el = region(alt_b, stop, loop)
                        out.append(('if', negate(c), th, el))
                        return out
                    st2 = set(stop) | {j}
                    th = region(nxt_b, st2, loop) if nxt_b != j else []
                    el = region(alt_b, st2, loop) if alt_b != j else []
                    out.append(('if', negate(c), th, el))
                    bid = j
                    continue
                raise Exception('unknown term ' + str(k))
            return out

        res = region(fn.entry_block.id, set())
        dup = [b for b, n in emitted.items() if n > 1]
        if dup:
            gen.log.append('proto %d: blocks emitted more than once: %s' % (fn.pid, [(b, blocks[b].term[0], len(blocks[b].stmts), emitted[b]) for b in sorted(dup)]))
        return res

    # ------------------------------------------------------------------ post passes
    def simplify(self, stmts):
        out = []
        for s in stmts:
            if s[0] == 'if':
                _, c, th, el = s
                th = self.simplify(th)
                el = self.simplify(el)
                if not th and el:
                    c, th, el = negate(c), el, []
                if not th and not el:
                    # empty if: keep the condition only if it calls something
                    if has_call(c):
                        out.append(('call_cond', c))
                    continue
                ends_ret = bool(th) and th[-1][0] in ('return', 'break', 'continue')
                if ends_ret and el:
                    out.append(self.mk_if(c, th, []))
                    out.extend(el)
                    continue
                el_ends = bool(el) and el[-1][0] in ('return', 'break', 'continue')
                if el_ends and th and not ends_ret and (len(el) <= 3 or len(el) <= len(th)):
                    out.append(self.mk_if(negate(c), el, []))
                    out.extend(th)
                    continue
                out.append(self.mk_if(c, th, el))
            elif s[0] == 'for':
                out.append(('for', s[1], s[2], s[3], s[4], self.simplify(s[5])))
            elif s[0] == 'forin':
                out.append(('forin', s[1], s[2], self.simplify(s[3])))
            elif s[0] == 'while':
                out.append(('while', s[1], self.simplify(s[2])))
            else:
                out.append(s)
        return out

    @staticmethod
    def and_join(a, b):
        if isinstance(b, Bin) and b.op == 'and':
            return Gen.and_join(Gen.and_join(a, b.l), b.r)
        return Bin('and', a, b)

    def mk_if(self, c, th, el):
        # if a then if b then X end end  ->  if a and b then X end   (same evaluation order, no else involved)
        if not el and len(th) == 1 and th[0][0] == 'if' and not th[0][3]:
            return self.mk_if(self.and_join(c, th[0][1]), th[0][2], [])
        return ('if', c, th, el)

    def strip_cont(self, stmts):
        if stmts and stmts[-1][0] == 'continue':
            return self.strip_cont(stmts[:-1])
        if stmts and stmts[-1][0] == 'if':
            _, c, th, el = stmts[-1]
            th2, el2 = self.strip_cont(th), self.strip_cont(el)
            if th2 or el2:
                if not th2 and el2:
                    c, th2, el2 = negate(c), el2, []
                return stmts[:-1] + [('if', c, th2, el2)]
            return self.strip_cont(stmts[:-1]) if not has_call(c) else stmts[:-1] + [('call_cond', c)]
        return stmts

    def strip_tail_return(self, stmts):
        if stmts and stmts[-1][0] == 'return' and not stmts[-1][1]:
            return self.strip_tail_return(stmts[:-1])
        if stmts and stmts[-1][0] == 'if':
            _, c, th, el = stmts[-1]
            th2, el2 = self.strip_tail_return(th), self.strip_tail_return(el)
            if th2 or el2:
                if not th2 and el2:
                    c, th2, el2 = negate(c), el2, []
                return stmts[:-1] + [('if', c, th2, el2)]
            return self.strip_tail_return(stmts[:-1]) if not has_call(c) else stmts[:-1] + [('call_cond', c)]
        return stmts

    def run(self, fn):
        self.dec.prepare(fn)
        for b in fn.blocks:
            self.eval_block(fn, b)
        self.merge_conds(fn)
        body = self.structure(fn)
        body = self.simplify(body)
        body = self.strip_tail_return(body)
        from .post import post_passes
        body = post_passes(body, fn)
        fn.body = body
        return fn


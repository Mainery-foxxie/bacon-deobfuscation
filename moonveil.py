"""MoonVeil support: a behaviour trace, NOT a decompiler.

MoonVeil compiles a script into its own private VM program (a Base85 blob plus a huge state machine), and nothing in the
file says what the original statements were.  What can be done honestly is to run the protected script in the Luau
sandbox against a recording fake Roblox environment (moonveil.lua.tpl) and write down what it did: every call, every
assignment, in order, with the real names and strings.  This module runs that harness, reads its log and turns it into a
readable Lua-like file that is clearly labelled as a reconstruction.

Not recovered: the original code structure, conditions, variable names, anything that did not run in the sandbox
(other branches, the answers of the network) and the code that is downloaded at run time.
"""
import collections
import os
import re
import time

from .astutil import LuauMissing
from .messages import scrub
from .sandbox import SandboxError, run_luau

BANNER = re.compile(r'--\s*This script was generated using MoonVeil\s+(v?[0-9][0-9A-Za-z.\-]*)')
CFG = dict(waitcap=40, maxevents=8000, maxcb=60, maxruns=400, maxdepth=6, maxprobe=40, far=1000000, maxstr=300)
TAB = '\t'


class TraceError(Exception):
    def __init__(self, code, message):
        Exception.__init__(self, message)
        self.code = code
        self.message = message


def detect(text):
    """-> dict(version=...) when the script carries the MoonVeil banner, else None"""
    head = text[:1500]
    m = BANNER.search(head)
    if m:
        return dict(version=m.group(1).lstrip('v'))
    if re.search(r'moonveil\.cc', head, re.I):
        return dict(version='?')
    return None


# ------------------------------------------------------------------------------------------------ running the script
def _template():
    with open(os.path.join(os.path.dirname(os.path.abspath(__file__)), 'moonveil.lua.tpl'), encoding='latin1') as f:
        return f.read()


def run_script(text, timeout=60, mem_mb=1024):
    """run the script in the recording harness; returns the harness log (str)"""
    cfg = '{' + ', '.join('%s=%s' % (k, v) for k, v in sorted(CFG.items())) + '}'
    code = _template().replace('@@CFG@@', cfg).replace('@@SRC@@', text)
    try:
        out = run_luau(code, timeout=timeout, mem_mb=mem_mb)
    except SandboxError as e:
        if e.code == 'timeout':
            raise TraceError('timeout', 'the script kept running for more than %d s without finishing (a busy loop?)' % timeout)
        msg = e.message
        if re.search(r'Expected |Malformed|Incomplete statement|Unexpected', msg):
            raise TraceError('syntax', msg)
        raise TraceError('sandbox', msg)
    except LuauMissing as e:
        raise TraceError('setup', str(e))
    return out.decode('utf-8', 'replace')


# ------------------------------------------------------------------------------------------------ reading the log
class Body(object):
    """one run of one function (or the main chunk) and the events it produced"""

    def __init__(self, bid, labels, probe):
        self.bid = bid
        self.labels = labels
        self.probe = probe
        self.events = []
        self.status = 'cut'

    @property
    def fid(self):
        return int(self.bid[1:]) if self.bid[:1] == 'F' else None


class Trace(object):
    def __init__(self):
        self.bodies = []
        self.main = None
        self.by_fn = collections.defaultdict(list)
        self.tags = set()
        self.globals = []


_STR = re.compile(r'"((?:[^"\\]|\\.)*)"')


def unquote(s):
    """the text of a Lua string literal written by the harness (a trailing `--[[...]]` marker is ignored)"""
    m = _STR.match(s)
    if not m:
        return s

    def rep(x):
        c = x.group(1)
        if c.isdigit():
            return chr(int(c))
        return {'n': '\n', 't': '\t', 'r': '\r'}.get(c, c)
    return re.sub(r'\\(\d{3}|.)', rep, m.group(1))


def parse(log):
    t = Trace()
    stack = []
    for line in log.split('\n'):
        f = line.split('\t')
        k = f[0]
        try:
            if k == 'B':
                labels = dict(p.split('=', 1) for p in f[2].split(';') if '=' in p)
                b = Body(f[1], labels, unquote(f[3]) if len(f) > 3 and f[3] else None)
                t.bodies.append(b)
                if b.bid == 'M':
                    t.main = b
                else:
                    t.by_fn[b.fid].append(b)
                stack.append(b)
            elif k == 'E':
                if stack:
                    stack.pop().status = f[2]
            elif k in 'CSNIX' and len(k) == 1 and stack:
                if k == 'C' and len(f) < 4:
                    f = f + [''] * (4 - len(f))
                stack[-1].events.append(tuple(f))
            elif k == 'T':
                t.tags.add(f[1])
            elif k == 'G':
                t.globals.append(f[1])
        except (IndexError, ValueError):
            continue
    return t


# ------------------------------------------------------------------------------------------------ naming
LUA_WORDS = set('and break do else elseif end false for function if in local nil not or repeat return then true until '
                'while continue'.split())
RESERVED = LUA_WORDS | set('game workspace script task Instance Enum math string table os debug coroutine utf8 buffer bit32 '
                           'print warn error pcall type typeof tostring tonumber next pairs ipairs select require '
                           'loadstring setclipboard inst enabled dt value text choice key getgenv _G shared wait '
                           'spawn delay tick time'.split())
FIND = ('FindFirstChild', 'WaitForChild', 'FindFirstDescendant', 'FindFirstAncestor', 'FindFirstChildOfClass',
        'FindFirstChildWhichIsA')
STRING_AT_START = re.compile(r'\s*"((?:[^"\\]|\\.)*)"')


def lower_camel(s):
    words = re.findall(r'[A-Za-z0-9]+', s)
    if not words:
        return ''
    out = [words[0].lower() if words[0].isupper() else words[0][0].lower() + words[0][1:]]
    out += [w[0].upper() + w[1:] for w in words[1:]]
    return ''.join(out)


def singular(w):
    if w.lower() == 'children':
        return w[:-3]
    for a, b in (('ies', 'y'), ('ses', 's'), ('s', '')):
        if w.endswith(a) and len(w) > len(a) + 1:
            return w[:-len(a)] + b
    return w


class Namer(object):
    """gives readable, unique local names; the same call text gets the same name"""

    def __init__(self):
        self.by_key = {}
        self.used = set(RESERVED)

    def pick(self, key, base):
        if key in self.by_key:
            return self.by_key[key]
        base = re.sub(r'[^A-Za-z0-9_]', '', base)[:28] or 'v'
        if base[0].isdigit():
            base = '_' + base
        name, n = base, 1
        while name in self.used:
            n += 1
            name = '%s%d' % (base, n)
        self.used.add(name)
        self.by_key[key] = name
        return name

    @staticmethod
    def base_for(callee, args):
        method = re.split(r'[:.]', callee)[-1] if callee else ''
        m = STRING_AT_START.match(args or '')
        first = unquote('"%s"' % m.group(1)) if m else ''
        if method == 'new' and '.' in callee:
            seg = callee.split('.')[-2]
            return seg[:1].lower() + seg[1:]
        if method in ('GetService', 'FindService') and first:
            return first
        if method in FIND and first:
            return lower_camel(first) or 'child'
        if callee == 'Instance.new' and first:
            return lower_camel(first) or 'instance'
        if method == 'CreateWindow':
            return 'Window'
        if method in ('AddFolder', 'AddTab', 'AddSection', 'AddPage', 'AddGroup') and first:
            return (lower_camel(first) or 'main') + method[3:]
        if method.startswith('Add') and len(method) > 3:
            return lower_camel(method[3:])
        if callee.startswith('loadstring(') or callee == 'loadstring':
            return 'lib'
        if method in ('Connect', 'Once', 'ConnectParallel'):
            return 'connection'
        if method.startswith('Get') and len(method) > 3:
            return lower_camel(method[3:])
        return lower_camel(method) or 'v'


# ------------------------------------------------------------------------------------------------ text layout
class Group(object):
    def __init__(self, open_, close, items):
        self.open, self.close, self.items = open_, close, items


class Fn(object):
    def __init__(self, fid):
        self.fid = fid


def _string_end(text, pos):
    i = pos + 1
    while i < len(text):
        if text[i] == '\\':
            i += 2
            continue
        if text[i] == '"':
            return i + 1
        i += 1
    return len(text)


def parse_nodes(text):
    """text with function markers (\\x02id\\x03) -> nodes: str | Group | Fn; top-level commas stay plain text"""
    pos = [0]
    closers = {'(': ')', '{': '}', '[': ']'}

    def seq(closer):
        items, cur = [], []
        buf = []

        def flush():
            if buf:
                cur.append(''.join(buf))
                del buf[:]
        while pos[0] < len(text):
            c = text[pos[0]]
            if closer and c == closer:
                break
            if c == '"':
                j = _string_end(text, pos[0])
                buf.append(text[pos[0]:j])
                pos[0] = j
            elif text.startswith('--[[', pos[0]):
                j = text.find(']]', pos[0])
                j = len(text) if j < 0 else j + 2
                buf.append(text[pos[0]:j])
                pos[0] = j
            elif c in closers:
                flush()
                pos[0] += 1
                inner = seq(closers[c])
                pos[0] += 1
                cur.append(Group(c, closers[c], inner))
            elif c == '\x02':
                flush()
                j = text.index('\x03', pos[0])
                cur.append(Fn(int(text[pos[0] + 1:j])))
                pos[0] = j + 1
            elif c == ',' and closer:
                flush()
                items.append(cur)
                cur = []
                pos[0] += 1
                while pos[0] < len(text) and text[pos[0]] == ' ':
                    pos[0] += 1
            else:
                buf.append(c)
                pos[0] += 1
        flush()
        if closer:
            items.append(cur)
            return items
        return cur
    return seq(None)


def _has_fn(node):
    if isinstance(node, Fn):
        return True
    if isinstance(node, Group):
        return any(_has_fn(n) for it in node.items for n in it)
    return False


def indent(text, n=1):
    pad = TAB * n
    return '\n'.join((pad + ln) if ln else ln for ln in text.split('\n'))


# ------------------------------------------------------------------------------------------------ renderer
MARK = re.compile(r'("(?:[^"\\]|\\.)*")|@([REF])(\d+)')
REF = re.compile(r'@R(\d+)')
EXECUTOR = ('setclipboard', 'toclipboard', 'fireproximityprompt', 'firetouchinterest', 'firesignal', 'fireclickdetector',
            'loadstring', 'request', 'http_request', 'getgenv', 'hookfunction', 'hookmetamethod', 'getrawmetatable',
            'queue_on_teleport', 'writefile', 'readfile', 'getconnections', 'getgc', 'identifyexecutor')
HTTP = ('HttpGet', 'HttpGetAsync', 'HttpPost', 'HttpPostAsync', 'request', 'http_request')
KNOWN_GLOBALS = set('''game workspace script Instance Enum Vector2 Vector3 CFrame Color3 UDim UDim2 BrickColor TweenInfo Ray Random Rect
Region3 NumberRange NumberSequence ColorSequence NumberSequenceKeypoint ColorSequenceKeypoint Font Axes Faces OverlapParams
RaycastParams DateTime Drawing task wait spawn delay tick time warn print typeof settings stats UserSettings plugin
PhysicalProperties Workspace elapsedTime version printidentity require getgenv getrenv getreg getgc getinstances
getnilinstances getconnections getscripts getloadedmodules getsenv getfenv setfenv hookfunction hookmetamethod newcclosure
checkcaller cloneref clonefunction iscclosure islclosure isexecutorclosure getrawmetatable setrawmetatable setreadonly
isreadonly make_writeable make_readonly getnamecallmethod setnamecallmethod fireproximityprompt firetouchinterest firesignal
fireclickdetector setclipboard toclipboard loadstring request http_request http syn fluxus KRNL_LOADED identifyexecutor
getexecutorname queue_on_teleport isfile readfile writefile appendfile delfile listfiles makefolder isfolder delfolder
getcustomasset gethui getthreadidentity setthreadidentity setidentity decompile saveinstance mouse1click mouse1press
mouse1release mousemoveabs mousemoverel keypress keyrelease rconsoleprint rconsoleclear messagebox setfpscap
getcallingscript lz4compress lz4decompress crypt base64 base64_encode base64_decode'''.split())
URL = re.compile(r'https?://[^\s"\\)]+')


class Renderer(object):
    def __init__(self, trace):
        self.t = trace
        self.refs = collections.Counter()
        self.calls = {}
        for b in trace.bodies:
            for e in b.events:
                if e[0] == 'C':
                    self.calls[int(e[1])] = (e[2], e[3])
                for x in e[1:]:
                    for m in REF.finditer(x):
                        self.refs[int(m.group(1))] += 1
        self.rid_text = {}
        self.evar = {}
        self.namer = Namer()
        self.active = []
        self.rendered = 0

    # ---------------------------------------------------------------------------------------------- substitution
    def subst(self, text, note=False):
        def rep(m):
            if m.group(1):
                lit = m.group(1)
                if '@R' in lit:         # a value that was glued into a string: {game.PlaceId}
                    lit = REF.sub(lambda r: self.fullexpr(r.group(0)).replace('\\', '\\\\').replace('"', '\\"'), lit)
                return lit
            kind, n = m.group(2), int(m.group(3))
            if kind == 'R':
                v = self.rid_text.get(n)
                if v is None:
                    v = self.fullexpr('@R%d' % n) if n in self.calls else 'nil --[[unknown value]]'
                return v
            if kind == 'E':
                return self.evar.get(n, 'item')
            return 'function() end' if note else '\x02%d\x03' % n
        return MARK.sub(rep, text)

    def fullexpr(self, text, depth=0):
        """a value as one expression, with every earlier call written out (used for summaries and notes)"""
        def rep(m):
            if m.group(1):
                return m.group(1)
            if m.group(2) != 'R' or depth > 6 or int(m.group(3)) not in self.calls:
                return '?' if m.group(2) == 'R' else 'function() end'
            callee, args = self.calls[int(m.group(3))]
            return '%s(%s)' % (self.fullexpr(callee, depth + 1), self.fullexpr(args, depth + 1))
        return MARK.sub(rep, text)

    def fn_mentions(self, fid, rid, seen=None):
        seen = seen or set()
        if fid in seen:
            return False
        seen.add(fid)
        needle = '@R%d' % rid
        for b in self.t.by_fn.get(fid, ()):
            for e in b.events:
                for x in e[1:]:
                    if re.search(re.escape(needle) + r'(?!\d)', x):
                        return True
                    for m in re.finditer(r'@F(\d+)', x):
                        if self.fn_mentions(int(m.group(1)), rid, seen):
                            return True
        return False

    @staticmethod
    def mentions(event, rid):
        return any(re.search(r'@R%d(?!\d)' % rid, x) for x in event[1:])

    # ---------------------------------------------------------------------------------------------- one body
    def items(self, body):
        """pass 1: events -> statements (names are chosen here, in order); loops become nested lists"""
        ev = body.events
        root = []
        stack = [(None, None, root)]
        for i, e in enumerate(ev):
            k = e[0]
            cur = stack[-1][2]
            if k == 'C':
                rid = int(e[1])
                callee, args = self.subst(e[2]), self.subst(e[3])
                text = '%s(%s)' % (callee, args)
                nxt = ev[i + 1] if i + 1 < len(ev) else None
                n = self.refs[rid]
                if n == 1 and nxt is not None and nxt[0] in 'CSI' and self.mentions(nxt, rid):
                    self.rid_text[rid] = text
                    continue
                if n:
                    base = Namer.base_for(callee, args)
                    name = self.namer.pick(re.sub(r'\x02\d+\x03', '\x02\x03', text), base)
                    self.rid_text[rid] = name
                    ids = [int(x) for x in re.findall(r'\x02(\d+)\x03', args)]
                    if any(self.fn_mentions(f, rid) for f in ids):
                        cur.append(('stmt', 'local ' + name))
                        cur.append(('stmt', '%s = %s' % (name, self.tag(text))))
                    else:
                        cur.append(('stmt', 'local %s = %s' % (name, self.tag(text))))
                else:
                    cur.append(('stmt', self.tag(text)))
            elif k == 'S':
                cur.append(('stmt', '%s = %s' % (self.subst(e[1]), self.subst(e[2]))))
            elif k == 'N':
                cur.append(('stmt', '-- ' + self.subst(e[1], note=True).replace('\n', ' ')))
            elif k == 'I':
                expr = self.subst(e[2])
                mm = re.search(r'[:.](\w+)\([^()]*\)$', expr)
                mth = mm.group(1) if mm else ''
                var = lower_camel(singular(mth[3:])) if mth.startswith('Get') and len(mth) > 3 else 'item'
                var = var or 'item'
                outer = [str(x[3]) for x in stack if len(x) > 3]
                while var in outer or var in self.namer.used:
                    var += '_'
                self.evar[int(e[1])] = var
                stack.append((int(e[1]), 'for _, %s in ipairs(%s) do' % (var, expr), [], var))
            elif k == 'X':
                if len(stack) > 1 and stack[-1][0] == int(e[1]):
                    top = stack.pop()
                    stack[-1][2].append(('for', top[1], top[2]))
        while len(stack) > 1:
            top = stack.pop()
            stack[-1][2].append(('for', top[1], top[2]))
        return root

    @staticmethod
    def tag(text):
        if 'loadstring(' in text and 'HttpGet' in text:
            return text + ' -- NOT FETCHED: the downloaded code is not part of this trace'
        return text

    def finish(self, items, cut):
        """pass 2 + 3: expand function bodies, close loops, fold repeated rounds"""
        texts = []
        for it in items:
            if it[0] == 'stmt':
                texts.append(self.expand(it[1]))
            else:
                inner = self.finish(it[2], False) or ['-- UNKNOWN: nothing observable was done with each item']
                texts.append(it[1] + '\n' + indent('\n'.join(inner)) + '\nend')
        return self.collapse(texts, cut)

    @staticmethod
    def collapse(texts, cut):
        out, i, n = [], 0, len(texts)
        while i < n:
            hit = None
            for p in range(1, 9):
                k = 1
                while i + (k + 1) * p <= n and texts[i + k * p:i + (k + 1) * p] == texts[i:i + p]:
                    k += 1
                if k >= 4 or (k >= 3 and p >= 2):
                    hit = (p, k)
                    break
            if hit is None:
                out.append(texts[i])
                i += 1
                continue
            p, k = hit
            inner = '\n'.join(texts[i:i + p])
            rest = texts[i + k * p:]
            if cut and len(rest) < p and rest == texts[i:i + len(rest)]:
                out.append('while true do -- UNKNOWN stop condition (the trace saw %d rounds, then stopped it)\n%s\nend'
                           % (k, indent(inner)))
                i = n
                continue
            else:
                out.append('for _ = 1, %d do -- the same %d step%s happened %d times in a row\n%s\nend'
                           % (k, p, '' if p == 1 else 's', k, indent(inner)))
            i += k * p
        return out

    def body_lines(self, body, with_status=True):
        items = self.items(body)
        cut = body.status == 'cut'
        lines = self.finish(items, cut)
        if with_status:
            if body.status.startswith('err:'):
                lines.append('-- STOPPED in the sandbox: %s (usually because of the fake objects)' % body.status[4:])
            elif cut:
                if not (lines and lines[-1].startswith('while true do')):
                    lines.append('-- (trace cut here: the sandbox stopped waiting; the real script continues)')
        return lines

    # ---------------------------------------------------------------------------------------------- functions
    def expand(self, text):
        if '\x02' not in text:
            return text
        return self.nodes_text(parse_nodes(text))

    def nodes_text(self, nodes):
        out = []
        for n in nodes:
            if isinstance(n, str):
                out.append(n)
            elif isinstance(n, Fn):
                out.append(self.fn_text(n.fid))
            else:
                out.append(self.group_text(n))
        return ''.join(out)

    def group_text(self, g):
        items = [it for it in g.items if it]
        if not any(_has_fn(n) for it in items for n in it):
            return g.open + ', '.join(self.nodes_text(it) for it in items) + g.close
        with_fn = [it for it in items if any(_has_fn(n) for n in it)]
        if len(with_fn) == 1 and len(with_fn[0]) == 1 and not isinstance(with_fn[0][0], str):
            # f(a, function() ... end, b): the one function (or table) is written in place, the rest stays on the line
            return g.open + ', '.join(self.nodes_text(it) for it in items) + g.close
        rows = [indent(self.nodes_text(it)) for it in items]
        # a trailing comma is fine in a table constructor but not in an argument list
        return g.open + '\n' + ',\n'.join(rows) + (',' if g.open == '{' else '') + '\n' + g.close

    def fn_text(self, fid):
        bodies = self.t.by_fn.get(fid)
        if not bodies:
            return 'function() end --[[not run in the trace]]'
        if fid in self.active or len(self.active) > 12:
            return 'function() end'
        self.active.append(fid)
        try:
            params, lines = self.fn_body(bodies)
        finally:
            self.active.pop()
        head = 'function(%s)' % params
        if not lines:
            return head + ' end'
        return head + '\n' + indent('\n'.join(lines)) + '\nend'

    def fn_body(self, bodies):
        first = bodies[0]
        kind = first.labels.get('kind', 'spawn')
        param = first.labels.get('param', '')
        if kind == 'spawn':
            return '', self.body_lines(first)
        plain = [b for b in bodies if b.probe is None]
        probes = [b for b in bodies if b.probe is not None]
        if kind == 'toggle':
            on = self.dist_group([b for b in plain if b.labels.get('val') == 'true'])
            off = self.dist_group([b for b in plain if b.labels.get('val') == 'false'])
            if on == off:
                lines = on
            elif on and off:
                lines = ['if %s then' % param, indent('\n'.join(on)), 'else', indent('\n'.join(off)), 'end']
            elif on:
                lines = ['if %s then' % param, indent('\n'.join(on)), 'end']
            else:
                lines = ['if not %s then' % param, indent('\n'.join(off)), 'end']
            return param, lines or ['-- (nothing observable happened in the sandbox)']
        lines = self.dist_group(plain)
        if not lines and probes:
            groups = collections.OrderedDict()
            for b in probes:
                text = '\n'.join(self.body_lines(b, with_status=False))
                if text.strip():
                    groups.setdefault(text, []).append(b.probe)
            lines = []
            for text, names in groups.items():
                cond = ' or '.join('%s.Name == %s' % (param or 'inst', '"%s"' % self.esc(n)) for n in names[:6])
                if len(names) > 6:
                    cond += ' or ... --[[%d more names]]' % (len(names) - 6)
                lines.append('if %s then -- found by trying the names this script used elsewhere' % cond)
                lines.append(indent(text))
                lines.append('end')
        if not lines:
            lines = ['-- (nothing observable happened in the sandbox: the handler reacts to something that was not '
                     'tried) -- UNKNOWN']
        return param, lines

    @staticmethod
    def esc(s):
        return s.replace('\\', '\\\\').replace('"', '\\"').replace('\n', '\\n')

    def dist_group(self, bodies):
        """one or two runs of the same call (distance 0 and far away); shown as two blocks only when they differ"""
        if not bodies:
            return []
        runs = [(b.labels.get('dist', 'near'), self.body_lines(b)) for b in bodies]
        if len(runs) == 1 or all(r[1] == runs[0][1] for r in runs):
            return runs[0][1]
        out = []
        for dist, lines in runs:
            out.append('-- %s:' % ('when the script measured a short distance' if dist == 'near'
                                    else 'when the script measured a long distance (the real limit is not observable)'))
            out.append('do')
            out.append(indent('\n'.join(lines)) if lines else TAB + '-- (nothing)')
            out.append('end')
        return out

    # ---------------------------------------------------------------------------------------------- everything
    def render(self):
        return '\n'.join(self.body_lines(self.t.main))

    def summary(self):
        downloads, remotes, clip, execs = [], collections.OrderedDict(), [], []
        for b in self.t.bodies:
            for e in b.events:
                if e[0] != 'C':
                    continue
                callee, args = self.fullexpr(e[2]), self.fullexpr(e[3])
                method = re.split(r'[:.]', callee)[-1]
                for u in URL.findall(args):
                    if method in HTTP:
                        if u not in downloads:
                            downloads.append(u)
                if method in ('FireServer', 'InvokeServer'):
                    key = re.sub(r':(FireServer|InvokeServer)$', '', callee)
                    key = re.sub(r'^.*GetService\("ReplicatedStorage"\)\??\(?\)?\.?', '', key) or key
                    remotes[key[:90]] = remotes.get(key[:90], 0) + 1
                if callee in ('setclipboard', 'toclipboard'):
                    m = STRING_AT_START.match(args)
                    if m and m.group(1) not in clip:
                        clip.append(m.group(1))
                if callee in EXECUTOR and callee not in execs:
                    execs.append(callee)
        links = []
        for b in self.t.bodies:
            for e in b.events:
                for u in URL.findall(self.fullexpr(' '.join(e[1:]))):
                    if u not in downloads and u not in links:
                        links.append(u)
        unset = [g for g in self.t.globals if g not in KNOWN_GLOBALS]
        return dict(downloads=downloads, remotes=list(remotes.items()), clipboard=clip, executor=execs, links=links,
                    unset=unset)


# ------------------------------------------------------------------------------------------------ the whole job
def _wrap(label, items, width=100):
    lines = ['  - %s:' % label]
    for it in items:
        lines.append('      ' + it[:width])
    return lines


def header(version, summ, stats, assumed=False):
    if assumed:
        # no banner: the script was handled as a MoonVeil one because the user said so (the `!moonveil` command)
        what = ['  The script has no MoonVeil banner; it was handled as a MoonVeil script because that was asked for.',
                '  MoonVeil turns a script into a private VM program, so the original code (its structure, conditions and',
                '  variable names) cannot be recovered by any tool.']
    else:
        who = 'MoonVeil (version not known)' if version in ('', '?') else 'MoonVeil %s' % version
        what = ['  The script is protected with %s. MoonVeil turns a script into a private VM program, so the original' % who,
                '  code (its structure, conditions and variable names) cannot be recovered by any tool.']
    out = ['--[[',
           '  BEHAVIOUR RECONSTRUCTION - THIS IS NOT THE ORIGINAL SOURCE.'] + what + [
           '',
           '  What this file is: the protected script was run once in a sandbox against fake Roblox objects (no network,',
           '  no files) and every call it made was written down, in order, with the real names, strings and numbers.',
           '  - Observed: the calls, their order, strings, numbers and the delays between them.',
           '  - Not observed: conditions, other branches, results of the network and everything downloaded at run time.',
           '    Such places are marked UNKNOWN. Loops were cut after %d waits; real runs go on longer.' % CFG['waitcap'],
           '  - Threads and callbacks ran one after another, so the real timing between them is not preserved.',
           '  - Local names (Players, gui, ...) were chosen by this tool. Not tested in a real game.',
           '']
    if summ['downloads']:
        out += _wrap('Downloads and runs other code (NOT fetched, so not included)', summ['downloads'])
    if summ['remotes']:
        out += _wrap('Remote events it fires', ['%s (%d x)' % (k, n) if n > 1 else k for k, n in summ['remotes']])
    if summ['clipboard']:
        out += _wrap('Copies to the clipboard', summ['clipboard'])
    if summ['executor']:
        out += _wrap('Executor-only functions used', [', '.join(summ['executor'])])
    if summ['unset']:
        out += _wrap('Names it reads but never defines (probably set by the downloaded code)', [', '.join(summ['unset'][:12])])
    other = [u for u in summ['links'] if u not in summ['clipboard']][:6]
    if other:
        out += _wrap('Other links in strings', other)
    out += ['  - Seen: %d calls in %d threads/callbacks.' % (stats['calls'], stats['threads']), ']]', '', '']
    return '\n'.join(out)


def deobfuscate(text, mv, timeout=60, mem_mb=1024, note=None, check=None):
    """-> (text, info).  Raises TraceError.  `check(text) -> (ok, message)` is an optional syntax check."""
    note = note or (lambda s: None)
    t0 = time.time()
    note('unpack')
    log = run_script(text, timeout=timeout, mem_mb=mem_mb)
    t1 = time.time()
    note('decompile')
    trace = parse(log)
    if trace.main is None:
        raise TraceError('unsupported', 'the sandbox produced no trace for this script')
    calls = sum(1 for b in trace.bodies for e in b.events if e[0] in 'CS')
    if calls == 0:
        why = trace.main.status[4:] if trace.main.status.startswith('err:') else 'it did not call anything'
        raise TraceError('unsupported', 'the MoonVeil script did nothing observable in the sandbox (%s)' % scrub(why))
    r = Renderer(trace)
    body = r.render()
    summ = r.summary()
    stats = dict(calls=calls, threads=len(set(b.bid for b in trace.bodies)))
    text_out = header(mv.get('version', '?'), summ, stats, assumed=bool(mv.get('assumed'))) + body.rstrip() + '\n'
    info = dict(kind='moonveil', trace_only=True, moonveil=dict(mv), lines=text_out.count('\n'), calls=calls,
                threads=stats['threads'], downloads=summ['downloads'], warnings=[])
    if 'cap' in trace.tags:
        info['warnings'].append('the script made so many calls that the trace was cut (the end of the script is missing)')
    if trace.main.status.startswith('err:'):
        info['warnings'].append('the script stopped with an error in the sandbox (%s); what came after is not in the '
                                'trace' % scrub(trace.main.status[4:]))
    elif trace.main.status == 'cut':
        info['warnings'].append('the script was still waiting when the trace was stopped')
    if check:
        note('verify')
        ok, msg = check(text_out)
        if ok:
            info['syntax_ok'] = True
        elif ok is False:
            info['warnings'].append('the reconstruction has a syntax problem: ' + scrub(msg))
    info['timings'] = dict(trace=round(t1 - t0, 2), total=round(time.time() - t0, 2))
    return text_out, info

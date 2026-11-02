"""The fake Roblox sandbox, driven from Python.

A script is run inside the bundled Luau command line tool, but not on the real globals: the runtime in bgdeobf/lua/*.lua
builds a pretend Roblox (game, services, a player with a character, a virtual clock) and a pretend script executor
(HttpGet, request, loadstring, writefile ...).  Nothing real is touched: no request leaves the machine, no file is read or
written, time is simulated.  Everything the script does is written to a log, and this module turns the log into a report:
what was fetched, which remotes were fired, which windows were built, which code was handed to loadstring, ...

It is an approximation.  Services answer with made-up values, and a value the sandbox cannot know is a "ghost" that
absorbs whatever the script does with it.  A script that stops here on an error may well run on in the real game."""
import hashlib
import os
import re

from .astutil import LuauMissing
from .sandbox import SandboxError, live_supported, run_luau_live

LUA_DIR = os.path.join(os.path.dirname(os.path.abspath(__file__)), 'lua')


class TraceError(Exception):
    """code: empty | too_big | setup | sandbox | internal"""

    def __init__(self, code, message):
        Exception.__init__(self, message)
        self.code = code
        self.message = message


class TraceOptions(object):
    """What the pretend world looks like and how long the run may take.  Plain values only (this object travels as JSON)."""

    def __init__(self, place_id=0, game_id=None, job_id=None, user_id=None, username=None, executor=None, executor_version=None,
                 game_name=None, responses=None, files=None, seed=None, max_time=30.0, settle_time=10.0, max_steps=200000,
                 max_events=8000, explore=False, other_players=0, timeout=30, mem_mb=512, max_output_mb=64,
                 max_bytes=3 * 1024 * 1024):
        self.place_id = place_id              # game.PlaceId
        self.game_id = game_id                # game.GameId (default: the place id)
        self.job_id = job_id                  # game.JobId
        self.user_id = user_id                # LocalPlayer.UserId
        self.username = username              # LocalPlayer.Name
        self.executor = executor              # what identifyexecutor() says
        self.executor_version = executor_version
        self.game_name = game_name
        self.responses = responses            # {url: text} answers for HttpGet / request ("prefix*" matches a prefix)
        self.files = files                    # {path: text} files the script finds with readfile
        self.seed = seed                      # seed of math.random
        self.max_time = max_time              # simulated seconds
        self.settle_time = settle_time        # simulated seconds after which a script that only has frame handlers left is done
        self.max_steps = max_steps            # how many times the scheduler may wake a thread
        self.max_events = max_events          # how many log lines are written
        self.explore = explore                # afterwards click every button the script made, one by one
        self.other_players = other_players    # more players in the server
        self.timeout = timeout                # real seconds
        self.mem_mb = mem_mb
        self.max_output_mb = max_output_mb
        self.max_bytes = max_bytes            # largest script

    def to_dict(self):
        return dict(vars(self))

    @classmethod
    def from_dict(cls, d):
        known = set(vars(cls()).keys())
        return cls(**dict((k, v) for k, v in (d or {}).items() if k in known))

    def config(self, source, chunkname='script', pad=0):
        """the table the runtime reads (CONFIG)"""
        cfg = {'source': source, 'chunkname': chunkname}
        for key, value in (('place_id', self.place_id), ('game_id', self.game_id), ('job_id', self.job_id),
                           ('user_id', self.user_id), ('username', self.username), ('executor', self.executor),
                           ('executor_version', self.executor_version), ('game_name', self.game_name),
                           ('responses', self.responses), ('files', self.files), ('seed', self.seed),
                           ('max_time', self.max_time), ('settle_time', self.settle_time), ('max_steps', self.max_steps),
                           ('max_events', self.max_events), ('other_players', self.other_players)):
            if value is not None:
                cfg[key] = value
        if self.explore:
            cfg['explore'] = True
        if pad:
            cfg['pad'] = pad
        return cfg


# ------------------------------------------------------------------------------------------------ talking to the runtime

_BYTE_TEXT = [chr(i) if 32 <= i < 127 else '\\%03d' % i for i in range(256)]
_BYTE_TEXT[ord('"')] = '\\"'
_BYTE_TEXT[ord('\\')] = '\\\\'
_BYTE_TEXT[10] = '\\n'
_BYTE_TEXT[13] = '\\r'
_BYTE_TEXT[9] = '\\t'


def lua_str(data):
    """a Luau string literal for these bytes (ASCII only; every other byte is a \\ddd escape, so the script arrives exactly)"""
    if isinstance(data, str):
        data = data.encode('utf-8')
    return '"' + ''.join(map(_BYTE_TEXT.__getitem__, data)) + '"'


def lit(value):
    """a Luau literal for a Python value (dict / list / str / bytes / number / bool)"""
    if value is True:
        return 'true'
    if value is False:
        return 'false'
    if value is None:
        return 'nil'
    if isinstance(value, (str, bytes)):
        return lua_str(value)
    if isinstance(value, (int, float)):
        if isinstance(value, float) and (value != value or value in (float('inf'), float('-inf'))):
            raise ValueError('not a finite number')
        return repr(value)
    if isinstance(value, dict):
        return '{' + ', '.join('[%s] = %s' % (lit(k), lit(v)) for k, v in value.items()) + '}'
    if isinstance(value, (list, tuple)):
        return '{' + ', '.join(lit(v) for v in value) + '}'
    raise TypeError('cannot write %r as a Luau value' % (type(value).__name__,))


_RUNTIME = []


def runtime_source():
    """the runtime parts, in order, as bytes"""
    if not _RUNTIME:
        try:
            names = sorted(n for n in os.listdir(LUA_DIR) if n.endswith('.lua'))
        except OSError:
            names = []
        if not names or not names[0].startswith('00_'):
            raise TraceError('setup', 'the sandbox runtime files (bgdeobf/lua/*.lua) were not found')
        parts = []
        for n in names:
            with open(os.path.join(LUA_DIR, n), 'rb') as f:
                parts.append(f.read())
        _RUNTIME.append(b'\n'.join(parts) + b'\n')
    return _RUNTIME[0]


def build_script(source, opts=None, live=True):
    """the complete Luau program: the configuration (with the script inside it) followed by the runtime"""
    opts = opts or TraceOptions()
    cfg = opts.config(source, pad=0 if live else 4096)
    return ('local CONFIG = %s\n' % lit(cfg)).encode('ascii') + runtime_source()


_UNESC = re.compile(rb'\\(x[0-9A-F]{2}|[\\nrt])')


def _unesc_bytes(b):
    def one(m):
        g = m.group(1)
        if g[:1] == b'x':
            return bytes([int(g[1:], 16)])
        return {b'\\': b'\\', b'n': b'\n', b'r': b'\r', b't': b'\t'}[g]
    return _UNESC.sub(one, b)


def _unesc(b):
    return _unesc_bytes(b).decode('utf-8', 'replace')


class Event(object):
    __slots__ = ('time', 'line', 'kind', 'text', 'a', 'b')

    def __init__(self, time, line, kind, text, a=None, b=None):
        self.time, self.line, self.kind, self.text, self.a, self.b = time, line, kind, text, a, b


class Payload(object):
    """a text the script handed to loadstring, wrote to a file or sent in a request"""
    __slots__ = ('id', 'kind', 'label', 'length', 'data')

    def __init__(self, id, kind, label, length, data):
        self.id, self.kind, self.label, self.length, self.data = id, kind, label, length, data

    @property
    def truncated(self):
        return len(self.data) < self.length

    @property
    def sha256(self):
        return hashlib.sha256(self.data).hexdigest()


class Repeat(object):
    """events that were left out of the log because they kept repeating"""
    __slots__ = ('times', 'kind', 'text', 'first', 'last')

    def __init__(self, times, kind, text, first, last):
        self.times, self.kind, self.text, self.first, self.last = times, kind, text, first, last


def _num(b, default=0.0):
    try:
        return float(b)
    except ValueError:
        return default


def parse_log(data):
    """The runtime's output -> (events, payloads, repeats, end).  Only lines that start with the control character the runtime
    uses count (a script cannot print one: every control character in a text is escaped); anything else, like the padding
    lines, is ignored.  A last line that was cut off is dropped.  `end` is None when the run did not reach its end."""
    events, payloads, repeats, end = [], [], [], None
    lines = data.split(b'\n')
    if lines and lines[-1] != b'':
        lines.pop()                                  # no newline: the run was cut in the middle of this line
    for raw in lines:
        if not raw.startswith(b'\x01'):
            continue
        f = raw[1:].split(b'\t')
        tag = f[0]
        try:
            if tag == b'T' and len(f) >= 4:
                stamp, _, where = f[1].partition(b'@')
                events.append(Event(_num(stamp), where.decode('latin1') or None, f[2].decode('latin1'), _unesc(f[3]),
                                    _unesc(f[4]) if len(f) > 4 else None, _unesc(f[5]) if len(f) > 5 else None))
            elif tag == b'P' and len(f) >= 6:
                payloads.append(Payload(int(f[1]), f[2].decode('latin1'), _unesc(f[3]), int(f[4]), _unesc_bytes(b'\t'.join(f[5:]))))
            elif tag == b'S' and len(f) >= 6:
                repeats.append(Repeat(int(f[1]), f[2].decode('latin1'), _unesc(f[3]), _num(f[4]), _num(f[5])))
            elif tag == b'X' and len(f) >= 2:
                end = {'reason': _unesc(f[1])}
                for part in f[2:]:
                    for k, v in re.findall(r'(\w+)=(\S+)', _unesc(part)):
                        end[k] = v
        except (ValueError, IndexError):
            continue
    return events, payloads, repeats, end


class TraceResult(object):
    """status: ok | timeout | output | killed | crashed   (how the sandbox process ended; `end` is the runtime's own account)"""

    def __init__(self, events, payloads, repeats, end, status, stderr, seconds, source, max_time=30.0):
        self.max_time = max_time
        self.events = events
        self.payloads = payloads
        self.repeats = repeats
        self.end = end
        self.status = status
        self.stderr = stderr
        self.seconds = seconds
        self.source_size = len(source)
        self.source_lines = source.count(b'\n') + (1 if source and not source.endswith(b'\n') else 0)
        self.source_sha256 = hashlib.sha256(source).hexdigest()
        self.source = source

    @property
    def finished(self):
        return self.end is not None

    @property
    def reason(self):
        """why the run stopped, in words"""
        if self.end is not None:
            return self.end['reason']
        return {
            'timeout': 'the real-time limit was reached and the script was stopped (it may be stuck in a loop that never waits)',
            'output': 'the script produced more output than allowed and was stopped',
            'killed': 'the sandbox process was killed (out of memory or out of CPU time)',
            'crashed': 'the sandbox process stopped unexpectedly',
        }.get(self.status, 'the sandbox process ended without a final line')


def normalize(data):
    """script bytes as the sandbox gets them: no byte order mark"""
    if data.startswith(b'\xef\xbb\xbf'):
        data = data[3:]
    return data


def run(source, opts=None, progress=None):
    """Run `source` (bytes or text) in the fake Roblox and return a TraceResult.  Raises TraceError."""
    opts = opts or TraceOptions()
    note = progress or (lambda s: None)
    if isinstance(source, str):
        source = source.encode('utf-8')
    source = normalize(source)
    if not source.strip():
        raise TraceError('empty', 'there is no script to run')
    if len(source) > opts.max_bytes:
        raise TraceError('too_big', 'the script is larger than %d bytes' % opts.max_bytes)
    live = live_supported()
    try:
        program = build_script(source, opts, live)
    except (TypeError, ValueError) as e:
        raise TraceError('internal', 'the options could not be written for the sandbox: %s' % e)
    note('sandbox')
    try:
        lr = run_luau_live(program, timeout=opts.timeout, mem_mb=opts.mem_mb, max_output_mb=opts.max_output_mb, live=live)
    except SandboxError as e:
        raise TraceError('sandbox', e.message)
    except LuauMissing as e:
        raise TraceError('setup', str(e))
    events, payloads, repeats, end = parse_log(lr.out)
    return TraceResult(events, payloads, repeats, end, lr.status, lr.err, lr.seconds, source, opts.max_time)


# ------------------------------------------------------------------------------------------------ what the script did

_RE_NEW = re.compile(r'Instance\.new\("([A-Za-z0-9_]+)"')
_RE_FUNC = re.compile(r'^(?:local \w+ = )?([A-Za-z_][\w\.]*)\(')
_RE_REMOTE = re.compile(r'^(.*?):(FireServer|InvokeServer|FireClient|FireAllClients|InvokeClient)\(')
_RE_WEBHOOK = re.compile(r'^https?://(?:[\w-]+\.)*discord(?:app)?\.com/api(?:/v\d+)?/webhooks/', re.I)
_RE_DOMAIN = re.compile(r'^[a-z][a-z0-9+.-]*://([^/:?#\s]+)', re.I)
_RE_URL = re.compile(rb'https?://[^\s"\'<>\\)\]}]{4,200}', re.I)
_RE_AUTOEXEC = re.compile(r'(^|/)(auto-?exec(?:ute)?|scripts?|workspace)(/|$)', re.I)
_PROBE_NAMES = ('identifyexecutor', 'getexecutorname', 'gethwid', 'get_hwid', 'getthreadidentity', 'getidentity', 'checkcaller')


def _where(ev):
    """the coordinate of an event: L12 (line 12 of the script) or loadstring:3 (line 3 of a captured payload)"""
    if not ev.line:
        return ''
    return ev.line if ':' in ev.line else 'L%s' % ev.line


def _short(text, n=140):
    text = text.replace('\n', ' ')
    return text if len(text) <= n else text[:n - 3] + '...'


# characters that move the cursor, start a new line, or change the order text is drawn in
_LAYOUT = re.compile('[\x00-\x1f\x7f-\x9f\u2028\u2029\u202a-\u202e\u2066-\u2069\ufeff]')
_VIS = {'\n': '\\n', '\r': '\\r', '\t': '\\t'}


def _vis(text):
    """A line of the report with every layout-changing character written out (`\\n`, `\\x1b`, `\\u202e`).  What a script prints
    is data: it must not be able to start a new line (and so write a line that looks like a real log entry), move the cursor
    of a terminal that shows the report, or reorder the text around it."""
    def one(m):
        c = m.group(0)
        return _VIS.get(c) or ('\\x%02x' % ord(c) if ord(c) < 256 else '\\u%04x' % ord(c))
    return _LAYOUT.sub(one, text)


class Summary(object):
    """the events grouped by what they say about the script"""

    def __init__(self, result):
        r = result
        self.requests = []          # (event, method, url)
        self.remotes = {}           # key -> dict(count, first event, example text)
        self.loads = []             # (event, payload or None)
        self.files_written = []     # events
        self.files_read = 0
        self.executor_calls = {}    # function -> count
        self.kicks = []
        self.teleports = []
        self.errors = []
        self.probes = []
        self.notes = []
        self.created = {}           # class -> count
        self.connections = {}       # signal -> count
        self.prints = 0
        self.clipboard = []
        self.queued = []
        pay = dict((p.id, p) for p in r.payloads)
        for ev in r.events:
            k = ev.kind
            if k == 'net':
                self.requests.append((ev, ev.b or '', ev.a or ''))
            elif k == 'remote':
                m = _RE_REMOTE.match(ev.text)
                key = (m.group(1), m.group(2)) if m else (ev.text.split('(')[0], '')
                d = self.remotes.setdefault(key, dict(count=0, first=ev, example=ev.text))
                d['count'] += 1
            elif k == 'load':
                try:
                    p = pay.get(int(ev.a)) if ev.a else None
                except ValueError:
                    p = None
                self.loads.append((ev, p))
            elif k == 'file':
                m = _RE_FUNC.match(ev.text)
                fn = m.group(1) if m else ''
                if fn in ('writefile', 'appendfile', 'makefolder', 'delfile', 'delfolder'):
                    self.files_written.append(ev)
                else:
                    self.files_read += 1
            elif k == 'exec':
                m = _RE_FUNC.match(ev.text)
                fn = m.group(1) if m else ev.text.split('(')[0]
                self.executor_calls[fn] = self.executor_calls.get(fn, 0) + 1
                if fn == 'setclipboard' or fn == 'toclipboard':
                    self.clipboard.append(ev)
                elif fn == 'queue_on_teleport':
                    self.queued.append(ev)
            elif k == 'kick':
                self.kicks.append(ev)
            elif k == 'tp':
                self.teleports.append(ev)
            elif k == 'err':
                self.errors.append(ev)
            elif k == 'probe':
                self.probes.append(ev)
            elif k == 'note':
                self.notes.append(ev)
            elif k == 'def':
                m = _RE_NEW.search(ev.text)
                if m:
                    self.created[m.group(1)] = self.created.get(m.group(1), 0) + 1
            elif k == 'conn':
                m = re.search(r'\.(\w+):(?:Connect|Once)\(', ev.text)
                name = m.group(1) if m else '?'
                self.connections[name] = self.connections.get(name, 0) + 1
            elif k == 'out':
                self.prints += 1
        # events that were left out because they kept repeating still count
        for rep in r.repeats:
            if rep.kind == 'remote':
                m = _RE_REMOTE.match(rep.text)
                key = (m.group(1), m.group(2)) if m else (rep.text.split('(')[0], '')
                d = self.remotes.setdefault(key, dict(count=0, first=None, example=rep.text))
                d['count'] += rep.times
            elif rep.kind == 'out':
                self.prints += rep.times
            elif rep.kind == 'conn':
                m = re.search(r'\.(\w+):(?:Connect|Once)\(', rep.text)
                name = m.group(1) if m else '?'
                self.connections[name] = self.connections.get(name, 0) + rep.times
            elif rep.kind == 'exec':
                m = _RE_FUNC.match(rep.text)
                fn = m.group(1) if m else rep.text.split('(')[0]
                self.executor_calls[fn] = self.executor_calls.get(fn, 0) + rep.times
        self.payload_urls = []
        for p in r.payloads:
            if p.kind in ('loadstring', 'queue_on_teleport', 'clipboard'):
                for m in _RE_URL.finditer(p.data[:200000]):
                    u = m.group(0).decode('latin1').rstrip('.,;')
                    if u not in self.payload_urls:
                        self.payload_urls.append(u)
        self.result = r

    @property
    def instances(self):
        return sum(self.created.values())

    def domains(self):
        out = []
        for _, _, url in self.requests:
            m = _RE_DOMAIN.match(url)
            if m and m.group(1).lower() not in out:
                out.append(m.group(1).lower())
        return out

    def findings(self):
        """things a reader should look at first: [(title, detail, first event, how many times)].  Facts about what the run did,
        not verdicts.  The same finding again (a queue_on_teleport every few seconds) is counted, not repeated."""
        grouped = []
        index = {}
        for title, detail, ev in self._findings():
            key = (title, detail)
            if key in index:
                grouped[index[key]][3] += 1
            else:
                index[key] = len(grouped)
                grouped.append([title, detail, ev, 1])
        # calls that were left out of the log because they repeated are still calls
        counted = {'Queues code for the next teleport (queue_on_teleport)': 'queue_on_teleport', 'Copies text to the clipboard': 'setclipboard'}
        for g in grouped:
            fn = counted.get(g[0])
            if fn and self.executor_calls.get(fn, 0) > g[3]:
                g[3] = self.executor_calls[fn]
        return [tuple(g) for g in grouped]

    def _findings(self):
        out = []
        # a GET followed by a loadstring on the same line or a few lines later: the answer is probably what gets run
        # (a loadstring whose text is written in the script itself is not)
        gets = [(ev, url) for ev, method, url in self.requests if method == 'GET' and ev.line and ev.line.isdigit()]
        used = set()
        for lev, p in self.loads:
            if not (lev.line and lev.line.isdigit()):
                continue
            if p is not None and not p.truncated and p.data and p.data in self.result.source:
                continue
            near = [(ev, url) for ev, url in gets if id(ev) not in used and ev.time <= lev.time and 0 <= int(lev.line) - int(ev.line) <= 5]
            if near:
                ev, url = near[-1]
                used.add(id(ev))
                out.append(('Downloads code and runs it',
                            '%s is fetched, then loadstring runs the answer (L%s)%s' % (
                                _short(url, 120), lev.line, ', %d bytes captured' % p.length if p else ''), ev))
        for ev, method, url in self.requests:
            if _RE_WEBHOOK.match(url) and method == 'POST':
                out.append(('Sends data to a Discord webhook', _short(url, 120), ev))
        for ev in self.files_written:
            path = ev.a or ''
            if _RE_AUTOEXEC.search(path) and re.search(r'(auto-?exec)', path, re.I):
                out.append(('Writes into an auto-execute folder', _short(path, 120), ev))
        for ev in self.queued:
            out.append(('Queues code for the next teleport (queue_on_teleport)', _short(ev.text, 120), ev))
        for ev in self.clipboard:
            out.append(('Copies text to the clipboard', _short(ev.text, 120), ev))
        for ev in self.kicks:
            out.append(('Kicks the player', _short(ev.text, 140), ev))
        for ev in self.teleports:
            out.append(('Teleports the player', _short(ev.text, 140), ev))
        for ev in self.probes:
            low = ev.text.lower()
            if any(n in low for n in _PROBE_NAMES):
                out.append(('Asks which executor / device this is', _short(ev.text, 140), ev))
        return out


# ------------------------------------------------------------------------------------------------ the report

def payload_name(stem, p):
    """file name for a captured payload: code gets .lua, other texts (a written file, a request body) .txt"""
    return '%s_payload%d.%s' % (stem, p.id, 'lua' if p.kind in ('loadstring', 'queue_on_teleport') else 'txt')


def summary_info(result):
    """the numbers of a run, for the command line and the bot"""
    s = Summary(result)
    return dict(
        events=len(result.events), repeated=sum(rep.times for rep in result.repeats), requests=len(s.requests),
        domains=s.domains(), remotes=sum(d['count'] for d in s.remotes.values()), payloads=len(result.payloads),
        loadstrings=len(s.loads), errors=len(s.errors), files_written=len(s.files_written), instances=s.instances,
        findings=[title for title, _, _, _ in s.findings()], finished=result.finished, reason=result.reason,
        simulated=(result.end or {}).get('time'), real_seconds=round(result.seconds, 2), status=result.status)


def render_report(result, name='script', lines_of='the script', stem=None):
    """The report as text: header, summary, the log with simulated time and script line, what was left out, and the limits.
    `lines_of`: which file the L<number> coordinates refer to."""
    r = result
    s = Summary(r)
    out = []

    def add(line):
        out.append(_vis(line))
    add('FAKE ROBLOX SANDBOX TRACE  -  %s' % name)
    add('=' * 72)
    add('Script: %d bytes, %d lines, sha256 %s' % (r.source_size, r.source_lines, r.source_sha256))
    if r.end:
        add('Ended: %s' % r.reason)
        add('Run: %s simulated seconds, %s scheduler steps, %.2f s real time' % (r.end.get('time', '?'), r.end.get('steps', '?'), r.seconds))
    else:
        add('Ended: %s' % r.reason)
        add('Run: stopped after %.1f s real time; the log below is what had been written until then' % r.seconds)
    add('How to read it: the first column is simulated time in seconds since the script started, L<number> is the line of %s '
        'that did it.' % lines_of)
    add('')
    add('SUMMARY')
    add('-' * 72)
    shown = len(r.events)
    left_out = sum(rep.times for rep in r.repeats)
    add('Events: %d logged, %d more were left out because they repeated' % (shown, left_out))
    if s.requests:
        add('Network (nothing was sent, the sandbox answered): %d requests to %s' % (len(s.requests), ', '.join(s.domains()) or 'unknown hosts'))
        seen = set()
        for ev, method, url in s.requests:
            if (method, url) in seen:
                continue
            seen.add((method, url))
            add('  %-4s %s   [%s %s]' % (method.split()[0] if method else '?', _short(url, 150), '%.3f s' % ev.time, _where(ev)))
            if len(seen) >= 25:
                add('  ... and more (see the log)')
                break
    else:
        add('Network: no requests')
    if s.loads:
        add('loadstring: %d call(s)' % len(s.loads))
    if r.payloads:
        add('Captured texts: %d (what loadstring ran, request bodies, files written, code queued for a teleport, long clipboard texts)'
            % len(r.payloads))
    for p in r.payloads[:20]:
        add('  payload %d (%s, %s): %d bytes%s, sha256 %s%s' % (
            p.id, p.kind, _short(p.label, 60), p.length, ' (first %d kept)' % len(p.data) if p.truncated else '', p.sha256[:16] + '...',
            ('  -> ' + payload_name(stem, p)) if stem else ''))
        first = ' '.join(p.data[:300].decode('utf-8', 'replace').split())
        add('    starts with: %s' % _short(first, 160))
    if len(r.payloads) > 20:
        add('  ... and %d more payloads (all are saved as files)' % (len(r.payloads) - 20))
    if s.payload_urls:
        add('Addresses found inside the captured payloads:')
        for u in s.payload_urls[:15]:
            add('  %s' % u)
    if s.remotes:
        add('Remotes (%d different, %d calls):' % (len(s.remotes), sum(d['count'] for d in s.remotes.values())))
        for (path, meth), d in sorted(s.remotes.items(), key=lambda kv: -kv[1]['count'])[:25]:
            when = ('  [first %.3f s %s]' % (d['first'].time, _where(d['first']))) if d['first'] else ''
            add('  x%-4d %s:%s%s' % (d['count'], path, meth, when))
            add('        e.g. %s' % _short(d['example'], 150))
    else:
        add('Remotes: none fired')
    if s.files_written or s.files_read:
        add('Files (a virtual folder, the real disk was not touched): %d writes/changes, %d reads/checks' % (len(s.files_written), s.files_read))
        for ev in s.files_written[:15]:
            add('  %s   [%.3f s %s]' % (_short(ev.text, 150), ev.time, _where(ev)))
    if s.executor_calls:
        add('Executor functions: ' + ', '.join('%s x%d' % (k, v) for k, v in sorted(s.executor_calls.items(), key=lambda kv: -kv[1])[:20]))
    if s.created:
        add('Instances created: %d (%s)' % (s.instances, ', '.join('%s %d' % (k, v) for k, v in sorted(s.created.items(), key=lambda kv: -kv[1])[:8])))
    if s.connections:
        add('Event handlers connected: %d (%s)' % (sum(s.connections.values()), ', '.join('%s %d' % (k, v) for k, v in sorted(s.connections.items(), key=lambda kv: -kv[1])[:8])))
    add('Console output: %d print/warn calls' % s.prints)
    if s.probes:
        add('Questions the script asked about its environment (answered with defaults):')
        for ev in s.probes[:15]:
            add('  %s   [%.3f s %s]' % (_short(ev.text, 150), ev.time, _where(ev)))
    if s.errors:
        add('Errors:')
        for ev in s.errors[:10]:
            flag = '   <- raised inside the fake environment, not by the script itself' if ev.b == 'internal' else ''
            add('  %s   [%.3f s %s]%s' % (_short(ev.text, 200), ev.time, _where(ev), flag))
    notes = [ev for ev in s.notes if not ev.text.startswith('explore:')]
    for ev in notes[:5]:
        add('Note: %s' % _short(ev.text, 200))
    add('')
    finds = s.findings()
    add('WORTH A LOOK FIRST (what the script did in this run; not a verdict)')
    add('-' * 72)
    if finds:
        for title, detail, ev, n in finds[:30]:
            add('* %s: %s   [%.3f s %s]%s' % (title, detail, ev.time, _where(ev), ' (%d times)' % n if n > 1 else ''))
    else:
        add('(nothing of the kinds this tool looks for)')
    add('')
    add('LOG')
    add('-' * 72)
    for ev in r.events:
        add('%9.3f  %-10s %-7s %s' % (ev.time, _where(ev), ev.kind, ev.text))
        if ev.kind == 'err' and ev.a:
            add('%29s%s' % ('', 'stack: ' + _short(ev.a, 400)))
    if not r.events:
        add('(no events: the script did nothing that the sandbox writes down)')
    if r.repeats:
        add('')
        add('LEFT OUT BECAUSE THEY KEPT REPEATING (the same line again and again; only the first few are in the log)')
        add('-' * 72)
        for rep in r.repeats:
            add('x%-6d %-7s %s   [%.3f s .. %.3f s]' % (rep.times, rep.kind, _short(rep.text, 170), rep.first, rep.last))
    add('')
    add('LIMITS OF THIS TRACE')
    add('-' * 72)
    for line in limits(r, s):
        add('* ' + line)
    return '\n'.join(out) + '\n'


def limits(r, s=None):
    """what this trace can not tell you, for this run"""
    out = ['This is a pretend Roblox: services answer with made-up values and the world has no real game in it. '
           'Code that waits for the real game (a server event, a particular window, another player) may not have run.',
           'Nothing was downloaded: every request was answered by the sandbox (an empty text unless --responses gave one), '
           'so code that a real server would have sent was not seen.',
           'Only the first %g simulated seconds are shown; later behavior (long timers, things that start on a button press) '
           'is not in this log. Use --explore to press every button the script made.' % r.max_time]
    if not r.finished:
        out.append('The run did not finish normally (%s), so the end of the log is missing.' % r.reason)
        if r.status == 'crashed' and r.stderr:
            out.append('First line the sandbox process wrote to its error output: %s' % _short(r.stderr.splitlines()[0], 200))
    internal = [ev for ev in r.events if ev.kind == 'err' and ev.b == 'internal']
    if internal:
        out.append('%d error(s) came from the fake environment itself. The real game may not raise them.' % len(internal))
    elif any(ev.kind == 'err' for ev in r.events):
        out.append('The script stopped with an error here. In the real game it might have gone on (the sandbox may lack something it needs).')
    if any(p.truncated for p in r.payloads):
        out.append('At least one captured payload is longer than the sandbox keeps; only the beginning is saved.')
    if r.end and r.end.get('cut') == '1':
        out.append('The log was cut at its size limit; later events are missing.')
    return out

import json
import os
import re
import subprocess
import tempfile

from .messages import scrub


class LuauError(Exception):
    pass


class LuauMissing(LuauError):
    """the Luau command line tools were not found"""


class LuauSyntax(LuauError):
    """the text is not valid Luau"""


class LuauTimeout(LuauError):
    pass


def find_bin(name):
    """locate a Luau CLI binary: $BGDEOBF_LUAU_DIR, ./luau_bin next to the package, then $PATH"""
    cand = []
    d = os.environ.get('BGDEOBF_LUAU_DIR')
    if d:
        cand.append(os.path.join(d, name))
    here = os.path.dirname(os.path.abspath(__file__))
    cand.append(os.path.join(here, '..', 'luau_bin', name))
    cand.append(os.path.join(here, 'luau_bin', name))
    for c in cand:
        for ext in (('', '.exe') if os.name == 'nt' else ('',)):
            if os.path.isfile(c + ext) and os.access(c + ext, os.X_OK):
                return os.path.abspath(c + ext)
    from shutil import which
    w = which(name)
    if w:
        return w
    raise LuauMissing('Luau binary not found: %s (set BGDEOBF_LUAU_DIR)' % name)


def parse_loc(s):
    a, b = s.split(' - ')
    l1, c1 = a.split(',')
    l2, c2 = b.split(',')
    return int(l1), int(c1), int(l2), int(c2)


class Src:
    """latin1 text with 0-based (line, col) <-> absolute offset"""

    def __init__(self, text):
        self.text = text
        starts = [0]
        for m in re.finditer('\n', text):
            starts.append(m.end())
        self.starts = starts

    def off(self, line, col):
        return self.starts[line] + col

    def span(self, node_or_loc):
        loc = node_or_loc['location'] if isinstance(node_or_loc, dict) else node_or_loc
        l1, c1, l2, c2 = parse_loc(loc)
        return self.off(l1, c1), self.off(l2, c2)

    def slice(self, node):
        a, b = self.span(node)
        return self.text[a:b]


def normalize_source(data):
    if isinstance(data, bytes):
        data = data.decode('latin1')
    if data.startswith('\xef\xbb\xbf'):
        data = data[3:]
    data = data.replace('\r\n', '\n').replace('\r', '\n')
    return data


def parse_ast(text, timeout=120):
    """run luau-ast on text, return the root block (dict)"""
    fd, path = tempfile.mkstemp(suffix='.lua')
    try:
        with os.fdopen(fd, 'wb') as f:
            f.write(text.encode('latin1'))
        r = subprocess.run([find_bin('luau-ast'), path], capture_output=True, timeout=timeout)
    except subprocess.TimeoutExpired:
        raise LuauTimeout('luau-ast took longer than %d s' % timeout)
    finally:
        try:
            os.unlink(path)
        except OSError:
            pass
    if r.returncode != 0:
        msg = scrub(r.stderr.decode('latin1', 'replace').strip(), path)
        if 'Parse error' in msg:
            raise LuauSyntax(' '.join(msg.split())[:300])
        raise LuauError('luau-ast failed: ' + msg[:300])
    try:
        return json.loads(r.stdout.decode('latin1'))['root']
    except (ValueError, KeyError) as e:
        raise LuauError('luau-ast output could not be read: %s' % type(e).__name__)


def kids(n):
    """child AST nodes (dicts with 'type') of a node, in key order"""
    if isinstance(n, dict):
        for k, v in n.items():
            if isinstance(v, dict) and 'type' in v:
                yield v
            elif isinstance(v, list):
                for x in v:
                    if isinstance(x, dict):
                        if 'type' in x:
                            yield x
                        else:
                            for vv in x.values():
                                if isinstance(vv, dict) and 'type' in vv:
                                    yield vv


def walk(n, parent=None):
    st = [(n, parent)]
    while st:
        x, p = st.pop()
        yield x, p
        ch = list(kids(x))
        for c in reversed(ch):
            st.append((c, x))


def lkey(l):
    """identity of an AstLocal (declaration location)"""
    return l['location']


def unparen(e):
    while e['type'] == 'AstExprGroup':
        e = e['expr']
    return e


def check_tools():
    """try every Luau tool on a trivial script; returns [(name, ok, message)]"""
    out = []
    fd, path = tempfile.mkstemp(suffix='.lua')
    try:
        with os.fdopen(fd, 'w') as f:
            f.write('print("ok")\n')
        for name in ('luau', 'luau-ast', 'luau-compile'):
            try:
                exe = find_bin(name)
            except LuauMissing:
                out.append((name, False, 'not found'))
                continue
            try:
                r = subprocess.run([exe, path], capture_output=True, timeout=30)
            except OSError as e:
                out.append((name, False, "can't be run (%s), probably a binary for another OS" % e.strerror))
                continue
            except subprocess.TimeoutExpired:
                out.append((name, False, 'not responding'))
                continue
            if r.returncode != 0:
                out.append((name, False, 'failed to run (exit %d)' % r.returncode))
            else:
                out.append((name, True, exe))
    finally:
        try:
            os.unlink(path)
        except OSError:
            pass
    return out

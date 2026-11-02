"""User-facing texts (English) for engine results and errors."""
import re

_ABS_LUA = r'(?:[A-Za-z]:)?[\\/][^\n:]*?\.lua'
_AT_PAREN = re.compile(r'(?:\bscript\.lua|' + _ABS_LUA + r')\((\d+),\s*(\d+)\):\s*')
_AT_COLON = re.compile(r'(?:\bscript\.lua|' + _ABS_LUA + r'):(\d+):\s*')
_HOST_PATH = re.compile(r'(?:[A-Za-z]:)?[\\/](?:[^\s\\/:()\'"<>|*?]+[\\/])+[^\s\\/:()\'"<>|*?]*')


def scrub(text, *paths):
    """Remove host file paths from tool output before a user sees it.
    The given paths become `script.lua`; `<file>.lua(8,14): msg` becomes `line 8:14: msg` and `<file>.lua:12: msg`
    becomes `line 12: msg` (the text before the path is kept); any other absolute path becomes `<path>`."""
    text = str(text)
    for p in paths:
        if p:
            for v in (p, p.replace('\\', '/'), p.replace('/', '\\')):
                text = text.replace(v, 'script.lua')
    text = _AT_PAREN.sub(lambda m: 'line %s:%s: ' % (m.group(1), m.group(2)), text)
    text = _AT_COLON.sub(lambda m: 'line %s: ' % m.group(1), text)
    return _HOST_PATH.sub('<path>', text)


ERRORS = {
    'empty': 'The file is empty.',
    'too_big': 'The file is too large.',
    'binary': "This file isn't text (it looks like binary / bytecode).",
    'syntax': "This file isn't valid Luau (it may be truncated or copied incorrectly). Send the original file.",
    'setup': 'The Luau tools were not found on this machine (check the luau_bin folder or BGDEOBF_LUAU_DIR).',
    'unsupported': "This script isn't a Bacon Guard / FORGE or MoonVeil build I recognize, or it uses parts I don't understand yet, so I won't guess.",
    'timeout': 'The job went over the time limit and was stopped.',
    'sandbox': "The loader couldn't be opened in the sandbox (the script is damaged, truncated, or an unsupported version).",
    'decode': 'The VM contents were read, but some instructions could not be translated.',
    'decompile': 'The VM contents were read, but the decompiler failed to rebuild the code.',
    'memory': 'Ran out of memory while processing this file.',
    'cancelled': 'The job was cancelled.',
    'internal': 'An internal error occurred.',
}


SHOW_DETAIL = ('syntax', 'unsupported', 'sandbox', 'decode', 'decompile', 'setup')


def explain(code, detail=None):
    base = ERRORS.get(code, ERRORS['internal'])
    if detail and code in SHOW_DETAIL:
        return '%s\nTechnical detail: %s' % (base, scrub(detail))
    return base


_HOST_OK = re.compile(r'^[a-z0-9][a-z0-9.-]{0,120}$')


def host_list(domains, markdown=False, limit=4):
    """A few host names for a message.  The script chooses these strings, so only plain names (letters, digits, dots, hyphens)
    are repeated; anything else (markdown, a mention, a link) is only counted.  `markdown`: put each in a code span so that
    Discord does not turn it into a link."""
    plain = [d for d in domains if _HOST_OK.match(d)]
    odd = len(domains) - len(plain)
    fmt = '`%s`' if markdown else '%s'
    txt = ', '.join(fmt % d for d in plain[:limit]) + (' ...' if len(plain) > limit else '')
    if odd:
        txt += (' and ' if txt else '') + '%d host name(s) with unusual characters' % odd
    return txt or 'unknown hosts'


def trace_lines(t, markdown=False):
    """English lines about a run in the fake Roblox (`info['trace']`, see tracer.summary_info)"""
    out = []
    parts = ['%d events' % t['events']]
    if t['requests']:
        parts.append('%d request(s) to %s' % (t['requests'], host_list(t['domains'], markdown)))
    if t['remotes']:
        parts.append('%d remote call(s)' % t['remotes'])
    if t['payloads']:
        parts.append('%d text(s) captured (loadstring / files / requests)' % t['payloads'])
    if t['instances']:
        parts.append('%d instances created' % t['instances'])
    if t['errors']:
        parts.append('%d error(s)' % t['errors'])
    out.append('Fake Roblox run: ' + ', '.join(parts))
    if t.get('findings'):
        seen = []
        for f in t['findings']:
            if f not in seen:
                seen.append(f)
        out.append('Worth a look: ' + '; '.join(seen[:6]))
    out.append('Run ended: %s%s' % (t['reason'], '' if t['finished'] else ' (the log may be incomplete)'))
    return out


def verdict(info, markdown=False):
    """short English summary of a successful run (`markdown`: the text goes to Discord)"""
    out = []
    if info.get('sandbox_only'):
        b = info.get('banner')
        out.append('Ran the script as given in the fake Roblox (nothing was deobfuscated)')
        if b:
            out.append('It carries a Bacon Guard v%s banner: the log shows the protected script itself, so most of it is the VM. '
                       'Deobfuscate it first (or use the trace option together with deobfuscation) to read what it does.' % b['version'])
        if info.get('trace'):
            out.extend(trace_lines(info['trace'], markdown))
        for w in info.get('warnings', []):
            out.append('Note: ' + scrub(w))
        return out
    if info.get('trace_only'):
        return _verdict_trace(info, markdown)
    b = info.get('banner')
    if b:
        out.append('Detected: Bacon Guard v%s (%s), build %s' % (b['version'], b['protection'], b['build']))
    else:
        out.append('No Bacon Guard banner, but the VM structure was recognized')
    out.append('%d functions, %d VM instructions, %d output lines' % (info.get('protos', 0), info.get('records', 0), info.get('lines', 0)))
    f = info.get('fidelity')
    if f:
        txt = 'Bytecode check: %d/%d functions identical' % (f['identical'], f['functions'])
        if f.get('reordered'):
            txt += ', %d with the same instructions in a different order (only plain reads and calculations moved)' % f['reordered']
        if f.get('cosmetic'):
            txt += ', %d with a cosmetic difference (extra `return`)' % f['cosmetic']
        if f.get('differing'):
            txt += ', %d differ (check manually)' % len(f['differing'])
        out.append(txt)
    elif info.get('syntax_ok'):
        out.append('Output syntax is valid (bytecode check unavailable)')
    if info.get('annotated'):
        out.append('Annotated copy: the `<@…>` keys are for building a names file, so this version is not runnable as is'
                   + (' (the checks ran on a clean copy)' if f or info.get('syntax_ok') else ''))
    if info.get('trace'):
        out.extend(trace_lines(info['trace'], markdown))
    for w in info.get('warnings', []):
        out.append('Note: ' + scrub(w))
    return out


def _verdict_trace(info, markdown=False):
    """summary of a behaviour trace (MoonVeil): never presented as a decompilation"""
    mv = info.get('moonveil') or {}
    v = mv.get('version')
    if mv.get('assumed'):
        out = ['No MoonVeil banner found; the script was handled as a MoonVeil one because that was asked for']
    else:
        out = ['Detected: MoonVeil%s (private VM protector)' % (' ' + v if v and v != '?' else '')]
    out.append("This is a behaviour reconstruction, NOT the original source: MoonVeil can't be reversed to source. "
               'The script was run in a sandbox with fake Roblox objects and what it did was written down')
    out.append('%d calls seen in %d threads/callbacks, %d output lines' % (info.get('calls', 0), info.get('threads', 0),
                                                                          info.get('lines', 0)))
    if info.get('downloads'):
        out.append('It downloads and runs %d other script%s (not fetched, so not included): %s'
                   % (len(info['downloads']), '' if len(info['downloads']) == 1 else 's', ', '.join(info['downloads'][:3])))
    if info.get('syntax_ok'):
        out.append('Output syntax is valid')
    if info.get('trace'):
        out.extend(trace_lines(info['trace'], markdown))
    for w in info.get('warnings', []):
        out.append('Note: ' + scrub(w))
    return out


def explain_fetch(code, detail=None, max_mb=3):
    """English text for a FetchError (bot: URL download)"""
    if code == 'html':
        return ('That URL points to a web page, not a raw file. Use the **Raw** link '
                '(e.g. raw.githubusercontent.com or pastebin.com/raw/...).')
    if code == 'too_big':
        return 'The file at that URL is too large (max %g MB).' % max_mb
    if code == 'empty':
        return 'The file at that URL is empty.'
    if code == 'timeout':
        return 'The server at that URL took too long to respond.'
    head = {'bad_url': "That URL can't be used", 'blocked': 'That URL was rejected', 'http': 'Failed to fetch the URL',
            'net': "Couldn't fetch that URL"}.get(code, 'Failed to fetch the URL')
    return '%s: %s.' % (head, (detail or '').rstrip('.')) if detail else head + '.'

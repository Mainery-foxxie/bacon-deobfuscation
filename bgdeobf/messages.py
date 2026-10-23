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
    'unsupported': "This script isn't a Bacon Guard / FORGE build I recognize, or the build uses parts I don't understand yet, so I won't guess.",
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


def verdict(info):
    """short English summary of a successful run"""
    out = []
    b = info.get('banner')
    if b:
        out.append('Detected: Bacon Guard v%s (%s), build %s' % (b['version'], b['protection'], b['build']))
    else:
        out.append('No Bacon Guard banner, but the VM structure was recognized')
    out.append('%d functions, %d VM instructions, %d output lines' % (info.get('protos', 0), info.get('records', 0), info.get('lines', 0)))
    f = info.get('fidelity')
    if f:
        n, same = f['functions'], f['identical'] + f['reordered']
        txt = 'Bytecode check: %d/%d functions identical' % (same, n)
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

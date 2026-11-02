"""Optional file with your own variable names: {"12:34": "playerList", "12:p1": "target"}.
Keys are shown next to every variable when you run with annotate/--annotate."""
import json
import re

KEY = re.compile(r'^\d{1,6}:(p\d{1,3}|\d{1,6})$')
VALUE = re.compile(r'^[A-Za-z_][A-Za-z0-9_]{0,39}$')


class NamesError(ValueError):
    pass


def parse_names(data, max_items=20000):
    """bytes/str of JSON -> {key: name}; raises NamesError with a short English message"""
    if isinstance(data, bytes):
        try:
            data = data.decode('utf-8-sig')
        except UnicodeDecodeError:
            raise NamesError('the names file must be UTF-8')
    try:
        obj = json.loads(data)
    except (ValueError, RecursionError):
        raise NamesError('the names file is not valid JSON')
    if not isinstance(obj, dict):
        raise NamesError('the names file must be a JSON object: {"12:34": "name"}')
    if len(obj) > max_items:
        raise NamesError('the names file has too many entries')
    out = {}
    for k, v in obj.items():
        if not isinstance(k, str) or not KEY.match(k):
            raise NamesError('unknown key: %r (valid examples: "12:34" or "12:p1")' % (str(k)[:30],))
        if not isinstance(v, str) or not VALUE.match(v):
            raise NamesError('invalid name for %s (letters/digits/_ only, must not start with a digit, max 40)' % k)
        out[k] = v
    return out

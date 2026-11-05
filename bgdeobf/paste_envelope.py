"""The "Secure Runtime Envelope" that some paste sites answer a raw link with.

Instead of the script, the site sends a short loader (about 1 KB): a few lines of Lua that check they run inside Roblox
and then `game:HttpGet` the real script from a signed link on the same site. This module reads that one link out of the
loader's text, so the downloader can ask for it too, the way it follows a redirect.

It only reads text. It never runs the loader, never touches the network, and gives back no link that is not on the very
site (scheme, host and port) that sent the loader."""
import re
from urllib.parse import urlsplit

MAX_LOADER = 8192        # the loader is about 1 KB; anything bigger is a script, not a loader
MAX_LINK = 2048
MARKER = 'Secure Runtime Envelope'

_DEFAULT_PORT = {'http': 80, 'https': 443}
_LINK_LINE = re.compile(r'''\blocal\s+url\s*=\s*(["'])(.*?)\1''')
_STREAM_PATH = re.compile(r'/api/stream/[A-Za-z0-9_-]{1,100}')


def _origin(url):
    """(scheme, host, port) of an http(s) URL, or None"""
    if not isinstance(url, str):
        return None
    try:
        sp = urlsplit(url)
        port = sp.port
    except ValueError:
        return None
    scheme = sp.scheme.lower()
    host = (sp.hostname or '').rstrip('.').lower()
    if scheme not in _DEFAULT_PORT or not host:
        return None
    return scheme, host, port or _DEFAULT_PORT[scheme]


def _is_stream_link(link, origin):
    if not link or len(link) > MAX_LINK or '\\' in link or any(not 32 < ord(c) < 127 for c in link):
        return False
    try:
        sp = urlsplit(link)
    except ValueError:
        return False
    if sp.username is not None or sp.password is not None or '@' in sp.netloc or sp.fragment or '#' in link:
        return False
    return _origin(link) == origin and _STREAM_PATH.fullmatch(sp.path) is not None


def stream_url(data, page_url):
    """the link a Secure Runtime Envelope loader fetches its script from; None when `data` is not such a loader, or
    when the link is not the stream link of the same site that `page_url` is on"""
    origin = _origin(page_url)
    if origin is None:
        return None
    if isinstance(data, (bytes, bytearray)):
        if len(data) > MAX_LOADER:
            return None
        text = bytes(data).decode('utf-8', 'replace')
    elif isinstance(data, str):
        if len(data) > MAX_LOADER:
            return None
        text = data
    else:
        return None
    text = text.lstrip('﻿')
    if MARKER not in text[:1000] or 'HttpGet' not in text:
        return None
    found = _LINK_LINE.findall(text)
    if len(found) != 1:
        return None
    link = found[0][1]
    return link if _is_stream_link(link, origin) else None

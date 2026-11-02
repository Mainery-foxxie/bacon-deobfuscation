"""Download a script from a URL for a chat user, without letting that user reach the host's own network.

- only http/https on ports 80/443, no user:password in the URL
- every address a name resolves to must be a public one; the check lives in the resolver the connection really
  uses, so a DNS answer cannot change between the check and the connect
- redirects are followed by hand and every hop is checked again
- hard limits on size (also after decompression) and time
- no cookies, no proxy from the environment"""
import asyncio
import ipaddress
import re
import socket
import zlib
from urllib.parse import urljoin, urlsplit

import aiohttp
from aiohttp import abc as aiohttp_abc

USER_AGENT = 'bgdeobf-bot/1.0 (script fetch)'
URL_RE = re.compile(r'https?://[^\s<>"\'`)\]]+', re.I)


class FetchError(Exception):
    """code: bad_url | blocked | http | html | too_big | empty | timeout | net"""

    def __init__(self, code, message):
        Exception.__init__(self, message)
        self.code = code
        self.message = message


class BlockedAddress(OSError):
    pass


_BLOCKED4 = [ipaddress.ip_network(n) for n in (
    '0.0.0.0/8', '10.0.0.0/8', '100.64.0.0/10', '127.0.0.0/8', '169.254.0.0/16', '172.16.0.0/12', '192.0.0.0/24',
    '192.0.2.0/24', '192.88.99.0/24', '192.168.0.0/16', '198.18.0.0/15', '198.51.100.0/24', '203.0.113.0/24',
    '224.0.0.0/4', '240.0.0.0/4')]
_BLOCKED6 = [ipaddress.ip_network(n) for n in (
    '::/128', '::1/128', '::ffff:0:0/96', '64:ff9b::/96', '64:ff9b:1::/48', '100::/64', '2001::/23', '2001:db8::/32',
    '2002::/16', 'fc00::/7', 'fe80::/10', 'fec0::/10', 'ff00::/8')]


def is_public(ip):
    """True only for addresses that belong to the public internet"""
    if ip.version == 6 and ip.ipv4_mapped is not None:
        return is_public(ip.ipv4_mapped)
    for net in (_BLOCKED4 if ip.version == 4 else _BLOCKED6):
        if ip in net:
            return False
    return bool(ip.is_global)


class SafeResolver(aiohttp_abc.AbstractResolver):
    """resolves names and refuses (the whole name) when any answer is not a public address"""

    async def resolve(self, host, port=0, family=socket.AF_INET):
        loop = asyncio.get_running_loop()
        try:
            infos = await loop.getaddrinfo(host, port, family=socket.AF_UNSPEC, type=socket.SOCK_STREAM)
        except socket.gaierror:
            raise OSError('cannot resolve %s' % host)
        out = []
        for fam, _t, proto, _c, sa in infos:
            ip = ipaddress.ip_address(sa[0].split('%')[0])
            if not is_public(ip):
                raise BlockedAddress('%s resolves to a non-public address' % host)
            out.append({'hostname': host, 'host': sa[0], 'port': port, 'family': fam, 'proto': proto,
                        'flags': socket.AI_NUMERICHOST})
        if not out:
            raise OSError('no address for %s' % host)
        return out

    async def close(self):
        return None


def extract_url(text):
    """first http(s) URL in a chat message; also finds it inside loadstring(game:HttpGet("..."))()"""
    m = URL_RE.search(text or '')
    if not m:
        return None
    return m.group(0).rstrip('.,;:!?')


def normalize_url(url):
    """turn the page address of a few common hosts into the raw address"""
    sp = urlsplit(url.strip())
    host = (sp.hostname or '').lower()
    parts = [p for p in sp.path.split('/') if p]
    if host in ('github.com', 'www.github.com') and len(parts) >= 5 and parts[2] == 'blob':
        user, repo, _blob, ref = parts[:4]
        return 'https://raw.githubusercontent.com/%s/%s/%s/%s' % (user, repo, ref, '/'.join(parts[4:]))
    if host in ('pastebin.com', 'www.pastebin.com') and len(parts) == 1 and re.fullmatch(r'[A-Za-z0-9]{6,12}', parts[0]):
        return 'https://pastebin.com/raw/' + parts[0]
    if host == 'gist.github.com' and len(parts) == 2 and re.fullmatch(r'[0-9a-fA-F]{8,40}', parts[1]):
        return 'https://gist.githubusercontent.com/%s/%s/raw' % (parts[0], parts[1])
    return url.strip()


def url_stem(url):
    """a file name stem for the result: last path segment of the URL"""
    seg = urlsplit(url).path.rstrip('/').rsplit('/', 1)[-1]
    seg = re.sub(r'\.(lua|luau|txt)$', '', seg, flags=re.I)
    return seg or 'script'


def check_url(url, hosts=None, ports=(80, 443)):
    """validate one URL (initial or redirect target); returns the SplitResult"""
    try:
        sp = urlsplit(url.strip())
        port = sp.port
    except ValueError:
        raise FetchError('bad_url', 'invalid URL')
    if sp.scheme.lower() not in ('http', 'https'):
        raise FetchError('bad_url', 'only http:// or https:// is allowed')
    if sp.username or sp.password or '@' in sp.netloc:
        raise FetchError('bad_url', 'the URL must not contain user:password')
    host = (sp.hostname or '').rstrip('.').lower()
    if not host or len(host) > 253:
        raise FetchError('bad_url', 'invalid URL')
    if port is not None and port not in ports:
        raise FetchError('bad_url', 'only port 80 or 443 is allowed')
    try:
        ip = ipaddress.ip_address(host)
    except ValueError:
        ip = None
    if ip is not None and not is_public(ip):
        raise FetchError('blocked', 'that address is not a public address')
    if hosts and not any(host == h or host.endswith('.' + h) for h in hosts):
        raise FetchError('blocked', 'that host is not on the allowed list')
    return sp


def _gunzip(data, max_bytes, encoding):
    wbits = 16 + zlib.MAX_WBITS if encoding == 'gzip' else zlib.MAX_WBITS
    d = zlib.decompressobj(wbits)
    try:
        out = d.decompress(data, max_bytes + 1)
    except zlib.error:
        raise FetchError('http', 'the compressed content is corrupt')
    if len(out) > max_bytes:
        raise FetchError('too_big', 'the decompressed content exceeds the limit')
    return out


async def _fetch(url, max_bytes, hosts, timeout, max_redirects, resolver, user_agent, ports):
    conn = aiohttp.TCPConnector(resolver=resolver or SafeResolver(), use_dns_cache=False, force_close=True, limit=1)
    cfg = aiohttp.ClientTimeout(total=timeout, connect=10, sock_read=15)
    headers = {'User-Agent': user_agent, 'Accept': 'text/plain, */*;q=0.5', 'Accept-Encoding': 'gzip, identity'}
    async with aiohttp.ClientSession(connector=conn, timeout=cfg, headers=headers, cookie_jar=aiohttp.DummyCookieJar(),
                                     trust_env=False, auto_decompress=False) as session:
        cur = url
        for _hop in range(max_redirects + 1):
            check_url(cur, hosts, ports)
            try:
                async with session.get(cur, allow_redirects=False) as resp:
                    if resp.status in (301, 302, 303, 307, 308):
                        loc = resp.headers.get('Location')
                        if not loc:
                            raise FetchError('http', 'redirect without a destination')
                        cur = urljoin(cur, loc)
                        continue
                    if resp.status != 200:
                        raise FetchError('http', 'the server answered HTTP %d' % resp.status)
                    if resp.content_length is not None and resp.content_length > max_bytes:
                        raise FetchError('too_big', 'the file exceeds the limit')
                    buf = bytearray()
                    async for chunk in resp.content.iter_chunked(65536):
                        buf.extend(chunk)
                        if len(buf) > max_bytes:
                            raise FetchError('too_big', 'the file exceeds the limit')
                    enc = (resp.headers.get('Content-Encoding') or 'identity').strip().lower()
                    ctype = (resp.headers.get('Content-Type') or '').lower()
            except FetchError:
                raise
            except asyncio.TimeoutError:
                raise FetchError('timeout', 'the server took too long to respond')
            except aiohttp.ClientConnectorError as e:
                if isinstance(getattr(e, 'os_error', None), BlockedAddress):
                    raise FetchError('blocked', 'that address is not a public address')
                raise FetchError('net', 'could not connect to that server')
            except aiohttp.InvalidURL:
                raise FetchError('bad_url', 'invalid URL')
            except (aiohttp.ClientError, OSError):
                raise FetchError('net', 'the connection to the server was lost')
            data = bytes(buf)
            if enc in ('gzip', 'x-gzip', 'deflate'):
                data = _gunzip(data, max_bytes, 'gzip' if 'gzip' in enc else 'deflate')
            elif enc != 'identity':
                raise FetchError('http', 'the server used an unsupported compression (%s)' % enc[:20])
            if not data.strip():
                raise FetchError('empty', 'the content is empty')
            head = data[:300].lstrip()
            if head[:1] == b'<' and ('html' in ctype or head[:15].lower().startswith((b'<!doctype html', b'<html'))):
                raise FetchError('html', 'that is a web page, not a raw file')
            return data
    raise FetchError('http', 'too many redirects')


async def fetch(url, max_bytes, hosts=None, timeout=25, max_redirects=3, resolver=None, user_agent=USER_AGENT,
                ports=(80, 443)):
    """download `url` (already extracted from the message); returns bytes or raises FetchError"""
    url = normalize_url(url)
    try:
        return await asyncio.wait_for(
            _fetch(url, max_bytes, hosts, timeout, max_redirects, resolver, user_agent, ports), timeout + 5)
    except asyncio.TimeoutError:
        raise FetchError('timeout', 'the server took too long to respond')

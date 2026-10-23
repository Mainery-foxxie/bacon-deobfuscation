"""URL fetcher tests: a local web server, a resolver that maps made-up names to it, and the real (strict) checks."""
import asyncio
import gzip
import ipaddress
import socket

import pytest
from aiohttp import abc as aiohttp_abc
from aiohttp import web

from bgdeobf import fetch as F

SCRIPT = b'print("hello from raw")\n'


@pytest.mark.parametrize('addr,public', [
    ('8.8.8.8', True), ('1.1.1.1', True), ('93.184.216.34', True), ('2606:4700:4700::1111', True),
    ('127.0.0.1', False), ('127.1.2.3', False), ('10.0.0.5', False), ('172.16.5.5', False), ('172.31.255.255', False),
    ('192.168.1.1', False), ('169.254.169.254', False), ('100.64.0.1', False), ('0.0.0.0', False),
    ('224.0.0.1', False), ('255.255.255.255', False), ('198.18.0.1', False), ('192.0.2.1', False),
    ('::1', False), ('::', False), ('fe80::1', False), ('fc00::1', False), ('fd12:3456::1', False),
    ('::ffff:127.0.0.1', False), ('::ffff:10.1.1.1', False), ('::ffff:8.8.8.8', True),
    ('64:ff9b::7f00:1', False), ('2002:7f00:1::', False), ('2001:db8::1', False), ('ff02::1', False),
])
def test_is_public(addr, public):
    assert F.is_public(ipaddress.ip_address(addr)) is public


def test_extract_and_normalize():
    assert F.extract_url('!uwu https://a.b/c.lua') == 'https://a.b/c.lua'
    assert F.extract_url('loadstring(game:HttpGet("https://raw.githubusercontent.com/u/r/main/x.lua"))()') == \
        'https://raw.githubusercontent.com/u/r/main/x.lua'
    assert F.extract_url("loadstring(game:HttpGet('https://x.io/y.lua', true))()") == 'https://x.io/y.lua'
    assert F.extract_url('see <https://x.io/y.lua>.') == 'https://x.io/y.lua'
    assert F.extract_url('```lua\nlocal u = "http://h/p.lua"\n```') == 'http://h/p.lua'
    assert F.extract_url('no url here') is None and F.extract_url(None) is None
    assert F.normalize_url('https://github.com/u/r/blob/main/dir/x.lua') == 'https://raw.githubusercontent.com/u/r/main/dir/x.lua'
    assert F.normalize_url('https://pastebin.com/AbCd1234') == 'https://pastebin.com/raw/AbCd1234'
    assert F.normalize_url('https://pastebin.com/raw/AbCd1234') == 'https://pastebin.com/raw/AbCd1234'
    assert F.normalize_url('https://gist.github.com/user/0123456789abcdef0123456789abcdef') == \
        'https://gist.githubusercontent.com/user/0123456789abcdef0123456789abcdef/raw'
    assert F.normalize_url('https://example.com/a.lua') == 'https://example.com/a.lua'
    assert F.url_stem('https://h/p/My%20Script.lua?x=1') == 'My%20Script'
    assert F.url_stem('https://h/') == 'script'


@pytest.mark.parametrize('url,code', [
    ('ftp://example.com/x', 'bad_url'), ('file:///etc/passwd', 'bad_url'), ('javascript:alert(1)', 'bad_url'),
    ('https://user:pw@example.com/x', 'bad_url'), ('https://example.com:8443/x', 'bad_url'),
    ('https://example.com:22/x', 'bad_url'), ('http://', 'bad_url'), ('http://[::1', 'bad_url'),
    ('http://127.0.0.1/', 'blocked'), ('http://10.1.2.3/x', 'blocked'), ('http://[::1]/', 'blocked'),
    ('http://169.254.169.254/latest/meta-data/', 'blocked'), ('http://[::ffff:7f00:1]/', 'blocked'),
    ('http://[fe80::1]/', 'blocked'), ('http://0.0.0.0/', 'blocked'),
])
def test_check_url_rejects(url, code):
    with pytest.raises(F.FetchError) as e:
        F.check_url(url)
    assert e.value.code == code


def test_check_url_accepts_and_host_allow_list():
    assert F.check_url('https://raw.githubusercontent.com/u/r/main/x.lua').hostname == 'raw.githubusercontent.com'
    assert F.check_url('http://example.com:80/x')
    F.check_url('https://raw.githubusercontent.com/x', hosts={'githubusercontent.com'})
    with pytest.raises(F.FetchError) as e:
        F.check_url('https://evil.example/x', hosts={'githubusercontent.com'})
    assert e.value.code == 'blocked'
    with pytest.raises(F.FetchError):
        F.check_url('https://notgithubusercontent.com/x', hosts={'githubusercontent.com'})


@pytest.mark.parametrize('url', ['http://localhost/x', 'http://2130706433/', 'http://127.1/', 'http://0x7f.0.0.1/',
                                 'http://localhost.localdomain/'])
def test_real_resolver_blocks_loopback_in_any_spelling(url):
    async def go():
        with pytest.raises(F.FetchError) as e:
            await F.fetch(url, 1000, timeout=5)
        return e.value.code
    assert asyncio.run(go()) in ('blocked', 'net', 'bad_url')


def test_safe_resolver_refuses_mixed_answers_and_accepts_public_ones(monkeypatch):
    answers = {}

    def fake_getaddrinfo(host, port, family=0, type=0, proto=0, flags=0):
        out = []
        for a in answers[host]:
            fam = socket.AF_INET6 if ':' in a else socket.AF_INET
            out.append((fam, socket.SOCK_STREAM, 6, '', (a, port) if fam == socket.AF_INET else (a, port, 0, 0)))
        return out

    monkeypatch.setattr(socket, 'getaddrinfo', fake_getaddrinfo)
    answers['good.example'] = ['93.184.216.34', '2606:4700:4700::1111']
    answers['rebind.example'] = ['93.184.216.34', '127.0.0.1']
    answers['meta.example'] = ['169.254.169.254']
    answers['v6local.example'] = ['::1']
    answers['mapped.example'] = ['::ffff:10.0.0.1']

    async def go():
        r = F.SafeResolver()
        good = await r.resolve('good.example', 443)
        assert [g['host'] for g in good] == ['93.184.216.34', '2606:4700:4700::1111']
        assert all(g['port'] == 443 and g['hostname'] == 'good.example' for g in good)
        for name in ('rebind.example', 'meta.example', 'v6local.example', 'mapped.example'):
            with pytest.raises(F.BlockedAddress):
                await r.resolve(name, 80)
    asyncio.run(go())


class FakeResolver(aiohttp_abc.AbstractResolver):
    """public.test -> the local server; private.test -> refused like the real resolver would"""

    async def resolve(self, host, port=0, family=socket.AF_INET):
        if host == 'public.test':
            return [{'hostname': host, 'host': '127.0.0.1', 'port': port, 'family': socket.AF_INET, 'proto': 0,
                     'flags': socket.AI_NUMERICHOST}]
        raise F.BlockedAddress('refused')

    async def close(self):
        pass


def make_app(port_holder):
    async def ok(request):
        return web.Response(body=SCRIPT, content_type='text/plain')

    async def html(request):
        return web.Response(text='<!DOCTYPE html><html><body>nope</body></html>', content_type='text/html')

    async def big(request):
        return web.Response(body=b'x' * 2_000_000, content_type='text/plain')

    async def stream_big(request):
        resp = web.StreamResponse()
        resp.content_type = 'text/plain'
        await resp.prepare(request)
        for _ in range(100):
            await resp.write(b'y' * 20000)
        return resp

    async def redir(request):
        raise web.HTTPFound('/ok.lua')

    async def loop_(request):
        raise web.HTTPFound('/loop')

    async def to_private(request):
        raise web.HTTPFound('http://private.test:%d/ok.lua' % port_holder[0])

    async def to_literal(request):
        raise web.HTTPFound('http://127.0.0.1:%d/ok.lua' % port_holder[0])

    async def to_file(request):
        raise web.HTTPFound('file:///etc/passwd')

    async def gz(request):
        return web.Response(body=gzip.compress(SCRIPT), headers={'Content-Encoding': 'gzip', 'Content-Type': 'text/plain'})

    async def gzbomb(request):
        return web.Response(body=gzip.compress(b'\0' * 20_000_000), headers={'Content-Encoding': 'gzip'})

    async def br(request):
        return web.Response(body=b'zzz', headers={'Content-Encoding': 'br'})

    async def empty(request):
        return web.Response(body=b'   \n', content_type='text/plain')

    async def slow(request):
        await asyncio.sleep(10)
        return web.Response(body=SCRIPT)

    async def mislabelled(request):
        return web.Response(body=SCRIPT, content_type='text/html')

    app = web.Application()
    for path, fn in (('/ok.lua', ok), ('/html', html), ('/big', big), ('/stream', stream_big), ('/redir', redir),
                     ('/loop', loop_), ('/to-private', to_private), ('/to-literal', to_literal), ('/to-file', to_file),
                     ('/gz', gz), ('/gzbomb', gzbomb), ('/br', br), ('/empty', empty), ('/slow', slow),
                     ('/mislabelled', mislabelled)):
        app.router.add_get(path, fn)
    return app


def run_against_server(cases):
    """cases: list of (path, kwargs); returns list of ('ok', bytes) or ('err', code)"""
    async def go():
        holder = [0]
        runner = web.AppRunner(make_app(holder))
        await runner.setup()
        site = web.TCPSite(runner, '127.0.0.1', 0)
        await site.start()
        holder[0] = site._server.sockets[0].getsockname()[1]
        out = []
        try:
            for path, kw in cases:
                kw = dict(kw)
                kw.setdefault('max_bytes', 100000)
                try:
                    data = await F.fetch('http://public.test:%d%s' % (holder[0], path), resolver=FakeResolver(),
                                         ports=(holder[0],), **kw)
                    out.append(('ok', data))
                except F.FetchError as e:
                    out.append(('err', e.code))
        finally:
            await runner.cleanup()
        return out
    return asyncio.run(go())


def test_downloads_and_follows_redirects():
    r = run_against_server([('/ok.lua', {}), ('/redir', {}), ('/gz', {}), ('/mislabelled', {})])
    assert r == [('ok', SCRIPT)] * 4


def test_rejections_against_local_server():
    cases = [('/html', {}), ('/big', {}), ('/stream', {'max_bytes': 50000}), ('/loop', {}), ('/to-private', {}),
             ('/to-literal', {}), ('/to-file', {}), ('/gzbomb', {'max_bytes': 1_000_000}), ('/br', {}), ('/empty', {}),
             ('/missing', {}), ('/slow', {'timeout': 1})]
    codes = [c for _, c in run_against_server(cases)]
    assert codes == ['html', 'too_big', 'too_big', 'http', 'blocked', 'blocked', 'bad_url', 'too_big', 'http', 'empty',
                     'http', 'timeout']


def test_host_allow_list_applies_to_redirects():
    r = run_against_server([('/redir', {'hosts': {'other.example'}})])
    assert r == [('err', 'blocked')]

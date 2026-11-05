"""fetch(): a paste host that answers with its Roblox-only loader gets one more request, for the script link written in
the loader. `_fetch` (the part that talks to the network) is replaced by a table of answers, so nothing leaves the
machine; the real checks of `_fetch` (public addresses only, size, time) are covered in test_fetch.py."""
import asyncio

import pytest

pytest.importorskip('aiohttp')

from bgdeobf import fetch as F                                         # noqa: E402
from bgdeobf.messages import explain_fetch                              # noqa: E402
from envelope_samples import LINK, OTHER_LINK, PAGE, REAL_SCRIPT, loader  # noqa: E402


class Net:
    """stands in for `_fetch`: answers by URL (bytes, or an exception to raise) and remembers every request"""

    def __init__(self, answers):
        self.answers = answers
        self.calls = []

    async def __call__(self, url, max_bytes, hosts, timeout, max_redirects, resolver, user_agent, ports):
        self.calls.append(dict(url=url, max_bytes=max_bytes, hosts=hosts, timeout=timeout, max_redirects=max_redirects,
                               resolver=resolver, user_agent=user_agent, ports=ports))
        a = self.answers[url]
        if isinstance(a, Exception):
            raise a
        return a

    @property
    def urls(self):
        return [c['url'] for c in self.calls]


def run(monkeypatch, answers, url=PAGE, **kw):
    net = Net(answers)
    monkeypatch.setattr(F, '_fetch', net)
    kw.setdefault('max_bytes', 100000)
    try:
        return asyncio.run(F.fetch(url, **kw)), net
    except F.FetchError as e:
        e.net = net
        raise


def test_follows_the_loader_to_the_real_script(monkeypatch):
    data, net = run(monkeypatch, {PAGE: loader().encode(), LINK: REAL_SCRIPT})
    assert data == REAL_SCRIPT
    assert net.urls == [PAGE, LINK]


def test_the_second_request_is_made_exactly_like_the_first(monkeypatch):
    resolver = object()
    _data, net = run(monkeypatch, {PAGE: loader().encode(), LINK: REAL_SCRIPT}, max_bytes=1234,
                     hosts={'paste.example'}, ports=(443,), user_agent='me/1', max_redirects=2, resolver=resolver)
    first, second = net.calls
    for key in ('max_bytes', 'hosts', 'ports', 'user_agent', 'max_redirects', 'resolver'):
        assert first[key] == second[key], key
    assert second['user_agent'] == 'me/1' and second['resolver'] is resolver


def test_the_second_request_does_not_get_more_time(monkeypatch):
    _data, net = run(monkeypatch, {PAGE: loader().encode(), LINK: REAL_SCRIPT}, timeout=7)
    assert net.calls[0]['timeout'] == 7
    assert 0 < net.calls[1]['timeout'] <= 7


def test_an_ordinary_script_costs_one_request(monkeypatch):
    data, net = run(monkeypatch, {PAGE: REAL_SCRIPT})
    assert data == REAL_SCRIPT and net.urls == [PAGE]


def test_a_loader_that_points_to_another_site_is_returned_as_it_is(monkeypatch):
    text = loader('https://evil.example/api/stream/x?k=1').encode()
    data, net = run(monkeypatch, {PAGE: text})
    assert data == text and net.urls == [PAGE]


def test_a_failure_of_the_first_request_keeps_its_own_code(monkeypatch):
    with pytest.raises(F.FetchError) as e:
        run(monkeypatch, {PAGE: F.FetchError('http', 'the server answered HTTP 404')})
    assert e.value.code == 'http' and e.value.net.urls == [PAGE]


@pytest.mark.parametrize('code,detail', [
    ('http', 'the server answered HTTP 403'), ('html', 'that is a web page, not a raw file'),
    ('timeout', 'the server took too long to respond'), ('net', 'could not connect to that server'),
    ('blocked', 'that address is not a public address'), ('bad_url', 'invalid URL'),
    ('empty', 'the content is empty')])
def test_a_refused_script_link_says_so(monkeypatch, code, detail):
    with pytest.raises(F.FetchError) as e:
        run(monkeypatch, {PAGE: loader().encode(), LINK: F.FetchError(code, detail)})
    assert e.value.code == 'loader'
    assert detail in e.value.message
    assert e.value.net.urls == [PAGE, LINK]


def test_a_real_script_that_is_too_big_keeps_its_code(monkeypatch):
    with pytest.raises(F.FetchError) as e:
        run(monkeypatch, {PAGE: loader().encode(), LINK: F.FetchError('too_big', 'the file exceeds the limit')})
    assert e.value.code == 'too_big'


@pytest.mark.parametrize('second', [LINK, OTHER_LINK])
def test_a_second_loader_is_not_followed(monkeypatch, second):
    answers = {PAGE: loader().encode(), LINK: loader(second).encode(), OTHER_LINK: REAL_SCRIPT}
    with pytest.raises(F.FetchError) as e:
        run(monkeypatch, answers)
    assert e.value.code == 'loader'
    assert e.value.net.urls == [PAGE, LINK]


def test_the_error_text_for_the_user():
    msg = explain_fetch('loader', 'the server answered HTTP 403')
    assert 'Roblox' in msg and 'HTTP 403' in msg and 'attach' in msg.lower()
    assert msg == msg.strip() and '..' not in msg
    assert explain_fetch('loader')           # also without a detail
    assert 'but x. That' in explain_fetch('loader', 'x.')          # one full stop, not two

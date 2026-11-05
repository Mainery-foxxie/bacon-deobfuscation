"""The paste host's loader ("Secure Runtime Envelope"): reading the one link it fetches its script from.
Pure text in, a link or None out: nothing here touches the network."""
import pytest

from bgdeobf import paste_envelope as P
from envelope_samples import LINK, PAGE, loader


def test_reads_the_script_link_from_the_loader():
    assert P.stream_url(loader().encode(), PAGE) == LINK


def test_str_crlf_and_a_byte_order_mark_are_fine():
    assert P.stream_url(loader(), PAGE) == LINK
    assert P.stream_url(loader().replace('\n', '\r\n').encode(), PAGE) == LINK
    assert P.stream_url(b'\xef\xbb\xbf' + loader().encode(), PAGE) == LINK


def test_the_link_is_returned_exactly_as_written():
    link = 'https://paste.example/api/stream/AbC123xyz?k=a.b-c_d==&x=1'
    assert P.stream_url(loader(link).encode(), PAGE) == link


def test_default_port_host_case_and_a_trailing_dot_do_not_matter():
    assert P.stream_url(loader().encode(), 'https://PASTE.example:443/raw/x') == LINK
    assert P.stream_url(loader().encode(), 'https://paste.example./raw/x') == LINK
    assert P.stream_url(loader('http://paste.example:8080/api/stream/x?k=1').encode(),
                        'http://paste.example:8080/raw/x') == 'http://paste.example:8080/api/stream/x?k=1'


@pytest.mark.parametrize('link', [
    'https://evil.example/api/stream/AbC123xyz?k=1',               # another site
    'https://paste.example.evil.example/api/stream/x?k=1',         # starts like the site, is another one
    'https://sub.paste.example/api/stream/x?k=1',                  # a sub-domain is another site
    'http://paste.example/api/stream/x?k=1',                       # another scheme
    'https://paste.example:8443/api/stream/x?k=1',                 # another port
    'https://user:pw@paste.example/api/stream/x?k=1',              # user:password in the link
    'https://127.0.0.1/api/stream/x?k=1',
    'https://paste.example/raw/x',                                 # not the stream path
    'https://paste.example/api/stream',
    'https://paste.example/api/stream/',
    'https://paste.example/api/streams/x',
    'https://paste.example/api/stream/../../raw/x',
    'https://paste.example/api/stream/x#frag',
    'https://paste.example/api/stream/x\\q',                       # a Lua escape: not read as text
    'https://paste.example/api/stream/x?k=a\\b',
    'https://paste.example/api/stream/x y',                        # a space, a tab: not a plain link
    'https://paste.example/api/stream/x?k=a\tb',
    'file:///etc/passwd',
    '//paste.example/api/stream/x',
])
def test_a_link_that_is_not_the_hosts_own_stream_link_is_not_followed(link):
    assert P.stream_url(loader(link).encode(), PAGE) is None


def test_ordinary_scripts_are_not_loaders():
    assert P.stream_url(b'print("hi")\n', PAGE) is None
    assert P.stream_url(b'', PAGE) is None
    assert P.stream_url(b'  \n', PAGE) is None
    assert P.stream_url(None, PAGE) is None


def test_a_big_file_is_never_a_loader():
    big = loader().encode() + b'-- padding\n' * 1000
    assert len(big) > P.MAX_LOADER
    assert P.stream_url(big, PAGE) is None


def test_bytecode_and_binary_noise_do_not_crash():
    assert P.stream_url(b'\x1bLuaQ\x00\x01\x02' + bytes(range(256)), PAGE) is None
    assert P.stream_url(b'\xff\xfe\x00' * 50, PAGE) is None


def test_the_marker_and_the_script_request_are_both_needed():
    assert P.stream_url(loader().replace('Secure Runtime Envelope', 'Something Else').encode(), PAGE) is None
    assert P.stream_url(loader().replace('game:HttpGet(url)', 'nil').encode(), PAGE) is None


def test_two_candidate_links_are_ambiguous():
    twice = loader().replace('    local ok, src', '    local url = "https://paste.example/api/stream/other?k=2"\n    local ok, src')
    assert P.stream_url(twice.encode(), PAGE) is None


def test_a_page_url_that_cannot_be_read_gives_none():
    assert P.stream_url(loader().encode(), 'http://[::1') is None
    assert P.stream_url(loader().encode(), '') is None
    assert P.stream_url(loader().encode(), None) is None

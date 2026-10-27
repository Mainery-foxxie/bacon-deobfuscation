"""Engine, sandbox and runner tests.  Tests that need a protected script use the files in tests/samples/ (or
$BGDEOBF_SAMPLES); they are skipped when there are none."""
import os
import subprocess
import time

import pytest

from conftest import sample_files
from bgdeobf import messages
from bgdeobf.astutil import check_tools
from bgdeobf.engine import EngineError, Options, detect
from bgdeobf.namesfile import NamesError, parse_names
from bgdeobf.runner import run
from bgdeobf.sandbox import SandboxError, run_luau

SAMPLES = sample_files()
needs_sample = pytest.mark.skipif(not SAMPLES, reason='no protected scripts in tests/samples/')


def smallest():
    return min(SAMPLES, key=os.path.getsize)


def code_of(data, **kw):
    with pytest.raises(EngineError) as ei:
        run(data, Options(**kw), hard_timeout=120)
    return ei.value.code


def test_luau_tools_work():
    res = check_tools()
    assert [n for n, ok, m in res if not ok] == [], res


@pytest.mark.parametrize('data,code', [
    (b'', 'empty'),
    (b'  \n\t ', 'empty'),
    (b'\x1bLua\x00\x01\x02\x03' * 50, 'binary'),
    (b'print("hello world")\nlocal x = 1 + 2\n', 'unsupported'),
    (b'-- Bacon Guard v0.42.0 | FORGE Protection | Build 0a89d3458f6c\nprint(1)\n', 'unsupported'),
    (b'local x = = 3 end end', 'syntax'),
    (b'<html><body>404 Not Found</body></html>' * 5, 'syntax'),
])
def test_rejections(data, code):
    assert code_of(data) == code


def test_size_limit():
    assert code_of(b'a' * 2000, max_bytes=1000) == 'too_big'


@needs_sample
def test_truncated_script_is_a_syntax_error():
    data = open(smallest(), 'rb').read()
    assert code_of(data[:len(data) // 2]) == 'syntax'


@needs_sample
@pytest.mark.parametrize('seed', [1, 2, 3])
def test_damaged_script_never_crashes_or_hangs(seed):
    """flip characters inside the protected data: the answer is a result or an explicit, coded error - never a crash"""
    import random
    rnd = random.Random(seed)
    data = bytearray(open(smallest(), 'rb').read())
    for _ in range(60):
        i = rnd.randrange(len(data) // 3, len(data) - 10)
        if chr(data[i]).isalnum():
            data[i] = ord(rnd.choice('abcXYZ019'))
    try:
        res = run(bytes(data), Options(), hard_timeout=200)
    except EngineError as e:
        assert e.code in ('sandbox', 'decode', 'unsupported', 'decompile', 'syntax', 'timeout', 'memory')
    else:
        assert res.text.strip()


def test_sandbox_limits():
    with pytest.raises(SandboxError) as e:
        run_luau('while true do end', timeout=2)
    assert e.value.code == 'timeout'
    with pytest.raises(SandboxError) as e:
        run_luau("local t={} for i=1,1e9 do t[i]=string.rep('x',1000000)..i end", timeout=30, mem_mb=200)
    assert e.value.code == 'sandbox'
    with pytest.raises(SandboxError) as e:
        run_luau("for i=1,1e7 do print(string.rep('y',100)) end", timeout=30, max_output_mb=5)
    assert 'output limit' in e.value.message
    with pytest.raises(SandboxError) as e:
        run_luau("error('boom')")
    assert 'boom' in e.value.message
    assert run_luau('print("ok")').strip() == b'ok'


def test_sandbox_has_no_io_or_os_escape():
    out = run_luau('print(type(io), type(os), type(os and os.execute), type(require), type(loadstring))')
    assert out.split()[:3] == [b'nil', b'table', b'nil']


@needs_sample
@pytest.mark.parametrize('path', SAMPLES, ids=[os.path.basename(p) for p in SAMPLES])
def test_sample_roundtrip(path):
    data = open(path, 'rb').read()
    res = run(data, Options(), hard_timeout=240)
    info = res.info
    assert res.text.strip()
    assert info['syntax_ok'] is True
    assert info['fidelity']['differing'] == []
    assert info['fidelity']['identical'] + info['fidelity']['reordered'] + info['fidelity']['cosmetic'] == info['fidelity']['functions']
    assert info['protos'] == info['fidelity']['functions']


@needs_sample
def test_output_is_deterministic():
    data = open(smallest(), 'rb').read()
    a = run(data, Options(verify=False)).text
    b = run(data, Options(verify=False)).text
    assert a == b


@needs_sample
def test_hard_timeout_and_cancel_leave_nothing_behind():
    data = open(max(SAMPLES, key=os.path.getsize), 'rb').read()
    t = time.time()
    with pytest.raises(EngineError) as e:
        run(data, Options(), hard_timeout=1)
    assert e.value.code == 'timeout' and time.time() - t < 6
    n = [0]

    def cancel():
        n[0] += 1
        return n[0] > 4

    with pytest.raises(EngineError) as e:
        run(data, Options(), hard_timeout=100, cancel=cancel)
    assert e.value.code == 'cancelled'
    time.sleep(0.5)
    left = subprocess.run(['pgrep', '-fl', r'bgdeobf/worker\.py'], capture_output=True, text=True).stdout
    assert left.strip() == ''


@needs_sample
def test_killed_jobs_leave_no_temp_files():
    import glob
    import tempfile
    data = open(max(SAMPLES, key=os.path.getsize), 'rb').read()
    pattern = os.path.join(tempfile.gettempdir(), 'bgdeobf_*')
    before = set(glob.glob(pattern))
    for limit in (0.3, 0.5, 0.7, 0.9, 1.2, 2.0, 4.0):
        with pytest.raises(EngineError):
            run(data, Options(), hard_timeout=limit)
    time.sleep(0.5)
    assert set(glob.glob(pattern)) - before == set()


@needs_sample
def test_names_file_changes_output():
    data = open(smallest(), 'rb').read()
    annotated = run(data, Options(verify=False, annotate=True)).text
    import re
    keys = re.findall(r'<@(\d+:[p0-9]+)>', annotated)
    assert keys
    key = keys[len(keys) // 2]
    renamed = run(data, Options(verify=False, names={key: 'myCustomName'})).text
    assert 'myCustomName' in renamed


def test_names_file_parser():
    assert parse_names(b'{"12:34": "a", "3:p1": "target"}') == {'12:34': 'a', '3:p1': 'target'}
    for bad in (b'[]', b'nope', b'{"x": "y"}', b'{"1:2": "1bad"}', b'{"1:2": 5}', b'\xff\xfe'):
        with pytest.raises(NamesError):
            parse_names(bad)


def test_messages():
    assert "won't guess" in messages.explain('unsupported')
    assert 'Technical detail: x' in messages.explain('sandbox', 'x')
    assert 'Traceback' not in messages.explain('internal', 'Traceback')
    v = messages.verdict({'banner': {'version': '1', 'protection': 'P', 'build': 'b'}, 'protos': 2, 'records': 3, 'lines': 4,
                          'fidelity': {'functions': 2, 'identical': 1, 'reordered': 0, 'cosmetic': 1, 'differing': []},
                          'warnings': ['w']})
    assert any('1 with a cosmetic difference' in x for x in v) and v[-1] == 'Note: w'


def test_verdict_does_not_count_a_function_with_moved_reads_as_identical():
    v = messages.verdict({'banner': None, 'protos': 70, 'records': 1, 'lines': 1, 'warnings': [],
                          'fidelity': {'functions': 70, 'identical': 69, 'reordered': 1, 'cosmetic': 0, 'differing': []}})
    line = [x for x in v if x.startswith('Bytecode check')][0]
    assert line.startswith('Bytecode check: 69/70 functions identical, 1 with the same instructions in a different order'), line
    assert 'differ' not in line.replace('different', '')


def test_verdict_explains_an_annotated_copy():
    base = {'banner': None, 'protos': 1, 'records': 1, 'lines': 1, 'warnings': [], 'syntax_ok': True}
    plain = messages.verdict(base)
    assert not any('Annotated' in x for x in plain)
    v = messages.verdict(dict(base, annotated=True))
    assert any('Annotated copy' in x and 'names file' in x and 'not runnable' in x and 'clean copy' in x for x in v)


@pytest.mark.parametrize('raw,paths,expected', [
    ('/tmp/tmpab12.lua(8,14): SyntaxError: boom', ['/tmp/tmpab12.lua'], 'line 8:14: SyntaxError: boom'),
    ('C:/Users/John Smith/AppData/Local/Temp/bgdeobf_job_x/tmpy3p62yo2.lua(8,14): SyntaxError: boom', [],
     'line 8:14: SyntaxError: boom'),
    ('C:\\Users\\Someone~1\\AppData\\Local\\Temp\\x\\dump.lua:12: attempt to index nil',
     ['C:\\Users\\Someone~1\\AppData\\Local\\Temp\\x\\dump.lua'], 'line 12: attempt to index nil'),
    ("[Errno 8] Exec format error: '/opt/app/luau_bin/luau'", [], "[Errno 8] Exec format error: '<path>'"),
    ('plain message with no paths (and/or a ratio 3/4)', [], 'plain message with no paths (and/or a ratio 3/4)'),
])
def test_scrub_removes_host_paths(raw, paths, expected):
    assert messages.scrub(raw, *paths) == expected


def test_scrub_is_applied_to_details_and_warnings():
    path = '/srv/bots/secret/tmp123.lua'
    assert 'secret' not in messages.explain('sandbox', 'unpacking failed: %s:3: boom' % path)
    v = messages.verdict({'banner': None, 'protos': 1, 'records': 1, 'lines': 1,
                          'warnings': ['the result has a syntax error: %s(8,14): SyntaxError: x' % path]})
    assert not any('secret' in x or '/srv' in x for x in v)
    assert v[-1] == 'Note: the result has a syntax error: line 8:14: SyntaxError: x'


def test_compile_errors_do_not_leak_the_temp_path():
    import tempfile
    from bgdeobf.engine import compile_check
    ok, msg, bc = compile_check('local x = = 1\n')
    assert ok is False and bc is None
    assert msg.startswith('line 1:11: SyntaxError') and '.lua' not in msg and tempfile.gettempdir() not in msg
    ok, msg, bc = compile_check('local x = = 1\n' * 8)
    assert ok is False and msg.count('SyntaxError') == 3 and msg.endswith('more)')
    ok, msg, bc = compile_check('print("fine")\n')
    assert ok is True and msg == '' and bc


@needs_sample
def test_annotated_output_is_checked_on_a_clean_copy():
    data = open(smallest(), 'rb').read()
    res = run(data, Options(annotate=True), hard_timeout=240)
    assert '<@' in res.text
    info = res.info
    assert info['annotated'] is True
    assert info['syntax_ok'] is True and info['warnings'] == []
    assert info['fidelity']['differing'] == []
    assert any(x.startswith('Annotated copy') for x in messages.verdict(info))
    clean = run(data, Options(), hard_timeout=240)
    assert 'annotated' not in clean.info
    import re
    assert re.sub(r'<@\d+:(?:p\d+|\d+)>', '', res.text) == clean.text


# ------------------------------------------------------------------------------------------------ detect (nothing is run)
BANNER_LINE = b'-- Bacon Guard v0.42.0 | FORGE Protection | Build 0a89d3458f6c\n'


@pytest.mark.parametrize('data,kind,confidence,needle', [
    (b'', 'unknown', 0.0, 'the file is empty'),
    (b'\x1bLua\x00\x01\x02\x03' * 50, 'unknown', 0.0, 'binary'),
    (b'print("hello world")\nlocal x = 1 + 2\n', 'unknown', 0.0, 'loader layout is not recognized'),
    (b'local x = = 3 end end', 'unknown', 0.0, 'not valid Luau'),
    (BANNER_LINE + b'print(1)\n', 'bacon_guard', 0.5, 'the banner is there, but the loader layout is not recognized'),
    (BANNER_LINE + b'local x = = 3 end end', 'bacon_guard', 0.5, 'not valid Luau'),
])
def test_detect_tells_what_a_script_is_without_running_it(data, kind, confidence, needle):
    d = detect(data)
    assert (d['kind'], d['confidence'], d['supported']) == (kind, confidence, False)
    assert needle in d['detail']
    assert '.lua' not in d['detail'] and 'bgdeobf' not in d['detail'] and '/tmp' not in d['detail']     # no host paths


def test_detect_reads_the_build_from_the_banner_and_refuses_a_file_that_is_too_big():
    d = detect(BANNER_LINE + b'print(1)\n')
    assert d['banner'] == dict(version='0.42.0', protection='FORGE Protection', build='0a89d3458f6c')
    assert d['label'] == 'Bacon Guard v0.42.0 (FORGE Protection), build 0a89d3458f6c'
    big = detect(b'a' * 2000, max_bytes=1000)
    assert (big['kind'], big['supported']) == ('unknown', False) and 'larger than 1000 bytes' in big['detail']


@needs_sample
@pytest.mark.parametrize('path', SAMPLES, ids=[os.path.basename(p) for p in SAMPLES])
def test_detect_recognizes_every_sample(path):
    data = open(path, 'rb').read()
    d = detect(data)
    assert (d['kind'], d['supported'], d['confidence']) == ('bacon_guard', True, 1.0), d
    if d['banner']:
        assert d['banner']['build'] in d['label']
    else:
        assert d['label'] == 'Bacon Guard-style VM (no banner)'


@needs_sample
def test_detect_does_not_accept_a_truncated_script():
    data = open(smallest(), 'rb').read()
    d = detect(data[:len(data) // 2])
    assert d['supported'] is False and d['confidence'] < 1.0 and 'not valid Luau' in d['detail']

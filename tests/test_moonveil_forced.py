"""A script handled as MoonVeil on request (`Options(moonveil=True)`, the `!moonveil` command), also when it has no banner.

The engine tests use the real Luau sandbox, like test_moonveil.py.  No Discord needed here (the chat side is in
test_moonveil_command.py)."""
import pytest

import bgdeobf.__main__ as M
from bgdeobf import moonveil, runner
from bgdeobf.engine import EngineError, Options, Result, deobfuscate
from bgdeobf.messages import verdict
from bgdeobf.service import Service
from test_bot import trace_dict
from test_moonveil import BANNER, INFO, SCRIPT

NO_BANNER = SCRIPT[len(BANNER):]
BACON = b'-- Bacon Guard v1.2 | Heavy | Build abc123\nprint(1)\n'


def test_a_script_without_a_banner_is_not_taken_for_moonveil_unless_asked():
    with pytest.raises(EngineError) as e:
        deobfuscate(NO_BANNER, Options(sandbox_timeout=30))
    assert e.value.code == 'unsupported'


def test_forced_moonveil_handles_a_script_without_a_banner_and_says_so():
    res = deobfuscate(NO_BANNER, Options(sandbox_timeout=30, moonveil=True))
    assert res.info['trace_only'] and res.info['moonveil'] == {'version': '?', 'assumed': True}
    head = res.text.split(']]')[0]
    assert head.startswith('--[[\n  BEHAVIOUR RECONSTRUCTION - THIS IS NOT THE ORIGINAL SOURCE.')
    assert 'no MoonVeil banner' in head and 'MoonVeil ?' not in head and 'protected with' not in head
    assert max(len(ln) for ln in head.splitlines()) <= 120
    assert 'example.com/lib.lua' in head                      # the usual summary is still there
    said = verdict(res.info)
    assert said[0].startswith('No MoonVeil banner found') and 'Detected: MoonVeil' not in '\n'.join(said)
    assert any('NOT the original source' in ln for ln in said)


def test_a_banner_still_decides_the_version_when_the_flag_is_set():
    res = deobfuscate(SCRIPT, Options(sandbox_timeout=30, moonveil=True))
    assert res.info['moonveil'] == {'version': '2.1.1'}
    assert 'protected with MoonVeil 2.1.1' in res.text.split(']]')[0]
    assert verdict(res.info)[0] == 'Detected: MoonVeil 2.1.1 (private VM protector)'


def test_a_moonveil_script_found_by_its_banner_has_no_trace_unless_asked():
    res = deobfuscate(SCRIPT, Options(sandbox_timeout=30))
    assert res.trace is None and 'trace' not in res.info


def test_forced_moonveil_never_runs_a_bacon_guard_script(monkeypatch):
    def boom(*a, **k):
        raise AssertionError('the script must not be run')
    monkeypatch.setattr(moonveil, 'run_script', boom)
    with pytest.raises(EngineError) as e:
        deobfuscate(BACON, Options(moonveil=True))
    assert e.value.code == 'unsupported' and 'Bacon Guard' in e.value.message


def test_trace_with_moonveil_adds_the_fake_roblox_report_of_the_script_as_given():
    res = deobfuscate(NO_BANNER, Options(sandbox_timeout=30, moonveil=True, trace=True))
    assert res.text.startswith('--[[')                       # the reconstruction is still the main file
    assert res.trace and res.trace['report'].startswith('FAKE ROBLOX SANDBOX TRACE')
    assert 'the script as given' in res.trace['report']
    assert res.info['trace']['requests'] == 1 and res.info['trace']['domains'] == ['example.com']
    text = '\n'.join(verdict(res.info, markdown=True))
    assert 'Fake Roblox run:' in text and '`example.com`' in text and 'Worth a look' in text


def test_the_service_passes_the_flag_on_but_a_sandbox_run_ignores_it():
    svc = Service(workers=1, max_queue=1, run=lambda *a, **k: None)
    try:
        assert svc._job_options(None, False).moonveil is False
        assert svc._job_options(None, False, moonveil=True).moonveil is True
        assert svc._job_options(None, False, trace=True, moonveil=True).trace is True
        s = svc._job_options(None, False, sandbox=True, moonveil=True)
        assert s.sandbox_only is True and s.moonveil is False
    finally:
        svc.close()


def test_the_flag_and_the_report_survive_the_worker_process():
    res = runner.run(NO_BANNER, Options(sandbox_timeout=30, moonveil=True, trace=True), hard_timeout=150)
    assert res.info['moonveil'] == {'version': '?', 'assumed': True} and res.text.startswith('--[[')
    assert res.trace and res.trace['report'].startswith('FAKE ROBLOX SANDBOX TRACE')
    assert res.trace['summary']['requests'] == 1


def test_cli_writes_the_reconstruction_and_the_report_side_by_side(monkeypatch, tmp_path, capsys):
    src = tmp_path / 'hub.lua'
    src.write_bytes(b'x')
    t = trace_dict(payloads=1)
    info = dict(INFO, trace=t['summary'])
    monkeypatch.setattr(M, 'run', lambda data, opts, hard_timeout=240, progress=None, cancel=None:
                        Result('print("t")\n', info, t))
    assert M.main([str(src), '--trace']) == 0
    assert (tmp_path / 'hub_reconstructed.lua').read_text() == 'print("t")\n'
    report = (tmp_path / 'hub_trace.txt').read_text()
    assert 'hub.lua' in report and '@@NAME@@' not in report and '@@STEM@@' not in report
    assert (tmp_path / 'hub_payload1.lua').exists()
    assert not (tmp_path / 'hub_deobf.lua').exists()
    assert 'Fake Roblox run' in capsys.readouterr().out

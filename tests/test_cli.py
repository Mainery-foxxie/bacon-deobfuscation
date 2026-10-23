"""Command line front end (the engine is stubbed, so no Luau is needed)."""
import bgdeobf.__main__ as M
from bgdeobf.engine import EngineError, Result

BASE = {'banner': None, 'protos': 1, 'records': 2, 'lines': 1, 'warnings': [], 'syntax_ok': True}


def fake(info):
    def run(data, opts, hard_timeout=240, progress=None, cancel=None):
        return Result('print("hi")\n', dict(info))
    return run


def test_default_output_names(monkeypatch, tmp_path):
    src = tmp_path / 'my script.lua'
    src.write_bytes(b'x')
    monkeypatch.setattr(M, 'run', fake(BASE))
    assert M.main([str(src), '-q']) == 0
    assert (tmp_path / 'my script_deobf.lua').read_text() == 'print("hi")\n'
    monkeypatch.setattr(M, 'run', fake(dict(BASE, annotated=True)))
    assert M.main([str(src), '-q', '--annotate']) == 0
    assert (tmp_path / 'my script_annotated.lua').read_text() == 'print("hi")\n'


def test_explicit_output_and_summary(monkeypatch, tmp_path, capsys):
    src = tmp_path / 'a.lua'
    src.write_bytes(b'x')
    out = tmp_path / 'result.lua'
    monkeypatch.setattr(M, 'run', fake(dict(BASE, annotated=True)))
    assert M.main([str(src), '-o', str(out), '--annotate']) == 0
    assert out.exists() and not (tmp_path / 'a_annotated.lua').exists()
    text = capsys.readouterr().out
    assert 'Annotated copy' in text and 'Done in ' in text and str(out) in text


def test_failure_is_explained_and_exits_with_2(monkeypatch, tmp_path, capsys):
    src = tmp_path / 'a.lua'
    src.write_bytes(b'x')

    def boom(data, opts, **kw):
        raise EngineError('unsupported', 'loader entry points not recognised: x')
    monkeypatch.setattr(M, 'run', boom)
    assert M.main([str(src), '-q']) == 2
    err = capsys.readouterr().err
    assert err.startswith('FAILED: ') and "won't guess" in err and 'loader entry points' in err
    assert not list(tmp_path.glob('*_deobf.lua'))


def test_unreadable_input_and_bad_names_file(monkeypatch, tmp_path, capsys):
    monkeypatch.setattr(M, 'run', fake(BASE))
    assert M.main([str(tmp_path / 'missing.lua')]) == 1
    assert 'Cannot read the file' in capsys.readouterr().err
    src = tmp_path / 'a.lua'
    src.write_bytes(b'x')
    names = tmp_path / 'names.json'
    names.write_text('[]')
    assert M.main([str(src), '--names', str(names)]) == 1
    assert 'Invalid --names file' in capsys.readouterr().err


def test_check_flag_reports_every_tool(monkeypatch, capsys):
    monkeypatch.setattr(M, 'check_tools', lambda: [('luau', True, '/x/luau'), ('luau-ast', False, 'not found')])
    assert M.main(['--check']) == 1
    out = capsys.readouterr().out
    assert 'OK' in out and 'FAIL' in out and 'not found' in out
    monkeypatch.setattr(M, 'check_tools', lambda: [('luau', True, '/x/luau')])
    assert M.main(['--check']) == 0

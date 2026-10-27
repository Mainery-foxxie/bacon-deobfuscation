"""Command line front end (the engine is stubbed, so no Luau is needed)."""
import json
import os

import pytest

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


# ------------------------------------------------------------------------------------------------ several files, folders, --detect
def stub_run(calls=None, fail=()):
    """a stand-in for runner.run: the text of the result is the input, so the tests can see which file went where"""
    def run(data, opts, hard_timeout=240, progress=None, cancel=None):
        if calls is not None:
            calls.append(data)
        if data in fail:
            raise EngineError('unsupported', 'loader entry points not recognised: x')
        return Result(data.decode(), dict(BASE))
    return run


def test_a_folder_is_processed_and_earlier_results_in_it_are_skipped(monkeypatch, tmp_path, capsys):
    folder = tmp_path / 'scripts'
    folder.mkdir()
    (folder / 'b.lua').write_bytes(b'B')
    (folder / 'a.txt').write_bytes(b'A')
    (folder / 'c.luau').write_bytes(b'C')
    (folder / 'old_deobf.lua').write_bytes(b'RESULT')
    (folder / 'old_annotated.lua').write_bytes(b'RESULT')
    (folder / 'notes.md').write_bytes(b'NOTES')
    (folder / 'sub').mkdir()
    (folder / 'sub' / 'deep.lua').write_bytes(b'DEEP')
    calls = []
    monkeypatch.setattr(M, 'run', stub_run(calls))
    assert M.main([str(folder)]) == 0
    assert calls == [b'A', b'B', b'C']                                  # sorted; no results, no notes, no sub-folders
    assert (folder / 'a_deobf.lua').read_text() == 'A' and (folder / 'b_deobf.lua').read_text() == 'B'
    assert (folder / 'c_deobf.lua').read_text() == 'C'
    out = capsys.readouterr().out
    assert '[3/3]' in out and 'Done: 3 of 3 files' in out and 'next to each input file' in out
    assert 'a.txt -> a_deobf.lua' in out and str(folder) not in out.split('Done:')[0]


def test_several_files_go_to_the_output_folder_and_equal_names_do_not_overwrite_each_other(monkeypatch, tmp_path, capsys):
    (tmp_path / 'one').mkdir()
    (tmp_path / 'two').mkdir()
    a, b = tmp_path / 'one' / 'x.lua', tmp_path / 'two' / 'x.lua'
    a.write_bytes(b'FIRST')
    b.write_bytes(b'SECOND')
    out = tmp_path / 'results' / 'deeper'
    monkeypatch.setattr(M, 'run', stub_run())
    assert M.main([str(a), str(b), '-o', str(out), '-q']) == 0
    assert sorted(p.name for p in out.iterdir()) == ['x_2_deobf.lua', 'x_deobf.lua']
    assert (out / 'x_deobf.lua').read_text() == 'FIRST' and (out / 'x_2_deobf.lua').read_text() == 'SECOND'
    assert not list((tmp_path / 'one').glob('*_deobf.lua'))
    assert capsys.readouterr().out == ''


def test_one_failing_file_does_not_stop_the_rest_and_sets_exit_code_2(monkeypatch, tmp_path, capsys):
    for name, data in (('a.lua', b'A'), ('b.lua', b'BAD'), ('c.lua', b'C')):
        (tmp_path / name).write_bytes(data)
    monkeypatch.setattr(M, 'run', stub_run(fail=(b'BAD',)))
    assert M.main([str(tmp_path / 'a.lua'), str(tmp_path / 'b.lua'), str(tmp_path / 'c.lua'), str(tmp_path / 'missing.lua')]) == 2
    cap = capsys.readouterr()
    assert (tmp_path / 'a_deobf.lua').exists() and (tmp_path / 'c_deobf.lua').exists() and not (tmp_path / 'b_deobf.lua').exists()
    assert cap.err.count('FAILED: ') == 2
    assert 'b.lua: ' in cap.err and "won't guess" in cap.err and 'Cannot read the file' in cap.err
    assert 'Done: 2 of 4 files' in cap.out and '(2 failed)' in cap.out


@pytest.mark.parametrize('jobs', ['1', '3'])
def test_jobs_run_every_file_once(monkeypatch, tmp_path, jobs):
    names = ['f%d.lua' % i for i in range(7)]
    for n in names:
        (tmp_path / n).write_bytes(n.encode())
    calls = []
    monkeypatch.setattr(M, 'run', stub_run(calls))
    assert M.main([str(tmp_path / n) for n in names] + ['--jobs', jobs, '-q']) == 0
    assert sorted(calls) == sorted(n.encode() for n in names)
    assert all((tmp_path / n.replace('.lua', '_deobf.lua')).read_text() == n for n in names)


def test_misuse_of_the_batch_options(monkeypatch, tmp_path, capsys):
    a, b = tmp_path / 'a.lua', tmp_path / 'b.lua'
    a.write_bytes(b'A')
    b.write_bytes(b'B')
    target = tmp_path / 'file.txt'
    target.write_bytes(b'x')
    monkeypatch.setattr(M, 'run', stub_run())
    assert M.main([str(a), str(b), '-o', str(target)]) == 1                         # several inputs need an output folder
    assert 'must be a folder' in capsys.readouterr().err
    empty = tmp_path / 'empty'
    empty.mkdir()
    assert M.main([str(empty)]) == 1
    assert 'No .lua / .luau / .txt scripts found' in capsys.readouterr().err
    for bad in (['--jobs', '0'], ['-', str(a)], ['--jobs', '2', '--in-process']):
        with pytest.raises(SystemExit) as ei:
            M.main([str(a)] + bad)
        assert ei.value.code == 2
    assert not list(tmp_path.glob('*_deobf.lua'))


def test_a_single_file_with_an_output_folder_is_written_inside_it(monkeypatch, tmp_path):
    src = tmp_path / 'a.lua'
    src.write_bytes(b'A')
    out = tmp_path / 'outdir'
    out.mkdir()
    monkeypatch.setattr(M, 'run', stub_run())
    assert M.main([str(src), '-o', str(out), '-q']) == 0
    assert (out / 'a_deobf.lua').read_text() == 'A'
    assert M.main([str(src), '-o', str(tmp_path / 'new') + os.sep, '-q']) == 0
    assert (tmp_path / 'new' / 'a_deobf.lua').read_text() == 'A'


def test_batch_json_lists_every_file(monkeypatch, tmp_path, capsys):
    (tmp_path / 'a.lua').write_bytes(b'A')
    (tmp_path / 'b.lua').write_bytes(b'BAD')
    monkeypatch.setattr(M, 'run', stub_run(fail=(b'BAD',)))
    assert M.main([str(tmp_path), '--json']) == 2
    rows = json.loads(capsys.readouterr().out)
    assert [(os.path.basename(r['file']), r['ok'], r['code']) for r in rows] == [('a.lua', True, None), ('b.lua', False, 'unsupported')]
    assert rows[0]['info']['protos'] == 1 and rows[1]['info'] is None


def fake_detect(answers):
    def detect(data, max_bytes=0):
        return dict(answers[data])
    return detect


G = dict(kind='bacon_guard', supported=True, confidence=1.0, banner=None, detail='the VM layout of the loader was recognized',
         label='Bacon Guard v0.43.0 (FORGE Protection), build af206410f1dd')
HALF = dict(kind='bacon_guard', supported=False, confidence=0.5, banner=None, detail='the banner is there, but the loader layout is not recognized: x',
            label='Bacon Guard v0.99.0 (FORGE Protection), build 0123456789ab')
NO = dict(kind='unknown', supported=False, confidence=0.0, banner=None, detail='not a Bacon Guard loader: no VM frame found',
          label='not a Bacon Guard / FORGE script')


def test_detect_prints_one_line_per_file_and_runs_nothing(monkeypatch, tmp_path, capsys):
    for name, data in (('good.lua', b'G'), ('half.lua', b'H'), ('no.txt', b'N')):
        (tmp_path / name).write_bytes(data)
    monkeypatch.setattr(M, 'detect', fake_detect({b'G': G, b'H': HALF, b'N': NO}))

    def never(*a, **k):
        raise AssertionError('--detect must not deobfuscate')
    monkeypatch.setattr(M, 'run', never)
    monkeypatch.setattr(M, 'deobfuscate', never)
    assert M.main(['--detect', str(tmp_path / 'good.lua')]) == 0
    assert capsys.readouterr().out == 'good.lua\tbacon_guard\t1.00\tBacon Guard v0.43.0 (FORGE Protection), build af206410f1dd\n'
    assert M.main(['--detect', str(tmp_path)]) == 2                                 # not every file is a supported build
    lines = capsys.readouterr().out.splitlines()
    assert [ln.split('\t')[:3] for ln in lines] == [['good.lua', 'bacon_guard', '1.00'], ['half.lua', 'bacon_guard', '0.50'],
                                                    ['no.txt', 'unknown', '0.00']]
    assert 'the banner is there, but' in lines[1] and 'no VM frame found' in lines[2]
    assert not list(tmp_path.glob('*_deobf.lua'))


def test_detect_json_and_unreadable_files(monkeypatch, tmp_path, capsys):
    (tmp_path / 'good.lua').write_bytes(b'G')
    monkeypatch.setattr(M, 'detect', fake_detect({b'G': G}))
    assert M.main(['--detect', '--json', str(tmp_path / 'good.lua')]) == 0
    rows = json.loads(capsys.readouterr().out)
    assert rows[0]['file'].endswith('good.lua') and rows[0]['supported'] is True and rows[0]['confidence'] == 1.0
    assert M.main(['--detect', str(tmp_path / 'missing.lua')]) == 2
    assert 'Cannot read the file' in capsys.readouterr().err


def test_detect_reports_a_broken_luau_setup(monkeypatch, tmp_path, capsys):
    (tmp_path / 'a.lua').write_bytes(b'x')

    def broken(data, max_bytes=0):
        raise EngineError('setup', 'luau-ast not found')
    monkeypatch.setattr(M, 'detect', broken)
    assert M.main(['--detect', str(tmp_path / 'a.lua')]) == 2
    assert 'FAILED: The Luau tools were not found' in capsys.readouterr().err

"""Bot tests: the real discord.py command machinery, a fake network, and a stub deobfuscation back end."""
import asyncio
import os
import threading

import discord
import pytest

import bgdeobf.bot as B
import fakediscord as F
from bgdeobf.engine import EngineError, Result
from bgdeobf.service import Service

INFO = {'banner': {'version': '0.42.0', 'protection': 'FORGE Protection', 'build': 'abc123'}, 'protos': 3, 'records': 10,
        'lines': 1, 'warnings': [], 'fidelity': {'functions': 3, 'identical': 3, 'reordered': 0, 'cosmetic': 0, 'differing': []}}


def ok_run(data, opts, hard_timeout=240, progress=None, cancel=None):
    for s in ('analyse', 'unpack', 'classify', 'decompile', 'verify'):
        progress(s)
    return Result('print("hi")\n', dict(INFO))


def make(run=ok_run, workers=1, max_queue=2, **env):
    # these tests check the plain-text status; the progress card has its own tests (test_card.py)
    e = dict(DISCORD_TOKEN='x', COOLDOWN='0', PROGRESS_CARD='0')
    e.update(env)
    cfg = B.Config(e)
    svc = Service(workers=workers, max_queue=max_queue, run=run)
    return B.DeobfClient(cfg, service=svc), svc


def deobf(token='t', user=123, name='My Script.lua', size=1234, private=None, extra=(), extra_att=(), guild=None):
    att = F.attachment(31337, name, size)
    opts = [{'type': 11, 'name': 'file', 'value': '31337'}]
    if private is not None:
        opts.append({'type': 5, 'name': 'private', 'value': private})
    opts.extend(extra)
    return F.slash(token, user, 'deobf', opts, [att] + list(extra_att), guild_id=guild), att


def go(coro):
    return asyncio.run(coro)


def test_commands_are_valid_for_discord():
    client, svc = make()
    cmds = dict((c.name, c.to_dict(client.tree)) for c in client.tree.get_commands())
    svc.close()
    assert set(cmds) == {'deobf', 'deobf-info', 'Deobfuscate file'}
    for d in cmds.values():
        assert 1 <= len(d['name']) <= 32
        assert len(d.get('description', '')) <= 100
        for o in d.get('options', []):
            assert len(o['description']) <= 100
    opt = dict((o['name'], o) for o in cmds['deobf']['options'])
    assert opt['file']['type'] == 11 and opt['url']['type'] == 3
    assert not any(opt[n]['required'] for n in ('file', 'url', 'names', 'private', 'annotate'))


def test_happy_path_private_by_default():
    async def scenario():
        client, svc = make()
        p, att = deobf()
        s = await F.invoke(client, p, cdn={att['url']: b'-- protected'})
        svc.close()
        return s
    s = go(scenario())
    assert s.callbacks()[0]['json'] == {'type': 5, 'data': {'flags': 64}}
    final = s.edits()[-1]
    assert final['files'][0]['filename'] == 'My_Script_deobf.lua'
    assert final['files'][0]['content'] == b'print("hi")\n'
    text = final['form_json']['content']
    assert '✅ Done' in text and 'Bacon Guard v0.42.0' in text and '3/3 functions identical' in text
    assert 'allowed to analyze' in text


def test_annotated_result_gets_its_own_file_name_and_note():
    def annotated_run(data, opts, hard_timeout=240, progress=None, cancel=None):
        assert opts.annotate is True
        return Result('local x<@1:2> = 1\n', dict(INFO, annotated=True))

    async def scenario():
        client, svc = make(run=annotated_run)
        p, att = deobf(extra=[{'type': 5, 'name': 'annotate', 'value': True}])
        s = await F.invoke(client, p, cdn={att['url']: b'-- protected'})
        svc.close()
        return s
    final = go(scenario()).edits()[-1]
    assert final['files'][0]['filename'] == 'My_Script_annotated.lua'
    assert 'Annotated copy' in final['form_json']['content']
    assert 'syntax error' not in final['form_json']['content']


def test_syntax_warning_shows_no_host_path():
    def noisy_run(data, opts, hard_timeout=240, progress=None, cancel=None):
        info = dict(INFO, warnings=['the result has a syntax error: C:/Users/Someone~1/AppData/Local/Temp/bgdeobf_job_x/'
                                    'tmp1.lua(8,14): SyntaxError: Expected identifier'])
        return Result('print(1)\n', info)

    async def scenario():
        client, svc = make(run=noisy_run)
        p, att = deobf()
        s = await F.invoke(client, p, cdn={att['url']: b'x'})
        svc.close()
        return s
    text = go(scenario()).edits()[-1]['form_json']['content']
    assert 'Note: the result has a syntax error: line 8:14: SyntaxError: Expected identifier' in text
    assert 'Someone' not in text and 'AppData' not in text


def test_public_reply_when_private_false():
    async def scenario():
        client, svc = make()
        p, att = deobf(private=False)
        s = await F.invoke(client, p, cdn={att['url']: b'x'})
        svc.close()
        return s
    s = go(scenario())
    assert s.callbacks()[0]['json'] == {'type': 5}


@pytest.mark.parametrize('name,size,needle', [
    ('evil.exe', 100, 'Send a'),
    ('big.lua', 10 * 1024 * 1024, 'too large'),
    ('empty.lua', 0, 'empty'),
])
def test_attachment_rejected_before_download(name, size, needle):
    async def scenario():
        client, svc = make()
        p, att = deobf(name=name, size=size)
        s = await F.invoke(client, p, cdn={})
        svc.close()
        return s
    s = go(scenario())
    cb = s.callbacks()
    assert len(cb) == 1 and cb[0]['json']['type'] == 4
    assert cb[0]['json']['data']['flags'] == 64
    assert needle in cb[0]['json']['data']['content']


def test_engine_error_is_explained_without_file():
    def failing(data, opts, **kw):
        raise EngineError('unsupported', 'unknown operation 77')

    async def scenario():
        client, svc = make(run=failing)
        p, att = deobf()
        s = await F.invoke(client, p, cdn={att['url']: b'x'})
        svc.close()
        return s
    s = go(scenario())
    final = s.edits()[-1]
    assert not final['files']
    text = (final['json'] or final['form_json'])['content']
    assert "won't guess" in text and 'unknown operation 77' in text


def test_internal_error_hides_details():
    def failing(data, opts, **kw):
        raise EngineError('internal', 'Traceback: /srv/secret/path')

    async def scenario():
        client, svc = make(run=failing)
        p, att = deobf()
        s = await F.invoke(client, p, cdn={att['url']: b'x'})
        svc.close()
        return s
    s = go(scenario())
    assert 'secret' not in s.last_text() and 'internal error' in s.last_text()


def test_crash_in_backend_does_not_kill_the_command():
    def boom(data, opts, **kw):
        raise RuntimeError('oops')

    async def scenario():
        client, svc = make(run=boom)
        p, att = deobf()
        s = await F.invoke(client, p, cdn={att['url']: b'x'})
        svc.close()
        return s
    assert 'internal error' in go(scenario()).last_text()


def test_same_user_cannot_run_two_jobs_and_queue_limit():
    gate = threading.Event()

    def slow(data, opts, hard_timeout=0, progress=None, cancel=None):
        gate.wait(15)
        return Result('x', dict(INFO))

    async def scenario():
        client, svc = make(run=slow, workers=1, max_queue=0)
        pa, att = deobf(token='a', user=1)
        cdn = {att['url']: b'x'}
        first = asyncio.ensure_future(F.invoke(client, pa, cdn=cdn))
        await asyncio.sleep(0.3)
        busy = await F.invoke(client, deobf(token='b', user=1)[0], cdn=cdn)
        full = await F.invoke(client, deobf(token='c', user=2)[0], cdn=cdn)
        gate.set()
        done = await first
        svc.close()
        return busy, full, done
    busy, full, done = go(scenario())
    assert 'already have a file' in busy.last_text()
    assert 'queue is full' in full.last_text()
    assert done.edits()[-1]['files']


def test_waiting_job_is_told_its_position_then_runs():
    gate = threading.Event()

    def slow(data, opts, hard_timeout=0, progress=None, cancel=None):
        gate.wait(15)
        return Result('x', dict(INFO))

    async def scenario():
        client, svc = make(run=slow, workers=1, max_queue=3)
        cdn = {deobf()[1]['url']: b'x'}
        first = asyncio.ensure_future(F.invoke(client, deobf(token='a', user=1)[0], cdn=cdn))
        await asyncio.sleep(0.3)
        second = asyncio.ensure_future(F.invoke(client, deobf(token='b', user=2)[0], cdn=cdn))
        await asyncio.sleep(0.5)
        gate.set()
        r1, r2 = await first, await second
        svc.close()
        return r2
    r2 = go(scenario())
    texts = [(c['json'] or {}).get('content') for c in r2.edits()]
    assert any(t and 'Queued (position 1)' in t for t in texts)
    assert r2.edits()[-1]['files']


def test_job_that_waits_too_long_is_dropped_before_the_discord_token_expires():
    gate = threading.Event()

    def slow(data, opts, hard_timeout=0, progress=None, cancel=None):
        for _ in range(200):
            if gate.is_set() or (cancel and cancel()):
                break
            threading.Event().wait(0.05)
        return Result('x', dict(INFO))

    async def scenario():
        client, svc = make(run=slow, workers=1, max_queue=3)
        client.cfg.max_wait = 0.6
        cdn = {deobf()[1]['url']: b'x'}
        s = await F.invoke(client, deobf(token='a', user=1)[0], cdn=cdn)
        await asyncio.sleep(0.5)
        left = (svc.running, svc.queued, set(svc._users))
        svc.close()
        return s, left
    s, left = go(scenario())
    assert 'took too long' in s.last_text()
    assert left == (0, 0, set())


def test_cooldown_between_jobs():
    async def scenario():
        client, svc = make(COOLDOWN='60')
        p, att = deobf(token='a')
        cdn = {att['url']: b'x'}
        await F.invoke(client, p, cdn=cdn)
        s = await F.invoke(client, deobf(token='b')[0], cdn=cdn)
        svc.close()
        return s
    s = go(scenario())
    assert 'Please wait' in s.callbacks()[0]['json']['data']['content']


def test_context_menu_uses_first_script_attachment():
    async def scenario():
        client, svc = make()
        a1 = F.attachment(1, 'pic.png', 10)
        a2 = F.attachment(2, 'hub.lua', 2000)
        s = await F.invoke(client, F.context_menu('m1', 123, 'Deobfuscate file', 555, [a1, a2]), cdn={a2['url']: b'x'})
        none = await F.invoke(client, F.context_menu('m2', 124, 'Deobfuscate file', 556, [a1]), cdn={})
        svc.close()
        return s, none
    s, none = go(scenario())
    assert s.callbacks()[0]['json'] == {'type': 5, 'data': {'flags': 64}}
    assert s.edits()[-1]['files'][0]['filename'] == 'hub_deobf.lua'
    assert 'has no .lua' in none.callbacks()[0]['json']['data']['content']


def test_info_command():
    async def scenario():
        client, svc = make()
        s = await F.invoke(client, F.slash('i', 1, 'deobf-info', []))
        svc.close()
        return s
    c = go(scenario()).callbacks()[0]['json']
    assert c['type'] == 4 and c['data']['flags'] == 64
    assert 'Bacon Guard' in c['data']['content'] and 'allowed to analyze' in c['data']['content']


def test_allow_list():
    async def scenario():
        client, svc = make(ALLOWED_USERS='5', ALLOWED_GUILDS='900')
        a, att = deobf(token='a', user=123)
        cdn = {att['url']: b'x'}
        no = await F.invoke(client, a, cdn=cdn)
        yes_user = await F.invoke(client, deobf(token='b', user=5)[0], cdn=cdn)
        yes_guild = await F.invoke(client, deobf(token='c', user=77, guild=900)[0], cdn=cdn)
        other_guild = await F.invoke(client, deobf(token='d', user=78, guild=901)[0], cdn=cdn)
        svc.close()
        return no, yes_user, yes_guild, other_guild
    no, yes_user, yes_guild, other_guild = go(scenario())
    assert "isn't open" in no.callbacks()[0]['json']['data']['content']
    assert yes_user.edits()[-1]['files'] and yes_guild.edits()[-1]['files']
    assert "isn't open" in other_guild.callbacks()[0]['json']['data']['content']


def test_names_file_validation_and_passthrough():
    seen = {}

    def spy(data, opts, **kw):
        seen['names'] = opts.names
        seen['annotate'] = opts.annotate
        return Result('x', dict(INFO))

    async def scenario():
        client, svc = make(run=spy)
        nm = F.attachment(7, 'names.json', 50)
        extra = [{'type': 11, 'name': 'names', 'value': '7'}, {'type': 5, 'name': 'annotate', 'value': True}]
        p, att = deobf(token='a', extra=extra, extra_att=[nm])
        good = await F.invoke(client, p, cdn={att['url']: b'x', nm['url']: b'{"12:34": "playerList"}'})
        p2, _ = deobf(token='b', user=2, extra=extra, extra_att=[nm])
        bad = await F.invoke(client, p2, cdn={att['url']: b'x', nm['url']: b'{"oops": "x"}'})
        svc.close()
        return good, bad
    good, bad = go(scenario())
    assert seen == {'names': {'12:34': 'playerList'}, 'annotate': True}
    assert good.edits()[-1]['files']
    assert 'unknown key' in bad.last_text()


def test_download_failure_is_reported():
    class R(object):
        status = 404
        reason = 'Not Found'

    async def scenario():
        client, svc = make()
        p, att = deobf()
        s = await F.invoke(client, p, cdn={att['url']: discord.NotFound(R(), 'gone')})
        svc.close()
        return s
    assert 'Failed to download' in go(scenario()).last_text()


def test_big_result_is_zipped_or_refused():
    big = os.urandom(150000).hex()

    def make_result(data, opts, **kw):
        return Result(big, dict(INFO))

    async def scenario():
        client, svc = make(run=make_result)
        p, att = deobf(token='a')
        p['attachment_size_limit'] = 200 * 1024 + 64 * 1024
        zipped = await F.invoke(client, p, cdn={att['url']: b'x'})
        p2, _ = deobf(token='b', user=2)
        p2['attachment_size_limit'] = 100 * 1024 + 64 * 1024
        refused = await F.invoke(client, p2, cdn={att['url']: b'x'})
        svc.close()
        return zipped, refused
    zipped, refused = go(scenario())
    f = zipped.edits()[-1]['files'][0]
    assert f['filename'] == 'My_Script_deobf.lua.zip' and f['content'][:2] == b'PK'
    assert not refused.edits()[-1]['files'] and 'too large' in refused.last_text()


def test_config_parsing_and_limits():
    assert B.Config({'DISCORD_TOKEN': ' abc '}).token == 'abc'
    assert B.Config({'ALLOWED_USERS': '1, 2 3'}).allowed_users == {1, 2, 3}
    for bad in ({'MAX_MB': '0'}, {'MAX_MB': '50'}, {'WORKERS': '0'}, {'JOB_TIMEOUT': '5'}, {'GUILD_ID': 'x'}):
        with pytest.raises(ValueError):
            B.Config(bad)


def test_dotenv_loader(tmp_path):
    f = tmp_path / '.env'
    f.write_text('# c\nDISCORD_TOKEN="tok en"\nexport MAX_MB=2\nBAD LINE\nWORKERS=3 \n')
    env = {'WORKERS': '9'}
    assert B.load_dotenv(str(f), env)
    assert env == {'WORKERS': '9', 'DISCORD_TOKEN': 'tok en', 'MAX_MB': '2'}
    assert not B.load_dotenv(str(tmp_path / 'missing'), env)


def test_safe_stem():
    assert B.safe_stem('../../etc/Pass wd.lua') == 'Pass_wd'
    assert B.safe_stem('...') == 'script'
    assert B.safe_stem(None) == 'script'


# ------------------------------------------------------------------------------------------ trace / sandbox options
import io
import zipfile

from bgdeobf.engine import NAME_MARK, STEM_MARK

SUMMARY = dict(events=40, repeated=3, requests=2, domains=['api.example.com', 'cdn.example.org'], remotes=1, payloads=1,
               loadstrings=1, errors=0, files_written=0, instances=12, findings=['Downloads code and runs it'], finished=True,
               reason='the simulated time limit of 30 s was reached', simulated=30.0, real_seconds=1.2, status='ok')


def trace_dict(summary=None, report=None, payloads=1):
    rep = report or ('FAKE ROBLOX SANDBOX TRACE  -  %s\nsaved as %s_payload1.lua\n' % (NAME_MARK, STEM_MARK))
    pl = [dict(id=i + 1, kind='loadstring', label='x', length=12, data=b'print("p%d")\n' % (i + 1), ext='lua') for i in range(payloads)]
    return dict(report=rep, payloads=pl, summary=dict(summary or SUMMARY))


def trace_run(summary=None, report=None, payloads=1, seen=None):
    def run(data, opts, hard_timeout=240, progress=None, cancel=None):
        if seen is not None:
            seen.append((opts.trace, opts.sandbox_only, opts.annotate))
        t = trace_dict(summary, report, payloads)
        if opts.sandbox_only:
            return Result(None, {'warnings': [], 'sandbox_only': True, 'trace': t['summary']}, t)
        return Result('print("hi")\n', dict(INFO, trace=t['summary']), t)
    return run


def slash_opts(**flags):
    return [{'type': 5, 'name': k, 'value': v} for k, v in flags.items()]


def test_commands_have_trace_and_sandbox_options():
    client, svc = make()
    cmds = dict((c.name, c.to_dict(client.tree)) for c in client.tree.get_commands())
    svc.close()
    opt = dict((o['name'], o) for o in cmds['deobf']['options'])
    assert opt['trace']['type'] == 5 and opt['sandbox']['type'] == 5
    assert not opt['trace']['required'] and not opt['sandbox']['required']
    assert all(len(o['description']) <= 100 for o in opt.values())


def test_trace_option_sends_the_result_the_report_and_the_payloads():
    seen = []

    async def scenario():
        client, svc = make(run=trace_run(seen=seen))
        p, att = deobf(extra=slash_opts(trace=True))
        s = await F.invoke(client, p, cdn={att['url']: b'-- protected'})
        svc.close()
        return s
    final = go(scenario()).edits()[-1]
    assert seen == [(True, False, False)]
    names = [f['filename'] for f in final['files']]
    assert names == ['My_Script_deobf.lua', 'My_Script_trace.txt', 'My_Script_payloads.zip']
    report = final['files'][1]['content'].decode()
    assert 'FAKE ROBLOX SANDBOX TRACE  -  My_Script' in report and 'saved as My_Script_payload1.lua' in report
    assert NAME_MARK not in report and STEM_MARK not in report
    z = zipfile.ZipFile(io.BytesIO(final['files'][2]['content']))
    assert z.namelist() == ['My_Script_payload1.lua'] and z.read('My_Script_payload1.lua') == b'print("p1")\n'
    text = final['form_json']['content']
    assert 'Fake Roblox run: 40 events' in text and '2 request(s) to `api.example.com`, `cdn.example.org`' in text
    assert 'Worth a look: Downloads code and runs it' in text and '✅ Done' in text


def test_trace_without_payloads_has_no_zip():
    async def scenario():
        client, svc = make(run=trace_run(payloads=0))
        p, att = deobf(extra=slash_opts(trace=True))
        s = await F.invoke(client, p, cdn={att['url']: b'x'})
        svc.close()
        return s
    names = [f['filename'] for f in go(scenario()).edits()[-1]['files']]
    assert names == ['My_Script_deobf.lua', 'My_Script_trace.txt']


def test_sandbox_option_runs_the_file_as_it_is_and_returns_only_the_report():
    seen = []

    async def scenario():
        client, svc = make(run=trace_run(seen=seen))
        p, att = deobf(extra=slash_opts(sandbox=True))
        s = await F.invoke(client, p, cdn={att['url']: b'-- protected'})
        svc.close()
        return s
    final = go(scenario()).edits()[-1]
    assert seen == [(False, True, False)]
    assert [f['filename'] for f in final['files']] == ['My_Script_trace.txt', 'My_Script_payloads.zip']
    text = final['form_json']['content']
    assert 'Ran the script as given in the fake Roblox (nothing was deobfuscated)' in text
    assert 'Bytecode check' not in text and 'Detected' not in text


@pytest.mark.parametrize('flags,needle', [
    ({'trace': True, 'sandbox': True}, 'Pick one'),
    ({'sandbox': True, 'annotate': True}, 'does not deobfuscate'),
])
def test_conflicting_options_are_refused_before_anything_runs(flags, needle):
    seen = []

    async def scenario():
        client, svc = make(run=trace_run(seen=seen))
        p, att = deobf(extra=slash_opts(**flags))
        s = await F.invoke(client, p, cdn={att['url']: b'x'})
        svc.close()
        return s
    s = go(scenario())
    assert needle in s.last_text() and seen == []


def test_names_file_and_sandbox_are_refused_together():
    seen = []

    async def scenario():
        client, svc = make(run=trace_run(seen=seen))
        nm = F.attachment(7, 'names.json', 50)
        extra = [{'type': 11, 'name': 'names', 'value': '7'}] + slash_opts(sandbox=True)
        p, att = deobf(extra=extra, extra_att=[nm])
        s = await F.invoke(client, p, cdn={att['url']: b'x', nm['url']: b'{}'})
        svc.close()
        return s
    s = go(scenario())
    assert 'does not deobfuscate' in s.last_text() and seen == []


def test_host_names_chosen_by_the_script_are_not_repeated_as_they_are():
    nasty = ['evil.example', '**bold**', '<@123456789>', '@everyone', '[click](x', 'a`b', 'xn--80ak6aa92e.com']
    summary = dict(SUMMARY, requests=7, domains=nasty)

    async def scenario():
        client, svc = make(run=trace_run(summary=summary))
        p, att = deobf(extra=slash_opts(trace=True))
        s = await F.invoke(client, p, cdn={att['url']: b'x'})
        svc.close()
        return s
    text = go(scenario()).edits()[-1]['form_json']['content']
    assert '`evil.example`' in text and '`xn--80ak6aa92e.com`' in text
    assert '5 host name(s) with unusual characters' in text
    for bad in ('**bold**', '<@', '@everyone', '[click]', 'a`b'):
        assert bad not in text


def test_a_big_report_is_zipped_and_one_that_cannot_fit_is_left_out_with_a_note():
    big = os.urandom(150000).hex() + '\n'          # does not compress well

    async def scenario():
        client, svc = make(run=trace_run(report=big, payloads=0))
        p, att = deobf(token='a', extra=slash_opts(trace=True))
        p['attachment_size_limit'] = 200 * 1024 + 64 * 1024
        zipped = await F.invoke(client, p, cdn={att['url']: b'x'})
        p2, _ = deobf(token='b', user=2, extra=slash_opts(trace=True))
        p2['attachment_size_limit'] = 1024 + 64 * 1024
        left_out = await F.invoke(client, p2, cdn={att['url']: b'x'})
        svc.close()
        return zipped, left_out
    zipped, left_out = go(scenario())
    files = zipped.edits()[-1]['files']
    assert [f['filename'] for f in files] == ['My_Script_deobf.lua', 'My_Script_trace.zip'] and files[1]['content'][:2] == b'PK'
    last = left_out.edits()[-1]
    assert [f['filename'] for f in last['files']] == ['My_Script_deobf.lua']
    assert 'trace report is too large' in last['form_json']['content']


def test_the_trace_stage_is_named_while_it_runs():
    def staged(data, opts, hard_timeout=240, progress=None, cancel=None):
        progress('sandbox')                          # the bot shows at most one stage a second
        threading.Event().wait(0.3)
        return trace_run()(data, opts)

    async def scenario():
        client, svc = make(run=staged)
        p, att = deobf(extra=slash_opts(trace=True))
        s = await F.invoke(client, p, cdn={att['url']: b'x'})
        svc.close()
        return s
    texts = [(c['json'] or c['form_json'] or {}).get('content') or '' for c in go(scenario()).edits()]
    assert any('running it in the fake Roblox' in t for t in texts)

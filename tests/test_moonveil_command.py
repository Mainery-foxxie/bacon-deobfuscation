"""!moonveil (chat command): real discord.py Message/Channel objects, fake network, stub back end (as in test_chat.py).
Needs discord.py; skipped without it.  What the engine does with the flag is in test_moonveil_forced.py."""
import pytest

pytest.importorskip('discord')

import bgdeobf.bot as B  # noqa: E402
import fakediscord as F  # noqa: E402
from bgdeobf.engine import EngineError, Result  # noqa: E402
from test_bot import go, make, trace_dict  # noqa: E402

MV = dict(kind='moonveil', trace_only=True, moonveil={'version': '?', 'assumed': True}, lines=40, calls=120, threads=5,
          downloads=[], warnings=[], syntax_ok=True)


def mv_run(seen=None, error=None):
    """stub back end: tells what it was asked to do, then answers like the engine does for a MoonVeil script"""
    def run(data, opts, hard_timeout=240, progress=None, cancel=None):
        if seen is not None:
            seen.append((opts.moonveil, opts.trace, opts.sandbox_only))
        if error is not None:
            raise error
        for s in ('analyse', 'unpack', 'decompile', 'verify'):
            progress(s)
        if opts.trace:
            t = trace_dict()
            return Result('print("trace")\n', dict(MV, trace=t['summary']), t)
        return Result('print("trace")\n', dict(MV))
    return run


def att(name='My Hub.lua'):
    return F.attachment(1, name, 500)


class FetchSpy(object):
    def __init__(self):
        self.calls = []

    async def __call__(self, url, max_bytes, hosts=None, timeout=25, **kw):
        self.calls.append(url)
        return b'-- fetched'


# ------------------------------------------------------------------------------------------ the command word
@pytest.mark.parametrize('content,args', [
    ('!moonveil', ''), ('!moonveil   ', ''), ('!MoonVeil trace', 'trace'), ('  !moonveil https://a.b/c.lua', 'https://a.b/c.lua'),
    ('!moonveil\ntrace', 'trace'), ('!moonveilx', None), ('!moonveils', None), ('halo !moonveil', None), ('!uwu', None),
    ('moonveil', None), ('', None), (None, None),
])
def test_parse_command_for_moonveil(content, args):
    assert B.parse_command(content, '!', B.MOONVEIL_COMMAND) == args


def test_the_two_commands_do_not_overlap():
    assert B.parse_command('!moonveil', '!') is None and B.parse_command('!uwu', '!', B.MOONVEIL_COMMAND) is None
    assert B.parse_command('?moonveil trace', '?', B.MOONVEIL_COMMAND) == 'trace'


# ------------------------------------------------------------------------------------------ the job
def test_moonveil_command_hands_the_script_over_as_a_moonveil_job():
    seen = []

    async def scenario():
        client, svc = make(run=mv_run(seen))
        a = att()
        s = await F.say(client, '!moonveil', [a], cdn={a['url']: b'-- no banner in here'})
        svc.close()
        return s
    s = go(scenario())
    assert seen == [(True, False, False)]
    assert [f['filename'] for f in s.delivered()] == ['My_Hub_reconstructed.lua']
    text = s.chat_texts()[-1]
    assert '✅ Done' in text and 'No MoonVeil banner found' in text and 'NOT the original source' in text
    assert 'Bytecode' not in text and 'functions,' not in text and 'allowed to analyze' in text


def test_uwu_does_not_force_it():
    seen = []

    async def scenario():
        client, svc = make(run=mv_run(seen))
        a = att()
        s = await F.say(client, '!uwu', [a], cdn={a['url']: b'x'})
        svc.close()
        return s
    go(scenario())
    assert seen == [(False, False, False)]


def test_trace_word_adds_the_fake_roblox_report_and_the_payloads():
    seen = []

    async def scenario():
        client, svc = make(run=mv_run(seen))
        a = att()
        s = await F.say(client, '!MoonVeil TRACE', [a], cdn={a['url']: b'x'})
        svc.close()
        return s
    s = go(scenario())
    assert seen == [(True, True, False)]
    assert [f['filename'] for f in s.delivered()] == ['My_Hub_reconstructed.lua', 'My_Hub_trace.txt', 'My_Hub_payloads.zip']
    text = s.chat_texts()[-1]
    assert 'Fake Roblox run: 40 events' in text and '`api.example.com`' in text
    for c in s.sent():
        assert (c['json'] or c['form_json'])['allowed_mentions']['parse'] == []


def test_a_url_is_fetched_and_the_result_is_named_after_it(monkeypatch):
    spy = FetchSpy()
    monkeypatch.setattr(B, 'fetch', spy)
    seen = []

    async def scenario():
        client, svc = make(run=mv_run(seen))
        s = await F.say(client, '!moonveil https://raw.githubusercontent.com/u/r/main/x.lua')
        svc.close()
        return s
    s = go(scenario())
    assert spy.calls == ['https://raw.githubusercontent.com/u/r/main/x.lua']
    assert seen == [(True, False, False)] and [f['filename'] for f in s.delivered()] == ['x_reconstructed.lua']


def test_sandbox_is_refused_and_points_to_the_other_command():
    seen = []

    async def scenario():
        client, svc = make(run=mv_run(seen))
        a = att()
        s = await F.say(client, '!moonveil sandbox', [a], cdn={a['url']: b'x'})
        svc.close()
        return s
    s = go(scenario())
    said = s.sent()[0]['json']['content']
    assert '!uwu sandbox' in said and seen == [] and not s.delivered()


def test_without_a_file_or_url_it_gets_its_own_usage():
    async def scenario():
        client, svc = make(run=mv_run(), COOLDOWN='0')
        bare = await F.say(client, '!moonveil', user_id=1)
        helped = await F.say(client, '!moonveil help', user_id=2)
        uwu = await F.say(client, '!uwu', user_id=3)
        svc.close()
        return bare, helped, uwu
    bare, helped, uwu = go(scenario())
    for s in (bare, helped):
        usage = s.sent()[0]['json']['content']
        assert '`!moonveil`' in usage and '_reconstructed.lua' in usage and 'trace' in usage and '!uwu' in usage
    assert '!moonveil' in uwu.sent()[0]['json']['content']          # the usual help points to it


def test_the_engines_refusal_of_a_bacon_guard_script_reaches_the_user():
    err = EngineError('unsupported', 'this is a Bacon Guard / FORGE script, not a MoonVeil one; use the normal command for it')

    async def scenario():
        client, svc = make(run=mv_run(error=err))
        a = att()
        s = await F.say(client, '!moonveil', [a], cdn={a['url']: b'x'})
        svc.close()
        return s
    s = go(scenario())
    text = s.chat_texts()[-1]
    assert 'Bacon Guard' in text and 'normal command' in text and not s.delivered()


def test_chat_commands_off_means_it_is_ignored_too():
    seen = []

    async def scenario():
        client, svc = make(run=mv_run(seen), PREFIX_COMMANDS='0')
        a = att()
        s = await F.say(client, '!moonveil', [a], cdn={a['url']: b'x'})
        svc.close()
        return s
    s = go(scenario())
    assert seen == [] and not s.sent()


def test_info_and_help_texts_know_the_command():
    client, svc = make()
    try:
        h = client.handler
        assert '`!moonveil`' in h.info_text() and '`!moonveil`' in h.chat_help()
        assert '`!moonveil`' in h.chat_help(moonveil=True) and '`!uwu`' in h.chat_help(moonveil=True)
        assert len(h.info_text()) < 2000                    # a Discord message holds 2000 characters
    finally:
        svc.close()


def test_run_refuses_moonveil_together_with_the_names_options():
    class Rec(B.Responder):
        def __init__(self):
            self.user_id, self.guild_id, self.upload_limit, self.texts = 1, None, 10 * 1024 * 1024, []

        async def refuse(self, text):
            self.texts.append(text)

    class Att(object):
        filename, size = 'a.lua', 10

        async def read(self):
            return b'x'

    seen = []

    async def scenario():
        client, svc = make(run=mv_run(seen))
        r = Rec()
        await client.handler.run(r, file=Att(), annotate=True, moonveil=True)
        svc.close()
        return r
    r = go(scenario())
    assert seen == [] and r.texts and 'annotate' in r.texts[0]

"""!uwu (chat command) tests: real discord.py Message/Channel/Guild objects, fake network, stub back end."""
import asyncio

import pytest

import bgdeobf.bot as B
import fakediscord as F
from bgdeobf.engine import EngineError, Result
from bgdeobf.fetch import FetchError
from bgdeobf.service import Service
from test_bot import INFO, deobf, go, make, ok_run
import time


def staged_run(data, opts, hard_timeout=240, progress=None, cancel=None):
    """tells the first stage, gives the bot time to show it, then finishes"""
    progress('analyse')
    time.sleep(0.15)
    for s in ('unpack', 'classify', 'decompile', 'verify'):
        progress(s)
    return Result('print("hi")\n', dict(INFO))


def att(aid=1, name='script.lua', size=500):
    return F.attachment(aid, name, size)


class FetchSpy(object):
    def __init__(self, data=b'-- fetched', error=None):
        self.calls = []
        self.data = data
        self.error = error

    async def __call__(self, url, max_bytes, hosts=None, timeout=25, **kw):
        self.calls.append(dict(url=url, max_bytes=max_bytes, hosts=hosts))
        if self.error:
            raise self.error
        return self.data


@pytest.fixture
def spy(monkeypatch):
    s = FetchSpy()
    monkeypatch.setattr(B, 'fetch', s)
    return s


@pytest.mark.parametrize('content,args', [
    ('!uwu', ''), ('!uwu   ', ''), ('!uwu https://a.b/c.lua', 'https://a.b/c.lua'), ('!UWU x', 'x'), ('  !uwu x', 'x'),
    ('!uwu\nloadstring(game:HttpGet("https://x.io/y.lua"))()', 'loadstring(game:HttpGet("https://x.io/y.lua"))()'),
    ('!uwux', None), ('!uwuu', None), ('halo !uwu', None), ('!help', None), ('', None), (None, None), ('uwu', None),
])
def test_parse_command(content, args):
    assert B.parse_command(content, '!') == args


def test_parse_command_custom_prefix():
    assert B.parse_command('?uwu a', '?') == 'a' and B.parse_command('!uwu a', '?') is None
    assert B.parse_command('$$uwu', '$$') == ''


def test_attachment_in_dm_gets_status_then_the_result_in_the_same_message():
    async def scenario():
        client, svc = make(run=staged_run)
        a = att(name='My Hub.lua')
        s = await F.say(client, '!uwu', [a], cdn={a['url']: b'-- protected'})
        svc.close()
        return s
    s = go(scenario())
    sent, edited = s.sent(), s.edited()
    assert len(sent) == 1 and 'message_reference' in sent[0]['json']
    assert sent[0]['json']['allowed_mentions'] == {'parse': [], 'replied_user': False} or sent[0]['json']['allowed_mentions']
    assert 'Processing' in sent[0]['json']['content']
    final = edited[-1]
    assert final['files'][0]['filename'] == 'My_Hub_deobf.lua' and final['files'][0]['content'] == b'print("hi")\n'
    text = final['form_json']['content']
    assert '✅ Done' in text and '3/3 functions identical' in text and 'allowed to analyze' in text


def test_fast_job_without_status_posts_one_reply_with_the_file():
    def instant(data, opts, hard_timeout=0, progress=None, cancel=None):
        return Result('print(1)\n', dict(INFO))

    async def scenario():
        client, svc = make(run=instant)
        a = att()
        s = await F.say(client, '!uwu', [a], cdn={a['url']: b'x'})
        svc.close()
        return s
    s = go(scenario())
    assert len(s.sent()) == 1 and s.sent()[0]['files'][0]['filename'] == 'script_deobf.lua'
    assert not s.edited()


def test_url_argument_is_fetched_and_named_after_the_url(spy):
    async def scenario():
        client, svc = make(URL_HOSTS='')
        s = await F.say(client, '!uwu https://raw.githubusercontent.com/u/r/main/Cool%20Hub.lua')
        svc.close()
        return s
    s = go(scenario())
    assert spy.calls[0]['url'] == 'https://raw.githubusercontent.com/u/r/main/Cool%20Hub.lua'
    assert spy.calls[0]['max_bytes'] == 3 * 1024 * 1024 and spy.calls[0]['hosts'] is None
    texts = s.chat_texts()
    assert any('Downloading' in t for t in texts)
    assert s.delivered()[-1]['filename'] == 'Cool_20Hub_deobf.lua'


def test_loadstring_snippet_and_github_blob_links_are_understood(spy):
    async def scenario():
        client, svc = make(COOLDOWN='0')
        await F.say(client, '!uwu loadstring(game:HttpGet("https://raw.githubusercontent.com/u/r/main/x.lua", true))()', user_id=1)
        await F.say(client, '!uwu <https://github.com/u/r/blob/main/dir/y.lua>', user_id=2)
        await F.say(client, '!uwu https://pastebin.com/AbCd1234', user_id=3)
        svc.close()
    go(scenario())
    assert [c['url'] for c in spy.calls] == ['https://raw.githubusercontent.com/u/r/main/x.lua',
                                             'https://raw.githubusercontent.com/u/r/main/dir/y.lua',
                                             'https://pastebin.com/raw/AbCd1234']


@pytest.mark.parametrize('content', ['!uwu', '!uwu help', '!uwu ?', '!uwu what is this'])
def test_no_source_gets_usage(content):
    async def scenario():
        client, svc = make()
        s = await F.say(client, content)
        svc.close()
        return s
    s = go(scenario())
    assert len(s.sent()) == 1 and '!uwu' in s.sent()[0]['json']['content'] and 'raw url' in s.sent()[0]['json']['content']
    assert not s.edited()


def test_wrong_extension_and_bad_urls_are_refused_without_fetching(spy):
    async def scenario():
        client, svc = make(COOLDOWN='0')
        out = []
        out.append(await F.say(client, '!uwu', [att(name='virus.exe')], user_id=1))
        out.append(await F.say(client, '!uwu http://127.0.0.1/secret', user_id=2))
        out.append(await F.say(client, '!uwu http://169.254.169.254/latest/meta-data/', user_id=3))
        out.append(await F.say(client, '!uwu https://example.com:8443/x.lua', user_id=4))
        out.append(await F.say(client, '!uwu http://user:pw@example.com/x.lua', user_id=5))
        svc.close()
        return out
    out = go(scenario())
    texts = [o.sent()[0]['json']['content'] for o in out]
    assert 'Send a' in texts[0]
    assert all('was rejected' in t or "can't be used" in t for t in texts[1:])
    assert spy.calls == []


@pytest.mark.parametrize('error,needle', [
    (FetchError('html', 'x'), 'web page'), (FetchError('too_big', 'x'), 'too large'),
    (FetchError('http', 'the server answered HTTP 404'), 'HTTP 404'), (FetchError('timeout', 'x'), 'too long'),
    (FetchError('blocked', 'that address is not a public address'), 'was rejected'), (FetchError('empty', 'x'), 'empty'),
    (FetchError('net', 'could not connect'), "Couldn't fetch"),
])
def test_fetch_errors_are_explained(monkeypatch, error, needle):
    monkeypatch.setattr(B, 'fetch', FetchSpy(error=error))

    async def scenario():
        client, svc = make()
        s = await F.say(client, '!uwu https://example.com/x.lua')
        svc.close()
        return s
    s = go(scenario())
    assert needle in s.chat_texts()[-1] and not s.delivered()


def test_host_allow_list(spy):
    async def scenario():
        client, svc = make(URL_HOSTS='githubusercontent.com, pastebin.com', COOLDOWN='0')
        a = await F.say(client, '!uwu https://evil.example/x.lua', user_id=1)
        b = await F.say(client, '!uwu https://raw.githubusercontent.com/u/r/main/x.lua', user_id=2)
        svc.close()
        return a, b
    a, b = go(scenario())
    assert 'was rejected' in a.sent()[0]['json']['content']
    assert [c['url'] for c in spy.calls] == ['https://raw.githubusercontent.com/u/r/main/x.lua']
    assert spy.calls[0]['hosts'] == {'githubusercontent.com', 'pastebin.com'}


def test_reply_to_a_message_with_a_file_or_a_url(spy):
    async def scenario():
        client, svc = make(COOLDOWN='0')
        a = att(7, 'theirs.lua')
        s1 = await F.say(client, '!uwu', user_id=1, cdn={a['url']: b'x'},
                         reference={'id': 4999, 'content': 'nih', 'atts': [a]})
        s2 = await F.say(client, '!uwu', user_id=2, reference={'id': 4998, 'content': 'script: https://x.io/z.lua'})
        s3 = await F.say(client, '!uwu', user_id=3,
                         reference={'id': 4997, 'content': 'https://x.io/fetched.lua', 'embed_resolved': False})
        s4 = await F.say(client, '!uwu', user_id=4, reference={'id': 4996, 'content': 'nothing here'})
        svc.close()
        return s1, s2, s3, s4
    s1, s2, s3, s4 = go(scenario())
    assert s1.delivered()[-1]['filename'] == 'theirs_deobf.lua'
    assert [c['url'] for c in spy.calls] == ['https://x.io/z.lua', 'https://x.io/fetched.lua']
    assert 'raw url' in s4.sent()[0]['json']['content']
    assert any(c['method'] == 'GET' for c in s3.calls)


def test_bots_and_other_messages_are_ignored():
    async def scenario():
        client, svc = make()
        a = await F.say(client, '!uwu', [att()], bot=True, cdn={att()['url']: b'x'})
        b = await F.say(client, 'hello everyone', [att()], cdn={att()['url']: b'x'})
        c = await F.say(client, '!uwuu', [att()], cdn={att()['url']: b'x'})
        svc.close()
        return a, b, c
    for s in go(scenario()):
        assert s.calls == []


def test_prefix_commands_can_be_switched_off_or_renamed():
    async def scenario():
        off, svc1 = make(PREFIX_COMMANDS='0')
        a = att()
        s_off = await F.say(off, '!uwu', [a], cdn={a['url']: b'x'})
        intents_off = (off.intents.message_content, off.intents.messages)
        renamed, svc2 = make(PREFIX='?')
        s_bang = await F.say(renamed, '!uwu', [a], cdn={a['url']: b'x'})
        s_q = await F.say(renamed, '?uwu', [a], cdn={a['url']: b'x'})
        on, svc3 = make()
        svc1.close(), svc2.close(), svc3.close()
        return s_off, intents_off, s_bang, s_q, (on.intents.message_content, on.intents.messages)
    s_off, intents_off, s_bang, s_q, intents_on = go(scenario())
    assert s_off.calls == [] and intents_off == (False, False)
    assert s_bang.calls == [] and s_q.sent()
    assert intents_on == (True, True)


def test_busy_cooldown_and_allow_list_apply_to_chat_too():
    import threading
    gate = threading.Event()

    def slow(data, opts, hard_timeout=0, progress=None, cancel=None):
        gate.wait(10)
        return Result('x', dict(INFO))

    async def scenario():
        client, svc = make(run=slow, workers=1, max_queue=2, COOLDOWN='60', ALLOWED_USERS='1,2')
        a = att()
        cdn = {a['url']: b'x'}
        sess = F.FakeSession()
        first = asyncio.ensure_future(F.say(client, '!uwu', [a], user_id=1, cdn=cdn, session=sess))
        await asyncio.sleep(0.3)
        await F.say(client, '!uwu', [a], user_id=1, cdn=cdn, mid=5002, session=sess)
        await F.say(client, '!uwu', [a], user_id=3, cdn=cdn, mid=5003, session=sess)
        gate.set()
        await first
        await F.say(client, '!uwu', [a], user_id=1, cdn=cdn, mid=5004, session=sess)
        svc.close()
        return sess
    sess = go(scenario())
    texts = sess.chat_texts()
    assert any('already have a file' in t for t in texts)
    assert any("isn't open" in t for t in texts)
    assert any('Please wait' in t for t in texts)
    assert len(sess.delivered()) == 1


def test_guild_channel_with_and_without_permission_to_speak():
    async def scenario():
        client, svc = make(COOLDOWN='0')
        F.ensure_bot_user(client)
        a = att()
        ok_ch = F.guild_channel(client, guild_id=900, channel_id=777, perms=101376)
        s_ok = await F.say(client, '!uwu', [a], cdn={a['url']: b'x'}, channel=ok_ch, user_id=1)
        mute_ch = F.guild_channel(client, guild_id=901, channel_id=778, perms=1024)
        s_mute = await F.say(client, '!uwu', [a], cdn={a['url']: b'x'}, channel=mute_ch, user_id=2)
        svc.close()
        return s_ok, s_mute
    s_ok, s_mute = go(scenario())
    assert s_ok.delivered() and s_mute.calls == []


def test_guild_allow_list_uses_the_guild_of_the_message():
    async def scenario():
        client, svc = make(COOLDOWN='0', ALLOWED_GUILDS='900')
        a = att()
        yes = await F.say(client, '!uwu', [a], cdn={a['url']: b'x'}, channel=F.guild_channel(client, 900, 777), user_id=1)
        no = await F.say(client, '!uwu', [a], cdn={a['url']: b'x'}, channel=F.guild_channel(client, 901, 778), user_id=2)
        dm = await F.say(client, '!uwu', [a], cdn={a['url']: b'x'}, user_id=3)
        svc.close()
        return yes, no, dm
    yes, no, dm = go(scenario())
    assert yes.delivered()
    assert "isn't open" in no.sent()[0]['json']['content'] and "isn't open" in dm.sent()[0]['json']['content']


def test_missing_attach_files_permission_is_explained():
    async def scenario():
        client, svc = make()
        a = att()
        sess = F.FakeSession()
        sess.forbid_files = True
        s = await F.say(client, '!uwu', [a], cdn={a['url']: b'x'}, session=sess)
        svc.close()
        return s
    s = go(scenario())
    assert any('Attach Files' in t for t in s.chat_texts())


def test_original_message_deleted_falls_back_to_a_plain_message():
    async def scenario():
        client, svc = make(run=staged_run)
        a = att()
        sess = F.FakeSession()
        sess.reject_reference = True
        s = await F.say(client, '!uwu', [a], cdn={a['url']: b'x'}, session=sess)
        svc.close()
        return s
    s = go(scenario())
    posted = [c for c in s.sent() if c['json']]
    assert posted and 'message_reference' not in posted[-1]['json']
    assert s.delivered()


def test_no_permission_to_answer_is_not_a_crash(caplog):
    async def scenario():
        client, svc = make()
        a = att()
        sess = F.FakeSession()
        sess.forbid_all = True
        s = await F.say(client, '!uwu', [a], cdn={a['url']: b'x'}, session=sess)
        svc.close()
        return s
    go(scenario())
    assert not [r for r in caplog.records if r.levelname == 'ERROR' and 'failure while handling' in r.getMessage()]


def test_engine_errors_in_chat():
    def failing(data, opts, **kw):
        raise EngineError('unsupported', 'unknown operation 77')

    async def scenario():
        client, svc = make(run=failing)
        a = att()
        s = await F.say(client, '!uwu', [a], cdn={a['url']: b'x'})
        svc.close()
        return s
    s = go(scenario())
    assert "won't guess" in s.chat_texts()[-1] and 'unknown operation 77' in s.chat_texts()[-1]


def test_slash_command_url_option_and_exclusivity(spy):
    async def scenario():
        client, svc = make(COOLDOWN='0')
        a = att(31337, 'My Script.lua', 1234)
        url_opt = [{'type': 3, 'name': 'url', 'value': 'loadstring(game:HttpGet("https://pastebin.com/raw/AbCd1234"))()'}]
        by_url = await F.invoke(client, F.slash('u1', 1, 'deobf', url_opt), cdn={})
        both = await F.invoke(client, F.slash('u2', 2, 'deobf', url_opt + [{'type': 11, 'name': 'file', 'value': '31337'}], [a]), cdn={})
        neither = await F.invoke(client, F.slash('u3', 3, 'deobf', []), cdn={})
        svc.close()
        return by_url, both, neither
    by_url, both, neither = go(scenario())
    assert spy.calls[0]['url'] == 'https://pastebin.com/raw/AbCd1234'
    assert by_url.callbacks()[0]['json'] == {'type': 5, 'data': {'flags': 64}}
    assert by_url.edits()[-1]['files'][0]['filename'] == 'AbCd1234_deobf.lua'
    assert 'Pick one' in both.callbacks()[0]['json']['data']['content']
    assert 'Include a' in neither.callbacks()[0]['json']['data']['content']
    assert len(spy.calls) == 1


def test_context_menu_falls_back_to_a_url_in_the_message(spy):
    async def scenario():
        client, svc = make(COOLDOWN='0')
        p = F.context_menu('m1', 1, 'Deobfuscate file', 555, [])
        p['data']['resolved']['messages']['555']['content'] = 'cek ini https://x.io/hub.lua ya'
        s = await F.invoke(client, p, cdn={})
        svc.close()
        return s
    s = go(scenario())
    assert spy.calls[0]['url'] == 'https://x.io/hub.lua'
    assert s.edits()[-1]['files'][0]['filename'] == 'hub_deobf.lua'


def test_info_mentions_the_chat_command_and_prefix():
    async def scenario():
        client, svc = make(PREFIX='?')
        s = await F.invoke(client, F.slash('i', 1, 'deobf-info', []))
        off, svc2 = make(PREFIX_COMMANDS='0')
        s2 = await F.invoke(off, F.slash('i2', 1, 'deobf-info', []))
        svc.close(), svc2.close()
        return s, s2
    s, s2 = go(scenario())
    assert '`?uwu`' in s.callbacks()[0]['json']['data']['content']
    assert 'disabled' in s2.callbacks()[0]['json']['data']['content']


def test_config_for_chat_commands():
    c = B.Config({'PREFIX': '$', 'URL_HOSTS': 'A.com, b.org.'})
    assert c.prefix == '$' and c.url_hosts == {'a.com', 'b.org'} and c.prefix_commands
    assert not B.Config({'PREFIX_COMMANDS': 'off'}).prefix_commands
    for bad in ({'PREFIX': 'abcd'}, {'PREFIX': 'a b'}, {'URL_HOSTS': 'ex ample!.com'}):
        with pytest.raises(ValueError):
            B.Config(bad)

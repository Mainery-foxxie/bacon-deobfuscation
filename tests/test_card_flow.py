"""How the bot drives the progress card (see test_card.py for the card itself).  Needs discord.py; skipped without it."""
import asyncio
import threading
import time

import pytest

pytest.importorskip('discord')

from bgdeobf.card import URL_STEPS  # noqa: E402
from bgdeobf.engine import EngineError, Result  # noqa: E402
import bgdeobf.bot as B  # noqa: E402
from bgdeobf.fetch import FetchError  # noqa: E402
from bgdeobf.service import Service  # noqa: E402

INFO = dict(lines=10, syntax_ok=True, warnings=[], banner=None,
            fidelity=dict(functions=3, identical=3, reordered=0, cosmetic=0, differing=[]))


@pytest.fixture(autouse=True)
def quick_redraws(monkeypatch):
    monkeypatch.setattr(B, 'CARD_INTERVAL', 0.05)


class Rec(B.Responder):
    """records what would be drawn"""

    def __init__(self, uid=1):
        self.user_id, self.guild_id, self.upload_limit = uid, None, 10 * 1024 * 1024
        self.shown, self.final, self.texts = [], None, []

    async def refuse(self, text):
        self.texts.append(text)

    async def status(self, text):
        self.texts.append(text)

    async def show(self, card):
        self.shown.append(card.data())

    async def finish(self, text, attachment=None, card=None):
        self.final = dict(text=text, file=attachment[0] if attachment else None,
                          card=card.data() if card is not None else None)


class Att(object):
    filename, size = 'My Hub.lua', 100

    async def read(self):
        return b'-- protected'


def staged(delay=0.15):
    def run(data, opts, hard_timeout=0, progress=None, cancel=None):
        for s in ('analyse', 'unpack', 'classify', 'decompile', 'verify'):
            progress(s)
            time.sleep(delay)
        return Result('print("hi")\n', dict(INFO))
    return run


def make(run, workers=2, **env):
    e = dict(DISCORD_TOKEN='x', COOLDOWN='0')
    e.update(env)
    svc = Service(workers=workers, max_queue=3, run=run)
    return B.Handler(B.Config(e), svc), svc


def steps(d):
    return d['description'].split('\n\n', 1)[1].split('\n')


def run(coro):
    return asyncio.run(coro)


def test_card_is_on_by_default():
    assert B.Config({'DISCORD_TOKEN': 'x'}).progress_card is True
    assert B.Config({'DISCORD_TOKEN': 'x', 'PROGRESS_CARD': 'off'}).progress_card is False


def test_file_job_walks_the_steps_and_ends_green_with_the_file():
    async def scenario():
        h, svc = make(staged())
        r = Rec()
        await h.run(r, file=Att())
        svc.close()
        return r
    r = run(scenario())
    assert r.shown[0]['footer'] == '0.0s elapsed'
    seen = {ln for d in r.shown for ln in steps(d) if ln.startswith('**')}
    assert any('Detecting obfuscator' in ln for ln in seen) and any('Verifying the result' in ln for ln in seen)
    assert len({d['title'][0] for d in r.shown}) > 1                      # the spinner moved
    assert r.final['file'] == 'My_Hub_deobf.lua' and r.final['text'].startswith('✅ Done')
    assert r.final['card']['title'].startswith('✅')
    assert all(ln.startswith('✅') for ln in steps(r.final['card']))
    assert not r.texts                                                    # no plain-text status next to the card


def test_url_job_starts_at_downloading(monkeypatch):
    async def fake_fetch(url, max_bytes, hosts=None, timeout=25, **kw):
        await asyncio.sleep(0.1)
        return b'-- fetched'
    monkeypatch.setattr(B, 'fetch', fake_fetch)

    async def scenario():
        h, svc = make(staged(0.05))
        r = Rec()
        await h.run(r, url='https://raw.githubusercontent.com/u/r/main/Cool%20Hub.lua')
        svc.close()
        return r
    r = run(scenario())
    first = steps(r.shown[0])
    assert first[0].startswith('✅') and 'Validating URL' in first[0]
    assert first[1].startswith('**') and 'Downloading content' in first[1]
    assert r.final['file'] == 'Cool_20Hub_deobf.lua' and len(steps(r.final['card'])) == len(URL_STEPS)


def test_failed_download_stops_on_the_downloading_step(monkeypatch):
    async def bad_fetch(url, max_bytes, hosts=None, timeout=25, **kw):
        raise FetchError('http', 'HTTP 403')
    monkeypatch.setattr(B, 'fetch', bad_fetch)

    async def scenario():
        h, svc = make(staged())
        r = Rec()
        await h.run(r, url='https://example.com/x.lua')
        svc.close()
        return r
    r = run(scenario())
    assert r.final['file'] is None and r.final['card']['title'].startswith('❌')
    assert any(ln.startswith('❌') and 'Downloading content' in ln for ln in steps(r.final['card']))


def test_engine_error_ends_red_and_explains_in_the_message():
    def boom(data, opts, **kw):
        time.sleep(0.1)
        raise EngineError('unsupported', 'this build uses handlers that are not understood yet: operation 99')

    async def scenario():
        h, svc = make(boom)
        r = Rec()
        await h.run(r, file=Att())
        svc.close()
        return r
    r = run(scenario())
    assert r.final['file'] is None and "I won't guess" in r.final['text']
    assert r.final['card']['title'].startswith('❌') and any(ln.startswith('❌') for ln in steps(r.final['card']))


def test_unexpected_crash_still_ends_with_a_red_card_and_no_leftover_task():
    def crash(data, opts, **kw):
        raise RuntimeError('kaput')

    async def scenario():
        h, svc = make(crash)
        r = Rec()
        await h.run(r, file=Att())
        svc.close()
        await asyncio.sleep(0.1)
        return r, [t for t in asyncio.all_tasks() if t is not asyncio.current_task()]
    r, leftovers = run(scenario())
    assert r.final['card']['title'].startswith('❌') and not leftovers


def test_second_user_sees_their_queue_position_on_the_card():
    gate = threading.Event()

    def slow(data, opts, hard_timeout=0, progress=None, cancel=None):
        progress('analyse')
        gate.wait(10)
        return Result('x', dict(INFO))

    async def scenario():
        h, svc = make(slow, workers=1)
        r1, r2 = Rec(1), Rec(2)
        t1 = asyncio.ensure_future(h.run(r1, file=Att()))
        await asyncio.sleep(0.3)
        t2 = asyncio.ensure_future(h.run(r2, file=Att()))
        await asyncio.sleep(0.4)
        waiting = [d for d in r2.shown if 'Waiting in the queue (position 1).' in d['description']]
        gate.set()
        await t1
        await t2
        svc.close()
        return r2, waiting
    r2, waiting = run(scenario())
    assert waiting and r2.final['file'] == 'My_Hub_deobf.lua' and 'queue' not in r2.final['card']['description']


def test_card_off_keeps_the_plain_text_status():
    async def scenario():
        h, svc = make(staged(0.3), PROGRESS_CARD='0')
        r = Rec()
        await h.run(r, file=Att())
        svc.close()
        return r
    r = run(scenario())
    assert not r.shown and any('Processing' in t for t in r.texts)
    assert r.final['card'] is None and r.final['file'] == 'My_Hub_deobf.lua'

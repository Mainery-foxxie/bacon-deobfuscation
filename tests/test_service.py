import asyncio
import threading

import pytest

from bgdeobf.engine import EngineError, Result
from bgdeobf.service import Busy, Service


def result():
    return Result('x', {})


def test_stats_and_cleanup():
    def run(data, opts, **kw):
        if data == b'bad':
            raise EngineError('unsupported', 'x')
        return result()

    async def scenario():
        svc = Service(workers=2, max_queue=2, run=run)
        await svc.submit(1, b'ok')
        with pytest.raises(EngineError):
            await svc.submit(1, b'bad')
        assert (svc.running, svc.queued) == (0, 0)
        assert svc._users == set()
        svc.close()
        return svc.stats
    assert asyncio.run(scenario()) == {'ok': 1, 'failed': 1, 'rejected': 0}


def test_options_are_per_job():
    seen = []

    def run(data, opts, **kw):
        seen.append((opts.names, opts.annotate, opts.max_bytes))
        return result()

    async def scenario():
        svc = Service(run=run)
        await svc.submit(1, b'a', names={'1:1': 'a'}, annotate=True)
        await svc.submit(1, b'b')
        svc.close()
    asyncio.run(scenario())
    assert seen[0][:2] == ({'1:1': 'a'}, True) and seen[1][:2] == (None, False)


def test_cancelling_the_waiter_cancels_the_job():
    started = threading.Event()
    flags = []

    def run(data, opts, cancel=None, **kw):
        started.set()
        for _ in range(100):
            if cancel():
                flags.append('cancelled')
                raise EngineError('cancelled', 'x')
            threading.Event().wait(0.05)
        return result()

    async def scenario():
        svc = Service(run=run)
        t = asyncio.ensure_future(svc.submit(1, b'a'))
        while not started.is_set():
            await asyncio.sleep(0.01)
        t.cancel()
        with pytest.raises(asyncio.CancelledError):
            await t
        await asyncio.sleep(0.3)
        assert (svc.running, svc.queued) == (0, 0) and svc._users == set()
        svc.close()
    asyncio.run(scenario())
    assert flags == ['cancelled']


def test_busy_reasons():
    gate = threading.Event()

    def run(data, opts, **kw):
        gate.wait(10)
        return result()

    async def scenario():
        svc = Service(workers=1, max_queue=1, run=run)
        a = asyncio.ensure_future(svc.submit(1, b'a'))
        await asyncio.sleep(0.2)
        b = asyncio.ensure_future(svc.submit(2, b'b'))
        await asyncio.sleep(0.1)
        with pytest.raises(Busy) as e1:
            await svc.submit(1, b'again')
        with pytest.raises(Busy) as e2:
            await svc.submit(3, b'c')
        gate.set()
        await a
        await b
        svc.close()
        return e1.value.reason, e2.value.reason
    assert asyncio.run(scenario()) == ('user_busy', 'queue_full')

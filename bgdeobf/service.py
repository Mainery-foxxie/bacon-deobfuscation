"""Job service shared by every front end (Discord bot, or anything else you plug in).

It owns the limits: how many jobs run at once, how many may wait, one job per user at a time, a hard deadline per job.
The heavy work happens in runner.run(), which uses a separate resource-limited process."""
import asyncio
import concurrent.futures
import threading

from . import runner as _runner
from .engine import Options


class Busy(Exception):
    """reason: 'queue_full' | 'user_busy'"""

    def __init__(self, reason):
        Exception.__init__(self, reason)
        self.reason = reason


class Service(object):
    def __init__(self, workers=2, max_queue=8, options=None, hard_timeout=240, run=None):
        self.workers = max(1, int(workers))
        self.max_queue = max(0, int(max_queue))
        self.options = options or Options()
        self.hard_timeout = hard_timeout
        self._run = run or _runner.run
        self._pool = concurrent.futures.ThreadPoolExecutor(max_workers=self.workers, thread_name_prefix='bgdeobf')
        self._sem = None
        self._queued = 0
        self._running = 0
        self._users = set()
        self.stats = dict(ok=0, failed=0, rejected=0)

    @property
    def queued(self):
        return self._queued

    @property
    def running(self):
        return self._running

    def _job_options(self, names, annotate):
        kw = dict(vars(self.options))
        kw['names'] = names
        kw['annotate'] = bool(annotate)
        return Options(**kw)

    async def submit(self, user_id, data, names=None, annotate=False, progress=None):
        """Run one job.  `progress` is an optional coroutine function called with ('queued', position) or ('stage', name).
        Raises Busy when the job is not accepted, EngineError when it fails."""
        if self._sem is None:
            self._sem = asyncio.Semaphore(self.workers)
        if user_id in self._users:
            self.stats['rejected'] += 1
            raise Busy('user_busy')
        if self._queued >= self.max_queue and self._running >= self.workers:
            self.stats['rejected'] += 1
            raise Busy('queue_full')
        self._users.add(user_id)
        self._queued += 1
        try:
            if progress is not None and self._running >= self.workers:
                await progress(('queued', self._queued))
            await self._sem.acquire()
        except BaseException:
            self._queued -= 1
            self._users.discard(user_id)
            raise
        self._queued -= 1
        self._running += 1
        pump = None
        stop = threading.Event()
        try:
            loop = asyncio.get_running_loop()
            stages = asyncio.Queue()
            opts = self._job_options(names, annotate)

            def note(stage):
                loop.call_soon_threadsafe(stages.put_nowait, stage)

            async def relay():
                while True:
                    stage = await stages.get()
                    try:
                        await progress(('stage', stage))
                    except Exception:
                        pass

            if progress is not None:
                pump = asyncio.ensure_future(relay())
            fut = loop.run_in_executor(self._pool, lambda: self._run(data, opts, hard_timeout=self.hard_timeout,
                                                                     progress=note, cancel=stop.is_set))
            try:
                res = await fut
            except asyncio.CancelledError:
                stop.set()
                raise
            self.stats['ok'] += 1
            return res
        except asyncio.CancelledError:
            raise
        except Exception:
            self.stats['failed'] += 1
            raise
        finally:
            if pump is not None:
                pump.cancel()
            self._running -= 1
            self._sem.release()
            self._users.discard(user_id)

    def close(self):
        self._pool.shutdown(wait=False, cancel_futures=True)

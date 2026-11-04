"""Regression tests for the hand-over of job options from service.py to engine.py.

The bug: Service._job_options adds `trace`, `sandbox_only` and `moonveil` to the keyword arguments of Options(...), and
engine.Options did not accept them, so every job died before it started with
    TypeError: Options.__init__() got an unexpected keyword argument 'trace'
engine.py has no trace / sandbox / moonveil mode yet, so it has to refuse a job that asks for one instead of quietly
running a normal job.

Plain unittest classes, so `python -m pytest -q tests/` and `python -m unittest` both run them."""
import asyncio
import unittest

from bgdeobf.engine import EngineError, Options, deobfuscate
from bgdeobf.service import Service

MODES = ('trace', 'sandbox_only', 'moonveil')


def options_seen_by_runner(**submit_kwargs):
    """Run one job through a real Service whose runner is a fake (the real one starts a separate process); returns the
    Options object the runner was handed."""
    seen = []

    def fake_run(data, opts, hard_timeout=None, progress=None, cancel=None):
        seen.append(opts)
        return 'finished'

    svc = Service(workers=1, run=fake_run)
    try:
        asyncio.run(svc.submit(1, b'print(1)', **submit_kwargs))
    finally:
        svc.close()
    return seen[0]


class ServiceHandsOptionsToTheRunner(unittest.TestCase):
    def test_plain_job_reaches_the_runner_with_its_options(self):
        # catches: Service building Options with a keyword Options does not accept (the reported crash)
        opts = options_seen_by_runner(names={'1:2': 'x'}, annotate=True)
        self.assertEqual(opts.names, {'1:2': 'x'})
        self.assertIs(opts.annotate, True)
        for mode in MODES:
            self.assertIs(getattr(opts, mode), False, mode)

    def test_requested_modes_reach_the_options(self):
        # catches: a mode dropped on the way, or `sandbox` not switching the other two off
        table = [
            (dict(trace=True), (True, False, False)),
            (dict(sandbox=True), (False, True, False)),
            (dict(moonveil=True), (False, False, True)),
            (dict(trace=True, sandbox=True, moonveil=True), (False, True, False)),
        ]
        for kwargs, want in table:
            with self.subTest(**kwargs):
                opts = options_seen_by_runner(**kwargs)
                self.assertEqual(tuple(getattr(opts, m) for m in MODES), want)


class EngineRefusesModesItDoesNotHave(unittest.TestCase):
    def test_each_unimplemented_mode_is_refused_before_any_work(self):
        # catches: a requested mode being silently ignored while a normal run starts.  An empty file fails with code
        # 'empty' once a run starts, so 'unsupported' proves the refusal came first.
        for mode in MODES:
            with self.subTest(mode=mode):
                with self.assertRaises(EngineError) as cm:
                    deobfuscate(b'', Options(**{mode: True}))
                self.assertEqual(cm.exception.code, 'unsupported')

    def test_ordinary_options_are_not_refused(self):
        # catches: the refusal firing on ordinary jobs
        with self.assertRaises(EngineError) as cm:
            deobfuscate(b'', Options())
        self.assertEqual(cm.exception.code, 'empty')


if __name__ == '__main__':
    unittest.main()

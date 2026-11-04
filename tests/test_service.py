"""The worker process has to start with the engine that is in the repository, and pass the engine's own errors on.

The bug: worker.py imported `sandbox` from bgdeobf.engine (and read `Result.trace`), but engine.py has neither, so the
worker died at start-up for every job and the bot answered `internal: the worker stopped unexpectedly (exit 1)`.

These tests go through the real worker subprocess (runner.run), not through the engine in this process."""
import pytest

from bgdeobf import runner
from bgdeobf.engine import EngineError, Options


@pytest.mark.parametrize('mode', ['trace', 'sandbox_only', 'moonveil'])
def test_worker_starts_and_relays_the_engine_refusal_of_a_mode_it_does_not_have(mode):
    # catches: the worker failing to import from engine.py (it then ends with code 'internal' and exit 1)
    with pytest.raises(EngineError) as info:
        runner.run(b'print("hello")', Options(**{mode: True}), hard_timeout=60)
    assert info.value.code == 'unsupported', info.value.message
    assert mode in info.value.message


def test_worker_reports_an_empty_file_without_starting():
    with pytest.raises(EngineError) as info:
        runner.run(b'   \n', Options(), hard_timeout=60)
    assert info.value.code == 'empty'

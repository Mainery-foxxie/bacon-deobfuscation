"""A MoonVeil job through the bot's progress-card flow.  Needs discord.py; skipped without it."""
import pytest

pytest.importorskip('discord')

from bgdeobf.engine import Result  # noqa: E402
from test_card_flow import Att, Rec, make, run  # noqa: E402

TRACE = dict(kind='moonveil', trace_only=True, moonveil={'version': '2.1.1'}, lines=40, calls=120, threads=5,
             downloads=[], warnings=[], syntax_ok=True)


def test_moonveil_job_is_delivered_as_a_labelled_reconstruction():
    def trace_run(data, opts, hard_timeout=0, progress=None, cancel=None):
        for s in ('analyse', 'unpack', 'decompile', 'verify'):
            progress(s)
        return Result('print("trace")\n', dict(TRACE))

    async def scenario():
        h, svc = make(trace_run)
        r = Rec()
        await h.run(r, file=Att())
        svc.close()
        return r
    r = run(scenario())
    assert r.final['file'] == 'My_Hub_reconstructed.lua'
    text = r.final['text']
    assert text.startswith('✅ Done') and 'MoonVeil 2.1.1' in text and 'NOT the original source' in text
    assert 'Bytecode' not in text and 'functions,' not in text

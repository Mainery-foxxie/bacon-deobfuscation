"""The live progress card: its state machine, which needs no Discord (the bot flow is in test_card_flow.py)."""
import pytest

from bgdeobf.card import Card, FILE_STEPS, URL_STEPS


# ---------------------------------------------------------------------------------------------- the card itself
class Clock(object):
    def __init__(self):
        self.t = 100.0

    def __call__(self):
        return self.t


def lines(card):
    return card.data()['description'].split('\n\n', 1)[1].split('\n')


def test_steps_follow_the_source():
    assert [k for k, _ in Card().steps] == [k for k, _ in FILE_STEPS]
    assert [k for k, _ in Card(from_url=True).steps][:2] == ['validate', 'download']
    assert len(URL_STEPS) == len(FILE_STEPS) + 2


def test_begin_ticks_everything_before_and_leaves_the_rest():
    c = Card(from_url=True)
    c.begin('download')
    ls = lines(c)
    assert ls[0].startswith('✅') and 'Validating URL' in ls[0]
    assert ls[1].startswith('**') and 'Downloading content' in ls[1]
    assert all(ln.startswith('▫️') for ln in ls[2:])


def test_engine_stages_move_the_card_and_unknown_ones_are_ignored():
    c = Card()
    for stage, label in (('analyse', 'Detecting obfuscator'), ('unpack', 'Opening the VM'),
                         ('classify', 'Recognizing handlers'), ('decompile', 'Rebuilding the code'),
                         ('verify', 'Verifying the result')):
        c.stage(stage)
        active = [ln for ln in lines(c) if ln.startswith('**')]
        assert len(active) == 1 and label in active[0]
    before = lines(c)
    c.stage('no-such-stage')
    assert lines(c) == before


def test_queue_note_shows_until_the_first_stage():
    c = Card()
    c.queued(2)
    assert 'Waiting in the queue (position 2).' in c.data()['description']
    c.stage('analyse')
    assert 'queue' not in c.data()['description']


def test_complete_ticks_all_and_freezes_the_timer():
    clock = Clock()
    c = Card(clock=clock)
    c.begin('unpack')
    clock.t += 3.5
    c.complete()
    clock.t += 50
    d = c.data()
    assert d['title'].startswith('✅') and d['footer'] == 'Finished in 3.5s'
    assert all(ln.startswith('✅') for ln in lines(c))
    c.fail()                     # a finished card does not change its mind
    assert c.status == 'done'


def test_fail_marks_the_running_step_and_freezes_the_timer():
    clock = Clock()
    c = Card(clock=clock)
    c.stage('classify')
    clock.t += 4
    c.fail()
    clock.t += 50
    ls = lines(c)
    assert ls[2].startswith('❌') and 'Recognizing handlers' in ls[2]
    assert ls[0].startswith('✅') and ls[1].startswith('✅') and ls[3].startswith('▫️')
    d = c.data()
    assert d['title'].startswith('❌') and d['footer'] == 'Stopped after 4.0s'
    c.begin('verify')            # nothing moves once it has stopped
    assert lines(c) == ls


def test_fail_before_anything_started_marks_the_first_step():
    c = Card()
    c.fail()
    assert lines(c)[0].startswith('❌')


def test_spinner_moves_on_tick_and_elapsed_counts_up():
    clock = Clock()
    c = Card(clock=clock)
    first = c.data()
    c.tick()
    clock.t += 1.5
    second = c.data()
    assert first['title'] != second['title']
    assert first['footer'] == '0.0s elapsed' and second['footer'] == '1.5s elapsed'


def test_embed_is_a_discord_embed():
    discord = pytest.importorskip('discord')
    c = Card()
    c.begin('detect')
    e = c.embed()
    assert isinstance(e, discord.Embed)
    assert e.title.endswith('Deobfuscating script') and 'Detecting obfuscator' in e.description
    assert e.footer.text.endswith('elapsed')

"""Startup paths of `python -m bgdeobf.bot` (no Discord connection is made)."""
import discord
import pytest

import bgdeobf.bot as B


class R(object):
    status = 400
    reason = 'Bad Request'


def run_main(monkeypatch, tmp_path, raises=None, env=None, tools_ok=True):
    monkeypatch.chdir(tmp_path)
    for k in ('DISCORD_TOKEN', 'PREFIX', 'PREFIX_COMMANDS', 'MAX_MB'):
        monkeypatch.delenv(k, raising=False)
    for k, v in (env or {}).items():
        monkeypatch.setenv(k, v)
    monkeypatch.setattr(B, 'check_tools', lambda: [('luau', tools_ok, 'x')] if tools_ok else [('luau', False, 'not found')])

    class Stub(object):
        def __init__(self, cfg):
            self.cfg = cfg

        def run(self, token, log_handler=None):
            if raises is not None:
                raise raises
    monkeypatch.setattr(B, 'DeobfClient', Stub)
    return B.main()


@pytest.mark.parametrize('raises,needle', [
    (discord.LoginFailure('bad token'), 'Discord rejected the token'),
    (discord.PrivilegedIntentsRequired(None), 'MESSAGE CONTENT INTENT'),
    (OSError('no route'), "Couldn't connect to Discord"),
    (discord.HTTPException(R(), 'nope'), 'Discord rejected the request'),
])
def test_connection_errors_are_explained(monkeypatch, tmp_path, capsys, raises, needle):
    code = run_main(monkeypatch, tmp_path, raises=raises, env={'DISCORD_TOKEN': 'x.y.z'})
    err = capsys.readouterr().err
    assert code == 1 and needle in err


def test_privileged_intent_message_offers_the_way_out(monkeypatch, tmp_path, capsys):
    run_main(monkeypatch, tmp_path, raises=discord.PrivilegedIntentsRequired(None), env={'DISCORD_TOKEN': 'x', 'PREFIX': '?'})
    err = capsys.readouterr().err
    assert 'PREFIX_COMMANDS=0' in err and '?uwu' in err


def test_missing_token_bad_config_and_missing_tools(monkeypatch, tmp_path, capsys):
    assert run_main(monkeypatch, tmp_path) == 1
    assert 'DISCORD_TOKEN is not set' in capsys.readouterr().err
    assert run_main(monkeypatch, tmp_path, env={'DISCORD_TOKEN': 'x', 'MAX_MB': '99'}) == 1
    assert 'Invalid configuration' in capsys.readouterr().err
    assert run_main(monkeypatch, tmp_path, env={'DISCORD_TOKEN': 'x'}, tools_ok=False) == 1
    assert 'Problem with a Luau tool' in capsys.readouterr().err


def test_clean_exit_when_the_client_stops(monkeypatch, tmp_path):
    assert run_main(monkeypatch, tmp_path, env={'DISCORD_TOKEN': 'x'}) == 0

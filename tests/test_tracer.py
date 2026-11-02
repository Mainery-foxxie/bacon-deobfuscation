"""Tests of the fake Roblox sandbox: tracer.py (the Python side) and bgdeobf/lua/*.lua (the pretend Roblox).

The first group needs no Luau at all (log parsing, the report, options).  The second group runs small scripts in the real
sandbox process and is skipped when the luau binary is missing.  The third group throws hostile scripts at it (endless
loops, memory bombs, escape attempts, forged log lines).  The last group needs the protected scripts of tests/samples/ (or
$BGDEOBF_SAMPLES) and is skipped without them."""
import glob
import hashlib
import json
import os
import re
import shutil
import subprocess
import tempfile
import time

import pytest

from conftest import sample_files
from bgdeobf import messages, tracer
from bgdeobf.astutil import LuauMissing, find_bin
from bgdeobf.sandbox import live_supported, run_luau_live


def _have(name):
    try:
        find_bin(name)
        return True
    except LuauMissing:
        return False


needs_luau = pytest.mark.skipif(not _have('luau'), reason='the luau binary was not found')
needs_pty = pytest.mark.skipif(not live_supported(), reason='this host has no pseudo terminal')
SAMPLES = sample_files()
needs_sample = pytest.mark.skipif(not SAMPLES, reason='no protected scripts in tests/samples/')


def trace(src, **opts):
    return tracer.run(src, tracer.TraceOptions(**opts))


def of(r, *kinds):
    return [e for e in r.events if e.kind in kinds]


def texts(r, *kinds):
    return [e.text for e in of(r, *kinds)]


def printed(r):
    """what the script printed, one string per print call"""
    return [e.a for e in of(r, 'out')]


def wire(*lines):
    return b''.join(b'\x01' + l + b'\n' for l in lines)


def ev(time, line, kind, text, a=None, b=None):
    return tracer.Event(time, line, kind, text, a, b)


def synthetic(events=(), payloads=(), repeats=(), end=None, status='ok', stderr='', source=b'print(1)\n'):
    return tracer.TraceResult(list(events), list(payloads), list(repeats), end, status, stderr, 0.5, source)


# ============================================================================================ no Luau needed

def test_lua_str_is_plain_ascii_without_control_characters():
    s = tracer.lua_str(bytes(range(256)))
    assert s[0] == s[-1] == '"'
    assert all(32 <= ord(c) < 127 for c in s)


def test_lit_writes_python_values_as_luau():
    assert (tracer.lit(True), tracer.lit(False), tracer.lit(None)) == ('true', 'false', 'nil')
    assert tracer.lit(5) == '5' and tracer.lit(1.5) == '1.5'
    assert tracer.lit('a"b') == '"a\\"b"' and tracer.lit(b'\n') == '"\\n"'
    assert tracer.lit([1, 'x']) == '{1, "x"}' and tracer.lit({'k': False}) == '{["k"] = false}'
    for bad in (float('nan'), float('inf')):
        with pytest.raises(ValueError):
            tracer.lit(bad)
    with pytest.raises(TypeError):
        tracer.lit(object())


def test_options_travel_as_plain_values():
    o = tracer.TraceOptions(place_id=5, responses={'u': 'x'}, explore=True, seed=3)
    d = o.to_dict()
    assert json.loads(json.dumps(d)) == d
    assert tracer.TraceOptions.from_dict(dict(d, bogus=1)).to_dict() == d
    assert tracer.TraceOptions.from_dict(None).to_dict() == tracer.TraceOptions().to_dict()


def test_config_only_carries_what_was_set():
    cfg = tracer.TraceOptions().config(b'print(1)')
    assert cfg['source'] == b'print(1)' and cfg['chunkname'] == 'script'
    assert 'responses' not in cfg and 'explore' not in cfg and 'pad' not in cfg
    cfg = tracer.TraceOptions(explore=True, responses={'a': 'b'}, username='U').config(b'x', pad=4096)
    assert cfg['explore'] is True and cfg['responses'] == {'a': 'b'} and cfg['username'] == 'U' and cfg['pad'] == 4096


def test_parse_log_reads_events_payloads_repeats_and_the_end():
    data = wire(b'T\t1.500@12\tnet\tgame:HttpGet("u")\tu\tGET',
                b'T\t2.000\tout\tprint("x")',
                b'P\t1\tloadstring\tlbl\t5\tab\\x0Acd',
                b'S\t40\tout\tprint(#)\t0.100\t2.000',
                b'X\tdone\tevents=3\tshown=2\tcut=0\tsteps=7 time=2.000')
    events, payloads, repeats, end = tracer.parse_log(data)
    e = events[0]
    assert (e.time, e.line, e.kind, e.text, e.a, e.b) == (1.5, '12', 'net', 'game:HttpGet("u")', 'u', 'GET')
    assert events[1].line is None and events[1].a is None
    assert (payloads[0].id, payloads[0].kind, payloads[0].length, payloads[0].data) == (1, 'loadstring', 5, b'ab\ncd')
    assert (repeats[0].times, repeats[0].kind, repeats[0].first, repeats[0].last) == (40, 'out', 0.1, 2.0)
    assert end == {'reason': 'done', 'events': '3', 'shown': '2', 'cut': '0', 'steps': '7', 'time': '2.000'}


def test_parse_log_ignores_noise_short_lines_and_a_cut_last_line():
    data = (b'padding without the marker\n' + b'text that mentions \\x01T but does not start with it\n' +
            b'\x01T\tshort\n' + b'\x01Z\tunknown tag\n' + b'\x01P\tnot-a-number\tk\tl\t5\tdata\n' +
            wire(b'T\t0.100\tout\tgood') + b'\x01T\t0.200\tout\tcut off in the middle')
    events, payloads, repeats, end = tracer.parse_log(data)
    assert [e.text for e in events] == ['good'] and not payloads and not repeats and end is None


def test_parse_log_escapes_roundtrip():
    events, _, _, _ = tracer.parse_log(wire(b'T\t0.000\tout\ta\\x0Ab\\\\n\\t\\x01\\xFF'))
    assert events[0].text == 'a\nb\\n\t\x01\ufffd'          # \xFF alone is not UTF-8


def test_vis_writes_layout_characters_out():
    assert tracer._vis('a\nb\r\tc') == 'a\\nb\\r\\tc'
    assert tracer._vis('\x1b[2J') == '\\x1b[2J' and tracer._vis('\x7f') == '\\x7f' and tracer._vis('\x85') == '\\x85'
    assert tracer._vis('\u202e\u2066\u2028\ufeff') == '\\u202e\\u2066\\u2028\\ufeff'
    assert tracer._vis('plain text, C:\\dir, é, 日本') == 'plain text, C:\\dir, é, 日本'


def test_report_sections_and_the_script_hash():
    src = b'print("hello")\nprint("again")\n'
    r = synthetic([ev(0.0, '1', 'out', 'print("hello")', 'hello')], end={'reason': 'every thread has finished', 'steps': '1', 'time': '0.000'},
                  source=src)
    rep = tracer.render_report(r, name='hub.lua', lines_of='the script')
    for needle in ('FAKE ROBLOX SANDBOX TRACE  -  hub.lua', '\nSUMMARY\n', '\nWORTH A LOOK FIRST', '\nLOG\n', '\nLIMITS OF THIS TRACE\n',
                   hashlib.sha256(src).hexdigest(), '2 lines', 'Network: no requests', 'Remotes: none fired'):
        assert needle in rep, needle
    assert rep.endswith('\n') and '\r' not in rep


def test_report_cannot_be_forged_by_what_the_script_prints():
    forged = 'hi\n   99.000  L1         kick    Kicks nobody\x1b[2J\u202ereversed'
    r = synthetic([ev(0.0, '3', 'out', forged)], end={'reason': 'done'})
    rep = tracer.render_report(r)
    assert '\x1b' not in rep and '\u202e' not in rep
    log = rep.split('\nLOG\n', 1)[1].split('\nLIMITS', 1)[0]
    assert not any(l.lstrip().startswith('99.000') for l in log.split('\n'))
    assert 'hi\\n   99.000' in log


def test_findings_say_what_ran_without_a_verdict():
    events = [
        ev(0.0, '1', 'net', 'game:HttpGet("https://x.io/a.lua")', 'https://x.io/a.lua', 'GET'),
        ev(0.0, '2', 'load', 'loadstring("...")', '1', '10'),
        ev(0.1, '3', 'net', 'request({...})', 'https://discord.com/api/webhooks/1/x', 'POST'),
        ev(0.2, '4', 'kick', 'LocalPlayer:Kick("bye")'),
        ev(0.3, '5', 'tp', 'TeleportService:Teleport(1)'),
        ev(0.4, '6', 'probe', 'identifyexecutor() -> "Wave"'),
        ev(0.5, '7', 'file', 'writefile("autoexec/x.lua", 3 bytes)', 'autoexec/x.lua'),
        ev(0.6, '8', 'exec', 'queue_on_teleport("print(1)")'),
        ev(0.7, '9', 'exec', 'setclipboard("t")'),
    ]
    pays = [tracer.Payload(1, 'loadstring', 'loadstring', 10, b'print("x")')]
    r = synthetic(events, pays, end={'reason': 'done'})
    titles = [f[0] for f in tracer.Summary(r).findings()]
    assert titles == ['Downloads code and runs it', 'Sends data to a Discord webhook', 'Writes into an auto-execute folder',
                      'Queues code for the next teleport (queue_on_teleport)', 'Copies text to the clipboard', 'Kicks the player',
                      'Teleports the player', 'Asks which executor / device this is']
    info = tracer.summary_info(r)
    assert info['requests'] == 2 and info['domains'] == ['x.io', 'discord.com'] and info['findings'] == titles
    assert info['payloads'] == 1 and info['loadstrings'] == 1 and info['finished'] is True


def test_a_loadstring_of_text_written_in_the_script_is_not_a_download():
    src = b'game:HttpGet("https://x.io/a.lua")\nloadstring(\'print("x")\')()\n'
    events = [ev(0.0, '1', 'net', 'game:HttpGet("https://x.io/a.lua")', 'https://x.io/a.lua', 'GET'),
              ev(0.0, '2', 'load', 'loadstring("...")', '1', '10')]
    r = synthetic(events, [tracer.Payload(1, 'loadstring', 'loadstring', 10, b'print("x")')], end={'reason': 'done'}, source=src)
    assert tracer.Summary(r).findings() == []


def test_the_same_finding_again_is_counted_not_repeated():
    events = [ev(float(i), '5', 'exec', 'queue_on_teleport("print(1)")') for i in range(3)]
    r = synthetic(events, end={'reason': 'done'}, repeats=[tracer.Repeat(57, 'exec', 'queue_on_teleport("print(1)")', 3.0, 60.0)])
    (title, _, _, count), = tracer.Summary(r).findings()
    assert title.startswith('Queues code') and count == 60


def test_limits_tell_when_the_run_did_not_finish():
    r = synthetic(status='timeout')
    rep = tracer.render_report(r)
    assert 'Ended: the real-time limit was reached' in rep and 'the end of the log is missing' in rep
    r = synthetic(status='crashed', stderr='runtime line 3: boom\nmore')
    assert tracer.summary_info(r)['reason'] == 'the sandbox process stopped unexpectedly'
    assert 'boom' in tracer.render_report(r) and 'boom' not in tracer.summary_info(r)['reason']


def test_summary_info_has_no_text_from_the_script_except_fixed_titles():
    events = [ev(0.0, '1', 'net', 'x', 'http://**evil**<@1>/x', 'GET'), ev(0.0, '2', 'out', '@everyone')]
    info = tracer.summary_info(synthetic(events, end={'reason': 'every thread has finished'}))
    assert info['domains'] == ['**evil**<@1>'] and info['findings'] == []     # kept as found: messages.host_list filters it


@pytest.mark.parametrize('name,ok', [('example.com', True), ('a-b.c0.example.org', True), ('xn--80ak6aa92e.com', True), ('**b**', False),
                                     ('<@123>', False), ('@everyone', False), ('a`b', False), ('a b', False), ('[::1]', False), ('', False)])
def test_host_list_repeats_only_plain_host_names(name, ok):
    out = messages.host_list([name], markdown=True)
    assert (('`%s`' % name) in out) == ok
    if not ok:
        assert name not in out or name == ''


def test_host_list_counts_the_ones_it_leaves_out_and_shortens_long_lists():
    assert messages.host_list(['a.io', '@x', '<@1>'], markdown=True) == '`a.io` and 2 host name(s) with unusual characters'
    assert messages.host_list(['@x'], markdown=False) == '1 host name(s) with unusual characters'
    assert messages.host_list([]) == 'unknown hosts'
    assert messages.host_list(['a.io', 'b.io', 'c.io', 'd.io', 'e.io']) == 'a.io, b.io, c.io, d.io ...'


def test_trace_lines_for_the_chat_and_the_command_line():
    t = dict(events=40, repeated=0, requests=2, domains=['api.example.com', '*x*'], remotes=1, payloads=1, loadstrings=1, errors=1,
             files_written=0, instances=3, findings=['Kicks the player', 'Kicks the player'], finished=False,
             reason='the real-time limit was reached', simulated=1, real_seconds=1, status='timeout')
    md = messages.trace_lines(t, markdown=True)
    assert md[0] == ('Fake Roblox run: 40 events, 2 request(s) to `api.example.com` and 1 host name(s) with unusual characters, '
                     '1 remote call(s), 1 text(s) captured (loadstring / files / requests), 3 instances created, 1 error(s)')
    assert md[1] == 'Worth a look: Kicks the player' and md[2].endswith('(the log may be incomplete)')
    assert '`' not in messages.trace_lines(t)[0]


# ============================================================================================ small scripts in the sandbox

@needs_luau
def test_everything_a_script_does_is_written_down_with_its_line():
    r = trace('''
print("hello", 1, true)
warn("careful")
local remote = game:GetService("ReplicatedStorage"):WaitForChild("Events"):WaitForChild("Buy")
remote:FireServer("sword", 5)
local body = game:HttpGet("https://example.com/a.lua")
loadstring(body)
task.wait(2.5)
game:GetService("Players").LocalPlayer:Kick("bye")
writefile("test.txt", "data")
print(readfile("test.txt"))
local gui = Instance.new("ScreenGui")
gui.Name = "MyGui"
gui.Parent = game:GetService("CoreGui")
error("boom")
''')
    assert r.status == 'ok' and r.finished
    by_line = lambda kind: [(e.line, e.time) for e in of(r, kind)]
    assert printed(r)[:2] == ['hello 1 true', 'careful']
    assert [(e.line, e.text, e.a, e.b) for e in of(r, 'remote')] == [
        ('5', 'ReplicatedStorage:WaitForChild("Events"):WaitForChild("Buy"):FireServer("sword", 5)', 'ReplicatedStorage.Events.Buy', 'FireServer')]
    assert [(e.line, e.a, e.b) for e in of(r, 'net')] == [('6', 'https://example.com/a.lua', 'GET')]
    assert by_line('kick') == [('9', 2.5)]                           # virtual time: 2.5 s passed without any waiting
    assert [e.text for e in of(r, 'file')] == ['writefile("test.txt", 4 bytes)', 'readfile("test.txt") --> 4 bytes']
    assert any(e.line == '11' and e.a == 'data' for e in of(r, 'out'))
    assert any('Instance.new("ScreenGui")' in e.text for e in of(r, 'def'))
    err, = of(r, 'err')
    assert err.line == '15' and err.text == 'error: script:15: boom' and err.b is None
    assert r.seconds < 10


@needs_luau
def test_the_same_script_gives_the_same_log_every_time():
    src = 'math.randomseed(1)\nfor i = 1, 5 do print(math.random(1, 1000), os.clock(), tick() > 0) task.wait(i) end\nprint(game.JobId)'
    a = [(e.time, e.line, e.kind, e.text) for e in trace(src).events]
    b = [(e.time, e.line, e.kind, e.text) for e in trace(src).events]
    assert a == b and len(a) == 7                          # five prints, the read of game.JobId, and the last print


@needs_luau
def test_time_is_simulated():
    t0 = time.time()
    r = trace('local a = os.time()\ntask.wait(1000)\nprint(os.time() - a)\nprint(os.clock() > 0, tick() > 0)\nwait(5)', max_time=2000)
    assert time.time() - t0 < 10                           # a wait of 1000 seconds takes no real time
    assert printed(r) == ['1000', 'true true'] and r.events[0].time == 1000.0
    assert r.end['time'] == '1005.000' and r.end['reason'].startswith('every thread has finished')
    r = trace('task.wait(1000)\nprint("late")')            # with the default limit of 30 simulated seconds the wait never ends
    assert printed(r) == [] and 'simulated time limit of 30 s' in r.reason


@needs_luau
def test_threads_run_in_the_order_roblox_would_run_them():
    r = trace('''task.spawn(function() task.wait(3) print("spawned") end)
task.delay(5, function() print("delayed") end)
task.defer(function() print("deferred") end)
print("main")''')
    assert [(e.time, e.a) for e in of(r, 'out')] == [(0.0, 'main'), (0.0, 'deferred'), (3.0, 'spawned'), (5.0, 'delayed')]


@needs_luau
def test_a_loop_that_waits_ends_at_the_simulated_time_limit():
    r = trace('while true do task.wait(1) end')
    assert r.finished and 'simulated time limit of 30 s' in r.reason and r.end['time'] == '30.000'
    assert r.seconds < 10
    r = trace('while true do task.wait(1) end', max_time=5)
    assert 'limit of 5 s' in r.reason and r.end['time'] == '5.000'


@needs_luau
def test_a_script_with_only_frame_handlers_left_is_done_after_the_settle_time():
    r = trace('game:GetService("RunService").Heartbeat:Connect(function() end)\nprint("armed")')
    assert 'only frame handlers' in r.reason and r.end['time'] and float(r.end['time']) < 11
    assert printed(r) == ['armed'] and texts(r, 'conn') == ['RunService.Heartbeat:Connect(function() --[[line 1]] end)']


@needs_luau
def test_the_step_limit_stops_a_thread_storm():
    r = trace('for i = 1, 10 do task.spawn(function() while true do task.wait() end end) end', max_steps=500)
    assert 'step limit of 500' in r.reason and r.finished


@needs_luau
def test_http_answers_come_from_the_responses_option_and_text_is_captured_for_loadstring():
    src = ('local code = game:HttpGet("https://example.com/hub.lua")\nloadstring(code)()\n'
           'print(game:HttpGet("https://api.example.com/v1/x?id=5"))\nprint(#game:HttpGet("https://nothing.example/"))')
    r = trace(src, responses={'https://example.com/hub.lua': 'print("payload ran")\nlocal p = game:GetService("Players")',
                              'https://api.example.com/*': 'PREFIX-ANSWER'})
    assert printed(r) == ['payload ran', 'PREFIX-ANSWER', '0']
    payload, = r.payloads
    assert payload.kind == 'loadstring' and payload.data == b'print("payload ran")\nlocal p = game:GetService("Players")'
    ran, = [e for e in of(r, 'out') if e.a == 'payload ran']
    assert ran.line == 'loadstring:1'                                # lines inside a payload say so
    assert [f[0] for f in tracer.Summary(r).findings()] == ['Downloads code and runs it']


@needs_luau
def test_an_error_inside_loadstring_code_stops_the_caller_like_in_roblox():
    r = trace('local f = loadstring("local x = 1\\nerror(\\"inner\\")")\nprint("before")\nf()\nprint("never")')
    assert printed(r) == ['before']
    err, = of(r, 'err')
    assert err.line == 'loadstring:2' and 'inner' in err.text


@needs_luau
def test_requests_are_answered_not_sent_and_the_body_is_kept():
    long_text = 'a long text for the clipboard ' * 10                 # 300 characters: more than an event line shows
    r = trace('local r = request({Url = "https://discord.com/api/webhooks/123/abc", Method = "POST", Body = \'{"content":"hi"}\'})\n'
              'print(r.StatusCode)\nqueue_on_teleport(\'print("again")\')\nsetclipboard("copied")\nsetclipboard("%s")' % long_text)
    assert printed(r) == ['204']
    assert [(e.a, e.b) for e in of(r, 'net')] == [('https://discord.com/api/webhooks/123/abc', 'POST')]
    # what an event line can show whole stays in the line; the rest is kept beside the log as a payload
    assert 'setclipboard("copied")' in texts(r, 'exec')
    assert sorted((p.kind, p.data) for p in r.payloads) == [('body', b'{"content":"hi"}'), ('clipboard', long_text.encode()),
                                                            ('queue_on_teleport', b'print("again")')]
    titles = [f[0] for f in tracer.Summary(r).findings()]
    assert 'Sends data to a Discord webhook' in titles and 'Copies text to the clipboard' in titles


@needs_luau
def test_the_same_payload_again_is_one_payload():
    r = trace('for i = 1, 40 do queue_on_teleport("print(1)") end')
    assert len(r.payloads) == 1
    (title, _, _, count), = tracer.Summary(r).findings()
    assert title.startswith('Queues code') and count == 40          # shown a few times, counted every time
    assert len(of(r, 'exec')) < 40 and r.repeats and r.repeats[0].times == 40 - len(of(r, 'exec'))
    # the same with a pause between the calls: the 30 simulated seconds end it after 31 calls (at 0, 1, ... 30 s)
    r = trace('for i = 1, 40 do queue_on_teleport("print(1)") task.wait(1) end')
    (title, _, _, count), = tracer.Summary(r).findings()
    assert len(r.payloads) == 1 and count == 31 and 'simulated time limit of 30 s' in r.reason


@needs_luau
def test_the_pretend_world_follows_the_options():
    r = trace('print(game.PlaceId, game.GameId, game.JobId)\nlocal p = game.Players.LocalPlayer\nprint(p.Name, p.UserId, identifyexecutor())\n'
              'print(readfile("cfg.json"), isfile("nope.txt"))',
              place_id=4321, game_id=99, job_id='JOB', username='Tester', user_id=7, executor='Synapse', executor_version='9',
              files={'cfg.json': '{"k":1}'})
    assert printed(r) == ['4321 99 JOB', 'Tester 7 Synapse 9', '{"k":1} false']


@needs_luau
def test_kick_teleport_and_remotes_are_events_of_their_own_kind():
    r = trace('local p = game.Players.LocalPlayer\ngame:GetService("TeleportService"):Teleport(123456, p)\np:Kick("x")\n'
              'game.ReplicatedStorage.Remote:FireServer(1)')
    assert [e.line for e in of(r, 'tp')] == ['2'] and [e.line for e in of(r, 'kick')] == ['3'] and [e.line for e in of(r, 'remote')] == ['4']
    titles = [f[0] for f in tracer.Summary(r).findings()]
    assert 'Kicks the player' in titles and 'Teleports the player' in titles


@needs_luau
def test_questions_about_the_environment_are_written_down_and_answered_with_defaults():
    r = trace('print(identifyexecutor())\nprint(os.execute, io, package)\nprint(pcall(os.execute, "id"))\nprint(someGlobal)')
    probes = texts(r, 'probe')
    assert any(t.startswith('identifyexecutor()') for t in probes)
    assert any('os.execute is not available' in t for t in probes)
    assert any('someGlobal' in t for t in probes)
    assert printed(r)[1:] == ['nil nil nil', 'false attempt to call a nil value', 'nil']


@needs_luau
def test_windows_only_exist_where_the_script_makes_them():
    r = trace('local pg = game.Players.LocalPlayer.PlayerGui\nprint(pg:FindFirstChild("NotThere"))\nprint(game.CoreGui:FindFirstChild("RobloxGui"))\n'
              'local sg = Instance.new("ScreenGui", pg)\nsg.Name = "Mine"\nprint(pg:FindFirstChild("Mine") == sg)')
    assert printed(r) == ['nil', 'RobloxGui', 'true']


@needs_luau
def test_values_the_sandbox_cannot_know_absorb_what_the_script_does_with_them():
    r = trace('''
local m = require(game.ReplicatedStorage.Module)
local function try(name, f) local ok, err = pcall(f) print(name, ok and "ok" or ("ERR " .. tostring(err))) end
try("concat", function() return "x" .. m.name .. 1 end)
try("math", function() return math.floor(m.Value) + math.max(m.a, 5) + math.clamp(m.a, 0, 10) end)
try("format", function() return string.format("%d %s %.2f", m.n, m.n, m.n) end)
try("methods", function() return m.name:sub(1, 3), m.str:split(","), m.str:gsub("a", "b") end)
try("tables", function() table.insert(m.list, 1) return table.concat(m.list, ","), table.find(m.list, 1), #m.list end)
try("loops", function() for i, v in ipairs(m.list) do end for k, v in pairs(m.list) do end end)
try("calls", function() return m.fn(1, 2), m:Foo(1), m.a.b.c.d end)
try("assign", function() m.a.b = 5 end)
try("vectors", function() return Vector3.new(m.a, m.b, m.c), UDim2.new(m.a, 0, m.b, 0), Color3.fromRGB(m.a, m.b, m.c) end)
try("random", function() return math.random(1, m.n), string.rep("x", m.n) end)
try("tostring", function() return tostring(m.n), tonumber(m.n), type(m.n), select("#", m.n) end)
try("wait", function() task.wait(m.a) end)
''')
    assert not of(r, 'err')
    assert [p for p in printed(r) if ' ERR ' in p] == []
    assert len(printed(r)) == 12


@needs_luau
def test_the_limits_of_ghosts_are_the_limits_of_luau():
    r = trace('local m = require(game.ReplicatedStorage.Module)\nprint(pcall(function() return m.a < 5 end))\n'
              'print(pcall(function() for i = 1, m.n do end end))')
    assert all(p.startswith('false') for p in printed(r))        # Luau itself refuses these; the report says so


@needs_luau
def test_an_error_in_the_script_and_an_error_in_the_sandbox_are_told_apart():
    r = trace('local x = nil\nprint(x.y)')
    err, = of(r, 'err')
    assert err.line == '2' and err.b is None and 'attempt to index nil' in err.text
    rep = tracer.render_report(r)
    assert 'raised inside the fake environment' not in rep and 'The script stopped with an error here' in rep
    r = trace('string.rep()')
    err, = of(r, 'err')
    assert err.line == '1' and err.text.startswith('error: script:1: invalid argument #1')       # a library error is the script's


@needs_luau
def test_a_syntax_error_is_reported_and_the_run_ends_normally():
    r = trace('local x = = 1\nprint(1)')
    err, = of(r, 'err')
    assert err.text.startswith('syntax error: script:1:') and r.reason == 'the script has a syntax error' and r.status == 'ok'


@needs_luau
def test_explore_presses_the_buttons_the_script_made():
    src = 'local b = Instance.new("TextButton")\nb.MouseButton1Click:Connect(function() print("clicked") game.Players.LocalPlayer:Kick("x") end)'
    r = trace(src)
    assert printed(r) == [] and not of(r, 'kick')
    r = trace(src, explore=True)
    assert printed(r) == ['clicked'] and len(of(r, 'kick')) == 1 and 'click handlers were called' in r.reason


@needs_luau
def test_repeats_are_counted_and_the_log_is_capped():
    r = trace('for i = 1, 1000 do print("same") end')
    assert len(r.events) < 50 and sum(rep.times for rep in r.repeats) == 1000 - len(r.events)
    assert r.end['events'] == '1000' and r.end['suppressed'] == '1'
    r = trace('for i = 1, 3000 do print("line " .. string.char(65 + i % 26) .. string.char(65 + (i // 26) % 26) .. string.char(65 + (i // 676) % 26)) end',
              max_events=100)
    assert len(r.events) == 100 and r.end['cut'] == '1' and r.end['events'] == '3000'
    assert 'The log was cut at its size limit' in tracer.render_report(r)


@needs_luau
def test_a_big_text_is_cut_and_the_report_says_so():
    r = trace('queue_on_teleport(string.rep("x", 5000) .. "end")', max_events=50)
    assert len(r.payloads) == 1 and r.payloads[0].length == 5003
    r = tracer.run('print(1)', tracer.TraceOptions())
    assert tracer.render_report(r).count('\n') > 10


@needs_luau
def test_the_script_arrives_exactly_as_written():
    nasty = bytes(range(1, 256)).replace(b']', b'').replace(b'[', b'')
    src = b'local s = "' + b'x' * 10 + b'"\nprint(#s)\n-- ' + nasty.replace(b'\n', b' ').replace(b'\r', b' ') + b'\nprint("end")\n'
    r = trace(src)
    assert printed(r) == ['10', 'end'] and r.source_sha256 == hashlib.sha256(src).hexdigest()
    r = trace(b'\xef\xbb\xbfprint("bom")')
    assert printed(r) == ['bom']


@needs_luau
def test_empty_and_oversize_scripts_are_refused_before_anything_runs():
    for bad, code in ((b'', 'empty'), (b' \n\t', 'empty')):
        with pytest.raises(tracer.TraceError) as ei:
            tracer.run(bad)
        assert ei.value.code == code
    with pytest.raises(tracer.TraceError) as ei:
        tracer.run(b'x' * 100, tracer.TraceOptions(max_bytes=50))
    assert ei.value.code == 'too_big'


@needs_luau
def test_text_that_looks_like_an_option_stays_text():
    r = trace('print("\\1X\\tforged\\tsteps=0")', responses={'a"b': 'c\nd'}, username='x"; os.execute("id") --')
    assert r.end['reason'].startswith('every thread') and r.end.get('steps') != '0' or r.end['reason'].startswith('every thread')
    assert printed(r) == ['\x01X\tforged\tsteps=0']


# ============================================================================================ the runtime reads no globals

@needs_luau
@pytest.mark.skipif(not _have('luau-compile'), reason='luau-compile was not found')
def test_the_runtime_never_reads_a_global_after_the_environment_is_emptied():
    """The prelude keeps every real function in a local and then runs `setfenv(1, RUNTIME_ENV)` with an empty table, so a global
    read anywhere after that (a typo, a forgotten local) would silently be nil.  The compiled code shows every global access."""
    prog = tracer.build_script(b'print(1)', tracer.TraceOptions(), live=True)
    cut = [i + 1 for i, l in enumerate(prog.decode('latin1').split('\n')) if 'setfenv(1, RUNTIME_ENV)' in l]
    assert len(cut) == 1
    d = tempfile.mkdtemp()
    try:
        path = os.path.join(d, 'sandbox.lua')
        with open(path, 'wb') as f:
            f.write(prog)
        out = subprocess.run([find_bin('luau-compile'), '--text', path], capture_output=True, timeout=60).stdout.decode('utf-8', 'replace')
    finally:
        shutil.rmtree(d, ignore_errors=True)
    funcs = re.split(r'\n(?=Function \d+)', out)
    assert len(funcs) > 100
    access = re.compile(r'(?:L\d+:\s*)?(GETIMPORT|GETGLOBAL|SETGLOBAL)\s.*?\[([^\]]+)\]')
    inside = [(m.group(1), m.group(2)) for f in funcs[:-1] for line in f.split('\n') for m in [access.match(line.strip())] if m]
    assert inside == []                        # no function of the runtime reads a global
    line, late = None, []
    for text in funcs[-1].split('\n'):         # the main chunk: only the prelude (before the setfenv line) may
        m = re.match(r'\s*(\d+):', text)
        if m:
            line = int(m.group(1))
            continue
        m = access.match(text.strip())
        if m and line is not None and line > cut[0]:
            late.append((line, m.group(2)))
    assert late == []


# ============================================================================================ hostile scripts

HOSTILE = r'''
local results = {}
local function try(name, f)
    local ok, a = pcall(f)
    results[#results + 1] = name .. "=" .. (ok and "ran" or "err") .. ":" .. tostring(a):sub(1, 60)
end
try("require rel", function() return require("./sandbox") end)
try("require up", function() return require("../../../../etc/hosts") end)
try("require abs", function() return require("/etc/passwd") end)
try("os.execute", function() return os.execute("id") end)
try("io", function() return io.open("/etc/passwd") end)
try("package", function() return package.loaded end)
try("dofile", function() return dofile("/etc/passwd") end)
try("loadfile", function() return loadfile("/etc/passwd") end)
try("load", function() return load("return 1")() end)
try("os.getenv", function() return os.getenv("HOME") end)
try("loadstring os", function() return loadstring("return os.execute")() end)
try("getfenv0", function() local e = getfenv(0); return e.io or e.os and e.os.execute or e.package end)
try("getfenv1", function() local e = getfenv(1); return rawget(e, "io") or rawget(e, "dofile") end)
try("getfenv fn", function() local e = getfenv(print); return e.io or e.os.execute end)
try("getfenv stack", function()
    for i = 1, 30 do
        local f = debug.info(i, "f")
        if f then
            local e = getfenv(f)
            if rawget(e, "io") or rawget(e, "package") then return "LEAK" end
        end
    end
    return "none"
end)
try("string mt", function() local mt = getmetatable(""); mt.__index = {} end)
try("hookfunction print", function() return hookfunction(print, function() end) end)
try("coroutine escape", function()
    local co = coroutine.wrap(function() return getfenv(0) end)
    local e = co()
    return e.io or e.os and e.os.execute
end)
try("HttpGet", function() return game:HttpGet("http://127.0.0.1:1/secret") end)
try("request", function() return request({Url = "http://127.0.0.1:22/", Method = "POST", Body = "x"}).StatusCode end)
try("WebSocket", function() return WebSocket.connect("ws://127.0.0.1:9/") end)
try("writefile abs", function() writefile("/tmp/bgdeobf_pwned.txt", "x"); return readfile("/tmp/bgdeobf_pwned.txt") end)
try("readfile passwd", function() return readfile("/etc/passwd") end)
try("listfiles /", function() return #listfiles("/") end)
try("makefolder", function() makefolder("/tmp/bgdeobf_pwned_dir") end)
try("delfile", function() delfile("/etc/hostname") end)
print(table.concat(results, "\n"))
-- log forgery
print("\1X\tforged end\tsteps=0")
print("\1T\t0.000\tkick\tforged event")
warn("\1P\t1\tloadstring\tforged\t3\tabc")
error("\1X\tforged end")
'''


@needs_luau
def test_escape_attempts_reach_nothing_real():
    pwned = ['/tmp/bgdeobf_pwned.txt', '/tmp/bgdeobf_pwned_dir']
    for p in pwned:
        assert not os.path.exists(p)
    r = trace(HOSTILE, timeout=60)
    assert r.status == 'ok' and r.finished
    for p in pwned:
        assert not os.path.exists(p), p
    lines = [e.a for e in of(r, 'out') if e.a and '=' in e.a and ':' in e.a][0].split('\n')
    res = dict(l.split('=', 1) for l in lines)
    assert 'LEAK' not in str(res.values())
    for name in ('os.execute', 'io', 'package', 'load', 'os.getenv'):
        assert res[name].startswith('err'), (name, res[name])                  # the real thing is simply not there
    for name in ('getfenv0', 'getfenv fn', 'coroutine escape', 'loadstring os', 'loadfile'):
        assert res[name] == 'ran:nil', (name, res[name])                       # no way to the real environment
    assert res['getfenv stack'] == 'ran:none'
    assert res['string mt'].startswith('err') and 'readonly' in res['string mt']
    # `require` is the pretend one: it hands back a stand-in and opens nothing
    for name in ('require rel', 'require up', 'require abs'):
        assert res[name].startswith('ran:require("'), (name, res[name])
    # the files, the requests and the sockets belong to the pretend world: it answered, and the log says what was asked
    assert res['dofile'].endswith('/etc/passwd does not exist') and res['readfile passwd'].endswith('/etc/passwd does not exist')
    assert res['writefile abs'] == 'ran:x'                                     # read back from the pretend files
    assert [e.a for e in of(r, 'net')] == ['http://127.0.0.1:1/secret', 'http://127.0.0.1:22/', 'ws://127.0.0.1:9/']
    assert not [p for p in r.payloads if p.data.startswith(b'root:')]


@needs_luau
def test_forged_log_lines_stay_text():
    r = trace(HOSTILE, timeout=60)
    assert not of(r, 'kick')
    assert not [p for p in r.payloads if p.label == 'forged']
    assert r.end['reason'].startswith('every thread has finished') and 'forged' not in r.end['reason']
    assert int(r.end['events']) == len(r.events)                 # the forged lines are events like any other print, not extra ones
    forged = [e for e in r.events if '\x01' in e.text or '\x01' in (e.a or '')]
    assert len(forged) == 4 and all(e.kind in ('out', 'err') for e in forged)
    rep = tracer.render_report(r)
    assert '\x01' not in rep


@needs_luau
@needs_pty
def test_an_endless_loop_is_stopped_and_what_it_did_before_is_kept():
    before = set(glob.glob(os.path.join(tempfile.gettempdir(), 'bgdeobf_trace_*')))
    t0 = time.time()
    r = trace('print("before the loop")\ngame:HttpGet("https://example.com/x")\nwhile true do end', timeout=3)
    assert 3 <= time.time() - t0 < 12
    assert r.status == 'timeout' and not r.finished and 'real-time limit' in r.reason
    assert printed(r) == ['before the loop'] and len(of(r, 'net')) == 1
    assert 'The run did not finish normally' in tracer.render_report(r)
    assert set(glob.glob(os.path.join(tempfile.gettempdir(), 'bgdeobf_trace_*'))) == before       # nothing was left behind


@needs_luau
def test_without_a_pseudo_terminal_the_log_is_kept_by_padding_it():
    opts = tracer.TraceOptions(timeout=3)
    prog = tracer.build_script(b'print("before the loop")\nwhile true do end', opts, live=False)
    lr = run_luau_live(prog, timeout=3, live=False)
    events, _, _, end = tracer.parse_log(lr.out)
    assert lr.status == 'timeout' and [e.a for e in events if e.kind == 'out'] == ['before the loop'] and end is None


@needs_luau
def test_a_memory_bomb_ends_with_an_error_not_with_the_machine():
    r = trace('print("before")\nlocal t, i = {}, 0\nprint(pcall(function() while true do i = i + 1 t[i] = string.rep("x", 1000000 + i) end end))\nprint("after")',
              mem_mb=200, timeout=30)
    assert r.status == 'ok' and printed(r)[0] == 'before' and printed(r)[1] == 'false not enough memory' and printed(r)[-1] == 'after'
    assert r.seconds < 20
    r = trace('local s = "x"\nwhile true do s = s .. s end', mem_mb=200, timeout=30)
    err, = of(r, 'err')
    assert 'not enough memory' in err.text and r.seconds < 20


@needs_luau
def test_the_output_limit_stops_a_process_that_writes_too_much():
    lr = run_luau_live(b'for i = 1, 10000000 do print(string.rep("x", 100)) end', timeout=30, max_output_mb=1, live=live_supported())
    assert lr.status == 'output' and 1024 * 1024 <= len(lr.out) <= 2 * 1024 * 1024


@needs_luau
def test_the_cpu_limit_is_enforced_by_the_system_too():
    # a process that ignores the wall clock still dies: RLIMIT_CPU is timeout + 2 seconds
    lr = run_luau_live(b'local x = 0\nwhile true do x = x + 1 end', timeout=1, live=live_supported())
    assert lr.status in ('timeout', 'killed') and lr.seconds < 10


@needs_luau
def test_the_child_has_no_environment_no_stdin_and_a_private_folder():
    secret = 'BGDEOBF_TEST_SECRET'
    os.environ[secret] = 'value-that-must-not-leak'
    try:
        r = trace('print(os.getenv, os.getenv and os.getenv("BGDEOBF_TEST_SECRET"))\nprint(io, debug.getinfo)', timeout=20)
    finally:
        del os.environ[secret]
    assert 'value-that-must-not-leak' not in repr([e.text for e in r.events])
    assert printed(r)[0] == 'nil nil' and printed(r)[1].startswith('nil')


@needs_luau
@pytest.mark.skipif(not shutil.which('strace'), reason='strace is not installed')
def test_the_sandbox_process_makes_no_network_process_or_write_system_calls():
    """Run the hostile script under strace and read what the process asked the system for."""
    prog = tracer.build_script(HOSTILE.encode('latin1'), tracer.TraceOptions(), live=False)
    work = tempfile.mkdtemp()
    log = os.path.join(tempfile.mkdtemp(), 'strace.txt')
    try:
        with open(os.path.join(work, 'sandbox.lua'), 'wb') as f:
            f.write(prog)
        try:
            subprocess.run(['strace', '-f', '-o', log, '-e', 'trace=network,process,file,desc', '-s', '120', find_bin('luau'), 'sandbox.lua'],
                           cwd=work, stdin=subprocess.DEVNULL, capture_output=True, timeout=120, env={'PATH': '/usr/bin:/bin'})
        except subprocess.TimeoutExpired:
            pytest.fail('the hostile script did not finish')
        if not os.path.exists(log) or not os.path.getsize(log):
            pytest.skip('strace cannot trace processes here')
        calls = open(log, encoding='latin1').read().split('\n')
        assert os.listdir(work) == ['sandbox.lua']                       # nothing was created next to the script
    finally:
        shutil.rmtree(work, ignore_errors=True)
        shutil.rmtree(os.path.dirname(log), ignore_errors=True)
    body = [re.sub(r'^\d+\s+', '', c) for c in calls if c.strip()]
    names = [b.split('(')[0] for b in body]
    assert 'execve' in names and names.count('execve') == 1             # strace starts luau; luau starts nothing
    for bad in ('socket', 'connect', 'bind', 'listen', 'accept', 'accept4', 'sendto', 'sendmsg', 'recvfrom', 'fork', 'vfork', 'clone',
                'clone3', 'unlink', 'unlinkat', 'rename', 'renameat', 'renameat2', 'mkdir', 'mkdirat', 'rmdir', 'symlink', 'symlinkat',
                'link', 'linkat', 'chmod', 'fchmodat', 'truncate', 'ftruncate'):
        assert bad not in names, bad
    for b in body:
        if b.startswith(('open(', 'openat(')) and 'ENOENT' not in b:
            assert not re.search(r'O_(WRONLY|RDWR|CREAT|APPEND|TRUNC)', b), b
        assert '/etc/passwd' not in b and 'bgdeobf_pwned' not in b and '/etc/shadow' not in b, b


# ============================================================================================ real protected scripts

def _kinds_of_interest(r):
    return [(e.kind, e.text, e.a, e.b) for e in r.events if e.kind in ('net', 'remote', 'kick', 'tp', 'load', 'exec', 'file')]


@needs_luau
@needs_sample
@pytest.mark.parametrize('path', SAMPLES, ids=lambda p: os.path.basename(p))
def test_a_protected_script_runs_as_given_in_the_sandbox(path):
    """The raw VM script is run as it is: it has to start, log things, and end (or be stopped) without the sandbox failing."""
    with open(path, 'rb') as f:
        data = f.read()
    r = tracer.run(data)
    assert r.status in ('ok', 'timeout'), r.status
    assert r.events, 'the script did nothing the sandbox writes down'
    assert not [e for e in of(r, 'err') if e.b == 'internal'], texts(r, 'err')
    rep = tracer.render_report(r, name=os.path.basename(path), lines_of='the script as given')
    assert 'FAKE ROBLOX SANDBOX TRACE' in rep and r.source_sha256 in rep
    info = tracer.summary_info(r)
    assert info['events'] == len(r.events) and isinstance(info['findings'], list)


def _normal(e):
    """An event as it has to read the same in the protected script and in its deobfuscation.  Two things legitimately differ: the
    line numbers inside the description of a function (they are lines of different files) and clock readings with many decimals
    (the two runs take a slightly different number of steps)."""
    text = re.sub(r'--\[\[line \d+\]\]', '--[[line N]]', e.text)
    return e.kind, re.sub(r'\b\d+\.\d{4,}\b', 'T', text)


# `getgenv()._xx = 0` or `= 1`: a flag the protection layer sets for itself; the deobfuscated script has no protection layer
PROTECTOR_FLAG = re.compile(r'^getgenv\(\)\._\w+ = [01]$')


@needs_luau
@needs_sample
@pytest.mark.parametrize('path', SAMPLES, ids=lambda p: os.path.basename(p))
def test_the_deobfuscated_script_does_what_the_protected_one_does(path):
    """The strongest check there is.  The fidelity check of the engine compares a script with its own decode, so it cannot see
    something the decode lost (that is how table templates went missing in one build without any test failing).  Here the VM
    script and the deobfuscated source run in the same pretend world: what they do to the outside (requests, remotes, kicks,
    teleports, loadstrings, executor calls, files) has to be the same, in the same order, and so has everything else they log."""
    from collections import Counter
    from bgdeobf.engine import Options, deobfuscate
    with open(path, 'rb') as f:
        data = f.read()
    raw = tracer.run(data)
    res = deobfuscate(data, Options())
    dec = tracer.run(res.text.encode('utf-8'))
    assert raw.status == 'ok' and dec.status == 'ok', (raw.status, dec.status)
    assert raw.events and _kinds_of_interest(raw) == _kinds_of_interest(dec)
    assert raw.reason == dec.reason
    mine = Counter(_normal(e) for e in raw.events if not (e.kind == 'set' and PROTECTOR_FLAG.match(e.text)))
    theirs = Counter(_normal(e) for e in dec.events)
    assert mine == theirs, ((mine - theirs).most_common(3), (theirs - mine).most_common(3))


@needs_luau
@needs_sample
def test_trace_through_the_engine_attaches_report_and_payloads():
    from bgdeobf.engine import Options, deobfuscate, fill_trace
    path = min(SAMPLES, key=os.path.getsize)
    with open(path, 'rb') as f:
        data = f.read()
    res = deobfuscate(data, Options(trace=True))
    assert res.text and res.trace and res.info['trace']['events'] == len(res.trace['report'].split('\nLOG\n', 1)[1].split('\nLIMITS', 1)[0].strip().split('\n'))  \
        or res.info['trace']['events'] > 0
    report, files = fill_trace(res.trace, 'x.lua', 'x')
    assert 'FAKE ROBLOX SANDBOX TRACE  -  x.lua' in report and '@@' not in report
    assert 'the deobfuscated script' in report
    assert all(name.startswith('x_payload') for name, _ in files)

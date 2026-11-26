use bgdeobf::{moonveil, tracer};
use serde_json::json;

#[test]
fn trace_keeps_payload_and_refuses_forged_records() {
    let trace = tracer::run(
        br#"print("\1X\tforged\tsteps=0")
local body=game:HttpGet('https://example.com/a.lua')
loadstring(body)()
game.Players.LocalPlayer:Kick('bye')"#,
        &json!({"responses":{"https://example.com/a.lua":"print('payload ran')"}}),
    )
    .unwrap();
    assert_eq!(trace["summary"]["requests"], 1);
    assert_eq!(trace["summary"]["payloads"], 1);
    assert!(trace["summary"]["finished"].as_bool().unwrap());
    let report = trace["report"].as_str().unwrap();
    assert!(report.contains("Downloads code and runs it"));
    assert!(report.contains("Kicks the player"));
    assert!(!report.contains('\u{1}'));
    assert!(report.contains("@@NAME@@"));
    assert!(report.contains("@@STEM@@_payload1.lua"));
}

#[test]
fn moonveil_detection_is_explicit() {
    assert_eq!(
        moonveil::detect("-- This script was generated using MoonVeil v2.1.1"),
        Some(json!({"version":"2.1.1"}))
    );
    assert_eq!(
        moonveil::detect("-- https://moonveil.cc"),
        Some(json!({"version":"?"}))
    );
    assert_eq!(moonveil::detect("print(1)"), None);
}

fn trace(src: &str, opts: serde_json::Value) -> tracer::TraceResult {
    tracer::trace(src.as_bytes(), &opts).unwrap()
}
fn printed(r: &tracer::TraceResult) -> Vec<&str> {
    r.events
        .iter()
        .filter(|e| e.kind == "out")
        .map(|e| e.a.as_deref().unwrap_or(""))
        .collect()
}

#[test]
fn traces_preserve_lines_events_time_and_errors() {
    let r=trace("print('hello',1,true)\nwarn('careful')\nlocal r=game:GetService('ReplicatedStorage'):WaitForChild('Events'):WaitForChild('Buy')\nr:FireServer('sword',5)\ngame:HttpGet('https://example.com/a.lua')\ntask.wait(2.5)\ngame.Players.LocalPlayer:Kick('bye')\nwritefile('test.txt','data')\nprint(readfile('test.txt'))\nlocal gui=Instance.new('ScreenGui')\nerror('boom')",json!({}));
    assert_eq!(r.status, "ok");
    assert!(r.end.is_some());
    assert_eq!(printed(&r), vec!["hello 1 true", "careful", "data"]);
    let remote = r.events.iter().find(|e| e.kind == "remote").unwrap();
    assert_eq!(remote.line.as_deref(), Some("4"));
    assert_eq!(remote.a.as_deref(), Some("ReplicatedStorage.Events.Buy"));
    let kick = r.events.iter().find(|e| e.kind == "kick").unwrap();
    assert_eq!((kick.line.as_deref(), kick.time), (Some("7"), 2.5));
    let err = r.events.iter().find(|e| e.kind == "err").unwrap();
    assert_eq!(err.line.as_deref(), Some("11"));
    assert_eq!(err.b, None);
    assert!(tracer::render_report(&r, "script", "the script", None)
        .contains("The script stopped with an error here"));
}

#[test]
fn scheduler_order_steps_simulated_time_and_settle_limits() {
    let r=trace("task.spawn(function() task.wait(3) print('spawned') end)\ntask.delay(5,function()print('delayed')end)\ntask.defer(function()print('deferred')end)\nprint('main')",json!({}));
    assert_eq!(
        r.events
            .iter()
            .filter(|e| e.kind == "out")
            .map(|e| (e.time, e.a.as_deref().unwrap()))
            .collect::<Vec<_>>(),
        vec![
            (0.0, "main"),
            (0.0, "deferred"),
            (3.0, "spawned"),
            (5.0, "delayed")
        ]
    );
    let r = trace(
        "local a=os.time() task.wait(1000) print(os.time()-a) print(os.clock()>0,tick()>0) wait(5)",
        json!({"max_time":2000}),
    );
    assert_eq!(printed(&r), vec!["1000", "true true"]);
    assert_eq!(r.events[0].time, 1000.0);
    assert!(r.seconds < 10.0);
    assert_eq!(r.end.unwrap()["time"], "1005.000");
    let r = trace("while true do task.wait(1) end", json!({"max_time":5}));
    assert!(r.reason().contains("limit of 5 s"));
    assert_eq!(r.end.unwrap()["time"], "5.000");
    let r = trace(
        "game:GetService('RunService').Heartbeat:Connect(function()end)\nprint('armed')",
        json!({}),
    );
    assert!(r.reason().contains("only frame handlers"));
    assert_eq!(printed(&r), vec!["armed"]);
    assert!(r.end.as_ref().unwrap()["time"].parse::<f64>().unwrap() < 11.0);
    assert_eq!(
        r.events
            .iter()
            .filter(|e| e.kind == "conn")
            .map(|e| e.text.as_str())
            .collect::<Vec<_>>(),
        vec!["RunService.Heartbeat:Connect(function() --[[line 1]] end)"]
    );
    let r = trace(
        "for i=1,10 do task.spawn(function()while true do task.wait()end end)end",
        json!({"max_steps":500}),
    );
    assert!(r.reason().contains("step limit of 500"));
}

#[test]
fn deterministic_virtual_world_and_configured_responses() {
    let src="math.randomseed(1) for i=1,5 do print(math.random(1,1000),os.clock(),tick()>0) task.wait(i)end print(game.JobId)";
    let a = trace(src, json!({}));
    let b = trace(src, json!({}));
    assert_eq!(a.events.len(), 7);
    assert_eq!(
        serde_json::to_value(a.events).unwrap(),
        serde_json::to_value(b.events).unwrap()
    );
    let r=trace("print(game.PlaceId,game.GameId,game.JobId) local p=game.Players.LocalPlayer print(p.Name,p.UserId,identifyexecutor()) print(readfile('cfg.json'),isfile('nope')) print(game:HttpGet('https://api.example.com/x'))",json!({"place_id":4321,"game_id":99,"job_id":"JOB","username":"Tester","user_id":7,"executor":"Synapse","executor_version":"9","files":{"cfg.json":"{\"k\":1}"},"responses":{"https://api.example.com/*":"PREFIX-ANSWER"}}));
    assert_eq!(
        printed(&r),
        vec![
            "4321 99 JOB",
            "Tester 7 Synapse 9",
            "{\"k\":1} false",
            "PREFIX-ANSWER"
        ]
    );
}

#[test]
fn request_body_clipboard_teleport_and_repeated_payloads() {
    let r=trace("local r=request({Url='https://discord.com/api/webhooks/123/abc',Method='POST',Body='{\"content\":\"hi\"}'}) print(r.StatusCode) for i=1,40 do queue_on_teleport('print(1)')end setclipboard('copied') setclipboard(string.rep('x',300)) game:GetService('TeleportService'):Teleport(123,game.Players.LocalPlayer)",json!({}));
    assert_eq!(printed(&r), vec!["204"]);
    assert_eq!(r.payloads.len(), 3);
    assert!(r
        .payloads
        .iter()
        .any(|p| p.kind == "body" && p.data == b"{\"content\":\"hi\"}"));
    assert!(r
        .payloads
        .iter()
        .any(|p| p.kind == "clipboard" && p.data == vec![b'x'; 300]));
    assert!(r
        .payloads
        .iter()
        .any(|p| p.kind == "queue_on_teleport" && p.data == b"print(1)"));
    assert!(r
        .events
        .iter()
        .any(|e| e.kind == "exec" && e.text == "setclipboard(\"copied\")"));
    assert_eq!(
        r.events
            .iter()
            .filter(|e| e.kind == "net")
            .map(|e| (e.a.as_deref(), e.b.as_deref()))
            .collect::<Vec<_>>(),
        vec![(
            Some("https://discord.com/api/webhooks/123/abc"),
            Some("POST")
        )]
    );
    let queued = r
        .events
        .iter()
        .filter(|e| e.kind == "exec" && e.text.starts_with("queue_on_teleport"))
        .count();
    assert!(queued < 40);
    assert_eq!(
        r.repeats.iter().find(|p| p.kind == "exec").unwrap().times,
        40 - queued as u64
    );
    let f = tracer::Summary::new(&r).findings();
    assert_eq!(
        f.iter()
            .find(|f| f.title.starts_with("Queues code"))
            .unwrap()
            .count,
        40
    );
    assert!(f
        .iter()
        .any(|f| f.title == "Sends data to a Discord webhook"));
    assert!(f.iter().any(|f| f.title == "Teleports the player"));
}

#[test]
fn error_inside_loadstring_stops_caller_and_retains_payload_line() {
    let r = trace(
        "local f=loadstring('local x=1\\nerror(\"inner\")') print('before') f() print('never')",
        json!({}),
    );
    assert_eq!(printed(&r), vec!["before"]);
    let e = r.events.iter().find(|e| e.kind == "err").unwrap();
    assert_eq!(e.line.as_deref(), Some("loadstring:2"));
    assert!(e.text.contains("inner"));
    let r = trace("local x = = 1", json!({}));
    assert_eq!(r.status, "ok");
    assert_eq!(r.reason(), "the script has a syntax error");
    let error = r.events.iter().find(|e| e.kind == "err").unwrap();
    assert!(error.text.starts_with("syntax error: script:1:"));
}

#[test]
fn explore_buttons_repeat_caps_and_truncated_payload_limit() {
    let src="local b=Instance.new('TextButton') b.MouseButton1Click:Connect(function()print('clicked')game.Players.LocalPlayer:Kick('x')end)";
    let quiet = trace(src, json!({}));
    assert!(printed(&quiet).is_empty());
    assert!(!quiet.events.iter().any(|e| e.kind == "kick"));
    let explored = trace(src, json!({"explore":true}));
    assert_eq!(printed(&explored), vec!["clicked"]);
    assert_eq!(
        explored.events.iter().filter(|e| e.kind == "kick").count(),
        1
    );
    assert!(explored.reason().contains("click handlers were called"));
    let r = trace("for i=1,1000 do print('same')end", json!({}));
    assert!(r.events.len() < 50);
    assert_eq!(
        r.repeats.iter().map(|s| s.times).sum::<u64>(),
        1000 - r.events.len() as u64
    );
    let r=trace("for i=1,3000 do print('line '..string.char(65+i%26)..string.char(65+(i//26)%26)..string.char(65+(i//676)%26))end",json!({"max_events":100}));
    assert_eq!(r.events.len(), 100);
    assert_eq!(r.end.as_ref().unwrap()["cut"], "1");
    assert_eq!(r.end.as_ref().unwrap()["events"], "3000");
    assert!(tracer::render_report(&r, "x", "the script", None)
        .contains("The log was cut at its size limit"));
    let r = trace("queue_on_teleport(string.rep('x',5000)..'end')", json!({}));
    assert_eq!(r.payloads[0].length, 5003);
}

#[test]
fn timeout_keeps_padded_partial_trace_and_cleans_directory() {
    if std::env::var_os("BGDEOBF_TEST_TRACE_PRIVATE_TEMP").is_none() {
        let root = tempfile::tempdir().unwrap();
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "timeout_keeps_padded_partial_trace_and_cleans_directory",
                "--nocapture",
            ])
            .env("BGDEOBF_TEST_TRACE_PRIVATE_TEMP", "1");
        for key in ["TMPDIR", "TEMP", "TMP"] {
            command.env(key, root.path());
        }
        let out = bgdeobf::process::capture(
            command,
            &bgdeobf::process::Limits {
                timeout: std::time::Duration::from_secs(20),
                memory_mb: 512,
                cpu_seconds: 0,
                max_output_bytes: 65536,
                new_group: true,
            },
        )
        .unwrap();
        assert_eq!(
            out.status,
            "ok",
            "{}\n{}",
            String::from_utf8_lossy(&out.out),
            String::from_utf8_lossy(&out.err)
        );
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
        return;
    }
    let started = std::time::Instant::now();
    let r = trace(
        "print('before') game:HttpGet('https://example.com/x') while true do end",
        json!({"timeout":3}),
    );
    assert!((3.0..12.0).contains(&started.elapsed().as_secs_f64()));
    assert_eq!(r.status, "timeout");
    assert!(r.end.is_none());
    assert_eq!(printed(&r), vec!["before"]);
    assert_eq!(r.events.iter().filter(|e| e.kind == "net").count(), 1);
    assert_eq!(std::fs::read_dir(std::env::temp_dir()).unwrap().count(), 0);
    assert!(tracer::render_report(&r, "x", "the script", None)
        .contains("The run did not finish normally"));
}

const HOSTILE: &str = r#"local results={}
local function try(name,f)local ok,a=pcall(f)results[#results+1]=name..'='..(ok and 'ran' or 'err')..':'..tostring(a):sub(1,60)end
try('require rel',function()return require('./sandbox')end)
try('require up',function()return require('../../../../etc/hosts')end)
try('require abs',function()return require('/etc/passwd')end)
try('os.execute',function()return os.execute('id')end)
try('io',function()return io.open('/etc/passwd')end)
try('package',function()return package.loaded end)
try('dofile',function()return dofile('/etc/passwd')end)
try('loadfile',function()return loadfile('/etc/passwd')end)
try('load',function()return load('return 1')()end)
try('os.getenv',function()return os.getenv('HOME')end)
try('loadstring os',function()return loadstring('return os.execute')()end)
try('getfenv0',function()local e=getfenv(0)return e.io or e.os and e.os.execute or e.package end)
try('getfenv1',function()local e=getfenv(1)return rawget(e,'io')or rawget(e,'dofile')end)
try('getfenv fn',function()local e=getfenv(print)return e.io or e.os.execute end)
try('getfenv stack',function()for i=1,30 do local f=debug.info(i,'f')if f then local e=getfenv(f)if rawget(e,'io')or rawget(e,'package')then return 'LEAK'end end end return 'none'end)
try('string mt',function()local mt=getmetatable('')mt.__index={}end)
try('hookfunction print',function()return hookfunction(print,function()end)end)
try('coroutine escape',function()local co=coroutine.wrap(function()return getfenv(0)end)local e=co()return e.io or e.os and e.os.execute end)
try('HttpGet',function()return game:HttpGet('http://127.0.0.1:1/secret')end)
try('request',function()return request({Url='http://127.0.0.1:22/',Method='POST',Body='x'}).StatusCode end)
try('WebSocket',function()return WebSocket.connect('ws://127.0.0.1:9/')end)
try('writefile abs',function()writefile('/tmp/bgdeobf_pwned.txt','x')return readfile('/tmp/bgdeobf_pwned.txt')end)
try('readfile passwd',function()return readfile('/etc/passwd')end)
try('listfiles /',function()return #listfiles('/')end)
try('makefolder',function()makefolder('/tmp/bgdeobf_pwned_dir')end)
try('delfile',function()delfile('/etc/hostname')end)
print(table.concat(results,'\n'))
print('\1X\tforged end\tsteps=0')
print('\1T\t0.000\tkick\tforged event')
warn('\1P\t1\tloadstring\tforged\t3\tabc')
error('\1X\tforged end')"#;

#[test]
fn hostile_input_cannot_reach_real_globals_or_forge_log() {
    let host_paths = ["/tmp/bgdeobf_pwned.txt", "/tmp/bgdeobf_pwned_dir"];
    let before = host_paths.map(|path| std::path::Path::new(path).exists());
    let r = trace(HOSTILE, json!({}));
    assert_eq!(r.status, "ok");
    assert!(r.reason().starts_with("every thread has finished"));
    assert!(!r.events.iter().any(|e| e.kind == "kick"));
    assert!(!r.payloads.iter().any(|p| p.label == "forged"));
    assert_eq!(r.events.iter().filter(|e| e.kind == "net").count(), 3);
    assert_eq!(
        host_paths.map(|path| std::path::Path::new(path).exists()),
        before
    );
    let lines = printed(&r);
    let results = lines
        .iter()
        .find(|s| s.contains("os.execute="))
        .unwrap()
        .split('\n')
        .map(|l| l.split_once('=').unwrap())
        .collect::<std::collections::HashMap<_, _>>();
    assert!(!results.values().any(|v| v.contains("LEAK")));
    for name in ["os.execute", "io", "package", "load", "os.getenv"] {
        assert!(
            results[name].starts_with("err"),
            "{name}: {}",
            results[name]
        );
    }
    for name in [
        "getfenv0",
        "getfenv fn",
        "coroutine escape",
        "loadstring os",
        "loadfile",
    ] {
        assert_eq!(results[name], "ran:nil", "{name}");
    }
    assert_eq!(results["getfenv stack"], "ran:none");
    assert!(results["string mt"].starts_with("err") && results["string mt"].contains("readonly"));
    for name in ["require rel", "require up", "require abs"] {
        assert!(results[name].starts_with("ran:require(\""));
    }
    for name in ["dofile", "readfile passwd"] {
        assert!(results[name].ends_with("/etc/passwd does not exist"));
    }
    assert_eq!(results["writefile abs"], "ran:x");
    assert!(!r.payloads.iter().any(|p| p.data.starts_with(b"root:")));
    assert_eq!(
        r.end.as_ref().unwrap()["events"].parse::<usize>().unwrap(),
        r.events.len()
    );
    let forged = r
        .events
        .iter()
        .filter(|e| e.text.contains('\x01') || e.a.as_deref().is_some_and(|a| a.contains('\x01')))
        .collect::<Vec<_>>();
    assert_eq!(forged.len(), 4);
    assert!(forged
        .iter()
        .all(|e| ["out", "err"].contains(&e.kind.as_str())));
    let report = tracer::render_report(&r, "x", "script", None);
    assert!(!report.contains('\x01'));
}

#[test]
fn byte_sources_bom_and_size_validation() {
    assert_eq!(
        printed(&tracer::trace(b"\xef\xbb\xbfprint('bom')", &json!({})).unwrap()),
        vec!["bom"]
    );
    for src in [b"".as_slice(), b" \n\t", b"\x0b"] {
        assert_eq!(tracer::run(src, &json!({})).unwrap_err().code, "empty");
    }
    assert_eq!(
        tracer::run(&[b'x'; 100], &json!({"max_bytes":50}))
            .unwrap_err()
            .code,
        "too_big"
    );
    let mut src = b"print('end')\n-- ".to_vec();
    src.extend((1..=255).filter(|b| !b"\n\r".contains(b)));
    src.push(b'\n');
    let r = tracer::trace(&src, &json!({})).unwrap();
    assert_eq!(r.source, src);
    assert_eq!(printed(&r), vec!["end"]);
}

#[test]
fn moonveil_reconstruction_preserves_callbacks_loops_and_labels() {
    let script=br#"-- This script was generated using MoonVeil 2.1.1
local Players=game:GetService('Players') local player=Players.LocalPlayer
task.spawn(function() local gui=player:WaitForChild('PlayerGui') local junk=gui:FindFirstChild('VHS')if junk then junk:Destroy()end gui.DescendantAdded:Connect(function(inst)if inst.Name=='VHS'then inst:Destroy()end end)end)
task.spawn(function()while true do task.wait(2)game:GetService('ReplicatedStorage').Remotes.Ping:FireServer('hi')end end)
local lib=loadstring(game:HttpGet('https://example.com/lib.lua'))()local win=lib:CreateWindow('Test')local folder=win:AddFolder('Main')
folder:AddToggle({text='Auto',state=false,callback=function(on)if on then local root=player.Character:WaitForChild('HumanoidRootPart')local part=workspace:FindFirstChild('Target')if(root.Position-part.Position).Magnitude>50 then root.CFrame=part.CFrame end fireproximityprompt(part.Prompt)end end})
folder:AddButton({text='Copy',callback=function()setclipboard('https://example.com/x')end})lib:Init()"#;
    let r =
        moonveil::deobfuscate(script, &json!({"version":"2.1.1"}), &Default::default()).unwrap();
    let t = r.text.unwrap();
    assert!(t.starts_with("--[[\n  BEHAVIOUR RECONSTRUCTION - THIS IS NOT THE ORIGINAL SOURCE."));
    for needle in [
        "local Players = game:GetService(\"Players\")",
        "local playerGui = Players.LocalPlayer:WaitForChild(\"PlayerGui\")",
        "playerGui:FindFirstChild(\"VHS\"):Destroy()",
        "NOT FETCHED",
        "loadstring(game:HttpGet(\"https://example.com/lib.lua\"))()",
        "local mainFolder = lib:CreateWindow(\"Test\"):AddFolder(\"Main\")",
        "mainFolder:AddToggle({",
        "text = \"Auto\",",
        "state = false,",
        "callback = function(enabled)",
        "while true do -- UNKNOWN stop condition (the trace saw 40 rounds",
        "DescendantAdded:Connect(function(inst)",
        "if inst.Name == \"VHS\" then",
        "short distance",
        "long distance",
        "humanoidRootPart.Position - target.Position).Magnitude",
        "fireproximityprompt(target.Prompt)",
        "setclipboard(\"https://example.com/x\")",
        "lib:Init()",
    ] {
        assert!(t.contains(needle), "missing {needle}\n{t}");
    }
    assert_eq!(t.matches("FireServer(\"hi\")").count(), 1);
    assert_eq!(t.matches(".CFrame = target.CFrame").count(), 1);
    assert_eq!(r.info["kind"], "moonveil");
    assert_eq!(r.info["moonveil"], json!({"version":"2.1.1"}));
    assert!(r.info["calls"].as_u64().unwrap() > 10);
    assert_eq!(r.info["syntax_ok"], true);
    assert_eq!(r.info["trace_only"], true);
    assert_eq!(r.info["downloads"], json!(["https://example.com/lib.lua"]));
}

#[test]
fn compiled_runtime_never_reads_globals_after_prelude() {
    use std::{process::Command, time::Duration};
    let program = tracer::build_script(b"print(1)", &json!({}), true).unwrap();
    let text = String::from_utf8_lossy(&program);
    let cut = text
        .lines()
        .position(|line| line.contains("setfenv(1, RUNTIME_ENV)"))
        .unwrap()
        + 1;
    assert_eq!(text.matches("setfenv(1, RUNTIME_ENV)").count(), 1);
    let work = tempfile::tempdir().unwrap();
    let path = work.path().join("sandbox.lua");
    std::fs::write(&path, program).unwrap();
    let mut command = Command::new(bgdeobf::astutil::find_bin("luau-compile").unwrap());
    command.arg("--text").arg(path);
    let out = bgdeobf::process::capture(
        command,
        &bgdeobf::process::Limits {
            timeout: Duration::from_secs(60),
            memory_mb: 1024,
            cpu_seconds: 62,
            max_output_bytes: 128 * 1024 * 1024,
            new_group: true,
        },
    )
    .unwrap();
    assert_eq!(out.status, "ok");
    let bytecode = String::from_utf8_lossy(&out.out);
    let funcs: Vec<_> = bytecode.split("\nFunction ").collect();
    assert!(funcs.len() > 100);
    let access =
        regex::Regex::new(r"^(?:L\d+:\s*)?(GETIMPORT|GETGLOBAL|SETGLOBAL)\s.*?\[([^\]]+)\]")
            .unwrap();
    for body in &funcs[..funcs.len() - 1] {
        assert!(
            !body.lines().any(|line| access.is_match(line.trim())),
            "runtime function accessed a global"
        );
    }
    let source_line = regex::Regex::new(r"^\s*(\d+):").unwrap();
    let mut line = 0;
    for text in funcs.last().unwrap().lines() {
        if let Some(m) = source_line.captures(text) {
            line = m[1].parse::<usize>().unwrap();
            continue;
        }
        assert!(
            !(line > cut && access.is_match(text.trim())),
            "global after runtime environment was emptied: {text}"
        );
    }
}

#[test]
fn report_findings_repeat_counts_and_source_literal_download_exclusion() {
    let event = |time, line: &str, kind: &str, text: &str, a: Option<&str>, b: Option<&str>| {
        tracer::Event {
            time,
            line: Some(line.into()),
            kind: kind.into(),
            text: text.into(),
            a: a.map(str::to_owned),
            b: b.map(str::to_owned),
        }
    };
    let mut r = tracer::TraceResult {
        events: vec![
            event(
                0.0,
                "1",
                "net",
                "fetch",
                Some("https://x.io/a.lua"),
                Some("GET"),
            ),
            event(0.0, "2", "load", "loadstring", Some("1"), Some("10")),
            event(
                0.1,
                "3",
                "net",
                "request",
                Some("https://discord.com/api/webhooks/1/x"),
                Some("POST"),
            ),
            event(0.2, "4", "kick", "LocalPlayer:Kick(\"bye\")", None, None),
            event(0.3, "5", "tp", "TeleportService:Teleport(1)", None, None),
            event(0.4, "6", "probe", "identifyexecutor()", None, None),
            event(
                0.5,
                "7",
                "file",
                "writefile(\"autoexec/x.lua\",3 bytes)",
                Some("autoexec/x.lua"),
                None,
            ),
            event(
                0.6,
                "8",
                "exec",
                "queue_on_teleport(\"print(1)\")",
                None,
                None,
            ),
            event(0.7, "9", "exec", "setclipboard(\"t\")", None, None),
            event(
                0.8,
                "10",
                "out",
                "hi\n   99.000  L1 kick nobody\u{1b}[2J\u{202e}reversed",
                None,
                None,
            ),
        ],
        payloads: vec![tracer::Payload {
            id: 1,
            kind: "loadstring".into(),
            label: "loadstring".into(),
            length: 10,
            data: b"print(\"x\")".to_vec(),
        }],
        repeats: vec![tracer::Repeat {
            times: 57,
            kind: "exec".into(),
            text: "queue_on_teleport(\"print(1)\")".into(),
            first: 3.0,
            last: 60.0,
        }],
        end: Some(std::collections::BTreeMap::from([(
            "reason".into(),
            "done".into(),
        )])),
        status: "ok".into(),
        stderr: String::new(),
        seconds: 0.5,
        source: b"print(1)\n".to_vec(),
        max_time: 30.0,
    };
    let titles = tracer::Summary::new(&r)
        .findings()
        .into_iter()
        .map(|f| f.title)
        .collect::<Vec<_>>();
    assert_eq!(
        titles,
        vec![
            "Downloads code and runs it",
            "Sends data to a Discord webhook",
            "Writes into an auto-execute folder",
            "Queues code for the next teleport (queue_on_teleport)",
            "Copies text to the clipboard",
            "Kicks the player",
            "Teleports the player",
            "Asks which executor / device this is"
        ]
    );
    assert_eq!(tracer::Summary::new(&r).findings()[3].count, 58);
    let report = tracer::render_report(&r, "hub.lua", "the script", Some("hub"));
    assert!(!report.contains('\u{1b}'));
    assert!(!report.contains('\u{202e}'));
    assert!(!report.lines().any(|l| l.trim_start().starts_with("99.000")));
    assert!(report.contains("hi\\n   99.000"));
    for needle in [
        "\nSUMMARY\n",
        "\nWORTH A LOOK FIRST",
        "\nLOG\n",
        "\nLIMITS OF THIS TRACE\n",
        "hub_payload1.lua",
    ] {
        assert!(report.contains(needle));
    }
    r.source = b"game:HttpGet('https://x.io/a.lua')\nloadstring('print(\"x\")')()\n".to_vec();
    assert!(!tracer::Summary::new(&r)
        .findings()
        .iter()
        .any(|f| f.title == "Downloads code and runs it"));
    r.end = None;
    r.status = "crashed".into();
    r.stderr = "runtime line 3: boom\nmore".into();
    assert!(tracer::render_report(&r, "x", "the script", None).contains("boom"));
    assert!(!tracer::summary_info(&r)["reason"]
        .as_str()
        .unwrap()
        .contains("boom"));
    r.events = vec![
        event(
            0.0,
            "1",
            "net",
            "x",
            Some("http://**evil**<@1>/x"),
            Some("GET"),
        ),
        event(0.0, "2", "out", "@everyone", None, None),
    ];
    r.payloads.clear();
    r.repeats.clear();
    let info = tracer::summary_info(&r);
    assert_eq!(info["domains"], json!(["**evil**<@1>"]));
    assert_eq!(info["findings"], json!([]));
    assert!(!info.to_string().contains("@everyone"));
    r.payloads.push(tracer::Payload {
        id: 1,
        kind: "file".into(),
        label: "short".into(),
        length: 10,
        data: b"abc".to_vec(),
    });
    assert!(tracer::limits(&r)
        .iter()
        .any(|line| line.contains("only the beginning is saved")));
}

#[test]
fn ghosts_virtual_gui_and_environment_values_preserve_runtime_behavior() {
    let r = trace(
        r#"local m=require(game.ReplicatedStorage.Module)
local function try(name,f)local ok,err=pcall(f)print(name,ok and 'ok' or ('ERR '..tostring(err)))end
try('concat',function()return 'x'..m.name..1 end)
try('math',function()return math.floor(m.Value)+math.max(m.a,5)+math.clamp(m.a,0,10)end)
try('format',function()return string.format('%d %s %.2f',m.n,m.n,m.n)end)
try('methods',function()return m.name:sub(1,3),m.str:split(','),m.str:gsub('a','b')end)
try('tables',function()table.insert(m.list,1)return table.concat(m.list,','),table.find(m.list,1),#m.list end)
try('loops',function()for i,v in ipairs(m.list)do end for k,v in pairs(m.list)do end end)
try('calls',function()return m.fn(1,2),m:Foo(1),m.a.b.c.d end)
try('assign',function()m.a.b=5 end)
try('vectors',function()return Vector3.new(m.a,m.b,m.c),UDim2.new(m.a,0,m.b,0),Color3.fromRGB(m.a,m.b,m.c)end)
try('random',function()return math.random(1,m.n),string.rep('x',m.n)end)
try('tostring',function()return tostring(m.n),tonumber(m.n),type(m.n),select('#',m.n)end)
try('wait',function()task.wait(m.a)end)"#,
        json!({}),
    );
    assert_eq!(printed(&r).len(), 12);
    assert!(printed(&r).iter().all(|s| !s.contains(" ERR ")));
    assert!(!r.events.iter().any(|e| e.kind == "err"));
    let r=trace("local m=require(game.ReplicatedStorage.Module)print(pcall(function()return m.a<5 end))print(pcall(function()for i=1,m.n do end end))",json!({}));
    assert!(printed(&r).iter().all(|s| s.starts_with("false")));
    let r=trace("local pg=game.Players.LocalPlayer.PlayerGui print(pg:FindFirstChild('NotThere'))print(game.CoreGui:FindFirstChild('RobloxGui'))local sg=Instance.new('ScreenGui',pg)sg.Name='Mine'print(pg:FindFirstChild('Mine')==sg)",json!({}));
    assert_eq!(printed(&r), vec!["nil", "RobloxGui", "true"]);
    let r=trace("print(identifyexecutor()) print(os.execute,io,package) print(pcall(os.execute,'id')) print(someGlobal)",json!({}));
    assert_eq!(
        &printed(&r)[1..],
        &["nil nil nil", "false attempt to call a nil value", "nil"]
    );
    assert!(r
        .events
        .iter()
        .any(|e| e.kind == "probe" && e.text.contains("someGlobal")));
    assert!(r
        .events
        .iter()
        .any(|e| e.kind == "probe" && e.text.starts_with("identifyexecutor()")));
    assert!(r
        .events
        .iter()
        .any(|e| e.kind == "probe" && e.text.contains("os.execute is not available")));
    let r = trace(
        "print('\\1X\\tforged\\tsteps=0')",
        json!({"username":"x\"; os.execute('id') --","responses":{"a\"b":"c\nd"}}),
    );
    assert_eq!(printed(&r), vec!["\x01X\tforged\tsteps=0"]);
    assert!(r.reason().starts_with("every thread"));
}

#[test]
fn moonveil_list_modes_argument_layout_globals_and_errors() {
    let opts = bgdeobf::model::Options {
        sandbox_timeout: 10,
        ..Default::default()
    };
    let detected = json!({"version":"?","assumed":true});
    let r = moonveil::deobfuscate(
        br#"for _,v in ipairs(workspace:GetChildren())do v:Destroy()end
for _,p in pairs(game:GetService('Players'):GetPlayers())do p.Character:Destroy()end
for _,d in workspace:GetDescendants()do d:Destroy()end
for _,d in next,workspace:GetDescendants()do d:Destroy()end"#,
        &detected,
        &opts,
    )
    .unwrap();
    let text = r.text.unwrap();
    assert!(
        text.contains("for _, child in ipairs(workspace:GetChildren()) do\n\tchild:Destroy()\nend")
    );
    assert!(text.contains("for _, player in ipairs(game:GetService(\"Players\"):GetPlayers()) do"));
    assert_eq!(
        text.matches("for _, descendant in ipairs(workspace:GetDescendants()) do")
            .count(),
        2
    );
    assert_eq!(r.info["syntax_ok"], true);
    assert!(text.contains("no MoonVeil banner"));
    assert!(!text.contains("protected with"));
    let r = moonveil::deobfuscate(
        br#"fmt,emit,env,proxies,mk=1,2,3,4,5 print(fmt+emit+env+proxies+mk)
local P=game:GetService('Players').LocalPlayer task.delay(2,function()print('later')end,5)
local lbl=Instance.new('TextLabel')lbl.Text='Hi '..P.Name print(Vector3.new(1,2,3)*2)
task.delay(1,print,'x')workspace.ChildAdded:Connect(print)
game:GetService('Players').PlayerAdded:Connect(function(p)print('hi',p.Name)end)"#,
        &detected,
        &opts,
    )
    .unwrap();
    let t = r.text.unwrap();
    for needle in ["print(15)","task.delay(2, function()\n\tprint(\"later\")\nend, 5)","Instance.new(\"TextLabel\").Text = \"Hi {game:GetService(\\\"Players\\\").LocalPlayer.Name}\"","print((Vector3.new(1, 2, 3) * 2))","task.delay(1, print, \"x\")","workspace.ChildAdded:Connect(print)"]{assert!(t.contains(needle),"missing {needle}\n{t}");}
    assert!(!t.contains("print(\"x\")"));
    assert!(t
        .replace("\n\t\t", "\n\t")
        .contains("function(inst)\n\tprint(\"hi\", inst.Name)\nend"));
    assert_eq!(r.info["syntax_ok"], true);
    let r = moonveil::deobfuscate(b"local x=1+1 error('boom')", &detected, &opts).unwrap_err();
    assert_eq!(r.code, "unsupported");
    assert!(r.message.contains("boom"));
    assert_eq!(
        moonveil::deobfuscate(b"local x = = 3 end end", &detected, &opts)
            .unwrap_err()
            .code,
        "syntax"
    );
    let opts = bgdeobf::model::Options {
        sandbox_timeout: 1,
        ..opts
    };
    assert_eq!(
        moonveil::deobfuscate(b"while true do end", &detected, &opts)
            .unwrap_err()
            .code,
        "timeout"
    );
}

#[test]
fn trace_literals_options_and_malformed_protocol_match_reference() {
    let defaults = tracer::default_options();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&serde_json::to_string(&defaults).unwrap())
            .unwrap(),
        defaults
    );
    assert_eq!(
        tracer::build_script(b"x", &json!({}), true).unwrap(),
        tracer::build_script(b"x", &serde_json::Value::Null, true).unwrap()
    );
    for (value, expected) in [
        (json!(true), "true"),
        (json!(false), "false"),
        (json!(null), "nil"),
        (json!(5), "5"),
        (json!(1.5), "1.5"),
        (json!("a\"b"), "\"a\\\"b\""),
        (json!([1, "x"]), "{1, \"x\"}"),
    ] {
        assert_eq!(tracer::lit(&value).unwrap(), expected);
    }
    let program = tracer::build_script(b"print(1)", &json!({"bogus":123}), true).unwrap();
    let cfg = std::str::from_utf8(program.split(|b| *b == b'\n').next().unwrap()).unwrap();
    for key in ["responses", "explore", "pad", "bogus", "timeout", "mem_mb"] {
        assert!(!cfg.contains(&format!("[\"{key}\"]")));
    }
    assert!(cfg.contains("[\"source\"] = \"print(1)\""));
    let padded = tracer::build_script(
        b"x",
        &json!({"explore":true,"responses":{"a":"b"},"username":"U"}),
        false,
    )
    .unwrap();
    let cfg = std::str::from_utf8(padded.split(|b| *b == b'\n').next().unwrap()).unwrap();
    for text in [
        "[\"explore\"] = true",
        "[\"responses\"] = {[\"a\"] = \"b\"}",
        "[\"username\"] = \"U\"",
        "[\"pad\"] = 4096",
    ] {
        assert!(cfg.contains(text));
    }
    let (events, payloads, repeats, end) = tracer::parse_log(b"padding\ntext mentions \\x01T\n\x01T\tshort\n\x01Z\tunknown tag\n\x01P\tnot-a-number\tk\tl\t5\tdata\n\x01T\t0.100\tout\tgood\n\x01T\t0.200\tout\tcut off");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].text, "good");
    assert!(events[0].a.is_none() && events[0].line.is_none());
    assert!(payloads.is_empty() && repeats.is_empty() && end.is_none());
    let (events,payloads,repeats,end)=tracer::parse_log(b"\x01T\t1.500@12\tnet\tgame:HttpGet(\"u\")\tu\tGET\n\x01T\t2.000\tout\tprint(\"x\")\n\x01P\t1\tloadstring\tlbl\t5\tab\\x0Acd\n\x01S\t40\tout\tprint(#)\t0.100\t2.000\n\x01X\tdone\tevents=3\tshown=2\tcut=0\tsteps=7 time=2.000\n");
    assert_eq!(
        (
            events[0].time,
            events[0].line.as_deref(),
            events[0].kind.as_str(),
            events[0].text.as_str(),
            events[0].a.as_deref(),
            events[0].b.as_deref()
        ),
        (
            1.5,
            Some("12"),
            "net",
            "game:HttpGet(\"u\")",
            Some("u"),
            Some("GET")
        )
    );
    assert!(events[1].line.is_none() && events[1].a.is_none());
    assert_eq!(
        (
            payloads[0].id,
            payloads[0].kind.as_str(),
            payloads[0].label.as_str(),
            payloads[0].length,
            payloads[0].data.as_slice()
        ),
        (1, "loadstring", "lbl", 5, b"ab\ncd".as_slice())
    );
    assert_eq!(
        (
            repeats[0].times,
            repeats[0].kind.as_str(),
            repeats[0].first,
            repeats[0].last
        ),
        (40, "out", 0.1, 2.0)
    );
    assert_eq!(
        serde_json::to_value(end.unwrap()).unwrap(),
        json!({"reason":"done","events":"3","shown":"2","cut":"0","steps":"7","time":"2.000"})
    );
    assert_eq!(
        tracer::vis("\x7f\u{85}plain text, C:\\dir, é, 日本"),
        "\\x7f\\x85plain text, C:\\dir, é, 日本"
    );
}

#[test]
fn trace_full_event_coordinates_and_error_origins_match_reference() {
    let r = trace("\nprint(\"hello\",1,true)\nwarn(\"careful\")\nlocal remote=game:GetService(\"ReplicatedStorage\"):WaitForChild(\"Events\"):WaitForChild(\"Buy\")\nremote:FireServer(\"sword\",5)\nlocal body=game:HttpGet(\"https://example.com/a.lua\")\nloadstring(body)\ntask.wait(2.5)\ngame:GetService(\"Players\").LocalPlayer:Kick(\"bye\")\nwritefile(\"test.txt\",\"data\")\nprint(readfile(\"test.txt\"))\nlocal gui=Instance.new(\"ScreenGui\")\ngui.Name=\"MyGui\"\ngui.Parent=game:GetService(\"CoreGui\")\nerror(\"boom\")", json!({}));
    assert_eq!(r.status, "ok");
    assert!(r.end.is_some());
    assert!(r.seconds < 10.0);
    let remote = r.events.iter().find(|e| e.kind == "remote").unwrap();
    assert_eq!((remote.line.as_deref(), remote.text.as_str(), remote.a.as_deref(), remote.b.as_deref()), (Some("5"), "ReplicatedStorage:WaitForChild(\"Events\"):WaitForChild(\"Buy\"):FireServer(\"sword\", 5)", Some("ReplicatedStorage.Events.Buy"), Some("FireServer")));
    let net = r.events.iter().find(|e| e.kind == "net").unwrap();
    assert_eq!(
        (net.line.as_deref(), net.a.as_deref(), net.b.as_deref()),
        (Some("6"), Some("https://example.com/a.lua"), Some("GET"))
    );
    assert_eq!(
        r.events
            .iter()
            .filter(|e| e.kind == "file")
            .map(|e| e.text.as_str())
            .collect::<Vec<_>>(),
        vec![
            "writefile(\"test.txt\", 4 bytes)",
            "readfile(\"test.txt\") --> 4 bytes"
        ]
    );
    assert!(r
        .events
        .iter()
        .any(|e| e.line.as_deref() == Some("11") && e.a.as_deref() == Some("data")));
    assert!(r
        .events
        .iter()
        .any(|e| e.kind == "def" && e.text.contains("Instance.new(\"ScreenGui\")")));
    let error = r.events.iter().find(|e| e.kind == "err").unwrap();
    assert_eq!(error.text, "error: script:15: boom");
    assert_eq!(error.line.as_deref(), Some("15"));
    for (src, line, prefix) in [
        (
            "local x=nil\nprint(x.y)",
            "2",
            "error: script:2: attempt to index nil",
        ),
        ("string.rep()", "1", "error: script:1: invalid argument #1"),
    ] {
        let r = trace(src, json!({}));
        let errors = r
            .events
            .iter()
            .filter(|e| e.kind == "err")
            .collect::<Vec<_>>();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].line.as_deref(), Some(line));
        assert!(errors[0].text.starts_with(prefix));
        assert!(errors[0].b.is_none());
        let report = tracer::render_report(&r, "x", "the script", None);
        assert!(!report.contains("raised inside the fake environment"));
        assert!(report.contains("The script stopped with an error here"));
    }
}

#[test]
fn trace_request_payloads_repeat_counts_and_simulated_limits_match_reference() {
    let r=trace("local code=game:HttpGet('https://example.com/hub.lua')\nloadstring(code)()\nprint(game:HttpGet('https://api.example.com/v1/x?id=5'))\nprint(#game:HttpGet('https://nothing.example/'))",json!({"responses":{"https://example.com/hub.lua":"print(\"payload ran\")\nlocal p = game:GetService(\"Players\")","https://api.example.com/*":"PREFIX-ANSWER"}}));
    assert_eq!(printed(&r), vec!["payload ran", "PREFIX-ANSWER", "0"]);
    assert_eq!(r.payloads.len(), 1);
    assert_eq!(
        r.payloads[0].data,
        b"print(\"payload ran\")\nlocal p = game:GetService(\"Players\")"
    );
    assert!(r.events.iter().any(
        |e| e.a.as_deref() == Some("payload ran") && e.line.as_deref() == Some("loadstring:1")
    ));
    assert_eq!(
        tracer::Summary::new(&r)
            .findings()
            .iter()
            .map(|f| f.title.as_str())
            .collect::<Vec<_>>(),
        vec!["Downloads code and runs it"]
    );
    let r = trace(
        "for i=1,40 do queue_on_teleport('print(1)')task.wait(1)end",
        json!({}),
    );
    assert_eq!(r.payloads.len(), 1);
    assert_eq!(tracer::Summary::new(&r).findings()[0].count, 31);
    assert!(r.reason().contains("simulated time limit of 30 s"));
    let r = trace("task.wait(1000)print('late')", json!({}));
    assert!(printed(&r).is_empty());
    assert!(r.reason().contains("simulated time limit of 30 s"));
    let r = trace("for i=1,1000 do print('same')end", json!({}));
    let end = r.end.unwrap();
    assert_eq!(end["events"], "1000");
    assert_eq!(end["suppressed"], "1");
    let r=trace("game:GetService('TeleportService'):Teleport(123,game.Players.LocalPlayer)\ngame.Players.LocalPlayer:Kick('x')\ngame.ReplicatedStorage.Remote:FireServer(1)",json!({}));
    for (kind, line) in [("tp", "1"), ("kick", "2"), ("remote", "3")] {
        assert_eq!(
            r.events
                .iter()
                .find(|e| e.kind == kind)
                .unwrap()
                .line
                .as_deref(),
            Some(line)
        );
    }
}

#[test]
fn trace_memory_errors_preserve_preceding_and_following_events() {
    let r=trace("print('before')local t,i={},0 print(pcall(function()while true do i+=1 t[i]=string.rep('x',1000000+i)end end))print('after')",json!({"mem_mb":200,"timeout":30}));
    assert_eq!(r.status, "ok");
    assert_eq!(
        printed(&r),
        vec!["before", "false not enough memory", "after"]
    );
    assert!(r.seconds < 20.0);
    let r = trace(
        "local s='x'while true do s=s..s end",
        json!({"mem_mb":200,"timeout":30}),
    );
    let errors = r
        .events
        .iter()
        .filter(|e| e.kind == "err")
        .collect::<Vec<_>>();
    assert_eq!(errors.len(), 1);
    assert!(errors[0].text.contains("not enough memory"));
    assert!(r.seconds < 20.0);
}

#[test]
fn moonveil_toggle_unrun_callbacks_truncated_parser_and_environment_match_reference() {
    let trace=moonveil::parse("B\tM\tkind=main\nC\t1\tlib:AddToggle\t{text = \"T\", callback = @F1}\nC\t2\tlib:AddButton\t{callback = @F2}\nE\tM\tok\nB\tF1\tkind=toggle;param=enabled;val=true;dist=near\nC\t3\tprint\t\"on\"\nE\tF1\tok\nB\tF1\tkind=toggle;param=enabled;val=false;dist=near\nE\tF1\tok");
    let out = moonveil::Renderer::new(trace).render();
    assert!(out.contains(
        "callback = function(enabled)\n\t\tif enabled then\n\t\t\tprint(\"on\")\n\t\tend\n\tend,"
    ));
    assert!(out.contains("callback = function() end --[[not run in the trace]],"));
    let trace = moonveil::parse("B\tM\tkind=main\nC\t1\tprint\t\"a\"\nC\t2");
    let main = &trace.bodies[trace.main.unwrap()];
    assert_eq!(main.status, "cut");
    assert_eq!(main.events.len(), 2);
    let text = moonveil::Renderer::new(moonveil::parse(
        "B\tM\tkind=main\nC\t1\tprint\t\"a\"\nE\tM\terr:attempt to index nil",
    ))
    .render();
    assert!(text.ends_with(
        "-- STOPPED in the sandbox: attempt to index nil (usually because of the fake objects)"
    ));
    let r=moonveil::deobfuscate(b"local env=getfenv() print(type(env.os.execute),type(env.dofile))print(getfenv(0)==env,rawequal(getfenv(1),env))print(type(ipairs),typeof(game))",&json!({"version":"2.1.1"}),&Default::default()).unwrap();
    let text = r.text.unwrap();
    for needle in [
        "print(\"nil\", \"table\")",
        "print(true, true)",
        "print(\"function\", \"Instance\")",
    ] {
        assert!(text.contains(needle), "{text}");
    }
    assert_eq!(r.info["syntax_ok"], true);
}

#[test]
fn runtime_event_and_moonveil_body_goldens() {
    // Independently recorded with Python tracer.run on this identical source.
    let r=trace("print('hello',1,true)\ngame.Players.LocalPlayer:Kick('bye')\nqueue_on_teleport('print(1)')",json!({}));
    assert_eq!(
        serde_json::to_value(&r.events).unwrap(),
        json!([
            {"time":0.0,"line":"1","kind":"out","text":"print(\"hello\", 1, true)","a":"hello 1 true","b":null},
            {"time":0.0,"line":"2","kind":"def","text":"local Players = game:GetService(\"Players\")","a":null,"b":null},
            {"time":0.0,"line":"2","kind":"def","text":"local LocalPlayer = Players.LocalPlayer","a":null,"b":null},
            {"time":0.0,"line":"2","kind":"kick","text":"LocalPlayer:Kick(\"bye\")","a":null,"b":null},
            {"time":0.0,"line":"3","kind":"exec","text":"queue_on_teleport(\"print(1)\")","a":null,"b":null}
        ])
    );
    assert_eq!(r.payloads.len(), 1);
    assert_eq!(r.payloads[0].kind, "queue_on_teleport");
    assert_eq!(r.payloads[0].data, b"print(1)");
    let r = moonveil::deobfuscate(
        b"print('hello',1,true)\nworkspace:FindFirstChild('Part'):Destroy()",
        &json!({"version":"2.1.1"}),
        &Default::default(),
    )
    .unwrap();
    let text = r.text.unwrap();
    let (_, body) = text.split_once("]]\n\n").unwrap();
    assert_eq!(
        body,
        "print(\"hello\", 1, true)\nworkspace:FindFirstChild(\"Part\"):Destroy()\n"
    );
    assert_eq!(r.info["calls"], 3);
    assert_eq!(r.info["threads"], 1);
    assert_eq!(r.info["warnings"], json!([]));
    assert_eq!(r.info["syntax_ok"], true);
}

#[test]
fn report_golden_preserves_ranking_tie_order_and_empty_source_hash() {
    let event = |kind: &str, text: &str| tracer::Event {
        time: 0.0,
        line: Some("1".into()),
        kind: kind.into(),
        text: text.into(),
        a: None,
        b: None,
    };
    let r = tracer::TraceResult {
        events: vec![
            event("def", "Instance.new(\"Zebra\")"),
            event("def", "Instance.new(\"Alpha\")"),
            event("out", "print(1)"),
        ],
        payloads: vec![],
        repeats: vec![],
        end: Some(std::collections::BTreeMap::from([
            ("reason".into(), "every thread has finished".into()),
            ("steps".into(), "1".into()),
            ("time".into(), "0.000".into()),
        ])),
        status: "ok".into(),
        stderr: String::new(),
        seconds: 0.5,
        source: vec![],
        max_time: 30.0,
    };
    let expected=concat!("FAKE ROBLOX SANDBOX TRACE  -  hub.lua\n",
        "========================================================================\n",
        "Script: 0 bytes, 0 lines, sha256 e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\n",
        "Ended: every thread has finished\nRun: 0.000 simulated seconds, 1 scheduler steps, 0.50 s real time\n",
        "How to read it: the first column is simulated time in seconds since the script started, L<number> is the line of the script that did it.\n\n",
        "SUMMARY\n------------------------------------------------------------------------\nEvents: 3 logged, 0 more were left out because they repeated\nNetwork: no requests\nRemotes: none fired\n",
        "Instances created: 2 (Zebra 1, Alpha 1)\nConsole output: 1 print/warn calls\n\n",
        "WORTH A LOOK FIRST (what the script did in this run; not a verdict)\n------------------------------------------------------------------------\n(nothing of the kinds this tool looks for)\n\n",
        "LOG\n------------------------------------------------------------------------\n",
        "    0.000  L1         def     Instance.new(\"Zebra\")\n",
        "    0.000  L1         def     Instance.new(\"Alpha\")\n",
        "    0.000  L1         out     print(1)\n\n",
        "LIMITS OF THIS TRACE\n------------------------------------------------------------------------\n",
        "* This is a pretend Roblox: services answer with made-up values and the world has no real game in it. Code that waits for the real game (a server event, a particular window, another player) may not have run.\n",
        "* Nothing was downloaded: every request was answered by the sandbox (an empty text unless --responses gave one), so code that a real server would have sent was not seen.\n",
        "* Only the first 30 simulated seconds are shown; later behavior (long timers, things that start on a button press) is not in this log. Use --explore to press every button the script made.\n");
    assert_eq!(
        tracer::render_report(&r, "hub.lua", "the script", None),
        expected
    );
}

#[test]
fn report_ranking_preserves_first_seen_ties_after_counting_repeats() {
    let (events, payloads, repeats, end) = tracer::parse_log(
        concat!(
            "\x01T\t0.000@1\tremote\tZebra:FireServer()\n",
            "\x01T\t0.000@1\tremote\tAlpha:FireServer()\n",
            "\x01T\t0.000@1\tremote\tFrequent:InvokeServer()\n",
            "\x01T\t0.000@1\texec\tzebra()\n",
            "\x01T\t0.000@1\texec\talpha()\n",
            "\x01T\t0.000@1\texec\tfrequent()\n",
            "\x01T\t0.000@1\tdef\tInstance.new(\"Zebra\")\n",
            "\x01T\t0.000@1\tdef\tInstance.new(\"Alpha\")\n",
            "\x01T\t0.000@1\tdef\tInstance.new(\"Frequent\")\n",
            "\x01T\t0.000@1\tdef\tInstance.new(\"Frequent\")\n",
            "\x01T\t0.000@1\tconn\tgame.Zebra:Connect(function() end)\n",
            "\x01T\t0.000@1\tconn\tgame.Alpha:Connect(function() end)\n",
            "\x01T\t0.000@1\tconn\tgame.Frequent:Connect(function() end)\n",
            "\x01S\t1\tremote\tFrequent:InvokeServer()\t0.000\t1.000\n",
            "\x01S\t1\tremote\tZulu:FireServer()\t0.000\t1.000\n",
            "\x01S\t1\tremote\tBeta:FireServer()\t0.000\t1.000\n",
            "\x01S\t1\texec\tfrequent()\t0.000\t1.000\n",
            "\x01S\t1\texec\tzulu()\t0.000\t1.000\n",
            "\x01S\t1\texec\tbeta()\t0.000\t1.000\n",
            "\x01S\t1\tconn\tgame.Frequent:Connect(function() end)\t0.000\t1.000\n",
            "\x01S\t1\tconn\tgame.Zulu:Connect(function() end)\t0.000\t1.000\n",
            "\x01S\t1\tconn\tgame.Beta:Connect(function() end)\t0.000\t1.000\n",
            "\x01X\tdone\ttime=0.000\tsteps=1\n"
        )
        .as_bytes(),
    );
    let r = tracer::TraceResult {
        events,
        payloads,
        repeats,
        end,
        status: "ok".into(),
        stderr: String::new(),
        seconds: 0.5,
        source: vec![],
        max_time: 30.0,
    };
    // Exact ranked rows from Python render_report, including repeat-only keys.
    assert!(
        tracer::render_report(&r, "script", "the script", None).contains(concat!(
            "Remotes (5 different, 6 calls):\n",
            "  x2    Frequent:InvokeServer  [first 0.000 s L1]\n",
            "        e.g. Frequent:InvokeServer()\n",
            "  x1    Zebra:FireServer  [first 0.000 s L1]\n",
            "        e.g. Zebra:FireServer()\n",
            "  x1    Alpha:FireServer  [first 0.000 s L1]\n",
            "        e.g. Alpha:FireServer()\n",
            "  x1    Zulu:FireServer\n",
            "        e.g. Zulu:FireServer()\n",
            "  x1    Beta:FireServer\n",
            "        e.g. Beta:FireServer()\n",
            "Executor functions: frequent x2, zebra x1, alpha x1, zulu x1, beta x1\n",
            "Instances created: 4 (Frequent 2, Zebra 1, Alpha 1)\n",
            "Event handlers connected: 6 (Frequent 2, Zebra 1, Alpha 1, Zulu 1, Beta 1)\n"
        ))
    );
}

#[test]
fn independently_recorded_python_runtime_goldens_match() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/runtime_reference.json")).unwrap();
    let bytes = |v: &serde_json::Value| {
        v.as_array()
            .unwrap()
            .iter()
            .map(|n| u8::try_from(n.as_u64().unwrap()).unwrap())
            .collect::<Vec<_>>()
    };
    let mut opts = fixture["trace"]["options"].clone();
    // This golden was rendered with this explicit coordinate label by the reference.
    opts["lines_of"] = json!("the script");
    let mut actual = tracer::run(&bytes(&fixture["trace"]["source"]), &opts).unwrap();
    let mut expected = fixture["trace"]["result"].clone();
    actual["summary"]
        .as_object_mut()
        .unwrap()
        .remove("real_seconds");
    expected["summary"]
        .as_object_mut()
        .unwrap()
        .remove("real_seconds");
    assert_eq!(actual["summary"], expected["summary"]);
    assert_eq!(actual["payloads"], expected["payloads"]);
    // Only elapsed wall time in the report varies; simulated time/steps are compared.
    let elapsed = regex::Regex::new(r", [0-9]+\.[0-9]{2} s real time").unwrap();
    assert_eq!(
        elapsed.replace_all(
            actual["report"].as_str().unwrap(),
            ", <elapsed> s real time"
        ),
        elapsed.replace_all(
            expected["report"].as_str().unwrap(),
            ", <elapsed> s real time"
        )
    );
    let mut actual = moonveil::deobfuscate(
        &bytes(&fixture["moonveil"]["source"]),
        &fixture["moonveil"]["info"]["moonveil"],
        &Default::default(),
    )
    .unwrap();
    assert_eq!(actual.text.as_deref(), fixture["moonveil"]["text"].as_str());
    let mut expected = fixture["moonveil"]["info"].clone();
    actual.info.as_object_mut().unwrap().remove("timings");
    expected.as_object_mut().unwrap().remove("timings");
    assert_eq!(actual.info, expected);
}

#[test]
fn safe_vm_source_and_recovered_full_traces_match() {
    use sha2::{Digest, Sha256};
    use std::collections::BTreeMap;

    let source = include_bytes!("samples/safe_vm.lua");
    let raw = tracer::trace(source, &json!({})).unwrap();
    assert!(["ok", "timeout"].contains(&raw.status.as_str()));
    assert!(!raw.events.is_empty(), "safe VM trace must not be vacuous");
    assert_eq!(printed(&raw), vec!["safe fixture 41"]);
    assert!(!raw
        .events
        .iter()
        .any(|e| e.kind == "err" && e.b.as_deref() == Some("internal")));
    let report = tracer::render_report(&raw, "safe_vm.lua", "the script as given", None);
    assert!(report.contains("FAKE ROBLOX SANDBOX TRACE"));
    assert!(report.contains(&format!("{:x}", Sha256::digest(source))));
    let summary = tracer::summary_info(&raw);
    assert_eq!(summary["events"], json!(raw.events.len()));
    assert!(summary["findings"].is_array());

    let output = bgdeobf::engine::run(source, &Default::default(), None).unwrap();
    let recovered = tracer::trace(output.text.as_deref().unwrap().as_bytes(), &json!({})).unwrap();
    assert_eq!(raw.status, "ok");
    assert_eq!(recovered.status, "ok");
    assert_eq!(raw.reason(), recovered.reason());
    let effects = |events: &[tracer::Event]| {
        events
            .iter()
            .filter(|e| {
                matches!(
                    e.kind.as_str(),
                    "net" | "remote" | "kick" | "tp" | "load" | "exec" | "file"
                )
            })
            .map(|e| (e.kind.clone(), e.text.clone(), e.a.clone(), e.b.clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(effects(&raw.events), effects(&recovered.events));

    // Exact Python _normal/PROTECTOR_FLAG rules; only the raw protector flag is excluded.
    let line = regex::Regex::new(r"--\[\[line \d+\]\]").unwrap();
    let clock = regex::Regex::new(r"\b\d+\.\d{4,}\b").unwrap();
    let protector = regex::Regex::new(r"^getgenv\(\)\._\w+ = [01]$").unwrap();
    let normalized = |events: &[tracer::Event], raw: bool| {
        let mut counts = BTreeMap::new();
        for e in events {
            if raw && e.kind == "set" && protector.is_match(&e.text) {
                continue;
            }
            let text = line.replace_all(&e.text, "--[[line N]]");
            let text = clock.replace_all(&text, "T").into_owned();
            *counts.entry((e.kind.clone(), text)).or_insert(0_usize) += 1;
        }
        counts
    };
    assert_eq!(
        normalized(&raw.events, true),
        normalized(&recovered.events, false)
    );
}

#[cfg(target_os = "linux")]
#[test]
fn hostile_runtime_makes_no_network_process_or_disk_write_syscalls() {
    use std::{process::Command, time::Duration};

    match Command::new("strace").arg("-V").output() {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("SKIP Linux syscall audit: strace is not installed");
            return;
        }
        result => assert!(result.unwrap().status.success(), "strace -V failed"),
    }
    let work = tempfile::tempdir().unwrap();
    let logs = tempfile::tempdir().unwrap();
    let log = logs.path().join("strace.txt");
    std::fs::write(
        work.path().join("sandbox.lua"),
        tracer::build_script(HOSTILE.as_bytes(), &json!({}), false).unwrap(),
    )
    .unwrap();
    let mut command = Command::new("strace");
    command
        .args(["-f", "-o"])
        .arg(&log)
        .args(["-e", "trace=network,process,file,desc", "-s", "120"])
        .arg(bgdeobf::astutil::find_bin("luau").unwrap())
        .arg("sandbox.lua")
        .current_dir(work.path())
        .stdin(std::process::Stdio::null())
        .env_clear()
        .env("PATH", "/usr/bin:/bin");
    let captured = bgdeobf::process::capture(
        command,
        &bgdeobf::process::Limits {
            timeout: Duration::from_secs(120),
            memory_mb: 1024,
            cpu_seconds: 0,
            max_output_bytes: 64 * 1024 * 1024,
            new_group: true,
        },
    )
    .unwrap();
    assert_ne!(
        captured.status, "timeout",
        "the hostile script did not finish"
    );
    let error = String::from_utf8_lossy(&captured.err);
    let calls = std::fs::read_to_string(&log).unwrap_or_default();
    if calls.is_empty()
        || (error.to_lowercase().contains("ptrace")
            && (error.contains("Operation not permitted") || error.contains("Permission denied")))
    {
        eprintln!("SKIP Linux syscall audit: strace cannot trace processes here: {error}");
        return;
    }
    assert_eq!(captured.status, "ok", "{error}");
    assert_eq!(
        std::fs::read_dir(work.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>(),
        vec![std::ffi::OsString::from("sandbox.lua")]
    );
    let pid = regex::Regex::new(r"^\d+\s+").unwrap();
    let body = calls
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| pid.replace(line, "").into_owned())
        .collect::<Vec<_>>();
    let names = body
        .iter()
        .filter_map(|line| line.split_once('(').map(|(name, _)| name))
        .collect::<Vec<_>>();
    assert_eq!(names.iter().filter(|name| **name == "execve").count(), 1);
    for forbidden in [
        "socket",
        "connect",
        "bind",
        "listen",
        "accept",
        "accept4",
        "sendto",
        "sendmsg",
        "recvfrom",
        "fork",
        "vfork",
        "clone",
        "clone3",
        "unlink",
        "unlinkat",
        "rename",
        "renameat",
        "renameat2",
        "mkdir",
        "mkdirat",
        "rmdir",
        "symlink",
        "symlinkat",
        "link",
        "linkat",
        "chmod",
        "fchmodat",
        "truncate",
        "ftruncate",
    ] {
        assert!(
            !names.contains(&forbidden),
            "forbidden syscall: {forbidden}"
        );
    }
    let write_open = regex::Regex::new(r"O_(WRONLY|RDWR|CREAT|APPEND|TRUNC)").unwrap();
    let writes = regex::Regex::new(r"^(?:write|writev|pwrite64|pwritev)\((\d+),").unwrap();
    for call in body {
        if (call.starts_with("open(") || call.starts_with("openat(")) && !call.contains("ENOENT") {
            assert!(!write_open.is_match(&call), "write-mode open: {call}");
        }
        if let Some(fd) = writes.captures(&call) {
            assert!(["1", "2"].contains(&&fd[1]), "non-console write: {call}");
        }
        for path in ["/etc/passwd", "/etc/shadow", "bgdeobf_pwned"] {
            assert!(!call.contains(path), "hostile path reached the OS: {call}");
        }
    }
}

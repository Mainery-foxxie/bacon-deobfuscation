use bgdeobf::{astutil, engine, model::Options, runner, sandbox};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;

const SAFE: &[u8] = include_bytes!("samples/safe_vm.lua");
const BANNER: &str = "-- Bacon Guard v0.42.0 | FORGE Protection | Build 0a89d3458f6c\n";

fn raw_options() -> Options {
    Options {
        clean: false,
        ..Options::default()
    }
}

#[test]
fn invalid_inputs_return_the_reference_error_codes() {
    for (source, code) in [
        (&b""[..], "empty"),
        (&b"  \n\t "[..], "empty"),
        (&b"\x0b"[..], "empty"),
        (&b"\x1bLua\0\x01\x02\x03"[..], "binary"),
        (
            &b"print(\"hello world\")\nlocal x = 1 + 2\n"[..],
            "unsupported",
        ),
        (&b"local x = = 3 end end"[..], "syntax"),
        (&b"<html><body>404 Not Found</body></html>"[..], "syntax"),
        (&SAFE[..SAFE.len() / 2], "syntax"),
    ] {
        assert_eq!(
            engine::run(source, &raw_options(), None).unwrap_err().code,
            code
        );
    }
    let options = Options {
        max_bytes: 1000,
        ..raw_options()
    };
    assert_eq!(
        engine::run(&vec![b'a'; 2000], &options, None)
            .unwrap_err()
            .code,
        "too_big"
    );
    let marked = format!("{BANNER}print(1)\n");
    assert_eq!(
        engine::run(marked.as_bytes(), &raw_options(), None)
            .unwrap_err()
            .code,
        "unsupported"
    );
}

#[test]
fn detection_keeps_binary_syntax_and_incomplete_banner_distinct() {
    for (source, kind, confidence, detail) in [
        ("".as_bytes().to_vec(), "unknown", 0.0, "the file is empty"),
        (b"\x1bLua\0\x01\x02\x03".to_vec(), "unknown", 0.0, "binary"),
        (
            b"print(1)".to_vec(),
            "unknown",
            0.0,
            "loader layout is not recognized",
        ),
        (
            b"local x = = 3 end end".to_vec(),
            "unknown",
            0.0,
            "not valid Luau",
        ),
        (
            format!("{BANNER}print(1)").into_bytes(),
            "bacon_guard",
            0.5,
            "the banner is there, but the loader layout is not recognized",
        ),
        (
            format!("{BANNER}local x = = 3 end end").into_bytes(),
            "bacon_guard",
            0.5,
            "not valid Luau",
        ),
    ] {
        let result = engine::detect(&source, 3 * 1024 * 1024).unwrap();
        assert_eq!(result["kind"], kind);
        assert_eq!(result["confidence"], confidence);
        assert_eq!(result["supported"], false);
        let actual = result["detail"].as_str().unwrap();
        assert!(
            actual.contains(detail),
            "{actual:?} did not contain {detail:?}"
        );
        assert!(
            !actual.contains(".lua") && !actual.contains("bgdeobf") && !actual.contains("/tmp")
        );
    }
    let result = engine::detect(format!("{BANNER}print(1)").as_bytes(), 1000).unwrap();
    assert_eq!(
        result["banner"],
        json!({"version":"0.42.0","protection":"FORGE Protection","build":"0a89d3458f6c"})
    );
    assert_eq!(
        result["label"],
        "Bacon Guard v0.42.0 (FORGE Protection), build 0a89d3458f6c"
    );
    assert!(engine::detect(&vec![b'a'; 2000], 1000).unwrap()["detail"]
        .as_str()
        .unwrap()
        .contains("larger than 1000 bytes"));
    let oversized = engine::detect(&vec![b'a'; 2000], 1000).unwrap();
    assert_eq!(oversized["kind"], "unknown");
    assert_eq!(oversized["supported"], false);
    let result = engine::detect(SAFE, 3 * 1024 * 1024).unwrap();
    assert_eq!(
        (result["kind"].as_str(), result["supported"].as_bool()),
        (Some("bacon_guard"), Some(true))
    );
    assert_eq!(result["confidence"], 1.0);
    assert_eq!(result["label"], "Bacon Guard-style VM (no banner)");
    let truncated = engine::detect(&SAFE[..SAFE.len() / 2], 3 * 1024 * 1024).unwrap();
    assert_eq!(truncated["supported"], false);
    assert!(truncated["confidence"].as_f64().unwrap() < 1.0);
    assert!(truncated["detail"]
        .as_str()
        .unwrap()
        .contains("not valid Luau"));
}

#[test]
fn isolated_fixture_roundtrip_preserves_output_return_and_progress() {
    let stages = Mutex::new(Vec::new());
    let note = |stage: &str| stages.lock().unwrap().push(stage.to_string());
    let result = runner::run(
        SAFE,
        &Options::default(),
        Duration::from_secs(120),
        Some(&note),
        None,
    )
    .unwrap();
    assert_eq!(result.info["protos"], 1);
    assert_eq!(result.info["records"], 7);
    assert_eq!(
        result.info["handlers"],
        json!({"simple":3,"hot":2,"ctrl":1,"ded":0})
    );
    assert_eq!(result.info["warnings"], json!([]));
    assert_eq!(result.info["fidelity"]["differing"], json!([]));
    assert_eq!(result.info["verification"]["raw_bytecode"], "passed");
    assert_eq!(result.info["syntax_ok"], true);
    assert_eq!(result.info["fidelity"]["functions"], result.info["protos"]);
    assert_eq!(result.info["fidelity"]["functions_recompiled"], 1);
    assert_eq!(
        result.info["fidelity"]["identical"].as_u64().unwrap()
            + result.info["fidelity"]["reordered"].as_u64().unwrap()
            + result.info["fidelity"]["cosmetic"].as_u64().unwrap(),
        result.info["fidelity"]["functions"].as_u64().unwrap()
    );
    for source in [
        std::str::from_utf8(SAFE).unwrap(),
        result.text.as_deref().unwrap(),
    ] {
        let wrapped =
            format!("local result = (function()\n{source}\nend)()\nprint(\"returned\", result)\n");
        let observed =
            sandbox::run_luau(wrapped.as_bytes(), Duration::from_secs(10), 256, 1).unwrap();
        assert_eq!(
            String::from_utf8(observed).unwrap().replace("\r\n", "\n"),
            "safe fixture\t41\nreturned\t41\n"
        );
    }
    let stages = stages.into_inner().unwrap();
    for name in [
        "analyse",
        "unpack",
        "classify",
        "decompile",
        "verify",
        "clean",
    ] {
        assert!(
            stages.iter().any(|stage| stage == name),
            "missing stage {name}: {stages:?}"
        );
    }
}

#[test]
fn balanced_dispatch_fixture_decompiles_without_changing_behavior() {
    let mut source = String::from_utf8(astutil::normalize_source(SAFE)).unwrap();
    let hot_start = source
        .find("                    if op == 30 then\n")
        .unwrap();
    let hot_end = hot_start
        + source[hot_start..]
            .find("                elseif activeSimpleSet")
            .unwrap();
    source.replace_range(
        hot_start..hot_end,
        concat!(
            "                    if op <= 30 then\n",
            "                        if op == 30 then\n",
            "                            if clear then table.clear(rec) end\n",
            "                            return table.unpack(regs, a, b == 0 and top or a + b - 2)\n",
            "                        else error(\"bad hot opcode\") end\n",
            "                    else\n",
            "                        if op == 31 then regs[a] = rf else error(\"bad hot opcode\") end\n",
            "                    end\n",
        ),
    );
    let control_start = source.find("                if op == 20 then\n").unwrap();
    let control_end = control_start
        + source[control_start..]
            .find("            end\n            local profile")
            .unwrap();
    let original = source[control_start..control_end].to_string();
    source.replace_range(
        control_start..control_end,
        &format!(
            "                if op <= 20 then\n{original}                else\n                     if op == 21 then return nxt, stackTop else error(\"bad control opcode\") end\n                 end\n"
        ),
    );
    let detected = engine::detect(source.as_bytes(), Options::default().max_bytes).unwrap();
    assert_eq!(detected["supported"], true);
    assert_eq!(detected["confidence"], 1.0);
    let stages = Mutex::new(Vec::new());
    let note = |stage: &str| stages.lock().unwrap().push(stage.to_string());
    let result = runner::run(
        source.as_bytes(),
        &Options::default(),
        Duration::from_secs(120),
        Some(&note),
        None,
    )
    .unwrap();
    assert_eq!(result.info["syntax_ok"], true);
    assert_eq!(result.info["protos"], 1);
    assert_eq!(result.info["fidelity"]["functions"], 1);
    assert_eq!(result.info["fidelity"]["functions_recompiled"], 1);
    assert_eq!(result.info["fidelity"]["differing"], json!([]));
    assert_eq!(
        result.info["fidelity"]["identical"].as_u64().unwrap()
            + result.info["fidelity"]["reordered"].as_u64().unwrap()
            + result.info["fidelity"]["cosmetic"].as_u64().unwrap(),
        1
    );
    assert_eq!(result.info["verification"]["raw_bytecode"], "passed");
    let stages = stages.into_inner().unwrap();
    for stage in ["classify", "decompile", "verify"] {
        assert!(stages.iter().any(|seen| seen == stage), "{stages:?}");
    }
    for text in [source.as_str(), result.text.as_deref().unwrap()] {
        let wrapped =
            format!("local result = (function()\n{text}\nend)()\nprint(\"returned\", result)\n");
        let observed =
            sandbox::run_luau(wrapped.as_bytes(), Duration::from_secs(10), 256, 1).unwrap();
        assert_eq!(
            String::from_utf8(observed).unwrap().replace("\r\n", "\n"),
            "safe fixture\t41\nreturned\t41\n"
        );
    }
}

fn check_damaged_fixture(seed: u64) {
    let vectors: Value =
        serde_json::from_str(include_str!("fixtures/engine_damage_vectors.json")).unwrap();
    let mut data = SAFE.to_vec();
    let source_hash = format!("{:x}", Sha256::digest(&data));
    let source = vectors
        .as_array()
        .unwrap()
        .iter()
        .find(|source| source["sha256"].as_str() == Some(source_hash.as_str()))
        .expect("safe fixture changed; regenerate vectors with the original Python random loop");
    assert_eq!(source["length"].as_u64().unwrap() as usize, data.len());
    let case = source["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["seed"] == seed)
        .unwrap();
    let attempts = case["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 60);
    for attempt in attempts {
        let index = attempt[0].as_u64().unwrap() as usize;
        assert!((data.len() / 3..data.len() - 10).contains(&index));
        assert_eq!(u64::from(data[index]), attempt[1].as_u64().unwrap());
        let replacement = attempt[2].as_u64();
        assert_eq!(replacement.is_some(), data[index].is_ascii_alphanumeric());
        if let Some(replacement) = replacement {
            let replacement = u8::try_from(replacement).unwrap();
            assert!(b"abcXYZ019".contains(&replacement));
            data[index] = replacement;
        }
    }
    assert_eq!(
        format!("{:x}", Sha256::digest(&data)),
        case["sha256"].as_str().unwrap()
    );
    match runner::run(
        &data,
        &Options::default(),
        Duration::from_secs(200),
        None,
        None,
    ) {
        Ok(result) => assert!(result
            .text
            .as_deref()
            .is_some_and(|text| !text.trim().is_empty())),
        Err(error) => assert!(
            matches!(
                error.code.as_str(),
                "sandbox"
                    | "decode"
                    | "unsupported"
                    | "decompile"
                    | "syntax"
                    | "timeout"
                    | "memory"
            ),
            "seed {seed}: {error:?}"
        ),
    }
}

#[test]
fn damaged_script_seed_1_never_crashes_or_hangs() {
    check_damaged_fixture(1);
}

#[test]
fn damaged_script_seed_2_never_crashes_or_hangs() {
    check_damaged_fixture(2);
}

#[test]
fn damaged_script_seed_3_never_crashes_or_hangs() {
    check_damaged_fixture(3);
}

#[test]
fn direct_sandbox_retains_runtime_error_code_and_detail() {
    let error = sandbox::run_luau(b"error('boom')", Duration::from_secs(10), 256, 1).unwrap_err();
    assert_eq!(error.code, "sandbox");
    assert!(error.message.contains("boom"), "{error:?}");
    assert_eq!(
        sandbox::run_luau(b"print('ok')", Duration::from_secs(10), 256, 1).unwrap(),
        b"ok\n"
    );
}

#[test]
fn direct_sandbox_getfenv_has_no_io_or_os_execute() {
    let output = sandbox::run_luau(
        concat!(
            "print(type(io), type(os), type(os and os.execute), type(require), type(loadstring))\n",
            "local env = getfenv()\n",
            "print(type(env.io), type(env.os), type(env.os and env.os.execute), type(env.require), type(env.loadstring))\n",
        )
        .as_bytes(),
        Duration::from_secs(10),
        256,
        1,
    )
    .unwrap();
    let output = String::from_utf8(output).unwrap();
    let lines: Vec<_> = output.lines().collect();
    assert_eq!(lines.len(), 2);
    for line in lines {
        assert_eq!(
            line.split_whitespace().take(3).collect::<Vec<_>>(),
            ["nil", "table", "nil"]
        );
    }
}

#[test]
fn no_verify_annotations_and_name_maps_preserve_their_scope() {
    let options = Options {
        verify: false,
        trace_opts: Some(json!({"timeout":0})),
        ..raw_options()
    };
    let stages = Mutex::new(Vec::new());
    let note = |stage: &str| stages.lock().unwrap().push(stage.to_string());
    let first = engine::run(SAFE, &options, Some(&note)).unwrap();
    let second = engine::run(SAFE, &options, None).unwrap();
    assert_eq!(first.text, second.text);
    assert!(first.info.get("fidelity").is_none() && first.info.get("syntax_ok").is_none());
    assert!(first.info.get("annotated").is_none());
    assert!(first.trace.is_none());
    assert!(first.info.get("trace").is_none());
    assert_eq!(first.info["warnings"], json!([]));
    let stages = stages.into_inner().unwrap();
    for stage in ["analyse", "unpack", "classify", "decompile"] {
        assert!(stages.iter().any(|seen| seen == stage), "{stages:?}");
    }
    assert!(
        !stages
            .iter()
            .any(|stage| matches!(stage.as_str(), "verify" | "clean" | "sandbox")),
        "{stages:?}"
    );
    for key in [
        "raw_syntax",
        "delivered_syntax",
        "raw_bytecode",
        "fake_trace",
    ] {
        assert_eq!(first.info["verification"][key], "not run");
    }
    let annotated = engine::run(
        SAFE,
        &Options {
            annotate: true,
            ..raw_options()
        },
        None,
    )
    .unwrap();
    assert_eq!(annotated.info["verification"]["raw_syntax"], "passed");
    assert_eq!(annotated.info["verification"]["raw_bytecode"], "passed");
    assert_eq!(annotated.info["annotated"], true);
    assert_eq!(annotated.info["syntax_ok"], true);
    assert_eq!(annotated.info["warnings"], json!([]));
    assert_eq!(annotated.info["fidelity"]["differing"], json!([]));
    assert_eq!(
        annotated.info["verification"]["delivered_syntax"],
        "not runnable"
    );
    assert_eq!(
        annotated.info["verification"]["raw_applies_to_delivery"],
        false
    );
    assert!(annotated.raw.is_none());
    let markers = regex::Regex::new(r"<@(\d+:(?:p\d+|\d+))>").unwrap();
    let text = annotated.text.unwrap();
    assert_eq!(markers.replace_all(&text, ""), first.text.unwrap());
    let key = markers.captures(&text).unwrap()[1].to_string();
    let named = engine::run(
        SAFE,
        &Options {
            names: Some(BTreeMap::from([(key, "myCustomName".into())])),
            ..options
        },
        None,
    )
    .unwrap();
    assert!(named.text.unwrap().contains("myCustomName"));
}

#[test]
fn trace_reports_identify_the_source_they_actually_observed() {
    let options = Options {
        trace: true,
        trace_opts: Some(json!({"max_time":0.1,"timeout":10,"lines_of":"wrong source"})),
        ..raw_options()
    };
    for annotate in [false, true] {
        let result = engine::run(
            SAFE,
            &Options {
                annotate,
                ..options.clone()
            },
            None,
        )
        .unwrap();
        assert_eq!(result.info["verification"]["fake_trace"], "observed");
        assert!(result.trace.unwrap()["report"]
            .as_str()
            .unwrap()
            .contains("the deobfuscated script"));
    }
    let result = engine::run(
        b"print(\"as given\")",
        &Options {
            sandbox_only: true,
            ..options
        },
        None,
    )
    .unwrap();
    assert!(result.text.is_none() && result.raw.is_none());
    assert_eq!(result.info["sandbox_only"], true);
    assert!(result.trace.unwrap()["report"]
        .as_str()
        .unwrap()
        .contains("the script as given"));
}

#[test]
fn optional_trace_failure_retains_source_and_reports_unavailable() {
    let baseline = engine::run(SAFE, &raw_options(), None).unwrap();
    let stages = Mutex::new(Vec::new());
    let note = |stage: &str| stages.lock().unwrap().push(stage.to_string());
    let result = engine::run(
        SAFE,
        &Options {
            trace: true,
            trace_opts: Some(json!({"timeout":0})),
            ..raw_options()
        },
        Some(&note),
    )
    .unwrap();
    assert!(result
        .text
        .as_deref()
        .is_some_and(|text| !text.trim().is_empty()));
    assert_eq!(result.text, baseline.text);
    assert_eq!(result.info["syntax_ok"], true);
    assert_eq!(result.info["verification"]["raw_bytecode"], "passed");
    assert_eq!(result.info["verification"]["fake_trace"], "unavailable");
    assert!(result.trace.is_none() && result.info.get("trace").is_none());
    assert!(result.raw.is_none());
    assert!(result.info["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|warning| {
            warning.as_str().is_some_and(|warning| {
                warning.contains("the fake Roblox run failed")
                    && warning.contains("invalid sandbox option timeout")
            })
        }));
    assert!(stages
        .into_inner()
        .unwrap()
        .iter()
        .any(|stage| stage == "sandbox"));
}

fn check_authorized_numeric_build_roundtrip(sample: &str) {
    assert_eq!(
        std::env::var("BGDEOBF_AUTHORIZED_SAMPLES").as_deref(),
        Ok("1"),
        "authorized numeric engine runs require BGDEOBF_AUTHORIZED_SAMPLES=1 and all three local artifacts"
    );
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("results");
    for required in [
        "loveyou_fetched.lua",
        "QxU6VyGTVW_fetched.lua",
        "F2naYujWnI_fetched.lua",
    ] {
        assert!(
            directory.join(required).is_file(),
            "authorized numeric engine fixture required: results/{required}"
        );
    }
    let data = std::fs::read(directory.join(sample)).unwrap();
    assert_eq!(
        engine::detect(&data, Options::default().max_bytes).unwrap()["supported"],
        true,
        "{sample}"
    );
    let result = runner::run(
        &data,
        &Options::default(),
        Duration::from_secs(240),
        None,
        None,
    )
    .unwrap();
    let text = result.text.as_deref().unwrap();
    assert!(!text.trim().is_empty(), "{sample}");
    assert_eq!(result.info["syntax_ok"], true, "{sample}");
    let fidelity = &result.info["fidelity"];
    assert_eq!(fidelity["differing"], json!([]), "{sample}");
    assert_eq!(
        fidelity["identical"].as_u64().unwrap()
            + fidelity["reordered"].as_u64().unwrap()
            + fidelity["cosmetic"].as_u64().unwrap(),
        fidelity["functions"].as_u64().unwrap(),
        "{sample}"
    );
    assert_eq!(fidelity["functions"], result.info["protos"], "{sample}");
    assert_eq!(
        fidelity["functions_recompiled"], fidelity["functions"],
        "{sample}"
    );
    assert_eq!(
        result.info["verification"]["raw_bytecode"], "passed",
        "{sample}"
    );
    let root = astutil::parse_ast(text.as_bytes()).unwrap();
    let mut declarations = Vec::new();
    for (node, _) in astutil::walk(&root) {
        match node["type"].as_str().unwrap_or("") {
            "AstStatLocal" | "AstStatForIn" => {
                declarations.extend(node["vars"].as_array().unwrap().iter());
            }
            "AstStatLocalFunction" => declarations.push(&node["name"]),
            "AstExprFunction" => declarations.extend(node["args"].as_array().unwrap().iter()),
            "AstStatFor" => declarations.push(&node["var"]),
            _ => {}
        }
    }
    assert!(!declarations.is_empty(), "{sample}");
    let generic_name = regex::Regex::new(r"^[vp]\d+$").unwrap();
    for declaration in declarations {
        let name = declaration["name"].as_str().unwrap();
        assert!(
            !generic_name.is_match(name),
            "{sample}: generic local name {name}"
        );
    }
    assert!(
        result.info["cleaned"]["statements"].as_u64().unwrap_or(0) > 0,
        "{sample}"
    );
}

#[test]
#[ignore = "requires opt-in BGDEOBF_AUTHORIZED_SAMPLES=1 and all three authorized local numeric artifacts, including results/loveyou_fetched.lua"]
fn authorized_numeric_loveyou_build_roundtrip() {
    check_authorized_numeric_build_roundtrip("loveyou_fetched.lua");
}

#[test]
#[ignore = "requires opt-in BGDEOBF_AUTHORIZED_SAMPLES=1 and all three authorized local numeric artifacts, including results/QxU6VyGTVW_fetched.lua"]
fn authorized_numeric_qxu6_build_roundtrip() {
    check_authorized_numeric_build_roundtrip("QxU6VyGTVW_fetched.lua");
}

#[test]
#[ignore = "requires opt-in BGDEOBF_AUTHORIZED_SAMPLES=1 and all three authorized local numeric artifacts, including results/F2naYujWnI_fetched.lua"]
fn authorized_numeric_f2na_build_roundtrip() {
    check_authorized_numeric_build_roundtrip("F2naYujWnI_fetched.lua");
}

#[test]
fn compiler_errors_hide_paths_and_depth_exhaustion_is_unavailable() {
    let result = engine::compile_check("local x = = 1\n").unwrap();
    assert_eq!(result["ok"], false);
    assert!(result["bytecode"].is_null());
    let message = result["message"].as_str().unwrap();
    assert!(message.starts_with("line 1:11: SyntaxError"), "{message}");
    assert!(!message.contains(".lua") && !message.contains("bgdeobf"));
    let result = engine::compile_check(&"local x = = 1\n".repeat(8)).unwrap();
    let message = result["message"].as_str().unwrap();
    assert_eq!(message.matches("SyntaxError").count(), 3);
    assert!(message.ends_with("more)"));
    let result = engine::compile_check("print(\"fine\")\n").unwrap();
    assert_eq!(result["ok"], true);
    assert_eq!(result["message"], "");
    assert!(!result["bytecode"].as_str().unwrap().is_empty());
    let nested = format!("return {}1{}\n", "(".repeat(1000), ")".repeat(1000));
    let result = engine::compile_check(&nested).unwrap();
    assert!(result["ok"].is_null() && result["bytecode"].is_null());
    assert!(result["message"]
        .as_str()
        .unwrap()
        .contains("compile check unavailable"));
}

#[test]
fn trace_payload_substitution_preserves_bytes_and_rejects_path_extensions() {
    let mut trace = json!({"report":"@@NAME@@ uses @@STEM@@_payload1.lua", "payloads":[{"id":1,"ext":"lua","data":[0,255,195,169]}]});
    let (report, files) = engine::fill_trace(&trace, "fixture.lua", "fixture").unwrap();
    assert_eq!(report, "fixture.lua uses fixture_payload1.lua");
    assert_eq!(
        files,
        vec![("fixture_payload1.lua".into(), vec![0, 255, 195, 169])]
    );
    trace["payloads"][0]["ext"] = Value::String("../escape".into());
    assert!(engine::fill_trace(&trace, "fixture.lua", "fixture").is_err());
}

#[test]
fn forced_moonveil_is_explicit_preserves_banners_and_survives_the_worker() {
    let source = b"print('fixture') local body=game:HttpGet('https://example.invalid/lib.lua')\n";
    assert_eq!(
        engine::run(source, &raw_options(), None).unwrap_err().code,
        "unsupported"
    );
    let options = Options {
        moonveil: true,
        trace: true,
        sandbox_timeout: 10,
        trace_opts: Some(json!({"max_time":0.1,"timeout":10})),
        ..raw_options()
    };
    let result = runner::run(source, &options, Duration::from_secs(60), None, None).unwrap();
    assert_eq!(
        result.info["moonveil"],
        json!({"version":"?","assumed":true})
    );
    assert_eq!(result.info["trace_only"], true);
    let text = result.text.unwrap();
    let header = text.split("]]").next().unwrap();
    assert!(
        header.starts_with("--[[\n  BEHAVIOUR RECONSTRUCTION - THIS IS NOT THE ORIGINAL SOURCE.")
    );
    assert!(header.contains("no MoonVeil banner") && !header.contains("MoonVeil ?"));
    let trace = result.trace.unwrap();
    assert!(trace["report"]
        .as_str()
        .unwrap()
        .contains("the script as given"));
    assert_eq!(trace["summary"]["requests"], 1);
    let marked = format!(
        "-- This script was generated using MoonVeil v2.1.1\n{}",
        std::str::from_utf8(source).unwrap()
    );
    let result = engine::run(
        marked.as_bytes(),
        &Options {
            trace: false,
            ..options.clone()
        },
        None,
    )
    .unwrap();
    assert_eq!(result.info["moonveil"], json!({"version":"2.1.1"}));
    assert!(result.trace.is_none() && result.info.get("trace").is_none());
    assert!(result
        .text
        .unwrap()
        .contains("protected with MoonVeil 2.1.1"));
    let error = engine::run(
        format!("{BANNER}error('must never execute')").as_bytes(),
        &options,
        None,
    )
    .unwrap_err();
    assert_eq!(error.code, "unsupported");
    assert!(error.message.contains("Bacon Guard"));
}

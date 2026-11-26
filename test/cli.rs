use bgdeobf::cli::expand_inputs;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

const SAFE: &[u8] = include_bytes!("samples/safe_vm.lua");

fn command(directory: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_bgdeobf"));
    command.current_dir(directory);
    command
}

fn success(output: &Output) {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn safe_variant(label: &str) -> Vec<u8> {
    let source = std::str::from_utf8(SAFE).unwrap().replace("\r\n", "\n");
    let constant = "[3] = \"safe fixture\"";
    assert_eq!(source.matches(constant).count(), 1);
    source
        .replace(constant, &format!("[3] = \"{label}\""))
        .into_bytes()
}

fn assert_output_label(path: &Path, label: &str) {
    let text = fs::read_to_string(path).unwrap();
    assert!(
        text.contains(&format!("\"{label}\"")),
        "{} did not retain {label:?}: {text}",
        path.display()
    );
}

fn file_names(directory: &Path) -> Vec<String> {
    let mut names: Vec<_> = fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

#[test]
fn folders_are_sorted_nonrecursive_and_skip_generated_artifacts() {
    let directory = tempfile::tempdir().unwrap();
    for name in [
        "b.lua",
        "a.txt",
        "c.luau",
        "notes.md",
        "a.deobf.lua",
        "a_trace.txt",
        "B_VERIFICATION.TXT",
        "c_payload12.lua",
        "d_annotated.lua",
        "e_reconstructed.lua",
        "f_display.txt",
    ] {
        fs::write(directory.path().join(name), b"fixture").unwrap();
    }
    fs::create_dir(directory.path().join("sub")).unwrap();
    fs::write(directory.path().join("sub/deep.lua"), b"fixture").unwrap();
    let (files, problems) = expand_inputs(&[directory.path().to_path_buf()]);
    assert!(problems.is_empty());
    let names: Vec<_> = files
        .iter()
        .map(|path| path.file_name().unwrap().to_string_lossy().to_string())
        .collect();
    assert_eq!(names, ["a.txt", "b.lua", "c.luau"]);
}

#[test]
fn explicitly_named_generated_files_are_kept_and_deduplicated() {
    let directory = tempfile::tempdir().unwrap();
    let input = directory.path().join("a_trace.txt");
    fs::write(&input, b"fixture").unwrap();
    let (files, problems) = expand_inputs(&[input.clone(), input.clone()]);
    assert_eq!(files, vec![input]);
    assert!(problems.is_empty());
}

#[test]
fn empty_directory_reports_the_existing_input_error() {
    let directory = tempfile::tempdir().unwrap();
    let (files, problems) = expand_inputs(&[directory.path().to_path_buf()]);
    assert!(files.is_empty());
    assert_eq!(problems.len(), 1);
    assert!(problems[0].starts_with("No .lua / .luau / .txt scripts found"));
}

#[test]
fn real_cli_preserves_default_annotated_and_explicit_output_names() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("my script.lua"), SAFE).unwrap();
    let output = command(directory.path())
        .args(["my script.lua", "-q"])
        .output()
        .unwrap();
    success(&output);
    assert!(output.stdout.is_empty() && output.stderr.is_empty());
    assert!(
        fs::read_to_string(directory.path().join("my script.deobf.lua"))
            .unwrap()
            .contains("safe fixture")
    );
    let output = command(directory.path())
        .args(["my script.lua", "--annotate", "-q"])
        .output()
        .unwrap();
    success(&output);
    assert!(
        fs::read_to_string(directory.path().join("my script_annotated.lua"))
            .unwrap()
            .contains("<@")
    );
    let explicit = tempfile::tempdir().unwrap();
    fs::write(explicit.path().join("my script.lua"), SAFE).unwrap();
    let output = command(explicit.path())
        .args(["my script.lua", "--annotate", "-o", "result.lua"])
        .output()
        .unwrap();
    success(&output);
    assert!(explicit.path().join("result.lua").is_file());
    assert!(!explicit.path().join("my script_annotated.lua").exists());
    assert!(!explicit.path().join("my script.deobf.lua").exists());
    let summary = String::from_utf8(output.stdout).unwrap();
    assert!(
        summary.contains("Annotated copy")
            && summary.contains("Done in ")
            && summary.contains("result.lua")
    );
}

#[test]
fn real_cli_errors_keep_exit_codes_and_leave_no_result() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("ordinary.lua"), b"print(1)").unwrap();
    let output = command(directory.path())
        .args(["ordinary.lua", "-q"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let message = String::from_utf8(output.stderr).unwrap();
    assert!(message.starts_with("FAILED: ") && message.contains("won't guess"));
    assert!(!directory.path().join("ordinary.deobf.lua").exists());
    let output = command(directory.path())
        .arg("missing.lua")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("Cannot read the file"));
    fs::write(directory.path().join("names.json"), b"[]").unwrap();
    let output = command(directory.path())
        .args(["ordinary.lua", "--names", "names.json"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("Invalid --names file"));
}

#[test]
fn real_cli_rejects_incompatible_modes_before_processing_input() {
    let directory = tempfile::tempdir().unwrap();
    for flags in [
        vec!["--jobs", "0"],
        vec!["--jobs", "2", "--in-process"],
        vec!["--trace", "--sandbox"],
        vec!["--sandbox", "--annotate"],
        vec!["--sandbox", "--names", "names.json"],
        vec!["--explore"],
        vec!["--user-id", "42"],
        vec!["--place-id", "42"],
        vec!["--players", "1"],
        vec!["--username", "FixturePlayer"],
        vec!["--executor", "FixtureExecutor"],
        vec!["--responses", "responses.json"],
        vec!["-", "other.lua"],
    ] {
        let output = command(directory.path())
            .arg("missing.lua")
            .args(&flags)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{flags:?}");
        assert!(
            !String::from_utf8_lossy(&output.stderr).contains("Cannot read the file"),
            "{flags:?}"
        );
    }
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
}

#[test]
fn real_cli_batch_json_retains_input_order_and_continues_after_errors() {
    let directory = tempfile::tempdir().unwrap();
    for name in ["one", "two", "results"] {
        fs::create_dir(directory.path().join(name)).unwrap();
    }
    fs::write(
        directory.path().join("one/x.lua"),
        safe_variant("FIRST_NATIVE"),
    )
    .unwrap();
    fs::write(
        directory.path().join("two/x.lua"),
        safe_variant("SECOND_NATIVE"),
    )
    .unwrap();
    fs::write(directory.path().join("bad.lua"), b"print(1)").unwrap();
    let output = command(directory.path())
        .args([
            "one/x.lua",
            "bad.lua",
            "two/x.lua",
            "missing.lua",
            "--jobs",
            "3",
            "-o",
            "results",
            "--json",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let rows: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let rows = rows.as_array().unwrap();
    assert_eq!(
        rows.iter()
            .map(|row| row["file"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["one/x.lua", "bad.lua", "two/x.lua", "missing.lua"]
    );
    assert_eq!(
        rows.iter()
            .map(|row| row["ok"].as_bool().unwrap())
            .collect::<Vec<_>>(),
        [true, false, true, false]
    );
    assert_eq!(rows[1]["code"], "unsupported");
    assert_eq!(rows[3]["code"], "unreadable");
    assert!(rows[1]["info"].is_null() && rows[3]["info"].is_null());
    for (index, label) in [(0, "FIRST_NATIVE"), (2, "SECOND_NATIVE")] {
        assert!(rows[index]["code"].is_null() && rows[index]["error"].is_null());
        assert_eq!(rows[index]["info"]["protos"], 1);
        let path = directory
            .path()
            .join(rows[index]["output"].as_str().unwrap());
        assert_output_label(&path, label);
    }
    for name in ["x.deobf.lua", "x_2.deobf.lua"] {
        assert!(directory.path().join("results").join(name).is_file());
    }
    assert!(!directory.path().join("results/bad.deobf.lua").exists());
    assert_eq!(
        file_names(&directory.path().join("results")),
        ["x.deobf.lua", "x_2.deobf.lua"]
    );
    assert!(!directory.path().join("one/x.deobf.lua").exists());
    assert!(!directory.path().join("two/x.deobf.lua").exists());
    assert_eq!(
        String::from_utf8_lossy(&output.stderr)
            .matches("FAILED: ")
            .count(),
        2
    );
}

#[test]
fn real_cli_single_output_folder_and_unverified_summary_work() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("fixture.lua"), SAFE).unwrap();
    let output = command(directory.path())
        .args(["fixture.lua", "--no-verify", "--no-clean", "-o", "nested/"])
        .output()
        .unwrap();
    success(&output);
    assert!(directory.path().join("nested/fixture.deobf.lua").is_file());
    assert!(String::from_utf8_lossy(&output.stdout).contains("PARTIAL"));
    assert!(!directory
        .path()
        .join("nested/fixture_verification.txt")
        .exists());
    let output = command(directory.path())
        .args([
            "fixture.lua",
            "--no-verify",
            "--no-clean",
            "--in-process",
            "-o",
            "direct.lua",
            "-q",
        ])
        .output()
        .unwrap();
    success(&output);
    assert_eq!(
        fs::read(directory.path().join("direct.lua")).unwrap(),
        fs::read(directory.path().join("nested/fixture.deobf.lua")).unwrap()
    );
}

#[test]
fn real_cli_detect_never_executes_the_script_and_reads_stdin() {
    let directory = tempfile::tempdir().unwrap();
    let source = String::from_utf8(SAFE.to_vec())
        .unwrap()
        .replace("\r\n", "\n")
        .replace(
            "local function module(host, env)\n",
            "local function module(host, env)\nerror(\"must never execute\")\n",
        );
    assert!(source.contains("must never execute"));
    fs::write(directory.path().join("detect.lua"), source).unwrap();
    let output = command(directory.path())
        .args(["detect.lua", "--detect", "--json"])
        .output()
        .unwrap();
    success(&output);
    let rows: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(rows[0]["supported"], true);
    assert_eq!(rows[0]["confidence"], 1.0);
    assert!(!directory.path().join("detect.deobf.lua").exists());
    let mut child = command(directory.path())
        .args(["-", "--detect", "--json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(SAFE).unwrap();
    let output = child.wait_with_output().unwrap();
    success(&output);
    let rows: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(rows[0]["file"], "-");
    assert_eq!(rows[0]["supported"], true);
}

#[test]
fn real_cli_sandbox_writes_named_reports_and_binary_payloads() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("fixture.lua"), b"local f=loadstring(game:HttpGet('https://example.invalid/fixture.lua')) if f then f() end").unwrap();
    fs::write(
        directory.path().join("responses.json"),
        br#"{"https://example.invalid/fixture.lua":"print(\"captured fixture\")"}"#,
    )
    .unwrap();
    let output = command(directory.path())
        .args([
            "fixture.lua",
            "--sandbox",
            "--responses",
            "responses.json",
            "--sandbox-time",
            "0.1",
            "--sandbox-timeout",
            "10",
            "--json",
        ])
        .output()
        .unwrap();
    success(&output);
    let info: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(info["sandbox_only"], true);
    let report = fs::read_to_string(directory.path().join("fixture_trace.txt")).unwrap();
    assert!(report.contains("fixture.lua") && report.contains("fixture_payload"));
    assert!(!report.contains("@@NAME@@") && !report.contains("@@STEM@@"));
    assert!(!directory.path().join("fixture.deobf.lua").exists());
    let payloads: Vec<_> = fs::read_dir(directory.path())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("fixture_payload")
        })
        .collect();
    assert!(!payloads.is_empty());
    assert!(payloads
        .iter()
        .any(|path| fs::read(path).unwrap() == b"print(\"captured fixture\")"));
}

#[test]
fn real_cli_reports_each_bundled_tool() {
    let directory = tempfile::tempdir().unwrap();
    let output = command(directory.path()).arg("--check").output().unwrap();
    success(&output);
    let text = String::from_utf8(output.stdout).unwrap();
    for tool in ["luau", "luau-ast", "luau-compile"] {
        assert!(
            text.lines()
                .any(|line| line.starts_with(tool) && line.contains("OK")),
            "{text}"
        );
    }
}

#[test]
fn compiler_process_failures_never_claim_invalid_lua_or_complete_verification() {
    let directory = tempfile::tempdir().unwrap();
    let stub_source = directory.path().join("compiler_fixture.rs");
    fs::write(&stub_source, r#"
fn main() {
    let exe = std::env::current_exe().unwrap();
    let mode = exe.parent().unwrap().file_name().unwrap().to_str().unwrap();
    match mode {
        "sentinel" => { std::fs::write(exe.parent().unwrap().join("invoked.txt"), b"compiler ran").unwrap(); std::process::exit(7); }
        "syntax" => { eprintln!("script.lua(1,1): SyntaxError: rejected"); std::process::exit(1); }
        "depth" => { eprintln!("script.lua(1,1): SyntaxError: Exceeded allowed recursion depth"); std::process::exit(1); }
        "memory" => { eprintln!("compiler could not allocate memory"); std::process::exit(1); }
        "syntax_crash" => { eprintln!("script.lua(1,1): SyntaxError: rejected before crash"); std::process::exit(139); }
        "access_violation" => std::process::exit(0xc0000005u32 as i32),
        _ => std::process::exit(139),
    }
}
"#).unwrap();
    let stub_binary = directory.path().join(if cfg!(windows) {
        "compiler_fixture.exe"
    } else {
        "compiler_fixture"
    });
    let compiler = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let built = Command::new(compiler)
        .arg("--edition=2021")
        .arg(&stub_source)
        .arg("-o")
        .arg(&stub_binary)
        .output()
        .unwrap();
    success(&built);
    fs::write(directory.path().join("fixture.lua"), SAFE).unwrap();
    for (mode, expected) in [
        ("crash", "unavailable"),
        ("access_violation", "unavailable"),
        ("memory", "unavailable"),
        ("syntax_crash", "unavailable"),
        ("depth", "unavailable"),
        ("syntax", "failed"),
    ] {
        let tools = directory.path().join(mode);
        fs::create_dir(&tools).unwrap();
        fs::copy(
            &stub_binary,
            tools.join(if cfg!(windows) {
                "luau-compile.exe"
            } else {
                "luau-compile"
            }),
        )
        .unwrap();
        let output = command(directory.path())
            .env("BGDEOBF_LUAU_DIR", &tools)
            .args([
                "fixture.lua",
                "--in-process",
                "--no-clean",
                "--json",
                "-q",
                "-o",
                "result.lua",
            ])
            .output()
            .unwrap();
        success(&output);
        let info: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(info["verification"]["raw_syntax"], expected, "mode {mode}");
        assert_eq!(
            info["verification"]["delivered_syntax"], expected,
            "mode {mode}"
        );
        assert_eq!(
            info["verification"]["raw_bytecode"], "incomplete",
            "mode {mode}"
        );
        let warnings = info["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(
            warnings.contains("the result has a syntax error"),
            expected == "failed",
            "mode {mode}: {warnings}"
        );
        assert!(info.get("fidelity").is_none(), "mode {mode}");
    }

    let tools = directory.path().join("sentinel");
    fs::create_dir(&tools).unwrap();
    fs::copy(
        &stub_binary,
        tools.join(if cfg!(windows) {
            "luau-compile.exe"
        } else {
            "luau-compile"
        }),
    )
    .unwrap();
    let output = command(directory.path())
        .env("BGDEOBF_LUAU_DIR", &tools)
        .args([
            "fixture.lua",
            "--in-process",
            "--no-verify",
            "--no-clean",
            "--json",
            "-q",
            "--sandbox-time=-1",
            "-o",
            "unverified.lua",
        ])
        .output()
        .unwrap();
    success(&output);
    let info: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    for key in [
        "raw_syntax",
        "delivered_syntax",
        "raw_bytecode",
        "fake_trace",
    ] {
        assert_eq!(info["verification"][key], "not run");
    }
    assert!(info.get("fidelity").is_none() && info.get("syntax_ok").is_none());
    assert!(info.get("trace").is_none());
    assert_eq!(info["warnings"], serde_json::json!([]));
    assert!(!tools.join("invoked.txt").exists());
    assert!(!directory.path().join("unverified_trace.txt").exists());

    let work = directory.path().join("skipped_worker");
    fs::create_dir(&work).unwrap();
    fs::write(work.join("in.lua"), SAFE).unwrap();
    let options = bgdeobf::model::Options {
        verify: false,
        clean: false,
        trace: false,
        trace_opts: Some(serde_json::json!({"timeout":0,"max_time":-1})),
        ..Default::default()
    };
    fs::write(work.join("cfg.json"), serde_json::to_vec(&options).unwrap()).unwrap();
    let output = command(directory.path())
        .env("BGDEOBF_LUAU_DIR", &tools)
        .arg("--worker")
        .arg(&work)
        .output()
        .unwrap();
    success(&output);
    let messages: Vec<serde_json::Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(!messages.iter().any(|message| matches!(
        message["stage"].as_str(),
        Some("verify" | "clean" | "sandbox")
    )));
    let result = messages.last().unwrap();
    assert_eq!(result["ok"], true);
    assert_eq!(result["info"]["verification"]["fake_trace"], "not run");
    assert!(result["info"].get("trace").is_none());
    assert_eq!(result["info"]["warnings"], serde_json::json!([]));
    assert!(!work.join("trace.json").exists());
    assert!(!tools.join("invoked.txt").exists());
    assert!(fs::read_to_string(work.join("out.lua"))
        .unwrap()
        .contains("safe fixture"));
}

#[test]
fn real_cli_batch_outputs_never_replace_explicit_inputs() {
    for jobs in ["1", "2"] {
        let directory = tempfile::tempdir().unwrap();
        let first = safe_variant("FIRST_INPUT");
        let protected = safe_variant("KEEP_THIS_INPUT");
        fs::write(directory.path().join("ui.lua"), &first).unwrap();
        fs::write(directory.path().join("ui.deobf.lua"), &protected).unwrap();
        let output = command(directory.path())
            .args([
                "ui.lua",
                "ui.deobf.lua",
                "--jobs",
                jobs,
                "--no-verify",
                "--no-clean",
                "-q",
            ])
            .output()
            .unwrap();
        success(&output);
        assert!(output.stdout.is_empty() && output.stderr.is_empty());
        assert_eq!(fs::read(directory.path().join("ui.lua")).unwrap(), first);
        assert_eq!(
            fs::read(directory.path().join("ui.deobf.lua")).unwrap(),
            protected
        );
        assert_output_label(&directory.path().join("ui_2.deobf.lua"), "FIRST_INPUT");
        assert_output_label(
            &directory.path().join("ui.deobf.deobf.lua"),
            "KEEP_THIS_INPUT",
        );
        assert_eq!(
            file_names(directory.path()),
            [
                "ui.deobf.deobf.lua",
                "ui.deobf.lua",
                "ui.lua",
                "ui_2.deobf.lua"
            ]
        );
    }
}

#[test]
fn real_cli_equal_basenames_keep_sequential_contents_in_a_new_output_folder() {
    let directory = tempfile::tempdir().unwrap();
    for folder in ["one", "two"] {
        fs::create_dir(directory.path().join(folder)).unwrap();
    }
    fs::write(directory.path().join("one/x.lua"), safe_variant("FIRST")).unwrap();
    fs::write(directory.path().join("two/x.lua"), safe_variant("SECOND")).unwrap();
    let output = command(directory.path())
        .args([
            "one/x.lua",
            "two/x.lua",
            "-o",
            "results/deeper",
            "--no-verify",
            "--no-clean",
            "-q",
        ])
        .output()
        .unwrap();
    success(&output);
    assert!(output.stdout.is_empty() && output.stderr.is_empty());
    let results = directory.path().join("results/deeper");
    assert_eq!(file_names(&results), ["x.deobf.lua", "x_2.deobf.lua"]);
    assert_output_label(&results.join("x.deobf.lua"), "FIRST");
    assert_output_label(&results.join("x_2.deobf.lua"), "SECOND");
    assert_eq!(file_names(&directory.path().join("one")), ["x.lua"]);
    assert_eq!(file_names(&directory.path().join("two")), ["x.lua"]);
}

#[test]
fn real_cli_jobs_process_every_distinct_input_once_and_keep_json_associations() {
    for jobs in ["1", "3"] {
        let directory = tempfile::tempdir().unwrap();
        let names: Vec<_> = (0..7).map(|index| format!("f{index}.lua")).collect();
        for (index, name) in names.iter().enumerate() {
            fs::write(
                directory.path().join(name),
                safe_variant(&format!("FILE_{index}")),
            )
            .unwrap();
        }
        let output = command(directory.path())
            .args(&names)
            .args(["--jobs", jobs, "--no-verify", "--no-clean", "--json", "-q"])
            .output()
            .unwrap();
        success(&output);
        assert!(output.stderr.is_empty());
        let rows: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let rows = rows.as_array().unwrap();
        assert_eq!(rows.len(), 7);
        let mut outputs = std::collections::BTreeSet::new();
        for (index, row) in rows.iter().enumerate() {
            assert_eq!(row["file"], names[index]);
            assert_eq!(row["ok"], true);
            assert!(row["code"].is_null() && row["error"].is_null());
            assert_eq!(row["info"]["protos"], 1);
            let path = directory.path().join(row["output"].as_str().unwrap());
            assert!(
                outputs.insert(path.clone()),
                "duplicate output for {}",
                names[index]
            );
            assert_eq!(
                path.file_name().unwrap().to_str().unwrap(),
                format!("f{index}.deobf.lua")
            );
            assert_output_label(&path, &format!("FILE_{index}"));
            assert_eq!(
                fs::read(directory.path().join(&names[index])).unwrap(),
                safe_variant(&format!("FILE_{index}"))
            );
        }
        assert_eq!(outputs.len(), 7);
        assert_eq!(file_names(directory.path()).len(), 14);
    }
}

#[test]
fn real_cli_partial_source_is_visible_in_single_and_folder_summaries() {
    for batch in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join("ui.lua"),
            safe_variant("PARTIAL_SOURCE"),
        )
        .unwrap();
        let output = command(directory.path())
            .arg(if batch {
                Path::new(".")
            } else {
                Path::new("ui.lua")
            })
            .args(["--no-verify", "--no-clean"])
            .output()
            .unwrap();
        success(&output);
        assert_output_label(&directory.path().join("ui.deobf.lua"), "PARTIAL_SOURCE");
        let summary = String::from_utf8_lossy(&output.stdout);
        assert!(summary.contains("PARTIAL"), "{summary}");
        assert!(!summary.contains("DISPLAY TEXT ONLY"));
        assert_eq!(file_names(directory.path()), ["ui.deobf.lua", "ui.lua"]);
        if batch {
            assert!(summary.contains("Done: 1 of 1 files"));
        }
    }
}

#[test]
fn real_cli_folder_processes_only_sorted_top_level_sources_and_summarizes_them() {
    let directory = tempfile::tempdir().unwrap();
    let folder = directory.path().join("scripts");
    fs::create_dir(&folder).unwrap();
    for (name, label) in [
        ("b.lua", "B_FOLDER"),
        ("a.txt", "A_FOLDER"),
        ("c.luau", "C_FOLDER"),
    ] {
        fs::write(folder.join(name), safe_variant(label)).unwrap();
    }
    for name in ["old_deobf.lua", "old_annotated.lua", "notes.md"] {
        fs::write(folder.join(name), b"KEEP THIS FILE").unwrap();
    }
    fs::create_dir(folder.join("sub")).unwrap();
    let deep = safe_variant("DEEP_MUST_NOT_RUN");
    fs::write(folder.join("sub/deep.lua"), &deep).unwrap();
    let output = command(directory.path())
        .arg(&folder)
        .args(["--no-verify", "--no-clean"])
        .output()
        .unwrap();
    success(&output);
    assert!(output.stderr.is_empty());
    for (name, label) in [
        ("a.deobf.lua", "A_FOLDER"),
        ("b.deobf.lua", "B_FOLDER"),
        ("c.deobf.lua", "C_FOLDER"),
    ] {
        assert_output_label(&folder.join(name), label);
    }
    let summary = String::from_utf8_lossy(&output.stdout);
    assert!(
        summary.contains("[3/3]") && summary.contains("Done: 3 of 3 files"),
        "{summary}"
    );
    assert!(summary.contains("next to each input file"));
    let rows: Vec<_> = summary
        .lines()
        .filter(|line| line.starts_with('['))
        .collect();
    assert_eq!(rows.len(), 3);
    for (row, mapping) in rows.iter().zip([
        "a.txt -> a.deobf.lua",
        "b.lua -> b.deobf.lua",
        "c.luau -> c.deobf.lua",
    ]) {
        assert!(row.contains(mapping), "{row}");
    }
    assert!(!summary
        .split("Done:")
        .next()
        .unwrap()
        .contains(folder.to_str().unwrap()));
    for name in ["old_deobf.lua", "old_annotated.lua", "notes.md"] {
        assert_eq!(fs::read(folder.join(name)).unwrap(), b"KEEP THIS FILE");
    }
    assert_eq!(file_names(&folder.join("sub")), ["deep.lua"]);
    assert_eq!(fs::read(folder.join("sub/deep.lua")).unwrap(), deep);
}

#[test]
fn real_cli_failed_files_do_not_stop_human_batch_reporting_or_other_outputs() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("a.lua"), safe_variant("A_SURVIVES")).unwrap();
    fs::write(directory.path().join("b.lua"), b"print('ordinary source')").unwrap();
    fs::write(directory.path().join("c.lua"), safe_variant("C_SURVIVES")).unwrap();
    let output = command(directory.path())
        .args([
            "a.lua",
            "b.lua",
            "c.lua",
            "missing.lua",
            "--no-verify",
            "--no-clean",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert_output_label(&directory.path().join("a.deobf.lua"), "A_SURVIVES");
    assert_output_label(&directory.path().join("c.deobf.lua"), "C_SURVIVES");
    assert!(!directory.path().join("b.deobf.lua").exists());
    assert!(!directory.path().join("missing.deobf.lua").exists());
    let errors = String::from_utf8_lossy(&output.stderr);
    assert_eq!(errors.matches("FAILED: ").count(), 2, "{errors}");
    assert!(
        errors.contains("b.lua: ") && errors.contains("won't guess"),
        "{errors}"
    );
    assert!(errors.contains("Cannot read the file"));
    let summary = String::from_utf8_lossy(&output.stdout);
    assert!(
        summary.contains("Done: 2 of 4 files") && summary.contains("(2 failed)"),
        "{summary}"
    );
}

#[test]
fn real_cli_batch_rejects_file_outputs_and_empty_directories_before_writing() {
    let directory = tempfile::tempdir().unwrap();
    for name in ["a.lua", "b.lua"] {
        fs::write(directory.path().join(name), SAFE).unwrap();
    }
    fs::write(directory.path().join("file.txt"), b"KEEP TARGET").unwrap();
    let output = command(directory.path())
        .args(["a.lua", "b.lua", "-o", "file.txt"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("must be a folder"));
    assert_eq!(
        fs::read(directory.path().join("file.txt")).unwrap(),
        b"KEEP TARGET"
    );
    fs::create_dir(directory.path().join("empty")).unwrap();
    let output = command(directory.path()).arg("empty").output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("No .lua / .luau / .txt scripts found")
    );
    assert_eq!(
        file_names(directory.path()),
        ["a.lua", "b.lua", "empty", "file.txt"]
    );
    assert!(file_names(&directory.path().join("empty")).is_empty());
}

#[test]
fn real_cli_single_input_uses_an_existing_output_directory() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join("a.lua"),
        safe_variant("EXISTING_FOLDER"),
    )
    .unwrap();
    fs::create_dir(directory.path().join("outdir")).unwrap();
    let output = command(directory.path())
        .args(["a.lua", "-o", "outdir", "--no-verify", "--no-clean", "-q"])
        .output()
        .unwrap();
    success(&output);
    assert!(output.stdout.is_empty() && output.stderr.is_empty());
    assert_output_label(
        &directory.path().join("outdir/a.deobf.lua"),
        "EXISTING_FOLDER",
    );
    assert_eq!(
        file_names(&directory.path().join("outdir")),
        ["a.deobf.lua"]
    );
    assert!(!directory.path().join("a.deobf.lua").exists());
}

#[test]
fn generated_name_matrix_preserves_user_names_and_explicit_generated_inputs() {
    let directory = tempfile::tempdir().unwrap();
    let mine = [
        "a.lua",
        "b.luau",
        "notes.txt",
        "my_trace_tool.lua",
        "payload1.lua",
    ];
    for name in mine {
        fs::write(directory.path().join(name), b"fixture").unwrap();
    }
    for name in [
        "a_deobf.lua",
        "b_annotated.lua",
        "c_reconstructed.lua",
        "a_trace.txt",
        "A_TRACE.TXT",
        "a_payload1.lua",
        "a_payload12.txt",
        "c_payload3.lua",
        "a_verification.txt",
        "B_VERIFICATION.TXT",
        "a_display.txt",
        "B_DISPLAY.TXT",
        "a.deobf.lua",
        "B.DEOBF.LUA",
    ] {
        fs::write(directory.path().join(name), b"generated").unwrap();
    }
    let (files, problems) = expand_inputs(&[directory.path().to_path_buf()]);
    assert!(problems.is_empty());
    let mut names: Vec<_> = files
        .iter()
        .map(|path| path.file_name().unwrap().to_str().unwrap())
        .collect();
    names.sort_unstable();
    let mut expected = mine;
    expected.sort_unstable();
    assert_eq!(names, expected);
    for name in ["a_trace.txt", "a_verification.txt"] {
        let input = directory.path().join(name);
        let (files, problems) = expand_inputs(std::slice::from_ref(&input));
        assert_eq!(files, [input]);
        assert!(problems.is_empty());
    }
}

#[test]
fn real_cli_help_retains_every_reference_option_and_hides_the_worker_protocol() {
    let directory = tempfile::tempdir().unwrap();
    let output = command(directory.path()).arg("--help").output().unwrap();
    success(&output);
    let help = String::from_utf8(output.stdout).unwrap();
    for option in [
        "--check",
        "--detect",
        "-o",
        "--output",
        "--jobs",
        "--names",
        "--annotate",
        "--no-verify",
        "--no-clean",
        "--trace",
        "--sandbox",
        "--explore",
        "--place-id",
        "--user-id",
        "--username",
        "--executor",
        "--players",
        "--responses",
        "--sandbox-time",
        "--sandbox-timeout",
        "--timeout",
        "--max-mb",
        "--in-process",
        "--json",
        "-q",
        "--quiet",
    ] {
        assert!(
            help.split_whitespace()
                .any(|word| word.trim_end_matches(',') == option),
            "missing {option}: {help}"
        );
    }
    assert!(help.contains("Debugging only") && help.contains("resource limits"));
    assert!(!help.contains("--worker"));
}

#[test]
fn real_cli_text_detection_keeps_all_confidences_details_and_unreadable_errors() {
    let directory = tempfile::tempdir().unwrap();
    let source = std::str::from_utf8(SAFE)
        .unwrap()
        .replace("\r\n", "\n")
        .replace(
            "local function module(host, env)\n",
            "local function module(host, env)\nerror(\"must never execute\")\n",
        );
    assert!(source.contains("must never execute"));
    fs::write(
        directory.path().join("good.lua"),
        format!("-- Bacon Guard v0.43.0 | FORGE Protection | Build af206410f1dd\n{source}"),
    )
    .unwrap();
    fs::write(
        directory.path().join("half.lua"),
        b"-- Bacon Guard v0.99.0 | FORGE Protection | Build 0123456789ab\nprint(1)\n",
    )
    .unwrap();
    fs::write(
        directory.path().join("no.txt"),
        b"print('ordinary source')\n",
    )
    .unwrap();
    let output = command(directory.path())
        .args(["--detect", "good.lua"])
        .output()
        .unwrap();
    success(&output);
    assert!(output.stderr.is_empty());
    assert_eq!(
        String::from_utf8(output.stdout)
            .unwrap()
            .replace("\r\n", "\n"),
        "good.lua\tbacon_guard\t1.00\tBacon Guard v0.43.0 (FORGE Protection), build af206410f1dd\n"
    );
    let output = command(directory.path())
        .args(["--detect", "."])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stderr.is_empty());
    let text = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<_> = text.lines().collect();
    assert_eq!(lines.len(), 3, "{text}");
    for (line, expected) in lines.iter().zip([
        ["good.lua", "bacon_guard", "1.00"],
        ["half.lua", "bacon_guard", "0.50"],
        ["no.txt", "unknown", "0.00"],
    ]) {
        assert_eq!(line.split('\t').take(3).collect::<Vec<_>>(), expected);
        assert_eq!(line.split('\t').count(), 4);
    }
    assert!(lines[1].contains("the banner is there, but"));
    assert!(
        lines[2].contains("loader layout is not recognized") && lines[2].contains("frame"),
        "{}",
        lines[2]
    );
    let output = command(directory.path())
        .args(["--detect", "--json", "good.lua"])
        .output()
        .unwrap();
    success(&output);
    let rows: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(rows[0]["file"], "good.lua");
    assert_eq!(rows[0]["supported"], true);
    assert_eq!(rows[0]["confidence"], 1.0);
    let output = command(directory.path())
        .args(["--detect", "missing.lua"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("Cannot read the file"));
    assert!(output.stdout.is_empty());
    assert_eq!(
        file_names(directory.path()),
        ["good.lua", "half.lua", "no.txt"]
    );
}

#[test]
fn real_cli_checks_every_tool_and_preserves_a_failed_ast_tool_diagnostic() {
    let directory = tempfile::tempdir().unwrap();
    let tools = directory.path().join("tools");
    fs::create_dir(&tools).unwrap();
    let source = directory.path().join("ast_fixture.rs");
    fs::write(
        &source,
        "fn main() { eprintln!(\"controlled AST fixture failure\"); std::process::exit(7); }\n",
    )
    .unwrap();
    let built = Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
        .arg("--edition=2021")
        .arg(&source)
        .arg("-o")
        .arg(tools.join(if cfg!(windows) {
            "luau-ast.exe"
        } else {
            "luau-ast"
        }))
        .output()
        .unwrap();
    success(&built);
    let output = command(directory.path())
        .env("BGDEOBF_LUAU_DIR", &tools)
        .arg("--check")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let text = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<_> = text.lines().collect();
    assert_eq!(lines.len(), 3, "{text}");
    for (line, (tool, status)) in
        lines
            .iter()
            .zip([("luau", "OK"), ("luau-ast", "FAIL"), ("luau-compile", "OK")])
    {
        assert_eq!(
            line.split_whitespace().take(2).collect::<Vec<_>>(),
            [tool, status]
        );
    }
    assert!(lines[1].contains("failed to run"), "{text}");
    assert!(output.stderr.is_empty());
    fs::write(
        directory.path().join("a.lua"),
        b"print('self-authored setup check')",
    )
    .unwrap();
    let output = command(directory.path())
        .env("BGDEOBF_LUAU_DIR", &tools)
        .args(["--detect", "a.lua"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stdout).contains("controlled AST fixture failure"));
    assert!(!directory.path().join("a.deobf.lua").exists());
}

#[test]
#[ignore = "package-isolation fixture: checkout tool directories must be absent; never move checkout tools for this test"]
fn real_cli_missing_tools_are_reported_by_check_and_detection_in_an_isolated_package() {
    let checkout = Path::new(env!("CARGO_MANIFEST_DIR"));
    for path in [checkout.join("lua_bin"), checkout.join("luau_bin")] {
        assert!(
            !path.exists(),
            "package-isolation prerequisite is not met: {}",
            path.display()
        );
    }
    let directory = tempfile::tempdir().unwrap();
    let binary = directory.path().join(if cfg!(windows) {
        "bgdeobf.exe"
    } else {
        "bgdeobf"
    });
    fs::copy(env!("CARGO_BIN_EXE_bgdeobf"), &binary).unwrap();
    fs::write(
        directory.path().join("a.lua"),
        b"print('self-authored setup check')",
    )
    .unwrap();
    for (args, code) in [(vec!["--check"], 1), (vec!["--detect", "a.lua"], 2)] {
        let mut isolated = Command::new(&binary);
        isolated
            .current_dir(directory.path())
            .env_clear()
            .env("PATH", "")
            .env("BGDEOBF_LUAU_DIR", directory.path().join("absent_tools"));
        #[cfg(windows)]
        if let Some(root) = std::env::var_os("SYSTEMROOT") {
            isolated.env("SYSTEMROOT", root);
        }
        let output = isolated.args(&args).output().unwrap();
        assert_eq!(output.status.code(), Some(code));
        if args == ["--check"] {
            let text = String::from_utf8(output.stdout).unwrap();
            assert_eq!(text.lines().count(), 3);
            assert_eq!(text.matches("FAIL").count(), 3);
            assert!(text.contains("not found"));
        } else {
            assert!(String::from_utf8_lossy(&output.stderr)
                .contains("FAILED: The Luau tools were not found"));
        }
    }
    assert!(!directory.path().join("a.deobf.lua").exists());
}

#[test]
fn real_cli_moonveil_reconstruction_and_trace_are_written_side_by_side() {
    for with_trace in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("hub.lua"), b"-- This script was generated using MoonVeil v2.1.1\nprint('CLI reconstruction')\nloadstring(game:HttpGet('https://example.invalid/cli.lua'))()\n").unwrap();
        let mut invocation = command(directory.path());
        invocation.arg("hub.lua");
        if with_trace {
            fs::write(
                directory.path().join("responses.json"),
                br#"{"https://example.invalid/cli.lua":"print(\"captured CLI payload\")"}"#,
            )
            .unwrap();
            invocation.args([
                "--trace",
                "--responses",
                "responses.json",
                "--sandbox-time",
                "0.1",
                "--sandbox-timeout",
                "10",
            ]);
        }
        let output = invocation.output().unwrap();
        success(&output);
        let reconstructed =
            fs::read_to_string(directory.path().join("hub_reconstructed.lua")).unwrap();
        assert!(reconstructed
            .starts_with("--[[\n  BEHAVIOUR RECONSTRUCTION - THIS IS NOT THE ORIGINAL SOURCE."));
        assert!(reconstructed.contains("CLI reconstruction"));
        assert!(!directory.path().join("hub.deobf.lua").exists());
        assert!(!directory.path().join("hub_deobf.lua").exists());
        let summary = String::from_utf8_lossy(&output.stdout);
        assert!(summary.contains("NOT the original source"), "{summary}");
        if with_trace {
            let report = fs::read_to_string(directory.path().join("hub_trace.txt")).unwrap();
            assert!(report.starts_with("FAKE ROBLOX SANDBOX TRACE  -  hub.lua"));
            assert!(report.contains("the script as given"));
            assert!(!report.contains("@@NAME@@") && !report.contains("@@STEM@@"));
            assert!(report.contains("hub_payload1.lua"));
            assert_eq!(
                fs::read(directory.path().join("hub_payload1.lua")).unwrap(),
                b"print(\"captured CLI payload\")"
            );
            assert!(summary.contains("Fake Roblox run"), "{summary}");
            assert_eq!(
                file_names(directory.path()),
                [
                    "hub.lua",
                    "hub_payload1.lua",
                    "hub_reconstructed.lua",
                    "hub_trace.txt",
                    "responses.json"
                ]
            );
        } else {
            assert_eq!(
                file_names(directory.path()),
                ["hub.lua", "hub_reconstructed.lua"]
            );
        }
    }
}

//! Isolated native worker protocol. No script output is read as worker messages.
use crate::{
    error::{Error, Result},
    model::{Options, Output, Progress},
    process::{self, Limits},
};
use serde_json::{json, Value};
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

const MAX_WORKER_OUTPUT: usize = 128 * 1024 * 1024;

fn read_bounded(path: &Path) -> Result<Vec<u8>> {
    if fs::metadata(path)?.len() > MAX_WORKER_OUTPUT as u64 {
        return Err(Error::new(
            "internal",
            "worker result exceeds the output limit",
        ));
    }
    Ok(fs::read(path)?)
}

fn executable() -> Result<PathBuf> {
    let current = std::env::current_exe()?;
    if current.file_stem().is_some_and(|n| n == "bgdeobf") {
        return Ok(current);
    }
    let name = if cfg!(windows) {
        "bgdeobf.exe"
    } else {
        "bgdeobf"
    };
    let parent = current
        .parent()
        .ok_or_else(|| Error::new("setup", "cannot locate worker executable"))?;
    // Only recognize Cargo's two test layouts; never search arbitrary ancestors.
    let directory = if parent.file_name().is_some_and(|n| n == "deps") {
        parent.parent().unwrap_or(parent)
    } else if parent.file_name().is_some_and(|n| n == "out")
        && parent
            .ancestors()
            .nth(3)
            .and_then(Path::file_name)
            .is_some_and(|n| n == "build")
    {
        parent.ancestors().nth(4).unwrap_or(parent)
    } else {
        parent
    };
    let path = directory.join(name);
    if path.is_file() {
        return Ok(path);
    }
    Err(Error::new(
        "setup",
        "the native bgdeobf worker executable was not found beside this application",
    ))
}

fn remove_work(path: &Path) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        match fs::remove_dir_all(path) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e)
                if cfg!(windows)
                    && matches!(e.raw_os_error(), Some(32 | 33))
                    && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(10))
            }
            Err(e) => return Err(e.into()),
        }
    }
}

fn configure_worker(command: &mut Command, work: &Path) {
    command.current_dir(work).stdin(Stdio::null()).env_clear();
    if let Some(path) = std::env::var_os("PATH") {
        command.env("PATH", path);
    }
    #[cfg(unix)]
    if std::env::var_os("PATH").is_none() {
        command.env("PATH", "/usr/bin:/bin");
    }
    for key in ["BGDEOBF_LUAU_DIR", "SYSTEMROOT", "LANG", "LC_ALL"] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    for key in ["TMPDIR", "TEMP", "TMP"] {
        command.env(key, work);
    }
}

pub fn run(
    data: &[u8],
    opts: &Options,
    hard_timeout: Duration,
    progress: Progress<'_>,
    cancel: Option<&AtomicBool>,
) -> Result<Output> {
    run_with_capture(
        data,
        opts,
        hard_timeout,
        progress,
        cancel,
        |command, limits, observer| {
            process::capture_with(command, limits, cancel, Some(observer), false)
        },
    )
}

pub(crate) fn run_with_capture(
    data: &[u8],
    opts: &Options,
    hard_timeout: Duration,
    progress: Progress<'_>,
    cancel: Option<&AtomicBool>,
    capture: impl FnOnce(Command, &Limits, &mut dyn FnMut(&[u8])) -> Result<process::Capture>,
) -> Result<Output> {
    if crate::astutil::is_blank_source(data) {
        return Err(Error::new("empty", "the file is empty"));
    }
    if data.len() > opts.max_bytes {
        return Err(Error::new(
            "too_big",
            format!("the file is larger than {} bytes", opts.max_bytes),
        ));
    }
    if cancel.is_some_and(|f| f.load(Ordering::Acquire)) {
        return Err(Error::new("cancelled", "the job was cancelled"));
    }
    let work = tempfile::Builder::new().prefix("bgdeobf_job_").tempdir()?;
    let result = (|| {
        fs::write(work.path().join("in.lua"), data)?;
        fs::write(work.path().join("cfg.json"), serde_json::to_vec(opts)?)?;
        let mut command = Command::new(executable()?);
        command.arg("--worker").arg(work.path());
        configure_worker(&mut command, work.path());
        let mut pending = Vec::new();
        let mut final_message = None;
        let mut observe = |bytes: &[u8]| {
            for &byte in bytes {
                if byte == b'\n' {
                    if let Ok(Value::Object(message)) = serde_json::from_slice::<Value>(&pending) {
                        if let Some(stage) = message.get("stage").and_then(Value::as_str) {
                            if let Some(note) = progress {
                                note(stage);
                            }
                        } else if message.get("ok").is_some_and(Value::is_boolean) {
                            final_message = Some(Value::Object(message));
                        }
                    }
                    pending.clear();
                } else if pending.len() < MAX_WORKER_OUTPUT {
                    pending.push(byte);
                }
            }
        };
        let capture = capture(
            command,
            &Limits {
                timeout: hard_timeout,
                memory_mb: 3072,
                cpu_seconds: 0,
                max_output_bytes: MAX_WORKER_OUTPUT,
                new_group: true,
            },
            &mut observe,
        )?;
        if cancel.is_some_and(|f| f.load(Ordering::Acquire)) {
            return Err(Error::new("cancelled", "the job was cancelled"));
        }
        if capture.status == "timeout" {
            return Err(Error::new(
                "timeout",
                format!(
                    "the job took longer than {} s and was stopped",
                    hard_timeout.as_secs_f64()
                ),
            ));
        }
        if capture.status == "output" {
            return Err(Error::new(
                "internal",
                "the worker exceeded the progress output limit",
            ));
        }
        let final_message = final_message.ok_or_else(|| {
            let tail = String::from_utf8_lossy(&capture.err)
                .trim()
                .chars()
                .rev()
                .take(300)
                .collect::<String>()
                .chars()
                .rev()
                .collect::<String>();
            Error::new(
                if capture.status == "killed"
                    || capture
                        .code
                        .is_some_and(|c| [-9, -6, -11, -1073741801].contains(&c))
                {
                    "memory"
                } else {
                    "internal"
                },
                format!(
                    "the worker stopped unexpectedly (exit {:?}) {}",
                    capture.code,
                    crate::messages::scrub(&tail)
                ),
            )
        })?;
        if final_message["ok"] != true {
            return Err(Error::new(
                final_message["code"].as_str().unwrap_or("internal"),
                final_message["message"].as_str().unwrap_or("failed"),
            ));
        }
        fn read_text(path: &Path) -> Result<Option<String>> {
            if !path.is_file() {
                return Ok(None);
            }
            let bytes = read_bounded(path)?;
            Ok(Some(String::from_utf8(bytes).map_err(|_| {
                Error::new("internal", "worker returned invalid UTF-8 source")
            })?))
        }
        Ok(Output {
            text: read_text(&work.path().join("out.lua"))?,
            raw: read_text(&work.path().join("raw.lua"))?,
            info: final_message["info"].clone(),
            trace: read_trace(work.path())?,
        })
    })();
    let path = work.keep();
    let cleanup = remove_work(&path);
    match cleanup {
        Err(e) => Err(e),
        Ok(()) => result,
    }
}

fn payload_bytes(value: &Value) -> Result<Vec<u8>> {
    value
        .as_array()
        .ok_or_else(|| Error::new("internal", "invalid trace payload bytes"))?
        .iter()
        .map(|n| {
            n.as_u64()
                .and_then(|n| u8::try_from(n).ok())
                .ok_or_else(|| Error::new("internal", "invalid trace payload byte"))
        })
        .collect()
}

fn write_trace(dir: &Path, trace: &Value) -> Result<()> {
    let mut meta = trace.clone();
    let payloads = meta["payloads"]
        .as_array_mut()
        .ok_or_else(|| Error::new("internal", "invalid trace payloads"))?;
    for payload in payloads {
        let id = payload["id"]
            .as_u64()
            .ok_or_else(|| Error::new("internal", "invalid trace payload id"))?;
        fs::write(
            dir.join(format!("payload_{id}.bin")),
            payload_bytes(&payload["data"])?,
        )?;
        payload
            .as_object_mut()
            .ok_or_else(|| Error::new("internal", "invalid trace payload"))?
            .remove("data");
    }
    fs::write(dir.join("trace.json"), serde_json::to_vec(&meta)?)?;
    Ok(())
}

fn read_trace(dir: &Path) -> Result<Option<Value>> {
    let path = dir.join("trace.json");
    if !path.exists() {
        return Ok(None);
    }
    let mut trace: Value = serde_json::from_slice(&read_bounded(&path)?)?;
    let payloads = trace["payloads"]
        .as_array_mut()
        .ok_or_else(|| Error::new("internal", "invalid worker trace metadata"))?;
    let mut total = 0_usize;
    for payload in payloads {
        let id = payload["id"]
            .as_u64()
            .ok_or_else(|| Error::new("internal", "invalid worker payload id"))?;
        let data = read_bounded(&dir.join(format!("payload_{id}.bin")))?;
        total = total
            .checked_add(data.len())
            .filter(|n| *n <= MAX_WORKER_OUTPUT)
            .ok_or_else(|| Error::new("internal", "worker payloads exceed the output limit"))?;
        payload["data"] = json!(data);
    }
    Ok(Some(trace))
}

fn emit(value: &Value) {
    let stdout = io::stdout();
    let mut lock = stdout.lock();
    let _ = serde_json::to_writer(&mut lock, value);
    let _ = lock.write_all(b"\n");
    let _ = lock.flush();
}

pub fn worker_main(dir: &Path) -> Result<()> {
    // Keep POSIX Luau children in this worker's group for cancellation.
    process::IN_WORKER.store(true, Ordering::Release);
    let result = std::panic::catch_unwind(|| -> Result<Output> {
        let opts: Options = serde_json::from_slice(&fs::read(dir.join("cfg.json"))?)?;
        let data = fs::read(dir.join("in.lua"))?;
        let progress = |stage: &str| emit(&json!({"stage":stage}));
        let out = crate::engine::run(&data, &opts, Some(&progress))?;
        if let Some(text) = &out.text {
            fs::write(dir.join("out.lua"), text.as_bytes())?;
        }
        if let Some(raw) = &out.raw {
            fs::write(dir.join("raw.lua"), raw.as_bytes())?;
        }
        if let Some(trace) = &out.trace {
            write_trace(dir, trace)?;
        }
        Ok(out)
    });
    match result {
        Ok(Ok(out)) => emit(&json!({"ok":true,"info":out.info})),
        Ok(Err(error)) => emit(&json!({"ok":false,"code":error.code,"message":error.message})),
        Err(_) => {
            emit(&json!({"ok":false,"code":"internal","message":"the native worker panicked"}))
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn child_receives_private_temp_and_no_inherited_secret() {
        let work = tempfile::tempdir().unwrap();
        std::env::set_var("BGDEOBF_TEST_WORKER_SECRET", "synthetic-do-not-inherit");
        #[cfg(windows)]
        let mut command = Command::new("cmd.exe");
        #[cfg(windows)]
        command.args([
            "/d",
            "/c",
            "if defined BGDEOBF_TEST_WORKER_SECRET (echo leaked) else (echo clean)& echo %TEMP%",
        ]);
        #[cfg(unix)]
        let mut command = Command::new("/bin/sh");
        #[cfg(unix)]
        command.args(["-c","if [ -n \"$BGDEOBF_TEST_WORKER_SECRET\" ]; then echo leaked; else echo clean; fi; echo \"$TMPDIR\""]);
        configure_worker(&mut command, work.path());
        let capture = process::capture(
            command,
            &Limits {
                timeout: Duration::from_secs(5),
                memory_mb: 256,
                cpu_seconds: 2,
                max_output_bytes: 4096,
                new_group: true,
            },
        );
        std::env::remove_var("BGDEOBF_TEST_WORKER_SECRET");
        let capture = capture.unwrap();
        assert_eq!(capture.status, "ok");
        let text = String::from_utf8_lossy(&capture.out);
        assert!(text.starts_with("clean"));
        assert!(text.contains(work.path().to_string_lossy().as_ref()));
    }
    #[test]
    fn trace_metadata_and_non_utf8_payload_roundtrip() {
        let work = tempfile::tempdir().unwrap();
        let trace = json!({"report":"evidence","summary":{"finished":true},"payloads":[{"id":1,"kind":"loadstring","label":"x","length":3,"data":[0,128,255],"ext":"lua"}]});
        write_trace(work.path(), &trace).unwrap();
        assert_eq!(
            fs::read(work.path().join("payload_1.bin")).unwrap(),
            [0, 128, 255]
        );
        assert_eq!(read_trace(work.path()).unwrap().unwrap(), trace);
    }
}

//! Execute Luau in a private empty directory with a stripped environment.
use crate::{
    error::{Error, Result},
    process::{self, Capture, Limits},
};
use std::{
    process::{Command, Stdio},
    time::Duration,
};

pub fn live_supported() -> bool {
    #[cfg(unix)]
    {
        use std::sync::OnceLock;
        static LIVE: OnceLock<bool> = OnceLock::new();
        *LIVE.get_or_init(|| {
            let (mut m, mut s) = (-1, -1);
            unsafe {
                if libc::openpty(
                    &mut m,
                    &mut s,
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    std::ptr::null(),
                ) != 0
                {
                    return false;
                }
                libc::close(m);
                libc::close(s);
            }
            true
        })
    }
    #[cfg(not(unix))]
    {
        false
    }
}

pub fn run_luau(
    source: &[u8],
    timeout: Duration,
    mem_mb: u64,
    max_output_mb: u64,
) -> Result<Vec<u8>> {
    let result = run(source, timeout, mem_mb, max_output_mb, false, false)?;
    let max_bytes = byte_limit(max_output_mb)?;
    if result.status == "timeout" {
        return Err(Error::new(
            "timeout",
            format!(
                "the protector took longer than {} s to unpack",
                timeout.as_secs_f64()
            ),
        ));
    }
    if result.status == "output" || (result.code != Some(0) && result.out.len() >= max_bytes) {
        return Err(Error::new(
            "sandbox",
            "the unpacked dump exceeded the output limit",
        ));
    }
    if result.status != "ok" {
        return Err(Error::new(
            "sandbox",
            format!(
                "unpacking failed (exit {:?}): {}",
                result.code,
                String::from_utf8_lossy(&result.err)
                    .chars()
                    .take(300)
                    .collect::<String>()
            ),
        ));
    }
    Ok(result.out)
}

pub fn run_luau_live(
    source: &[u8],
    timeout: Duration,
    mem_mb: u64,
    max_output_mb: u64,
) -> Result<Capture> {
    run(
        source,
        timeout,
        mem_mb,
        max_output_mb,
        live_supported(),
        true,
    )
}

fn byte_limit(mb: u64) -> Result<usize> {
    mb.checked_mul(1024 * 1024)
        .and_then(|n| usize::try_from(n).ok())
        .filter(|n| *n > 0)
        .ok_or_else(|| Error::new("sandbox", "invalid output limit"))
}

fn run(
    source: &[u8],
    timeout: Duration,
    mem_mb: u64,
    max_output_mb: u64,
    live: bool,
    new_group: bool,
) -> Result<Capture> {
    let exe = crate::astutil::find_bin("luau").map_err(|e| Error::new("setup", e.message))?;
    let work = tempfile::Builder::new()
        .prefix("bgdeobf_trace_")
        .tempdir()?;
    std::fs::write(work.path().join("sandbox.lua"), source)?;
    let max_bytes = byte_limit(max_output_mb)?;
    // The Windows C runtime expands LF; reserve wire space, then normalize once.
    let wire_limit = max_bytes
        .checked_mul(if cfg!(windows) { 2 } else { 1 })
        .ok_or_else(|| Error::new("sandbox", "output limit overflow"))?;
    let mut command = Command::new(exe);
    command
        .arg("sandbox.lua")
        .current_dir(work.path())
        .stdin(Stdio::null())
        .env_clear();
    #[cfg(unix)]
    command.env("PATH", "/usr/bin:/bin");
    #[cfg(windows)]
    command.env(
        "SYSTEMROOT",
        std::env::var_os("SYSTEMROOT").unwrap_or_default(),
    );
    let mut result = process::capture_with(
        command,
        &Limits {
            timeout,
            memory_mb: mem_mb,
            cpu_seconds: timeout.as_secs().saturating_add(2),
            max_output_bytes: wire_limit,
            new_group: new_group && !process::IN_WORKER.load(std::sync::atomic::Ordering::Acquire),
        },
        None,
        None,
        live,
    )?;
    #[cfg(windows)]
    {
        let mut normalized = Vec::with_capacity(result.out.len());
        let mut i = 0;
        while i < result.out.len() {
            if result.out[i] == b'\r' && result.out.get(i + 1) == Some(&b'\n') {
                i += 1;
            }
            normalized.push(result.out[i]);
            i += 1;
        }
        result.out = normalized;
    }
    if result.out.len() > max_bytes {
        result.out.truncate(max_bytes);
        if result.status == "ok" {
            result.status = "output".into();
        }
    }
    let err: String = result.err.iter().map(|b| char::from(*b)).collect();
    let err = regex::Regex::new(r"(?:\./)?sandbox\.lua:(\d+):\s*")
        .unwrap()
        .replace_all(&err, "runtime line $1: ");
    let err = err.replace(&work.path().to_string_lossy().to_string(), "[work]");
    result.err = crate::messages::scrub(err.trim())
        .chars()
        .take(2000)
        .collect::<String>()
        .into_bytes();
    work.close()?;
    Ok(result)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn forced_file_stdout_keeps_padded_partial_trace_without_a_pty() {
        let program = crate::tracer::build_script(
            b"print('before the loop')\nwhile true do end",
            &serde_json::json!({"timeout":3}),
            false,
        )
        .unwrap();
        let captured = run(&program, Duration::from_secs(3), 512, 64, false, true).unwrap();
        let (events, _, _, end) = crate::tracer::parse_log(&captured.out);
        assert_eq!(captured.status, "timeout");
        assert_eq!(
            events
                .iter()
                .filter(|e| e.kind == "out")
                .map(|e| e.a.as_deref())
                .collect::<Vec<_>>(),
            vec![Some("before the loop")]
        );
        assert!(end.is_none());
    }
}

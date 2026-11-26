use bgdeobf::{
    process::{capture, Limits},
    sandbox,
};
use std::{process::Command, time::Duration};

fn limits() -> Limits {
    Limits {
        timeout: Duration::from_secs(5),
        memory_mb: 256,
        cpu_seconds: 3,
        max_output_bytes: 1024,
        new_group: true,
    }
}

#[test]
fn capture_keeps_stdout_stderr_and_exit_status() {
    #[cfg(windows)]
    let mut command = Command::new("cmd.exe");
    #[cfg(windows)]
    command.args(["/d", "/c", "echo hello& echo error 1>&2& exit /b 7"]);
    #[cfg(unix)]
    let mut command = Command::new("/bin/sh");
    #[cfg(unix)]
    command.args(["-c", "echo hello; echo error >&2; exit 7"]);
    let got = capture(command, &limits()).unwrap();
    assert!(String::from_utf8_lossy(&got.out).contains("hello"));
    assert!(String::from_utf8_lossy(&got.err).contains("error"));
    assert_eq!(got.code, Some(7));
    assert_eq!(got.status, "crashed");
}

#[test]
fn capture_deadline_retains_partial_output() {
    let mut lim = limits();
    lim.timeout = Duration::from_millis(350);
    #[cfg(windows)]
    let mut command = Command::new("cmd.exe");
    #[cfg(windows)]
    command.args(["/d", "/c", "echo before& ping -n 30 127.0.0.1 >nul"]);
    #[cfg(unix)]
    let mut command = Command::new("/bin/sh");
    #[cfg(unix)]
    command.args(["-c", "echo before; sleep 30"]);
    let got = capture(command, &lim).unwrap();
    assert_eq!(got.status, "timeout");
    assert!(String::from_utf8_lossy(&got.out).contains("before"));
    assert!(got.seconds < 6.0);
}

#[test]
fn noisy_stderr_is_drained_and_only_the_prefix_is_retained() {
    if std::env::var_os("BGDEOBF_TEST_STDERR_WRITER").is_some() {
        use std::io::Write;
        std::io::stderr()
            .write_all(&b"long-stderr-line\n".repeat(30000))
            .unwrap();
        return;
    }
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "noisy_stderr_is_drained_and_only_the_prefix_is_retained",
            "--nocapture",
        ])
        .env("BGDEOBF_TEST_STDERR_WRITER", "1");
    let mut lim = limits();
    lim.timeout = Duration::from_secs(15);
    let got = capture(command, &lim).unwrap();
    assert_eq!(got.status, "ok");
    assert_eq!(got.err.len(), 65536);
    assert!(got.err.starts_with(b"long-stderr-line"));
}

#[test]
fn sandbox_preserves_carriage_returns() {
    let source = br#"print("a\r\nb\rc\\r")"#;
    assert_eq!(
        sandbox::run_luau(source, Duration::from_secs(5), 256, 1).unwrap(),
        b"a\r\nb\rc\\r\n"
    );
    assert_eq!(
        sandbox::run_luau_live(source, Duration::from_secs(5), 256, 1)
            .unwrap()
            .out,
        b"a\r\nb\rc\\r\n"
    );
}

#[test]
fn sandbox_enforces_output_memory_and_deadline() {
    let out = sandbox::run_luau_live(
        b"for i=1,10000000 do print(string.rep('x',100)) end",
        Duration::from_secs(10),
        200,
        1,
    )
    .unwrap();
    assert_eq!(out.status, "output");
    assert_eq!(out.out.len(), 1024 * 1024);
    let out =
        sandbox::run_luau_live(b"while true do end", Duration::from_millis(250), 200, 1).unwrap();
    assert_eq!(out.status, "timeout");
    let out = sandbox::run_luau_live(
        b"local t={} local i=0 while true do i+=1 table.insert(t,string.rep('x',1000000+i)) end",
        Duration::from_secs(10),
        80,
        1,
    )
    .unwrap();
    assert_ne!(out.status, "ok");
    assert!(out.seconds < 10.0);
}

#[cfg(unix)]
#[test]
fn dump_output_limit_recognizes_the_exact_posix_file_size_boundary() {
    let error = sandbox::run_luau(
        b"for i=1,10000000 do print(string.rep('x',100))end",
        Duration::from_secs(15),
        200,
        1,
    )
    .unwrap_err();
    assert_eq!(error.code, "sandbox");
    assert!(error.message.contains("output limit"));
}

#[cfg(windows)]
#[test]
fn deadline_terminates_descendant_before_return() {
    use windows_sys::Win32::{
        Foundation::CloseHandle,
        System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE},
    };
    let mut command = Command::new("powershell.exe");
    command.args(["-NoProfile","-NonInteractive","-Command","$p=Start-Process -FilePath $env:ComSpec -ArgumentList '/d /c ping -n 60 127.0.0.1' -PassThru; Write-Output $p.Id; Start-Sleep -Seconds 60"]);
    let mut lim = limits();
    lim.timeout = Duration::from_secs(3);
    lim.memory_mb = 256;
    let got = capture(command, &lim).unwrap();
    assert_eq!(got.status, "timeout");
    let pid = String::from_utf8_lossy(&got.out)
        .trim()
        .parse::<u32>()
        .expect("descendant must start under the job");
    unsafe {
        let handle = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if !handle.is_null() {
            assert_eq!(
                WaitForSingleObject(handle, 0),
                0,
                "descendant survived job teardown"
            );
            CloseHandle(handle);
        }
    }
}

#[test]
fn system_cpu_limit_is_independent_of_wall_deadline() {
    let mut command = Command::new(bgdeobf::astutil::find_bin("luau").unwrap());
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(
        temp.path().join("busy.lua"),
        "local x=0 while true do x+=1 end",
    )
    .unwrap();
    command
        .arg("busy.lua")
        .current_dir(temp.path())
        .stdin(std::process::Stdio::null());
    let mut lim = limits();
    lim.timeout = Duration::from_secs(15);
    lim.cpu_seconds = 1;
    let got = capture(command, &lim).unwrap();
    assert_ne!(got.status, "ok");
    assert_ne!(got.status, "timeout");
    assert!(got.seconds < 12.0);
}

#[test]
fn runner_validation_happens_before_worker_creation() {
    let opts = bgdeobf::model::Options {
        max_bytes: 5,
        ..Default::default()
    };
    assert_eq!(
        bgdeobf::runner::run(b"  ", &opts, Duration::from_secs(1), None, None)
            .unwrap_err()
            .code,
        "empty"
    );
    assert_eq!(
        bgdeobf::runner::run(b"\x0b", &opts, Duration::from_secs(1), None, None)
            .unwrap_err()
            .code,
        "empty"
    );
    assert_eq!(
        bgdeobf::runner::run(b"123456", &opts, Duration::from_secs(1), None, None)
            .unwrap_err()
            .code,
        "too_big"
    );
    let cancelled = std::sync::atomic::AtomicBool::new(true);
    assert_eq!(
        bgdeobf::runner::run(
            b"print",
            &opts,
            Duration::from_secs(1),
            None,
            Some(&cancelled)
        )
        .unwrap_err()
        .code,
        "cancelled"
    );
}

#[tokio::test]
async fn service_keeps_options_per_job_counts_failures_and_reports_queue_position() {
    use std::sync::{Arc, Mutex};
    let service = Arc::new(bgdeobf::service::Service::new(
        1,
        1,
        Duration::ZERO,
        Duration::from_secs(15),
    ));
    let opts = bgdeobf::model::Options {
        sandbox_only: true,
        trace: true,
        moonveil: true,
        ..Default::default()
    };
    let running = service.clone();
    let first = tokio::spawn(async move {
        running
            .submit(11, b"while true do end".to_vec(), opts, None)
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        while service.running() != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let note = seen.clone();
    let queued = service.clone();
    let second = tokio::spawn(async move {
        queued
            .submit(
                22,
                b"print('ok')".to_vec(),
                bgdeobf::model::Options {
                    sandbox_only: true,
                    trace: true,
                    moonveil: true,
                    ..Default::default()
                },
                Some(Arc::new(move |s| note.lock().unwrap().push(s.to_owned()))),
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        while service.queued() != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    first.abort();
    let _ = first.await;
    let result = second.await.unwrap().unwrap();
    assert!(result.text.is_none());
    assert!(result.trace.is_some());
    assert!(seen.lock().unwrap().iter().any(|s| s == "queued:1"));
    assert!(seen.lock().unwrap().iter().any(|s| s == "sandbox"));
    let error = service
        .submit(22, b"print('ordinary')".to_vec(), Default::default(), None)
        .await
        .unwrap_err();
    assert_eq!(error.code, "unsupported");
    assert_eq!((service.running(), service.queued()), (0, 0));
    assert_eq!(
        service.stats(),
        serde_json::json!({"ok":1,"failed":1,"rejected":0})
    );
}

#[test]
fn worker_roundtrips_real_trace_payloads_and_calling_thread_progress() {
    use std::sync::Mutex;
    let opts = bgdeobf::model::Options {
        sandbox_only: true,
        ..Default::default()
    };
    let owner = std::thread::current().id();
    let stages = Mutex::new(vec![]);
    let note = |s: &str| {
        assert_eq!(std::thread::current().id(), owner);
        stages.lock().unwrap().push(s.to_owned());
    };
    let out = bgdeobf::runner::run(
        b"print('{\"stage\":\"forged\"}') queue_on_teleport('print(1)')",
        &opts,
        Duration::from_secs(10),
        Some(&note),
        None,
    )
    .unwrap();
    assert!(out.text.is_none());
    assert_eq!(
        out.trace.as_ref().unwrap()["payloads"][0]["data"],
        serde_json::json!(b"print(1)".to_vec())
    );
    assert!(stages.lock().unwrap().iter().any(|s| s == "sandbox"));
    assert!(!stages.lock().unwrap().iter().any(|s| s == "forged"));
}

#[test]
fn worker_cancellation_reaps_luau_and_removes_work() {
    // A separate test process gives this run its own temp root without racing other tests.
    if std::env::var_os("BGDEOBF_TEST_PRIVATE_TEMP").is_none() {
        let root = tempfile::tempdir().unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap());
        child
            .args([
                "--exact",
                "worker_cancellation_reaps_luau_and_removes_work",
                "--nocapture",
            ])
            .env("BGDEOBF_TEST_PRIVATE_TEMP", "1");
        for key in ["TMPDIR", "TEMP", "TMP"] {
            child.env(key, root.path());
        }
        let mut lim = limits();
        lim.timeout = Duration::from_secs(15);
        lim.max_output_bytes = 65536;
        let result = capture(child, &lim).unwrap();
        assert_eq!(
            result.status,
            "ok",
            "{}\n{}",
            String::from_utf8_lossy(&result.out),
            String::from_utf8_lossy(&result.err)
        );
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
        return;
    }
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    let stop = Arc::new(AtomicBool::new(false));
    let other = stop.clone();
    let (started, wait) = std::sync::mpsc::sync_channel(1);
    let trigger = std::thread::spawn(move || {
        wait.recv_timeout(Duration::from_secs(5)).unwrap();
        std::thread::sleep(Duration::from_millis(200));
        other.store(true, Ordering::Release);
    });
    let opts = bgdeobf::model::Options {
        sandbox_only: true,
        ..Default::default()
    };
    let before = std::time::Instant::now();
    let progress = |stage: &str| {
        if stage == "sandbox" {
            let _ = started.try_send(());
        }
    };
    let error = bgdeobf::runner::run(
        b"while true do end",
        &opts,
        Duration::from_secs(15),
        Some(&progress),
        Some(&stop),
    )
    .unwrap_err();
    trigger.join().unwrap();
    assert_eq!(error.code, "cancelled");
    assert!(before.elapsed() < Duration::from_secs(6));
    assert_eq!(std::fs::read_dir(std::env::temp_dir()).unwrap().count(), 0);
}

#[test]
fn worker_deadline_and_success_leave_no_temporary_files() {
    if std::env::var_os("BGDEOBF_TEST_PRIVATE_TEMP").is_none() {
        let root = tempfile::tempdir().unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap());
        child
            .args([
                "--exact",
                "worker_deadline_and_success_leave_no_temporary_files",
                "--nocapture",
            ])
            .env("BGDEOBF_TEST_PRIVATE_TEMP", "1");
        for key in ["TMPDIR", "TEMP", "TMP"] {
            child.env(key, root.path());
        }
        let mut lim = limits();
        lim.timeout = Duration::from_secs(20);
        lim.max_output_bytes = 65536;
        let result = capture(child, &lim).unwrap();
        assert_eq!(
            result.status,
            "ok",
            "{}\n{}",
            String::from_utf8_lossy(&result.out),
            String::from_utf8_lossy(&result.err)
        );
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
        return;
    }
    let opts = bgdeobf::model::Options {
        sandbox_only: true,
        ..Default::default()
    };
    let error = bgdeobf::runner::run(
        b"while true do end",
        &opts,
        Duration::from_millis(500),
        None,
        None,
    )
    .unwrap_err();
    assert_eq!(error.code, "timeout");
    assert_eq!(std::fs::read_dir(std::env::temp_dir()).unwrap().count(), 0);
    let result = bgdeobf::runner::run(
        b"print('clean')",
        &opts,
        Duration::from_secs(10),
        None,
        None,
    )
    .unwrap();
    assert!(result.trace.unwrap()["report"]
        .as_str()
        .unwrap()
        .contains("clean"));
    assert_eq!(std::fs::read_dir(std::env::temp_dir()).unwrap().count(), 0);
}

#[cfg(windows)]
#[test]
#[allow(
    clippy::zombie_processes,
    reason = "fixture parent must exit before its descendant"
)]
fn successful_parent_also_terminates_surviving_descendants() {
    use windows_sys::Win32::{
        Foundation::CloseHandle,
        System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE},
    };
    if std::env::var_os("BGDEOBF_TEST_DESCENDANT_PARENT").is_some() {
        let child = Command::new("cmd.exe")
            .args(["/d", "/c", "ping -n 60 127.0.0.1 >nul"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        println!("CHILD_PID={}", child.id());
        // Dropping std::process::Child leaves it running; the capture job must reap it.
        return;
    }
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "successful_parent_also_terminates_surviving_descendants",
            "--nocapture",
        ])
        .env("BGDEOBF_TEST_DESCENDANT_PARENT", "1");
    let mut lim = limits();
    lim.timeout = Duration::from_secs(10);
    let got = capture(command, &lim).unwrap();
    assert_eq!(got.status, "ok");
    let pid = String::from_utf8_lossy(&got.out)
        .split("CHILD_PID=")
        .nth(1)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .parse::<u32>()
        .unwrap();
    unsafe {
        let handle = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if !handle.is_null() {
            let eventually = WaitForSingleObject(handle, 5000);
            CloseHandle(handle);
            assert_eq!(
                eventually, 0,
                "descendant {pid} survived successful parent after 5s"
            );
        }
    }
}

#[tokio::test]
async fn service_enforces_queue_users_cancellation_and_stats() {
    use std::sync::Arc;
    let svc = Arc::new(bgdeobf::service::Service::new(
        1,
        1,
        Duration::ZERO,
        Duration::from_secs(15),
    ));
    let opts = bgdeobf::model::Options {
        sandbox_only: true,
        ..Default::default()
    };
    let run = |user| {
        let svc = svc.clone();
        let opts = opts.clone();
        tokio::spawn(async move {
            svc.submit(user, b"while true do end".to_vec(), opts, None)
                .await
        })
    };
    let a = run(1);
    tokio::time::timeout(Duration::from_secs(5), async {
        while svc.running() != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let b = run(2);
    tokio::time::timeout(Duration::from_secs(5), async {
        while svc.queued() != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        svc.submit(1, b"x".to_vec(), opts.clone(), None)
            .await
            .unwrap_err()
            .code,
        "user_busy"
    );
    assert_eq!(
        svc.submit(3, b"x".to_vec(), opts.clone(), None)
            .await
            .unwrap_err()
            .code,
        "queue_full"
    );
    b.abort();
    let _ = b.await;
    a.abort();
    let _ = a.await;
    tokio::time::timeout(Duration::from_secs(6), async {
        while svc.running() != 0 || svc.queued() != 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(svc
        .submit(1, b"print('ok')".to_vec(), opts, None)
        .await
        .is_ok());
    assert_eq!(svc.stats()["ok"], 1);
    assert_eq!(svc.stats()["rejected"], 2);
}

#[tokio::test]
async fn service_ignores_progress_failures_and_releases_failed_jobs() {
    use std::sync::Arc;
    let svc = bgdeobf::service::Service::new(1, 0, Duration::ZERO, Duration::from_secs(10));
    let failed = svc
        .submit(
            1,
            b"print('unsupported ordinary source')".to_vec(),
            Default::default(),
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(failed.code, "unsupported");
    assert_eq!(svc.running(), 0);
    assert_eq!(svc.queued(), 0);
    let opts = bgdeobf::model::Options {
        sandbox_only: true,
        trace: true,
        moonveil: true,
        ..Default::default()
    };
    let result = svc
        .submit(
            1,
            b"print('still runs')".to_vec(),
            opts,
            Some(Arc::new(|_| panic!("test progress failure"))),
        )
        .await
        .unwrap();
    assert!(result.text.is_none());
    assert_eq!(result.info["sandbox_only"], true);
    assert_eq!(
        svc.stats(),
        serde_json::json!({"ok":1,"failed":1,"rejected":0})
    );
}

#[tokio::test]
async fn service_runs_queued_users_in_arrival_order() {
    use std::sync::{Arc, Mutex};
    let service = Arc::new(bgdeobf::service::Service::new(
        1,
        2,
        Duration::ZERO,
        Duration::from_secs(15),
    ));
    let opts = bgdeobf::model::Options {
        sandbox_only: true,
        ..Default::default()
    };
    let initial = service.clone();
    let initial_opts = opts.clone();
    let first = tokio::spawn(async move {
        initial
            .submit(1, b"while true do end".to_vec(), initial_opts, None)
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        while service.running() != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let order = Arc::new(Mutex::new(Vec::new()));
    let queued = |user| {
        let service = service.clone();
        let opts = opts.clone();
        let order = order.clone();
        tokio::spawn(async move {
            service
                .submit(
                    user,
                    b"print('done')".to_vec(),
                    opts,
                    Some(Arc::new(move |stage| {
                        if stage == "sandbox" {
                            order.lock().unwrap().push(user);
                        }
                    })),
                )
                .await
        })
    };
    let second = queued(2);
    tokio::time::timeout(Duration::from_secs(5), async {
        while service.queued() != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let third = queued(3);
    tokio::time::timeout(Duration::from_secs(5), async {
        while service.queued() != 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    first.abort();
    let _ = first.await;
    second.await.unwrap().unwrap();
    third.await.unwrap().unwrap();
    assert_eq!(*order.lock().unwrap(), vec![2, 3]);
    assert_eq!((service.running(), service.queued()), (0, 0));
    assert_eq!(service.stats()["ok"], 2);
}

#[cfg(unix)]
#[test]
fn posix_dump_child_shares_worker_group_and_exits_when_the_group_is_killed() {
    use std::{os::unix::fs::PermissionsExt, time::Instant};

    if std::env::var_os("BGDEOBF_TEST_POSIX_DUMP_GROUP").is_some() {
        let source = std::str::from_utf8(include_bytes!("samples/safe_vm.lua"))
            .unwrap()
            .replace("\r\n", "\n");
        let entry = "local function module(host, env)\n";
        assert_eq!(source.matches(entry).count(), 1);
        let delayed = source.replacen(entry, &format!("{entry}    local deadline = os.clock() + 30\n    while os.clock() < deadline do end\n"), 1);
        let error = bgdeobf::runner::run(
            delayed.as_bytes(),
            &Default::default(),
            Duration::from_secs(30),
            None,
            None,
        )
        .unwrap_err();
        assert!(
            ["memory", "internal"].contains(&error.code.as_str()),
            "{error}"
        );
        return;
    }
    let tools = tempfile::tempdir().unwrap();
    let jobs = tempfile::tempdir().unwrap();
    let observations = tempfile::tempdir().unwrap();
    let marker = observations.path().join("luau-pids");
    let quote =
        |path: &std::path::Path| format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"));
    let runtime = bgdeobf::astutil::find_bin("luau").unwrap();
    let shim = tools.path().join("luau");
    std::fs::write(
        &shim,
        format!(
            "#!/bin/sh\nprintf '%s %s\\n' \"$$\" \"$PPID\" > {}\nexec {} \"$@\"\n",
            quote(&marker),
            quote(&runtime),
        ),
    )
    .unwrap();
    std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "posix_dump_child_shares_worker_group_and_exits_when_the_group_is_killed",
            "--nocapture",
        ])
        .env("BGDEOBF_TEST_POSIX_DUMP_GROUP", "1")
        .env("BGDEOBF_LUAU_DIR", tools.path());
    for key in ["TMPDIR", "TEMP", "TMP"] {
        command.env(key, jobs.path());
    }
    let mut lim = limits();
    lim.timeout = Duration::from_secs(20);
    lim.cpu_seconds = 0;
    lim.memory_mb = 1024;
    lim.max_output_bytes = 65536;
    std::thread::scope(|scope| {
        let capture = scope.spawn(move || capture(command, &lim).unwrap());
        let deadline = Instant::now() + Duration::from_secs(10);
        let (luau, worker) = loop {
            if let Ok(text) = std::fs::read_to_string(&marker) {
                let ids = text
                    .split_whitespace()
                    .map(str::parse::<i32>)
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .unwrap();
                if let [luau, worker] = ids.as_slice() {
                    break (*luau, *worker);
                }
            }
            assert!(
                Instant::now() < deadline,
                "worker did not start its Luau dump child"
            );
            std::thread::sleep(Duration::from_millis(10));
        };
        assert!(worker > 1 && luau > 1 && worker != unsafe { libc::getpgrp() });
        struct KillOnFailure(i32, i32, bool);
        impl Drop for KillOnFailure {
            fn drop(&mut self) {
                if self.2 {
                    unsafe {
                        libc::kill(-self.0, libc::SIGKILL);
                        libc::kill(self.0, libc::SIGKILL);
                        libc::kill(self.1, libc::SIGKILL);
                    }
                }
            }
        }
        let mut cleanup = KillOnFailure(worker, luau, true);
        #[cfg(target_os = "linux")]
        while std::fs::read_link(format!("/proc/{luau}/exe"))
            .ok()
            .as_deref()
            != Some(runtime.as_path())
        {
            assert!(
                Instant::now() < deadline,
                "Luau shim never executed the real runtime"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(unsafe { libc::getpgid(worker) }, worker);
        assert_eq!(
            unsafe { libc::getpgid(luau) },
            worker,
            "Luau escaped the worker group"
        );
        #[cfg(target_os = "linux")]
        let pidfd = {
            use std::os::fd::{FromRawFd, OwnedFd};
            let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, luau, 0) } as i32;
            if fd < 0 {
                let error = std::io::Error::last_os_error();
                assert!(
                    matches!(
                        error.raw_os_error(),
                        Some(libc::ENOSYS | libc::EINVAL | libc::EPERM)
                    ),
                    "pidfd_open: {error}"
                );
                eprintln!("SKIP descendant pidfd assertion: pidfd_open unavailable: {error}");
                None
            } else {
                Some(unsafe { OwnedFd::from_raw_fd(fd) })
            }
        };
        assert_eq!(unsafe { libc::kill(-worker, libc::SIGKILL) }, 0);
        cleanup.2 = false;
        #[cfg(target_os = "linux")]
        if let Some(pidfd) = pidfd {
            use std::os::fd::AsRawFd;
            let mut poll = libc::pollfd {
                fd: pidfd.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            assert_eq!(
                unsafe { libc::poll(&mut poll, 1, 5000) },
                1,
                "Luau child survived worker group cancellation"
            );
            assert_ne!(poll.revents & libc::POLLIN, 0);
        }
        #[cfg(not(target_os = "linux"))]
        eprintln!("SKIP descendant pidfd assertion: pidfds are Linux-specific; group identity/kill and worker cleanup are still checked");
        let result = capture.join().unwrap();
        assert_eq!(
            result.status,
            "ok",
            "{}\n{}",
            String::from_utf8_lossy(&result.out),
            String::from_utf8_lossy(&result.err)
        );
    });
    assert_eq!(std::fs::read_dir(jobs.path()).unwrap().count(), 0);
}

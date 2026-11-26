//! Resource-bounded child processes. A child cannot run before its OS limits exist.
use crate::error::{Error, Result};
use std::{
    io::{Read, Seek, SeekFrom},
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Debug)]
pub struct Limits {
    pub timeout: Duration,
    pub memory_mb: u64,
    pub cpu_seconds: u64,
    pub max_output_bytes: usize,
    pub new_group: bool,
}

#[derive(Debug)]
pub struct Capture {
    pub out: Vec<u8>,
    pub err: Vec<u8>,
    pub status: String,
    pub code: Option<i32>,
    pub seconds: f64,
}

pub fn capture(command: Command, limits: &Limits) -> Result<Capture> {
    capture_with(command, limits, None, None, false)
}

pub(crate) static IN_WORKER: AtomicBool = AtomicBool::new(false);
type OutputObserver<'a> = Option<&'a mut dyn FnMut(&[u8])>;

/// The observer runs on the caller's thread and sees bounded incremental stdout.
pub(crate) fn capture_with(
    command: Command,
    limits: &Limits,
    cancel: Option<&AtomicBool>,
    observer: OutputObserver<'_>,
    terminal: bool,
) -> Result<Capture> {
    #[cfg(windows)]
    {
        capture_child(
            command,
            limits,
            cancel,
            observer,
            terminal,
            |_| Ok(()),
            |mut job| job.terminate(),
        )
    }
    #[cfg(not(windows))]
    {
        capture_child(command, limits, cancel, observer, terminal, |_| Ok(()))
    }
}

fn capture_child(
    mut command: Command,
    limits: &Limits,
    cancel: Option<&AtomicBool>,
    mut observer: OutputObserver<'_>,
    terminal: bool,
    after_spawn: impl FnOnce(&Child) -> Result<()>,
    #[cfg(windows)] finish: impl FnOnce(platform::Job) -> Result<()>,
) -> Result<Capture> {
    if limits.timeout.is_zero() || limits.max_output_bytes == 0 {
        return Err(Error::new(
            "sandbox",
            "process deadline and output limit must be positive",
        ));
    }
    let start = Instant::now();
    let out_path = tempfile::NamedTempFile::new()?;
    let mut out_file = out_path.reopen()?;
    // Reopen rather than duplicate: polling must not move the child's write offset.
    let out_writer = out_path.reopen()?;
    command
        .stdout(Stdio::from(out_writer))
        .stderr(Stdio::piped());
    #[cfg(unix)]
    let mut pty = if terminal {
        platform::Pty::open()?
    } else {
        None
    };
    #[cfg(unix)]
    if let Some(pty) = &pty {
        command.stdout(Stdio::from(
            pty.slave.as_ref().expect("open PTY slave").try_clone()?,
        ));
    }
    #[cfg(not(unix))]
    let _ = terminal;
    platform::prepare(&mut command, limits)?;
    #[cfg(windows)]
    let mut job = platform::Job::new(limits)?;
    let child = command
        .spawn()
        .map_err(|e| Error::new("sandbox", format!("cannot start isolated process: {e}")))?;
    let mut child = Reap {
        child,
        #[cfg(unix)]
        group: limits.new_group,
    };
    #[cfg(windows)]
    job.attach(&mut child.child)?;
    after_spawn(&child.child)?;
    #[cfg(unix)]
    if let Some(pty) = &mut pty {
        pty.close_slave();
    }
    let mut stderr = child
        .child
        .stderr
        .take()
        .ok_or_else(|| Error::new("sandbox", "isolated child stderr was not captured"))?;
    let err_buffer = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let drain_buffer = err_buffer.clone();
    let (done_tx, done_rx) = std::sync::mpsc::sync_channel(1);
    thread::Builder::new()
        .name("bgdeobf-stderr".into())
        .spawn(move || {
            let mut block = [0; 4096];
            loop {
                match stderr.read(&mut block) {
                    Ok(0) => break,
                    Ok(n) => {
                        let mut buffer = drain_buffer.lock().unwrap_or_else(|e| e.into_inner());
                        let keep = n.min(65536_usize.saturating_sub(buffer.len()));
                        buffer.extend_from_slice(&block[..keep]);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
            let _ = done_tx.send(());
        })?;
    let wire_limit = limits.max_output_bytes;
    let mut out = Vec::new();
    let mut status = None;
    let mut offset = 0_u64;
    let exit = loop {
        #[cfg(unix)]
        if let Some(pty) = &mut pty {
            let before = out.len();
            pty.drain(&mut out, wire_limit.saturating_add(1))?;
            if let Some(note) = observer.as_mut() {
                note(&out[before..]);
            }
        }
        #[cfg(unix)]
        let file_stdout = pty.is_none();
        #[cfg(not(unix))]
        let file_stdout = true;
        if file_stdout && observer.is_some() {
            let total = out_file
                .metadata()?
                .len()
                .min(wire_limit.saturating_add(1) as u64);
            if total > offset {
                out_file.seek(SeekFrom::Start(offset))?;
                let mut part = vec![0; (total - offset) as usize];
                out_file.read_exact(&mut part)?;
                offset = total;
                if let Some(note) = observer.as_mut() {
                    note(&part);
                }
            }
        }
        let stdout_size = if file_stdout {
            out_file.metadata()?.len()
        } else {
            out.len() as u64
        };
        if stdout_size > wire_limit as u64 {
            status = Some("output");
        } else if cancel.is_some_and(|flag| flag.load(Ordering::Acquire)) {
            status = Some("killed");
        } else if start.elapsed() >= limits.timeout {
            status = Some("timeout");
        }
        if status.is_some() {
            #[cfg(windows)]
            job.terminate()?;
            child.kill();
            break child.child.wait()?;
        }
        if let Some(exit) = child.child.try_wait()? {
            break exit;
        }
        thread::sleep(
            Duration::from_millis(10).min(limits.timeout.saturating_sub(start.elapsed())),
        );
    };
    // Kill even a successful child's surviving descendants before touching its files.
    #[cfg(windows)]
    finish(job)?;
    #[cfg(unix)]
    child.kill();
    #[cfg(unix)]
    if let Some(pty) = &mut pty {
        pty.drain(&mut out, wire_limit.saturating_add(1))?;
    }
    #[cfg(unix)]
    let file_stdout = pty.is_none();
    #[cfg(not(unix))]
    let file_stdout = true;
    if file_stdout {
        out_file.seek(SeekFrom::Start(0))?;
        out_file
            .take(wire_limit.saturating_add(1) as u64)
            .read_to_end(&mut out)?;
        if offset < out.len() as u64 {
            if let Some(note) = observer.as_mut() {
                note(&out[offset as usize..]);
            }
        }
    }
    let _ = done_rx.recv_timeout(Duration::from_secs(1));
    let err = err_buffer.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if out.len() > wire_limit {
        status = status.or(Some("output"));
        out.truncate(wire_limit);
    }
    let code = exit.code();
    let status = status.unwrap_or_else(|| {
        if exit.success() {
            "ok"
        } else if code.is_none() || code.is_some_and(|c| c < 0) {
            "killed"
        } else {
            "crashed"
        }
    });
    Ok(Capture {
        out,
        err,
        status: status.into(),
        code,
        seconds: start.elapsed().as_secs_f64(),
    })
}

struct Reap {
    child: Child,
    #[cfg(unix)]
    group: bool,
}
impl Reap {
    fn kill(&mut self) {
        #[cfg(unix)]
        if self.group {
            // The child is the session leader; no signal ever targets our own group.
            unsafe {
                libc::kill(-(self.child.id() as i32), libc::SIGKILL);
            }
        }
        let _ = self.child.kill();
    }
}
impl Drop for Reap {
    fn drop(&mut self) {
        self.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Worker {
        pid: u32,
        directory: std::path::PathBuf,
        #[cfg(windows)]
        handle: std::os::windows::io::OwnedHandle,
    }

    impl Worker {
        fn record(child: &Child, directory: std::path::PathBuf) -> Result<Self> {
            #[cfg(windows)]
            use std::os::windows::io::AsHandle;
            Ok(Self {
                pid: child.id(),
                directory,
                #[cfg(windows)]
                handle: child.as_handle().try_clone_to_owned()?,
            })
        }

        fn assert_reaped_and_removed(&self) {
            #[cfg(windows)]
            {
                use std::os::windows::io::AsRawHandle;
                assert_eq!(
                    unsafe {
                        windows_sys::Win32::System::Threading::WaitForSingleObject(
                            self.handle.as_raw_handle(),
                            0,
                        )
                    },
                    0,
                    "worker {} survived return",
                    self.pid
                );
            }
            #[cfg(unix)]
            {
                assert_eq!(
                    unsafe { libc::kill(self.pid as i32, 0) },
                    -1,
                    "worker {} survived return",
                    self.pid
                );
                assert_eq!(
                    std::io::Error::last_os_error().raw_os_error(),
                    Some(libc::ESRCH)
                );
            }
            assert!(
                !self.directory.exists(),
                "worker directory survived: {:?}",
                self.directory
            );
        }
    }

    fn isolated_temp(test: &str, timeout: Duration) -> bool {
        if std::env::var("BGDEOBF_TEST_LIFECYCLE_CHILD").as_deref() == Ok(test) {
            return false;
        }
        let root = tempfile::tempdir().unwrap();
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", &format!("process::tests::{test}"), "--nocapture"])
            .env("BGDEOBF_TEST_LIFECYCLE_CHILD", test);
        for key in ["TMPDIR", "TEMP", "TMP"] {
            command.env(key, root.path());
        }
        let result = capture(
            command,
            &Limits {
                timeout,
                memory_mb: 1024,
                cpu_seconds: 0,
                max_output_bytes: 65536,
                new_group: true,
            },
        )
        .unwrap();
        assert_eq!(
            result.status,
            "ok",
            "{}\n{}",
            String::from_utf8_lossy(&result.out),
            String::from_utf8_lossy(&result.err)
        );
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
        true
    }

    fn recording_capture(
        command: Command,
        limits: &Limits,
        cancel: Option<&AtomicBool>,
        observer: &mut dyn FnMut(&[u8]),
        workers: &mut Vec<Worker>,
    ) -> Result<Capture> {
        let directory = command.get_current_dir().unwrap().to_path_buf();
        let record = |child: &Child| {
            workers.push(Worker::record(child, directory)?);
            Ok(())
        };
        #[cfg(windows)]
        {
            capture_child(
                command,
                limits,
                cancel,
                Some(observer),
                false,
                record,
                |mut job| job.terminate(),
            )
        }
        #[cfg(not(windows))]
        {
            capture_child(command, limits, cancel, Some(observer), false, record)
        }
    }

    #[test]
    fn delayed_unpack_deadlines_and_cancellation_reap_workers_and_remove_private_roots() {
        if isolated_temp(
            "delayed_unpack_deadlines_and_cancellation_reap_workers_and_remove_private_roots",
            Duration::from_secs(40),
        ) {
            return;
        }
        let source = std::str::from_utf8(include_bytes!("../test/samples/safe_vm.lua"))
            .unwrap()
            .replace("\r\n", "\n");
        let entry = "local function module(host, env)\n";
        assert_eq!(source.matches(entry).count(), 1);
        let source = source.replacen(entry, &format!("{entry}    local deadline = os.clock() + 30\n    while os.clock() < deadline do end\n"), 1);
        let mut workers = Vec::new();
        // The first case retains the original single hard-timeout assertion; the rest are its seven-case matrix.
        for millis in [1000, 300, 500, 700, 900, 1200, 2000, 4000] {
            let start = Instant::now();
            let error = crate::runner::run_with_capture(
                source.as_bytes(),
                &Default::default(),
                Duration::from_millis(millis),
                None,
                None,
                |command, limits, observer| {
                    recording_capture(command, limits, None, observer, &mut workers)
                },
            )
            .unwrap_err();
            assert_eq!(error.code, "timeout", "hard deadline {millis} ms: {error}");
            if millis == 1000 {
                assert!(start.elapsed() < Duration::from_secs(6));
            }
            workers.last().unwrap().assert_reaped_and_removed();
            assert_eq!(std::fs::read_dir(std::env::temp_dir()).unwrap().count(), 0);
        }
        let cancelled = AtomicBool::new(false);
        let progress = |stage: &str| {
            if stage == "unpack" {
                cancelled.store(true, Ordering::Release);
            }
        };
        let error = crate::runner::run_with_capture(
            source.as_bytes(),
            &Default::default(),
            Duration::from_secs(100),
            Some(&progress),
            Some(&cancelled),
            |command, limits, observer| {
                recording_capture(command, limits, Some(&cancelled), observer, &mut workers)
            },
        )
        .unwrap_err();
        assert!(
            cancelled.load(Ordering::Acquire),
            "worker never reached unpack"
        );
        assert_eq!(error.code, "cancelled");
        assert_eq!(workers.len(), 9);
        for worker in workers {
            worker.assert_reaped_and_removed();
        }
        assert_eq!(std::fs::read_dir(std::env::temp_dir()).unwrap().count(), 0);
    }

    #[cfg(windows)]
    #[test]
    fn worker_cleanup_preserves_job_teardown_error_after_real_termination() {
        if isolated_temp(
            "worker_cleanup_preserves_job_teardown_error_after_real_termination",
            Duration::from_secs(20),
        ) {
            return;
        }
        let mut workers = Vec::new();
        let opts = crate::model::Options {
            sandbox_only: true,
            ..Default::default()
        };
        let error = crate::runner::run_with_capture(
            b"while true do end",
            &opts,
            Duration::from_millis(500),
            None,
            None,
            |command, limits, observer| {
                let directory = command.get_current_dir().unwrap().to_path_buf();
                capture_child(
                    command,
                    limits,
                    None,
                    Some(observer),
                    false,
                    |child| {
                        workers.push(Worker::record(child, directory)?);
                        Ok(())
                    },
                    |mut job| {
                        job.terminate()?;
                        drop(job);
                        Err(Error::new("sandbox", "injected job teardown failure"))
                    },
                )
            },
        )
        .unwrap_err();
        assert_eq!(error.code, "sandbox");
        assert_eq!(error.message, "injected job teardown failure");
        assert_eq!(workers.len(), 1);
        workers[0].assert_reaped_and_removed();
        assert_eq!(std::fs::read_dir(std::env::temp_dir()).unwrap().count(), 0);
    }

    #[cfg(windows)]
    #[test]
    fn suspended_child_only_runs_after_job_assignment() {
        let work = tempfile::tempdir().unwrap();
        let limits = Limits {
            timeout: Duration::from_secs(5),
            memory_mb: 128,
            cpu_seconds: 2,
            max_output_bytes: 1024,
            new_group: true,
        };
        let mut command = Command::new("cmd.exe");
        command
            .args(["/d", "/c", "echo resumed>marker.txt"])
            .current_dir(work.path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        platform::prepare(&mut command, &limits).unwrap();
        let mut job = platform::Job::new(&limits).unwrap();
        let mut child = Reap {
            child: command.spawn().unwrap(),
        };
        thread::sleep(Duration::from_millis(50));
        assert!(
            !work.path().join("marker.txt").exists(),
            "child executed while still suspended"
        );
        job.attach(&mut child.child).unwrap();
        assert!(child.child.wait().unwrap().success());
        job.terminate().unwrap();
        assert!(std::fs::read(work.path().join("marker.txt"))
            .unwrap()
            .starts_with(b"resumed"));
    }
}

#[cfg(windows)]
mod platform {
    use super::*;
    use std::{
        mem::{size_of, zeroed},
        os::windows::{io::AsRawHandle, process::CommandExt},
        ptr,
    };
    use windows_sys::Win32::{
        Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE},
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD,
                THREADENTRY32,
            },
            JobObjects::*,
            Threading::{OpenThread, ResumeThread, CREATE_SUSPENDED, THREAD_SUSPEND_RESUME},
        },
    };
    fn failure(context: &str) -> Error {
        Error::new(
            "sandbox",
            format!("{context}: {}", std::io::Error::last_os_error()),
        )
    }
    pub fn prepare(command: &mut Command, _: &Limits) -> Result<()> {
        command.creation_flags(CREATE_SUSPENDED);
        Ok(())
    }
    struct Handle(HANDLE);
    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
    pub struct Job {
        handle: Handle,
        stopped: bool,
    }
    impl Job {
        pub fn new(limits: &Limits) -> Result<Self> {
            unsafe {
                let handle = CreateJobObjectW(ptr::null(), ptr::null());
                if handle.is_null() {
                    return Err(failure("cannot create process job"));
                }
                let job = Self {
                    handle: Handle(handle),
                    stopped: false,
                };
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                if limits.memory_mb > 0 {
                    let bytes = limits
                        .memory_mb
                        .checked_mul(1024 * 1024)
                        .and_then(|b| usize::try_from(b).ok())
                        .ok_or_else(|| Error::new("sandbox", "memory limit overflow"))?;
                    info.ProcessMemoryLimit = bytes;
                    info.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_PROCESS_MEMORY;
                }
                if limits.cpu_seconds > 0 {
                    info.BasicLimitInformation.PerProcessUserTimeLimit =
                        i64::try_from(limits.cpu_seconds)
                            .ok()
                            .and_then(|v| v.checked_mul(10_000_000))
                            .ok_or_else(|| Error::new("sandbox", "CPU limit overflow"))?;
                    info.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_PROCESS_TIME;
                }
                if SetInformationJobObject(
                    handle,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const _,
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                ) == 0
                {
                    return Err(failure("cannot set process job limits"));
                }
                Ok(job)
            }
        }
        pub fn attach(&mut self, child: &mut Child) -> Result<()> {
            unsafe {
                if AssignProcessToJobObject(self.handle.0, child.as_raw_handle() as HANDLE) == 0 {
                    return Err(failure("cannot assign suspended child to job"));
                }
                let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
                if snapshot == INVALID_HANDLE_VALUE {
                    return Err(failure("cannot enumerate suspended thread"));
                }
                let snapshot = Handle(snapshot);
                let mut entry: THREADENTRY32 = zeroed();
                entry.dwSize = size_of::<THREADENTRY32>() as u32;
                let mut found = Thread32First(snapshot.0, &mut entry);
                while found != 0 {
                    if entry.th32OwnerProcessID == child.id() {
                        let thread = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                        if thread.is_null() {
                            return Err(failure("cannot open suspended child thread"));
                        }
                        let thread = Handle(thread);
                        if ResumeThread(thread.0) == u32::MAX {
                            return Err(failure("cannot resume isolated child"));
                        }
                        return Ok(());
                    }
                    found = Thread32Next(snapshot.0, &mut entry);
                }
                Err(Error::new(
                    "sandbox",
                    "the suspended child thread was not found",
                ))
            }
        }
        pub fn terminate(&mut self) -> Result<()> {
            if self.stopped {
                return Ok(());
            }
            unsafe {
                if TerminateJobObject(self.handle.0, 1) == 0 {
                    return Err(failure("cannot terminate process job"));
                }
                let deadline = Instant::now() + Duration::from_secs(5);
                loop {
                    let mut accounting: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = zeroed();
                    if QueryInformationJobObject(
                        self.handle.0,
                        JobObjectBasicAccountingInformation,
                        &mut accounting as *mut _ as *mut _,
                        size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                        ptr::null_mut(),
                    ) == 0
                    {
                        return Err(failure("cannot query terminating job"));
                    }
                    if accounting.ActiveProcesses == 0 {
                        self.stopped = true;
                        return Ok(());
                    }
                    if Instant::now() >= deadline {
                        return Err(Error::new(
                            "sandbox",
                            "Windows job descendants did not terminate",
                        ));
                    }
                    thread::sleep(Duration::from_millis(10));
                }
            }
        }
    }
    impl Drop for Job {
        fn drop(&mut self) {
            let _ = self.terminate();
        }
    }
}

#[cfg(unix)]
mod platform {
    use super::*;
    use std::fs::File;
    use std::os::{fd::FromRawFd, unix::process::CommandExt};
    pub fn prepare(command: &mut Command, limits: &Limits) -> Result<()> {
        let limits = limits.clone();
        let memory = limits
            .memory_mb
            .checked_mul(1024 * 1024)
            .ok_or_else(|| Error::new("sandbox", "memory limit overflow"))?;
        let parent_pid = std::process::id();
        unsafe {
            command.pre_exec(move || {
                #[cfg(any(target_os = "linux", target_os = "android"))]
                type Resource = libc::__rlimit_resource_t;
                #[cfg(not(any(target_os = "linux", target_os = "android")))]
                type Resource = libc::c_int;
                fn set(resource: Resource, soft: u64, hard: u64) -> std::io::Result<()> {
                    let r = libc::rlimit {
                        rlim_cur: soft as libc::rlim_t,
                        rlim_max: hard as libc::rlim_t,
                    };
                    if unsafe { libc::setrlimit(resource, &r) } != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                }
                if limits.new_group && libc::setsid() < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if memory > 0 {
                    set(libc::RLIMIT_AS, memory, memory)?;
                }
                if limits.cpu_seconds > 0 {
                    set(
                        libc::RLIMIT_CPU,
                        limits.cpu_seconds,
                        limits.cpu_seconds.saturating_add(1),
                    )?;
                }
                set(
                    libc::RLIMIT_FSIZE,
                    limits.max_output_bytes as u64,
                    limits.max_output_bytes as u64,
                )?;
                set(libc::RLIMIT_NOFILE, 32, 32)?;
                set(libc::RLIMIT_CORE, 0, 0)?;
                #[cfg(target_os = "linux")]
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0
                    || libc::getppid() != parent_pid as i32
                {
                    return Err(std::io::Error::last_os_error());
                }
                #[cfg(not(target_os = "linux"))]
                let _ = parent_pid;
                Ok(())
            });
        }
        Ok(())
    }
    pub struct Pty {
        master: File,
        pub slave: Option<File>,
    }
    impl Pty {
        pub fn open() -> Result<Option<Self>> {
            unsafe {
                let (mut master, mut slave) = (-1, -1);
                if libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    std::ptr::null(),
                ) != 0
                {
                    return Err(Error::new(
                        "sandbox",
                        format!(
                            "cannot open the sandbox terminal: {}",
                            std::io::Error::last_os_error()
                        ),
                    ));
                }
                let pty = Self {
                    master: File::from_raw_fd(master),
                    slave: Some(File::from_raw_fd(slave)),
                };
                let mut term: libc::termios = std::mem::zeroed();
                if libc::tcgetattr(slave, &mut term) != 0 {
                    return Err(std::io::Error::last_os_error().into());
                }
                libc::cfmakeraw(&mut term);
                if libc::tcsetattr(slave, libc::TCSANOW, &term) != 0
                    || libc::fcntl(master, libc::F_SETFL, libc::O_NONBLOCK) < 0
                    || libc::fcntl(master, libc::F_SETFD, libc::FD_CLOEXEC) < 0
                    || libc::fcntl(slave, libc::F_SETFD, libc::FD_CLOEXEC) < 0
                {
                    return Err(std::io::Error::last_os_error().into());
                }
                Ok(Some(pty))
            }
        }
        pub fn close_slave(&mut self) {
            self.slave.take();
        }
        pub fn drain(&mut self, out: &mut Vec<u8>, cap: usize) -> Result<()> {
            let mut buf = [0; 65536];
            while out.len() < cap {
                let size = buf.len().min(cap - out.len());
                match self.master.read(&mut buf[..size]) {
                    Ok(0) => break,
                    Ok(n) => out.extend_from_slice(&buf[..n]),
                    Err(e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            || e.raw_os_error() == Some(libc::EIO) =>
                    {
                        break
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(e) => return Err(e.into()),
                }
            }
            Ok(())
        }
    }
}

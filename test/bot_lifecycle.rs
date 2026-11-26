use bgdeobf::{
    bot::{self, Delivery, Handler, Identity, InputFile, Network, Request, Responder},
    error::Result,
    model::Options,
    service::Service,
};
use serenity::async_trait;
use std::{
    collections::BTreeMap,
    process::Command,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::sync::Notify;

const HANGING_SANDBOX: &[u8] = b"while true do end";
const FINITE_SANDBOX: &[u8] = b"print('lifecycle complete')";

#[derive(Default)]
struct OfflineNetwork {
    calls: Mutex<Vec<String>>,
}

#[async_trait]
impl Network for OfflineNetwork {
    async fn download(&self, url: &str, _: usize, _: &[String], _: Duration) -> Result<Vec<u8>> {
        self.calls.lock().unwrap().push(url.to_owned());
        Ok(if url.contains("hang") {
            HANGING_SANDBOX.to_vec()
        } else {
            FINITE_SANDBOX.to_vec()
        })
    }
}

#[derive(Clone)]
struct Probe {
    user: u64,
    started: Arc<AtomicBool>,
    started_notify: Arc<Notify>,
    queued: Arc<AtomicBool>,
    queued_notify: Arc<Notify>,
    delivery: Arc<Mutex<Option<Delivery>>>,
}

impl Probe {
    fn new(user: u64) -> Self {
        Self {
            user,
            started: Arc::new(AtomicBool::new(false)),
            started_notify: Arc::new(Notify::new()),
            queued: Arc::new(AtomicBool::new(false)),
            queued_notify: Arc::new(Notify::new()),
            delivery: Arc::new(Mutex::new(None)),
        }
    }

    async fn wait_started(&self) {
        wait_flag(&self.started, &self.started_notify).await;
    }

    async fn wait_queued(&self) {
        wait_flag(&self.queued, &self.queued_notify).await;
    }

    fn delivery(&self) -> Delivery {
        self.delivery.lock().unwrap().clone().unwrap()
    }
}

async fn wait_flag(flag: &AtomicBool, notify: &Notify) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !flag.load(Ordering::Acquire) {
            notify.notified().await;
        }
    })
    .await
    .expect("worker progress was not observed");
}

#[async_trait]
impl Responder for Probe {
    fn identity(&self) -> Identity {
        Identity {
            user: self.user,
            guild: None,
            upload_limit: bot::DM_UPLOAD_LIMIT,
        }
    }

    async fn status(&mut self, _: &str) -> Result<()> {
        Ok(())
    }

    async fn show(&mut self, card: bgdeobf::card::CardData) -> Result<()> {
        if card.description.contains("running it in the fake Roblox") {
            self.started.store(true, Ordering::Release);
            self.started_notify.notify_one();
        }
        if card.description.contains("Waiting in the queue") {
            self.queued.store(true, Ordering::Release);
            self.queued_notify.notify_one();
        }
        Ok(())
    }

    async fn finish(
        &mut self,
        delivery: Delivery,
        _: Option<bgdeobf::card::CardData>,
    ) -> Result<()> {
        *self.delivery.lock().unwrap() = Some(delivery);
        Ok(())
    }
}

fn config(max_wait: Duration) -> bot::Config {
    let mut cfg = bot::Config::from_env(&BTreeMap::from([
        ("WORKERS".into(), "1".into()),
        ("MAX_QUEUE".into(), "1".into()),
        ("COOLDOWN".into(), "0".into()),
    ]))
    .unwrap();
    cfg.max_wait = max_wait;
    cfg.progress_card = true;
    cfg
}

fn request(name: &str) -> Request {
    Request {
        file: Some(InputFile {
            filename: name.into(),
            size: HANGING_SANDBOX.len(),
            url: format!("https://offline.invalid/{name}"),
        }),
        sandbox: true,
        ..Default::default()
    }
}

async fn wait_empty(service: &Service) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if service.running() == 0 && service.queued() == 0 {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

fn isolated_temp(test: &str) -> bool {
    if std::env::var("BGDEOBF_TEST_SERVICE_LIFECYCLE").as_deref() == Ok(test) {
        return false;
    }
    let root = tempfile::tempdir().unwrap();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", test, "--nocapture"])
        .env("BGDEOBF_TEST_SERVICE_LIFECYCLE", test);
    for key in ["TMPDIR", "TEMP", "TMP"] {
        command.env(key, root.path());
    }
    let result = bgdeobf::process::capture(
        command,
        &bgdeobf::process::Limits {
            timeout: Duration::from_secs(40),
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn handler_service_admission_timeout_and_re_admission_use_native_worker() {
    if isolated_temp("handler_service_admission_timeout_and_re_admission_use_native_worker") {
        return;
    }
    let service = Arc::new(Service::new(1, 1, Duration::ZERO, Duration::from_secs(15)));
    let handler = Arc::new(Handler::new(
        config(Duration::from_secs(8)),
        service.clone(),
        Arc::new(OfflineNetwork::default()),
    ));

    let first_probe = Probe::new(101);
    let first_wait = first_probe.clone();
    let first_handler = handler.clone();
    let first = tokio::spawn(async move {
        let mut probe = first_probe;
        first_handler
            .run(&mut probe, request("hang-first.lua"))
            .await
    });
    first_wait.wait_started().await;

    let second_probe = Probe::new(102);
    let second_wait = second_probe.clone();
    let second_handler = handler.clone();
    let second = tokio::spawn(async move {
        let mut probe = second_probe;
        second_handler
            .run(&mut probe, request("hang-second.lua"))
            .await
    });
    second_wait.wait_queued().await;

    let third_probe = Probe::new(103);
    let third_wait = third_probe.clone();
    let third_handler = handler.clone();
    let third = tokio::spawn(async move {
        let mut probe = third_probe;
        third_handler
            .run(&mut probe, request("hang-third.lua"))
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), third)
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    first.await.unwrap().unwrap();
    second.await.unwrap().unwrap();
    wait_empty(&service).await;
    assert!(third_wait.delivery().text.contains("queue is full"));
    assert!(first_wait.delivery().text.contains("too long"));
    assert!(second_wait.delivery().text.contains("too long"));

    let mut re_admitted = Probe::new(101);
    let mut finite = request("finite.lua");
    finite.file.as_mut().unwrap().size = FINITE_SANDBOX.len();
    handler.run(&mut re_admitted, finite).await.unwrap();
    assert!(re_admitted.delivery().ok);
    assert_eq!(std::fs::read_dir(std::env::temp_dir()).unwrap().count(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn service_close_cancels_running_and_queued_native_workers_and_waits_for_cleanup() {
    if isolated_temp(
        "service_close_cancels_running_and_queued_native_workers_and_waits_for_cleanup",
    ) {
        return;
    }
    let service = Arc::new(Service::new(1, 1, Duration::ZERO, Duration::from_secs(30)));
    let first_started = Arc::new(AtomicBool::new(false));
    let first_notify = Arc::new(Notify::new());
    let first_service = service.clone();
    let first_flag = first_started.clone();
    let first_signal = first_notify.clone();
    let first = tokio::spawn(async move {
        first_service
            .submit(
                201,
                HANGING_SANDBOX.to_vec(),
                Options {
                    sandbox_only: true,
                    ..Default::default()
                },
                Some(Arc::new(move |stage| {
                    if stage == "sandbox" {
                        first_flag.store(true, Ordering::Release);
                        first_signal.notify_one();
                    }
                })),
            )
            .await
    });
    wait_flag(&first_started, &first_notify).await;

    let queued = Arc::new(AtomicBool::new(false));
    let queued_notify = Arc::new(Notify::new());
    let queued_service = service.clone();
    let queued_flag = queued.clone();
    let queued_signal = queued_notify.clone();
    let second = tokio::spawn(async move {
        queued_service
            .submit(
                202,
                HANGING_SANDBOX.to_vec(),
                Options {
                    sandbox_only: true,
                    ..Default::default()
                },
                Some(Arc::new(move |stage| {
                    if stage == "queued:1" {
                        queued_flag.store(true, Ordering::Release);
                        queued_signal.notify_one();
                    }
                })),
            )
            .await
    });
    wait_flag(&queued, &queued_notify).await;

    tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(service.close(), service.close());
    })
    .await
    .expect("service shutdown did not reap its worker");
    let first_result = tokio::time::timeout(Duration::from_secs(1), first)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    let second_result = tokio::time::timeout(Duration::from_secs(1), second)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(first_result.code, "cancelled");
    assert_eq!(second_result.code, "internal");
    assert_eq!((service.running(), service.queued()), (0, 0));
    service.close().await;
    assert_eq!(std::fs::read_dir(std::env::temp_dir()).unwrap().count(), 0);
    assert_eq!(
        service
            .submit(203, FINITE_SANDBOX.to_vec(), Options::default(), None)
            .await
            .unwrap_err()
            .code,
        "internal"
    );
}

#[test]
fn bot_executable_rejects_missing_token_and_invalid_config_without_connecting() {
    for (key, value, diagnostic) in [
        ("DISCORD_TOKEN", "", "DISCORD_TOKEN is not set"),
        ("WORKERS", "0", "Invalid configuration"),
    ] {
        let root = tempfile::tempdir().unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_bgdeobf-bot"));
        command.current_dir(root.path()).env_clear();
        for name in ["SYSTEMROOT", "WINDIR", "PATH"] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        for name in ["TMPDIR", "TEMP", "TMP"] {
            command.env(name, root.path());
        }
        command.env(key, value);
        let output = bgdeobf::process::capture(
            command,
            &bgdeobf::process::Limits {
                timeout: Duration::from_secs(10),
                memory_mb: 1024,
                cpu_seconds: 0,
                max_output_bytes: 65536,
                new_group: true,
            },
        )
        .unwrap();
        assert_eq!(output.code, Some(1), "{key}: {output:?}");
        assert!(output.out.is_empty());
        assert!(String::from_utf8_lossy(&output.err).contains(diagnostic));
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    }
}

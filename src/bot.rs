//! Native Discord command adapter and independently testable job/delivery logic.
use crate::{
    card::{Card, CardData},
    error::{Error, Result},
    fetch, messages,
    model::{Options, Output},
    namesfile::parse_names,
    service::Service,
};
use regex::Regex;
use serenity::{all::*, async_trait};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    future::Future,
    io::{Cursor, Write},
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

pub const NAMES_MAX_BYTES: usize = 256 * 1024;
pub const DM_UPLOAD_LIMIT: usize = 10 * 1024 * 1024;
pub const NOTE:&str="Generated locals use clean neutral names (v1, v2, p1); original local names are unavailable. Only analyze scripts you own or are allowed to analyze.";
pub type Progress = Arc<dyn Fn(&str) + Send + Sync>;

// Deliberately no Debug: configuration contains the Discord credential.
#[derive(Clone)]
pub struct Config {
    pub token: String,
    pub guild_id: Option<u64>,
    pub allowed_guilds: HashSet<u64>,
    pub allowed_users: HashSet<u64>,
    pub max_mb: f64,
    pub workers: usize,
    pub max_queue: usize,
    pub timeout: Duration,
    pub cooldown: Duration,
    pub prefix: String,
    pub prefix_commands: bool,
    pub progress_card: bool,
    pub spinner_emoji: Option<String>,
    pub url_hosts: Vec<String>,
    pub url_timeout: Duration,
    pub max_wait: Duration,
}
impl Config {
    pub fn from_env(env: &BTreeMap<String, String>) -> Result<Self> {
        let get = |k: &str, default: &str| {
            env.get(k)
                .filter(|s| !s.is_empty())
                .cloned()
                .unwrap_or_else(|| default.into())
        };
        let bad = || Error::new("config", "invalid value in .env");
        let ids = |key: &str| -> Result<HashSet<u64>> {
            get(key, "")
                .split(|c: char| c == ',' || c.is_whitespace())
                .filter(|s| !s.is_empty())
                .map(|s| s.parse().map_err(|_| bad()))
                .collect()
        };
        let flag = |key: &str| {
            !matches!(
                get(key, "true").trim().to_ascii_lowercase().as_str(),
                "0" | "false" | "no" | "off" | "tidak"
            )
        };
        let max_mb = get("MAX_MB", "3")
            .trim()
            .parse::<f64>()
            .map_err(|_| bad())?;
        let workers = get("WORKERS", "2")
            .trim()
            .parse::<usize>()
            .map_err(|_| bad())?;
        let max_queue = get("MAX_QUEUE", "8")
            .trim()
            .parse::<usize>()
            .map_err(|_| bad())?;
        let timeout = get("JOB_TIMEOUT", "240")
            .trim()
            .parse::<u64>()
            .map_err(|_| bad())?;
        let cooldown = get("COOLDOWN", "15")
            .trim()
            .parse::<f64>()
            .map_err(|_| bad())?;
        if !max_mb.is_finite()
            || !(0.0 < max_mb
                && max_mb <= 8.0
                && (1..=16).contains(&workers)
                && max_queue <= 100
                && (30..=900).contains(&timeout))
        {
            return Err(Error::new("config","MAX_MB (0-8), WORKERS (1-16), MAX_QUEUE (0-100), JOB_TIMEOUT (30-900) out of range"));
        }
        if !cooldown.is_finite() || cooldown < 0.0 || cooldown >= u64::MAX as f64 {
            return Err(Error::new(
                "config",
                "COOLDOWN must be a finite nonnegative number",
            ));
        }
        let prefix = get("PREFIX", "!").trim().to_owned();
        let prefix = if prefix.is_empty() {
            "!".into()
        } else {
            prefix
        };
        if prefix.chars().count() > 3 || prefix.chars().any(char::is_whitespace) {
            return Err(Error::new(
                "config",
                "PREFIX must be 1-3 characters with no spaces",
            ));
        }
        let spinner = get("SPINNER_EMOJI", "").trim().to_owned();
        if !spinner.is_empty()
            && !Regex::new(r"^<a?:\w{2,32}:\d{15,25}>$")
                .unwrap()
                .is_match(&spinner)
        {
            return Err(Error::new(
                "config",
                "SPINNER_EMOJI must look like <a:name:123456789012345678>",
            ));
        }
        let mut url_hosts: Vec<_> = get("URL_HOSTS", "")
            .split(|c: char| c == ',' || c.is_whitespace())
            .filter(|s| !s.is_empty())
            .map(|s| s.trim_matches('.').to_ascii_lowercase())
            .collect();
        url_hosts.sort();
        url_hosts.dedup();
        if url_hosts
            .iter()
            .any(|s| !Regex::new(r"^[a-z0-9.-]{1,253}$").unwrap().is_match(s))
        {
            return Err(Error::new(
                "config",
                "URL_HOSTS contains an invalid host name",
            ));
        }
        let guild_id = env
            .get("GUILD_ID")
            .filter(|s| !s.is_empty())
            .map(|s| s.trim().parse::<u64>().map_err(|_| bad()))
            .transpose()?
            .filter(|id| *id != 0);
        Ok(Self {
            token: get("DISCORD_TOKEN", "").trim().into(),
            guild_id,
            allowed_guilds: ids("ALLOWED_GUILDS")?,
            allowed_users: ids("ALLOWED_USERS")?,
            max_mb,
            workers,
            max_queue,
            timeout: Duration::from_secs(timeout),
            cooldown: Duration::from_secs_f64(cooldown),
            prefix,
            prefix_commands: flag("PREFIX_COMMANDS"),
            progress_card: flag("PROGRESS_CARD"),
            spinner_emoji: if spinner.is_empty() {
                None
            } else {
                Some(spinner)
            },
            url_hosts,
            url_timeout: Duration::from_secs(25),
            max_wait: Duration::from_secs(13 * 60),
        })
    }
    pub fn max_bytes(&self) -> usize {
        (self.max_mb * 1048576.0) as usize
    }
}

pub fn parse_dotenv(text: &str, env: &mut BTreeMap<String, String>) {
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        if let Some((key, value)) = line.split_once('=') {
            let v = value.trim();
            let v = if v.len() >= 2
                && ((v.starts_with('"') && v.ends_with('"'))
                    || (v.starts_with('\'') && v.ends_with('\'')))
            {
                &v[1..v.len() - 1]
            } else {
                v
            };
            env.entry(key.trim().into()).or_insert_with(|| v.into());
        }
    }
}
pub fn load_dotenv(path: &Path, env: &mut BTreeMap<String, String>) -> bool {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            parse_dotenv(&text, env);
            true
        }
        Err(_) => false,
    }
}
pub fn safe_stem(filename: &str) -> String {
    let name = filename.rsplit(['/', '\\']).next().unwrap_or("script");
    let stem = name
        .rsplit_once('.')
        .filter(|(s, _)| s.chars().any(|c| c != '.'))
        .map(|(s, _)| s)
        .unwrap_or(name);
    let clean = Regex::new(r"[^A-Za-z0-9_.-]+")
        .unwrap()
        .replace_all(stem, "_")
        .trim_matches(['.', '_', '-'])
        .chars()
        .take(60)
        .collect::<String>();
    if clean.is_empty() {
        "script".into()
    } else {
        clean
    }
}
pub fn parse_command(content: &str, prefix: &str, command: &str) -> Option<String> {
    let text = content.trim_start();
    let head = format!("{prefix}{command}");
    let head_chars = head.chars().count();
    let start: String = text.chars().take(head_chars).collect();
    if start.to_lowercase() != head.to_lowercase() {
        return None;
    }
    let rest = text
        .char_indices()
        .nth(head_chars)
        .map_or("", |(index, _)| &text[index..]);
    if rest.chars().next().is_some_and(|c| !c.is_whitespace()) {
        return None;
    }
    Some(rest.trim().into())
}

#[derive(Clone, Debug)]
pub struct InputFile {
    pub filename: String,
    pub size: usize,
    pub url: String,
}
impl From<&Attachment> for InputFile {
    fn from(a: &Attachment) -> Self {
        Self {
            filename: a.filename.clone(),
            size: a.size as usize,
            url: a.url.clone(),
        }
    }
}
#[derive(Clone, Default, Debug)]
pub struct Request {
    pub file: Option<InputFile>,
    pub url: Option<String>,
    pub names_file: Option<InputFile>,
    pub annotate: bool,
    pub trace: bool,
    pub sandbox: bool,
    pub moonveil: bool,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Delivery {
    pub text: String,
    pub files: Vec<(String, Vec<u8>)>,
    pub ok: bool,
}
#[derive(Clone, Copy)]
pub struct Identity {
    pub user: u64,
    pub guild: Option<u64>,
    pub upload_limit: usize,
}

#[async_trait]
pub trait Responder: Send {
    fn identity(&self) -> Identity;
    async fn accept(&mut self) -> Result<()> {
        Ok(())
    }
    async fn status(&mut self, text: &str) -> Result<()>;
    async fn show(&mut self, card: CardData) -> Result<()>;
    async fn finish(&mut self, delivery: Delivery, card: Option<CardData>) -> Result<()>;
}
#[async_trait]
pub trait Backend: Send + Sync {
    async fn submit(
        &self,
        user: u64,
        data: Vec<u8>,
        opts: Options,
        progress: Option<Progress>,
    ) -> Result<Output>;
}
#[async_trait]
impl Backend for Service {
    async fn submit(
        &self,
        user: u64,
        data: Vec<u8>,
        opts: Options,
        progress: Option<Progress>,
    ) -> Result<Output> {
        Service::submit(self, user, data, opts, progress).await
    }
}
#[async_trait]
pub trait Network: Send + Sync {
    async fn download(
        &self,
        url: &str,
        max: usize,
        hosts: &[String],
        timeout: Duration,
    ) -> Result<Vec<u8>>;
}
pub struct SafeNetwork;
#[async_trait]
impl Network for SafeNetwork {
    async fn download(
        &self,
        url: &str,
        max: usize,
        hosts: &[String],
        timeout: Duration,
    ) -> Result<Vec<u8>> {
        fetch::fetch(url, max, hosts, timeout).await
    }
}

#[derive(Default)]
struct Admission {
    inflight: HashSet<u64>,
    last: HashMap<u64, Instant>,
}
struct JobGuard<'a> {
    state: &'a Mutex<Admission>,
    user: u64,
    cooldown: Duration,
    started: bool,
}
impl Drop for JobGuard<'_> {
    fn drop(&mut self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.inflight.remove(&self.user);
        if self.started {
            state.last.retain(|_, t| t.elapsed() < self.cooldown);
            state.last.insert(self.user, Instant::now());
        }
    }
}
pub struct Handler {
    pub cfg: Config,
    backend: Arc<dyn Backend>,
    network: Arc<dyn Network>,
    admission: Mutex<Admission>,
    fetching: tokio::sync::Semaphore,
}
async fn catch_job<T>(future: impl Future<Output = Result<T>>) -> Result<T> {
    let mut future = Box::pin(future);
    std::future::poll_fn(move |cx| {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| future.as_mut().poll(cx)))
            .unwrap_or_else(|_| {
                std::task::Poll::Ready(Err(Error::new("internal", "job execution panicked")))
            })
    })
    .await
}

impl Handler {
    pub fn new(cfg: Config, backend: Arc<dyn Backend>, network: Arc<dyn Network>) -> Self {
        Self {
            cfg,
            backend,
            network,
            admission: Mutex::new(Admission::default()),
            fetching: tokio::sync::Semaphore::new(4),
        }
    }
    pub fn allowed(&self, id: Identity) -> bool {
        let c = &self.cfg;
        (c.allowed_guilds.is_empty() && c.allowed_users.is_empty())
            || c.allowed_users.contains(&id.user)
            || id.guild.is_some_and(|g| c.allowed_guilds.contains(&g))
    }
    pub fn check_attachment(&self, a: &InputFile, names: bool) -> Result<()> {
        let exts: &[&str] = if names {
            &[".json", ".txt"]
        } else {
            &[".lua", ".luau", ".txt"]
        };
        let limit = if names {
            NAMES_MAX_BYTES
        } else {
            self.cfg.max_bytes()
        };
        if !exts
            .iter()
            .any(|ext| a.filename.to_ascii_lowercase().ends_with(ext))
        {
            return Err(Error::new(
                "input",
                format!("Send a {} file.", exts.join(" / ")),
            ));
        }
        if a.size == 0 {
            return Err(Error::new("input", "The file is empty."));
        }
        if a.size > limit {
            return Err(Error::new(
                "input",
                format!(
                    "The file is too large ({:.1} MB, max {}).",
                    a.size as f64 / 1048576.0,
                    if names {
                        "256 KB".into()
                    } else {
                        format!("{} MB", self.cfg.max_mb)
                    }
                ),
            ));
        }
        Ok(())
    }
    pub fn validate(&self, request: &mut Request) -> Result<()> {
        let err = |s: &str| Error::new("input", s);
        if request.file.is_none() && request.url.as_ref().is_none_or(|s| s.is_empty()) {
            return Err(err("Include a .lua / .luau / .txt file or a raw URL."));
        }
        if request.file.is_some() && request.url.is_some() {
            return Err(err("Pick one: a file or a URL, not both."));
        }
        if request.trace && request.sandbox {
            return Err(err("Pick one: `trace` (deobfuscate, then run the result in the fake Roblox) or `sandbox` (run the file as it is, without deobfuscating)."));
        }
        if request.sandbox && (request.annotate || request.names_file.is_some()) {
            return Err(err(
                "`sandbox` does not deobfuscate, so `annotate` and `names` do not apply.",
            ));
        }
        if request.moonveil && request.sandbox {
            return Err(err(&format!("`{}moonveil` makes the behaviour reconstruction itself. To run a file as it is in the fake Roblox use `{}uwu sandbox`.",self.cfg.prefix,self.cfg.prefix)));
        }
        if request.moonveil && (request.annotate || request.names_file.is_some()) {
            return Err(err("A MoonVeil script has no variable names to recover, so `annotate` and `names` do not apply."));
        }
        if let Some(file) = &request.file {
            self.check_attachment(file, false)?;
        } else if let Some(url) = &mut request.url {
            *url = fetch::normalize_url(&fetch::extract_url(url).unwrap_or_else(|| url.clone()));
            fetch::check_url(url, &self.cfg.url_hosts).map_err(|e| {
                err(&messages::explain_fetch(
                    &e.code,
                    &e.message,
                    self.cfg.max_mb,
                ))
            })?;
        }
        if let Some(file) = &request.names_file {
            self.check_attachment(file, true)?;
        }
        Ok(())
    }
    fn reserve(&self, id: Identity) -> Result<JobGuard<'_>> {
        let mut state = self.admission.lock().unwrap_or_else(|e| e.into_inner());
        if !self.allowed(id) {
            return Err(Error::new(
                "input",
                "This bot isn't open to this server/account.",
            ));
        }
        if state.inflight.contains(&id.user) {
            return Err(Error::new(
                "input",
                "You already have a file being processed. Wait until it finishes.",
            ));
        }
        if let Some(left) = state
            .last
            .get(&id.user)
            .and_then(|t| self.cfg.cooldown.checked_sub(t.elapsed()))
            .filter(|d| !d.is_zero())
        {
            return Err(Error::new(
                "input",
                format!(
                    "Please wait {} s before sending the next file.",
                    left.as_secs() + 1
                ),
            ));
        }
        state.inflight.insert(id.user);
        Ok(JobGuard {
            state: &self.admission,
            user: id.user,
            cooldown: self.cfg.cooldown,
            started: false,
        })
    }
    async fn read_source(
        &self,
        q: &Request,
    ) -> Result<(Vec<u8>, String, Option<BTreeMap<String, String>>)> {
        let (data, stem) = if let Some(file) = &q.file {
            let bytes = self
                .network
                .download(&file.url, self.cfg.max_bytes(), &[], self.cfg.url_timeout)
                .await
                .map_err(|_| {
                    Error::new(
                        "attachment",
                        "Failed to download the attachment from Discord. Try sending it again.",
                    )
                })?;
            (bytes, safe_stem(&file.filename))
        } else {
            let _permit = self
                .fetching
                .acquire()
                .await
                .map_err(|_| Error::new("internal", "download service closed"))?;
            let url = q.url.as_deref().unwrap();
            let data = self
                .network
                .download(
                    url,
                    self.cfg.max_bytes(),
                    &self.cfg.url_hosts,
                    self.cfg.url_timeout,
                )
                .await
                .map_err(|e| {
                    Error::new(
                        "input",
                        messages::explain_fetch(&e.code, &e.message, self.cfg.max_mb),
                    )
                })?;
            (data, safe_stem(&fetch::url_stem(url)))
        };
        if data.len() > self.cfg.max_bytes() {
            return Err(Error::new(
                "input",
                format!("The file is too large (max {} MB).", self.cfg.max_mb),
            ));
        }
        let names = if let Some(file) = &q.names_file {
            let bytes = self
                .network
                .download(&file.url, NAMES_MAX_BYTES, &[], self.cfg.url_timeout)
                .await
                .map_err(|_| {
                    Error::new(
                        "attachment",
                        "Failed to download the attachment from Discord. Try sending it again.",
                    )
                })?;
            if bytes.len() > NAMES_MAX_BYTES {
                return Err(Error::new(
                    "input",
                    "The names file is too large (max 256 KB).",
                ));
            }
            Some(parse_names(&bytes)?)
        } else {
            None
        };
        Ok((data, stem, names))
    }
    async fn drive<T>(
        &self,
        future: impl Future<Output = Result<T>>,
        r: &mut dyn Responder,
        card: &mut Option<Card>,
        mut events: Option<&mut tokio::sync::mpsc::UnboundedReceiver<String>>,
    ) -> Result<T> {
        tokio::pin!(future);
        let mut tick = tokio::time::interval(Duration::from_millis(1200));
        tick.tick().await;
        let mut last = Instant::now() - Duration::from_secs(2);
        loop {
            tokio::select! {
                result=&mut future=>return result,
                event=async {match events.as_mut(){Some(rx)=>rx.recv().await,None=>std::future::pending().await}}=>{
                    if let Some(event)=event {
                        let queued=event.strip_prefix("queued:").and_then(|s|s.parse::<usize>().ok());
                        if let Some(c)=card.as_mut(){if let Some(n)=queued{c.queued(n);}else{c.stage(&event);}}
                        else if queued.is_some() || last.elapsed()>=Duration::from_secs(1){last=Instant::now();let text=if let Some(n)=queued{format!("⏳ Queued (position {n}).")}else{format!("⚙️ Processing: {}.",stage_label(&event))};let _=r.status(&text).await;}
                    }else{events=None;}
                },
                _=tick.tick(),if card.is_some()=>{let c=card.as_mut().unwrap();c.tick();let _=r.show(c.data()).await;}
            }
        }
    }
    pub async fn run(&self, r: &mut dyn Responder, mut q: Request) -> Result<()> {
        let id = r.identity();
        let mut guard = match self.reserve(id) {
            Ok(g) => g,
            Err(e) => {
                return r
                    .finish(
                        Delivery {
                            text: e.message,
                            ..Default::default()
                        },
                        None,
                    )
                    .await
            }
        };
        // Admission errors take priority; malformed input never starts cooldown.
        if let Err(e) = self.validate(&mut q) {
            drop(guard);
            return r
                .finish(
                    Delivery {
                        text: e.message,
                        ..Default::default()
                    },
                    None,
                )
                .await;
        }
        guard.started = true;
        r.accept().await?;
        let started = Instant::now();
        let mut card = if self.cfg.progress_card {
            Some(Card::new(q.file.is_none(), self.cfg.spinner_emoji.clone()))
        } else {
            None
        };
        if let Some(c) = card.as_mut() {
            c.begin(if q.file.is_some() {
                "detect"
            } else {
                "download"
            });
            let _ = r.show(c.data()).await;
        } else if q.file.is_none() {
            let _ = r.status("⬇️ Downloading from the URL.").await;
        }
        let result=catch_job(async {
            let (data,stem,names)=self.drive(self.read_source(&q),r,&mut card,None).await?;
            if let Some(c)=card.as_mut(){c.begin("detect");}
            let opts=Options{max_bytes:self.cfg.max_bytes(),names,annotate:q.annotate,trace:q.trace,sandbox_only:q.sandbox,moonveil:q.moonveil,..Options::default()};
            let (tx,mut rx)=tokio::sync::mpsc::unbounded_channel();let progress:Progress=Arc::new(move|event|{let _=tx.send(event.to_owned());});
            let work=tokio::time::timeout(self.cfg.max_wait,self.backend.submit(id.user,data,opts,Some(progress)));
            let result=self.drive(async{work.await.map_err(|_|Error::new("wait_timeout","The queue took too long and the job was cancelled. Try again later."))?},r,&mut card,Some(&mut rx)).await?;
            if let Some(c)=card.as_mut(){c.begin("deliver");}
            package(&stem,&result,started.elapsed().as_secs_f64(),id.upload_limit)
        }).await;
        let outcome = result
            .as_ref()
            .map(|d| if d.ok { "ok" } else { "upload" })
            .unwrap_or_else(|e| e.code.as_str())
            .to_owned();
        let delivery = match result {
            Ok(d) => d,
            Err(e) => Delivery {
                text: match e.code.as_str() {
                    "input" | "attachment" | "bad_names" | "wait_timeout" => e.message,
                    "user_busy" => {
                        "You already have a file being processed. Wait until it finishes.".into()
                    }
                    "queue_full" => "The queue is full. Try again in a few minutes.".into(),
                    "cooldown" => "Please wait before sending the next file.".into(),
                    "too_big" => format!("The file is too large (max {} MB).", self.cfg.max_mb),
                    _ => {
                        let text = messages::explain(&e.code, &e.message);
                        if e.code == "unsupported" && q.url.is_some() && !q.sandbox && !q.moonveil {
                            format!("Downloaded the file, but couldn't deobfuscate it. Check the raw link or attach the original `.lua` file.\n{text}")
                        } else {
                            text
                        }
                    }
                },
                ..Default::default()
            },
        };
        if let Some(c) = card.as_mut() {
            if delivery.ok {
                c.complete();
            } else {
                c.fail();
            }
        }
        eprintln!(
            "job user={} guild={:?} src={} size={} -> {} {:.1}s",
            id.user,
            id.guild,
            if q.file.is_some() { "file" } else { "url" },
            q.file.as_ref().map(|f| f.size).unwrap_or(0),
            outcome,
            started.elapsed().as_secs_f64()
        );
        r.finish(delivery, card.as_ref().map(Card::data)).await
    }
    pub fn chat_help(&self, moonveil: bool) -> String {
        let p = &self.cfg.prefix;
        if moonveil {
            format!("Usage: `{p}moonveil` + a .lua attachment, or `{p}moonveil <raw url>`, or reply to a message that has a file/URL with `{p}moonveil`. MoonVeil cannot be turned back into source, so you get a `_reconstructed.lua`: a labelled record of what the script did when it was run in a fake Roblox (its calls, strings and their order). Add `trace` to also get the fake Roblox report (hosts, remotes, captured texts). Use `{p}uwu` for Bacon Guard / FORGE scripts.")
        } else {
            format!("Usage: `{p}uwu` + a .lua attachment, or `{p}uwu <raw url>`. You can also reply to a message that has a file/URL with `{p}uwu`. Add `trace` to also run the result in a fake Roblox, or `sandbox` to run the file as it is without deobfuscating. For a MoonVeil script use `{p}moonveil` instead. Example: `{p}uwu https://raw.githubusercontent.com/user/repo/main/script.lua")
        }
    }
    pub fn info_text(&self) -> String {
        let p = &self.cfg.prefix;
        let off = if self.cfg.prefix_commands {
            ""
        } else {
            " (disabled on this bot)"
        };
        format!("**Bacon Guard / FORGE deobfuscator**\n• `/deobf file:<attachment>` or `/deobf url:<raw url>`\n• `{p}uwu` + attachment, `{p}uwu <raw url>`, or reply to a message that has a file/URL with `{p}uwu`{off}\n• Right-click a message with an attachment → Apps → **Deobfuscate file**\n• Formats: .lua / .luau / .txt, max {} MB. GitHub `blob`, Pastebin and Gist links are converted to raw automatically\n• Takes about 5-60 seconds; the result is re-checked (syntax + bytecode)\n• Builds that are not recognized are rejected with a clear reason, never guessed\n• MoonVeil scripts cannot be turned back into source: you get a `_reconstructed.lua` that lists what the script does, clearly labelled. `{p}moonveil` + attachment (or raw URL/reply) asks for that even without a MoonVeil banner; add `trace` for the fake Roblox report\n• `annotate` writes a key next to every variable; send a JSON file `{{\"12:34\": \"name\"}}` via `names`\n• `trace` also runs the result in a fake Roblox (no network, no files) and attaches requests, remotes, windows, loadstring payloads and kicks. `sandbox` runs the file as given. Chat: `{p}uwu trace` / `{p}uwu sandbox`\n• Slash results are visible only to you by default (`private`). `{p}uwu` replies in the channel\n-# Use only on scripts you own or are allowed to analyze.",self.cfg.max_mb)
    }
}
fn stage_label(stage: &str) -> &str {
    match stage {
        "analyse" => "reading the loader",
        "unpack" => "opening the VM in the sandbox",
        "classify" => "recognizing this build's handlers",
        "decompile" => "rebuilding the code",
        "verify" => "verifying the result",
        "clean" => "cleaning up the result",
        "sandbox" => "running it in the fake Roblox",
        _ => "processing the file",
    }
}
fn truncate(s: &str, count: usize) -> String {
    s.chars().take(count).collect()
}

pub fn archive(files: &[(String, Vec<u8>)]) -> Result<Vec<u8>> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .compression_level(Some(9));
    for (name, data) in files {
        zip.start_file(name, options)
            .map_err(|_| Error::new("internal", "could not create result archive"))?;
        zip.write_all(data)?;
    }
    Ok(zip
        .finish()
        .map_err(|_| Error::new("internal", "could not finish result archive"))?
        .into_inner())
}
pub fn package(stem: &str, result: &Output, seconds: f64, upload_limit: usize) -> Result<Delivery> {
    let stem = safe_stem(&format!("{stem}.lua"));
    let mut limit = upload_limit.saturating_sub(64 * 1024);
    let mut files = Vec::new();
    let mut notes = Vec::new();
    if let Some(text) = &result.text {
        let mut body = text.as_bytes().to_vec();
        let mut name = format!(
            "{stem}{}",
            if result.info["trace_only"] == true {
                "_reconstructed.lua"
            } else if result.info["annotated"] == true {
                "_annotated.lua"
            } else {
                ".deobf.lua"
            }
        );
        if body.len() > limit {
            body = archive(&[(name.clone(), body)])?;
            name.push_str(".zip");
            notes.push(
                "The result was zipped because it exceeds the server upload limit.".to_owned(),
            );
        }
        if body.len() > limit {
            return Ok(Delivery{text:format!("The result is too large to upload on this server ({} lines). Run it through the CLI on your computer.",result.info["lines"].as_u64().unwrap_or(0)),..Default::default()});
        }
        limit -= body.len();
        files.push((name, body));
    }
    let status = messages::verification_status(&result.info);
    if result.text.is_some() && status.is_some() {
        let report = messages::verification_report(&result.info).into_bytes();
        if report.len() <= limit {
            limit -= report.len();
            files.push((format!("{stem}_verification.txt"), report));
        } else {
            notes.push("The verification report did not fit within the server upload limit; the result is still attached.".into());
        }
    }
    if let Some(trace) = &result.trace {
        let report = trace["report"]
            .as_str()
            .unwrap_or("")
            .replace("@@NAME@@", &stem)
            .replace("@@STEM@@", &stem);
        let mut body = report.into_bytes();
        let mut name = format!("{stem}_trace.txt");
        if body.len() > limit {
            body = archive(&[(name.clone(), body)])?;
            name = format!("{stem}_trace.zip");
        }
        if body.len() <= limit {
            limit -= body.len();
            files.push((name, body));
        } else {
            notes.push("The trace report is too large to upload on this server. Use the CLI on your computer.".into());
        }
        let mut payloads = Vec::new();
        if let Some(items) = trace["payloads"].as_array() {
            for p in items {
                let ext = if p["ext"] == "lua" { "lua" } else { "txt" };
                let name = format!("{stem}_payload{}.{ext}", p["id"].as_u64().unwrap_or(0));
                let data = if let Some(a) = p["data"].as_array() {
                    a.iter()
                        .map(|n| {
                            n.as_u64()
                                .filter(|n| *n <= 255)
                                .map(|n| n as u8)
                                .ok_or_else(|| Error::new("internal", "invalid trace payload byte"))
                        })
                        .collect::<Result<Vec<_>>>()?
                } else if let Some(s) = p["data"].as_str() {
                    s.as_bytes().to_vec()
                } else {
                    return Err(Error::new("internal", "invalid trace payload data"));
                };
                payloads.push((name, data));
            }
        }
        if !payloads.is_empty() {
            let body = archive(&payloads)?;
            if body.len() <= limit {
                files.push((format!("{stem}_payloads.zip"), body));
            } else {
                notes.push("The captured payloads are too large to upload on this server. Use the CLI on your computer.".into());
            }
        }
    }
    let summary = messages::verdict_markdown(&result.info, true);
    let mut header = format!("✅ Done ({seconds:.0} s)");
    if let Some(status) = status.as_ref() {
        header.push_str(&format!("\nVerification: {status}"));
    }
    for note in notes {
        header.push('\n');
        header.push_str(&note);
    }
    let footer = format!("\n-# {NOTE}");
    let verification_line = status.map(|s| format!("Verification: {s}"));
    let details = summary
        .into_iter()
        .filter(|s| Some(s) != verification_line.as_ref())
        .collect::<Vec<_>>()
        .join("\n");
    let budget = 1900usize.saturating_sub(header.chars().count() + footer.chars().count() + 1);
    Ok(Delivery {
        text: format!("{header}\n{}{footer}", truncate(&details, budget)),
        files,
        ok: true,
    })
}

pub fn select_message_source(
    attachments: &[InputFile],
    content: &str,
    allow_other: bool,
) -> (Option<InputFile>, Option<String>) {
    let file = attachments
        .iter()
        .find(|a| {
            [".lua", ".luau", ".txt"]
                .iter()
                .any(|ext| a.filename.to_ascii_lowercase().ends_with(ext))
        })
        .cloned()
        .or_else(|| {
            if allow_other {
                attachments.first().cloned()
            } else {
                None
            }
        });
    let url = if file.is_none() {
        fetch::extract_url(content)
    } else {
        None
    };
    (file, url)
}
pub fn chat_request(
    args: &str,
    attachments: &[InputFile],
    reference: Option<(&[InputFile], &str)>,
    moonveil: bool,
) -> Request {
    let words: Vec<_> = args.split_whitespace().collect();
    let trace = words.iter().any(|s| s.eq_ignore_ascii_case("trace"));
    let sandbox = words.iter().any(|s| s.eq_ignore_ascii_case("sandbox"));
    let rest = words
        .into_iter()
        .filter(|s| !s.eq_ignore_ascii_case("trace") && !s.eq_ignore_ascii_case("sandbox"))
        .collect::<Vec<_>>()
        .join(" ");
    let (mut file, mut url) = select_message_source(attachments, &rest, true);
    if file.is_none() && url.is_none() {
        if let Some((a, c)) = reference {
            (file, url) = select_message_source(a, c, false);
        }
    }
    Request {
        file,
        url,
        trace,
        sandbox,
        moonveil,
        ..Default::default()
    }
}

pub fn interaction_request(command: &CommandInteraction) -> Result<Request> {
    let mut request = Request::default();
    if command.data.name == "Deobfuscate file" {
        let target = command.data.target_id.and_then(|id| {
            command
                .data
                .resolved
                .messages
                .get(&MessageId::new(id.get()))
        });
        if let Some(message) = target {
            let atts: Vec<_> = message.attachments.iter().map(InputFile::from).collect();
            (request.file, request.url) = select_message_source(&atts, &message.content, false);
        }
        if request.file.is_none() && request.url.is_none() {
            return Err(Error::new(
                "input",
                "That message has no .lua / .luau / .txt attachment or URL.",
            ));
        }
    } else if command.data.name == "deobf" {
        for option in &command.data.options {
            match (option.name.as_str(), &option.value) {
                ("file", CommandDataOptionValue::Attachment(id)) => {
                    request.file = command
                        .data
                        .resolved
                        .attachments
                        .get(id)
                        .map(InputFile::from)
                }
                ("names", CommandDataOptionValue::Attachment(id)) => {
                    request.names_file = command
                        .data
                        .resolved
                        .attachments
                        .get(id)
                        .map(InputFile::from)
                }
                ("url", CommandDataOptionValue::String(value)) => request.url = Some(value.clone()),
                ("annotate", CommandDataOptionValue::Boolean(v)) => request.annotate = *v,
                ("trace", CommandDataOptionValue::Boolean(v)) => request.trace = *v,
                ("sandbox", CommandDataOptionValue::Boolean(v)) => request.sandbox = *v,
                _ => {}
            }
        }
    } else {
        return Err(Error::new("input", "Unknown command."));
    }
    Ok(request)
}

pub fn commands() -> Vec<CreateCommand> {
    let mut deobf = CreateCommand::new("deobf")
        .description("Deobfuscate Roblox scripts (Bacon Guard / FORGE; MoonVeil: behaviour trace)");
    for (kind, name, description) in [
        (
            CommandOptionType::Attachment,
            "file",
            "Protected .lua / .luau / .txt file",
        ),
        (
            CommandOptionType::String,
            "url",
            "Or the raw URL of the script (https://...)",
        ),
        (
            CommandOptionType::Boolean,
            "private",
            "Only you can see the result (default: yes)",
        ),
        (
            CommandOptionType::Boolean,
            "annotate",
            "Write a key next to every variable (to build a names file)",
        ),
        (
            CommandOptionType::Attachment,
            "names",
            "JSON file {key: name} used to name variables",
        ),
        (
            CommandOptionType::Boolean,
            "trace",
            "Also run the result in a fake Roblox and attach a report of what it does",
        ),
        (
            CommandOptionType::Boolean,
            "sandbox",
            "Do not deobfuscate: run the file as it is in the fake Roblox",
        ),
    ] {
        deobf = deobf.add_option(CreateCommandOption::new(kind, name, description).required(false));
    }
    vec![
        deobf,
        CreateCommand::new("deobf-info").description("How to use it, limits, and usage notes"),
        CreateCommand::new("Deobfuscate file").kind(CommandType::Message),
    ]
}
fn mentions() -> CreateAllowedMentions {
    CreateAllowedMentions::new().replied_user(false)
}
fn discord_error(e: serenity::Error) -> Error {
    if forbidden(&e) {
        Error::new("discord_forbidden", "Discord refused permission to reply")
    } else {
        Error::new("discord", "Discord could not deliver the reply")
    }
}
fn forbidden(e: &serenity::Error) -> bool {
    matches!(e,serenity::Error::Http(HttpError::UnsuccessfulRequest(r)) if r.status_code.as_u16()==403)
}
fn attachments(files: &[(String, Vec<u8>)]) -> Vec<CreateAttachment> {
    files
        .iter()
        .map(|(n, b)| CreateAttachment::bytes(b.clone(), n))
        .collect()
}

pub struct InteractionResponder {
    http: Arc<Http>,
    interaction: CommandInteraction,
    private: bool,
    deferred: bool,
}
impl InteractionResponder {
    pub fn new(http: Arc<Http>, interaction: CommandInteraction, private: bool) -> Self {
        Self {
            http,
            interaction,
            private,
            deferred: false,
        }
    }
}
#[async_trait]
impl Responder for InteractionResponder {
    fn identity(&self) -> Identity {
        Identity {
            user: self.interaction.user.id.get(),
            guild: self.interaction.guild_id.map(|g| g.get()),
            upload_limit: if self.interaction.attachment_size_limit > 0 {
                self.interaction.attachment_size_limit as usize
            } else {
                DM_UPLOAD_LIMIT
            },
        }
    }
    async fn accept(&mut self) -> Result<()> {
        self.interaction
            .create_response(
                &self.http,
                CreateInteractionResponse::Defer(
                    CreateInteractionResponseMessage::new()
                        .ephemeral(self.private)
                        .allowed_mentions(mentions()),
                ),
            )
            .await
            .map_err(discord_error)?;
        self.deferred = true;
        Ok(())
    }
    async fn status(&mut self, text: &str) -> Result<()> {
        self.interaction
            .edit_response(
                &self.http,
                EditInteractionResponse::new()
                    .content(truncate(text, 1900))
                    .allowed_mentions(mentions()),
            )
            .await
            .map(|_| ())
            .map_err(discord_error)
    }
    async fn show(&mut self, card: CardData) -> Result<()> {
        self.interaction
            .edit_response(
                &self.http,
                EditInteractionResponse::new()
                    .content("")
                    .embeds(vec![card.embed()])
                    .allowed_mentions(mentions()),
            )
            .await
            .map(|_| ())
            .map_err(discord_error)
    }
    async fn finish(&mut self, d: Delivery, card: Option<CardData>) -> Result<()> {
        if self.deferred {
            let mut edit = EditInteractionResponse::new()
                .content(truncate(&d.text, 1900))
                .embeds(card.map(|c| vec![c.embed()]).unwrap_or_default())
                .allowed_mentions(mentions())
                .attachments(EditAttachments::new());
            for f in attachments(&d.files) {
                edit = edit.new_attachment(f);
            }
            self.interaction
                .edit_response(&self.http, edit)
                .await
                .map(|_| ())
                .map_err(discord_error)
        } else {
            self.interaction
                .create_response(
                    &self.http,
                    CreateInteractionResponse::Message(
                        CreateInteractionResponseMessage::new()
                            .content(truncate(&d.text, 1900))
                            .ephemeral(true)
                            .allowed_mentions(mentions())
                            .add_files(attachments(&d.files)),
                    ),
                )
                .await
                .map_err(discord_error)
        }
    }
}

pub struct MessageResponder {
    http: Arc<Http>,
    message: Message,
    status_message: Option<Message>,
    upload_limit: usize,
}
impl MessageResponder {
    pub fn new(http: Arc<Http>, message: Message, upload_limit: usize) -> Self {
        Self {
            http,
            message,
            status_message: None,
            upload_limit,
        }
    }
    async fn send(&self, d: &Delivery, card: Option<&CardData>) -> Result<Message> {
        let build = || {
            let mut m = CreateMessage::new()
                .content(truncate(&d.text, 1900))
                .allowed_mentions(mentions())
                .files(attachments(&d.files));
            if let Some(c) = card {
                m = m.embed(c.embed());
            }
            m
        };
        match self
            .message
            .channel_id
            .send_message(&self.http, build().reference_message(&self.message))
            .await
        {
            Ok(m) => Ok(m),
            Err(e) if forbidden(&e) => Err(discord_error(e)),
            Err(_) => self
                .message
                .channel_id
                .send_message(&self.http, build())
                .await
                .map_err(discord_error),
        }
    }
    async fn update(&mut self, d: &Delivery, card: Option<&CardData>) -> Result<()> {
        if let Some(message) = &mut self.status_message {
            let mut edit = EditMessage::new()
                .content(truncate(&d.text, 1900))
                .embeds(card.map(|c| vec![c.embed()]).unwrap_or_default())
                .allowed_mentions(mentions())
                .attachments(EditAttachments::new());
            for f in attachments(&d.files) {
                edit = edit.new_attachment(f);
            }
            if message.edit(&self.http, edit).await.is_ok() {
                return Ok(());
            }
        }
        match self.send(d, card).await {
            Ok(m) => {
                self.status_message = Some(m);
                Ok(())
            }
            Err(e) if e.code == "discord_forbidden" && !d.files.is_empty() => {
                let fallback=Delivery{text:"I don't have the **Attach Files** permission in this channel, so I can't send the result.".into(),..Default::default()};
                self.send(&fallback, None).await.map(|_| ())
            }
            Err(e) => Err(e),
        }
    }
}
#[async_trait]
impl Responder for MessageResponder {
    fn identity(&self) -> Identity {
        Identity {
            user: self.message.author.id.get(),
            guild: self.message.guild_id.map(|g| g.get()),
            upload_limit: self.upload_limit,
        }
    }
    async fn status(&mut self, text: &str) -> Result<()> {
        self.update(
            &Delivery {
                text: text.into(),
                ..Default::default()
            },
            None,
        )
        .await
    }
    async fn show(&mut self, c: CardData) -> Result<()> {
        self.update(&Delivery::default(), Some(&c)).await
    }
    async fn finish(&mut self, d: Delivery, c: Option<CardData>) -> Result<()> {
        self.update(&d, c.as_ref()).await
    }
}

impl Handler {
    pub async fn from_chat(
        &self,
        http: Arc<Http>,
        message: Message,
        upload_limit: usize,
        can_reply: bool,
    ) -> Result<()> {
        if !self.cfg.prefix_commands || message.author.bot || !can_reply {
            return Ok(());
        }
        let (args, moonveil) = if let Some(args) =
            parse_command(&message.content, &self.cfg.prefix, "uwu")
        {
            (args, false)
        } else if let Some(args) = parse_command(&message.content, &self.cfg.prefix, "moonveil") {
            (args, true)
        } else {
            return Ok(());
        };
        let mut responder = MessageResponder::new(http.clone(), message.clone(), upload_limit);
        if matches!(args.to_ascii_lowercase().as_str(), "help" | "?" | "bantuan") {
            return responder
                .finish(
                    Delivery {
                        text: self.chat_help(moonveil),
                        ..Default::default()
                    },
                    None,
                )
                .await;
        }
        let atts: Vec<_> = message.attachments.iter().map(InputFile::from).collect();
        let mut request = chat_request(&args, &atts, None, moonveil);
        if request.file.is_none() && request.url.is_none() {
            let reference = if let Some(m) = message.referenced_message.as_deref() {
                Some(m.clone())
            } else if let Some(id) = message
                .message_reference
                .as_ref()
                .and_then(|r| r.message_id)
            {
                message.channel_id.message(&http, id).await.ok()
            } else {
                None
            };
            if let Some(reference) = reference {
                let atts: Vec<_> = reference.attachments.iter().map(InputFile::from).collect();
                request = chat_request(&args, &[], Some((&atts, &reference.content)), moonveil);
            }
        }
        if request.file.is_none() && request.url.is_none() {
            return responder
                .finish(
                    Delivery {
                        text: self.chat_help(moonveil),
                        ..Default::default()
                    },
                    None,
                )
                .await;
        }
        self.run(&mut responder, request).await
    }

    pub async fn from_interaction(
        &self,
        http: Arc<Http>,
        command: CommandInteraction,
    ) -> Result<()> {
        let private = command
            .data
            .options
            .iter()
            .find(|o| o.name == "private")
            .and_then(|o| o.value.as_bool())
            .unwrap_or(true);
        let mut r = InteractionResponder::new(http, command.clone(), private);
        if command.data.name == "deobf-info" {
            return r
                .finish(
                    Delivery {
                        text: self.info_text(),
                        ..Default::default()
                    },
                    None,
                )
                .await;
        }
        if !matches!(command.data.name.as_str(), "deobf" | "Deobfuscate file") {
            return Ok(());
        }
        let request = match interaction_request(&command) {
            Ok(q) => q,
            Err(e) => {
                return r
                    .finish(
                        Delivery {
                            text: e.message,
                            ..Default::default()
                        },
                        None,
                    )
                    .await
            }
        };
        match self.run(&mut r, request).await {
            Ok(()) => Ok(()),
            Err(_) => {
                r.finish(
                    Delivery {
                        text: messages::explain("internal", ""),
                        ..Default::default()
                    },
                    None,
                )
                .await
            }
        }
    }
}

struct DiscordHandler {
    handler: Handler,
    registered: std::sync::atomic::AtomicBool,
}
fn guild_upload_limit(ctx: &Context, id: Option<GuildId>) -> usize {
    match id.and_then(|id| ctx.cache.guild(id).map(|g| g.premium_tier)) {
        Some(PremiumTier::Tier2) => 50 * 1024 * 1024,
        Some(PremiumTier::Tier3) => 100 * 1024 * 1024,
        _ => DM_UPLOAD_LIMIT,
    }
}
fn can_reply(ctx: &Context, m: &Message) -> bool {
    let Some(guild) = m.guild_id.and_then(|id| ctx.cache.guild(id)) else {
        return true;
    };
    let id = ctx.cache.current_user().id;
    let Some(channel) = guild
        .channels
        .get(&m.channel_id)
        .or_else(|| guild.threads.iter().find(|c| c.id == m.channel_id))
    else {
        return true;
    };
    let Some(member) = guild.members.get(&id) else {
        return true;
    };
    let p = guild.user_permissions_in(channel, member);
    if matches!(
        channel.kind,
        ChannelType::PublicThread | ChannelType::PrivateThread | ChannelType::NewsThread
    ) {
        p.send_messages_in_threads()
    } else {
        p.send_messages()
    }
}
#[async_trait]
impl EventHandler for DiscordHandler {
    async fn ready(&self, ctx: Context, _ready: Ready) {
        use std::sync::atomic::Ordering;
        if self.registered.swap(true, Ordering::AcqRel) {
            return;
        }
        let result = if let Some(guild) = self.handler.cfg.guild_id {
            GuildId::new(guild)
                .set_commands(&ctx.http, commands())
                .await
        } else {
            Command::set_global_commands(&ctx.http, commands()).await
        };
        if result.is_err() {
            self.registered.store(false, Ordering::Release);
            eprintln!("Discord command registration failed");
        }
    }
    async fn message(&self, ctx: Context, message: Message) {
        let upload_limit = guild_upload_limit(&ctx, message.guild_id);
        let speak = can_reply(&ctx, &message);
        if let Err(e) = self
            .handler
            .from_chat(ctx.http, message, upload_limit, speak)
            .await
        {
            if e.code != "discord_forbidden" {
                eprintln!("Discord message job failed: {}", e.code);
            }
        }
    }
    async fn interaction_create(&self, ctx: Context, interaction: Interaction) {
        let Interaction::Command(command) = interaction else {
            return;
        };
        let _ = self.handler.from_interaction(ctx.http, command).await;
    }
}

pub async fn run() -> Result<()> {
    let mut env: BTreeMap<_, _> = std::env::vars().collect();
    load_dotenv(Path::new(".env"), &mut env);
    // The native tool/worker modules also consume non-secret settings loaded from this file.
    for (key, value) in &env {
        if std::env::var_os(key).is_none() {
            if key.is_empty() || key.contains(['=', '\0']) || value.contains('\0') {
                return Err(Error::new("config", "invalid environment key or value"));
            }
            std::env::set_var(key, value);
        }
    }
    start_with(&env, crate::astutil::check_tools, connect).await
}

/// Startup boundaries keep configuration/tool checks testable without a gateway login.
pub async fn start_with<F, Fut>(
    env: &BTreeMap<String, String>,
    tools: impl FnOnce() -> Vec<(String, bool, String)>,
    connect: F,
) -> Result<()>
where
    F: FnOnce(Config, GatewayIntents) -> Fut,
    Fut: Future<Output = Result<()>>,
{
    let cfg = Config::from_env(env)
        .map_err(|e| Error::new("config", format!("Invalid configuration: {}", e.message)))?;
    if cfg.token.is_empty() {
        return Err(Error::new(
            "config",
            "DISCORD_TOKEN is not set. Copy .env.example to .env and fill in the token.",
        ));
    }
    let mut bad: Vec<_> = tools()
        .into_iter()
        .filter(|(_, ok, _)| !*ok)
        .map(|(name, _, message)| {
            format!(
                "Problem with a Luau tool: {name}: {}",
                messages::scrub(&message)
            )
        })
        .collect();
    if !bad.is_empty() {
        bad.push("See the \"Luau binaries\" section of the README.".into());
        return Err(Error::new("setup", bad.join("\n")));
    }
    let mut intents = GatewayIntents::GUILDS;
    if cfg.prefix_commands {
        intents |= GatewayIntents::GUILD_MESSAGES
            | GatewayIntents::DIRECT_MESSAGES
            | GatewayIntents::MESSAGE_CONTENT;
    }
    connect(cfg, intents).await
}

pub fn startup_error(error: serenity::Error, prefix: &str) -> Error {
    let text = match error {
        serenity::Error::Gateway(GatewayError::InvalidAuthentication) =>
            "Discord rejected the token. Check DISCORD_TOKEN in .env.".into(),
        serenity::Error::Gateway(GatewayError::DisallowedGatewayIntents) =>
            format!("The {prefix}uwu command needs the \"Message Content Intent\". Enable it in the Discord Developer Portal -> Bot -> Privileged Gateway Intents -> MESSAGE CONTENT INTENT, then run again.\nOr set PREFIX_COMMANDS=0 in .env to use slash commands only."),
        serenity::Error::Http(HttpError::UnsuccessfulRequest(response)) if response.status_code.as_u16() == 401 =>
            "Discord rejected the token. Check DISCORD_TOKEN in .env.".into(),
        serenity::Error::Http(HttpError::UnsuccessfulRequest(response)) =>
            format!("Discord rejected the request (HTTP {}). Check your bot configuration.", response.status_code.as_u16()),
        _ => "Couldn't connect to Discord. Check your internet connection and bot configuration.".into(),
    };
    Error::new("discord", text)
}

async fn connect(cfg: Config, intents: GatewayIntents) -> Result<()> {
    let service = Arc::new(Service::new(
        cfg.workers,
        cfg.max_queue,
        Duration::ZERO,
        cfg.timeout,
    ));
    let token = cfg.token.clone();
    let prefix = cfg.prefix.clone();
    let handler = DiscordHandler {
        handler: Handler::new(cfg, service.clone(), Arc::new(SafeNetwork)),
        registered: std::sync::atomic::AtomicBool::new(false),
    };
    let mut client = Client::builder(token, intents)
        .event_handler(handler)
        .await
        .map_err(|e| startup_error(e, &prefix))?;
    let result = tokio::select! {
        result = client.start() => result.map_err(|e| startup_error(e, &prefix)),
        result = tokio::signal::ctrl_c() => result.map_err(Error::from),
    };
    client.shard_manager.shutdown_all().await;
    service.close().await;
    result
}

#[cfg(test)]
mod boundary_tests {
    use super::*;
    use base64::Engine;
    use serde_json::json;

    // Serenity exposes a shard messenger only after a WebSocket handshake.
    fn websocket_accept(key: &str) -> String {
        let mut bytes = format!("{key}258EAFA5-E914-47DA-95CA-C5AB0DC85B11").into_bytes();
        let bits = (bytes.len() as u64) * 8;
        bytes.push(0x80);
        while bytes.len() % 64 != 56 {
            bytes.push(0);
        }
        bytes.extend_from_slice(&bits.to_be_bytes());
        let mut hash = [
            0x67452301u32,
            0xefcdab89,
            0x98badcfe,
            0x10325476,
            0xc3d2e1f0,
        ];
        for block in bytes.as_chunks::<64>().0 {
            let mut words = [0u32; 80];
            for (word, bytes) in words.iter_mut().zip(block.as_chunks::<4>().0) {
                *word = u32::from_be_bytes(*bytes);
            }
            for i in 16..80 {
                words[i] =
                    (words[i - 3] ^ words[i - 8] ^ words[i - 14] ^ words[i - 16]).rotate_left(1);
            }
            let [mut a, mut b, mut c, mut d, mut e] = hash;
            for (i, word) in words.into_iter().enumerate() {
                let (f, k) = match i {
                    0..=19 => ((b & c) | (!b & d), 0x5a827999u32),
                    20..=39 => (b ^ c ^ d, 0x6ed9eba1),
                    40..=59 => ((b & c) | (b & d) | (c & d), 0x8f1bbcdc),
                    _ => (b ^ c ^ d, 0xca62c1d6),
                };
                let next = a
                    .rotate_left(5)
                    .wrapping_add(f)
                    .wrapping_add(e)
                    .wrapping_add(k)
                    .wrapping_add(word);
                (a, b, c, d, e) = (next, a, b.rotate_left(30), c, d);
            }
            for (state, word) in hash.iter_mut().zip([a, b, c, d, e]) {
                *state = state.wrapping_add(word);
            }
        }
        base64::engine::general_purpose::STANDARD.encode(
            hash.into_iter()
                .flat_map(u32::to_be_bytes)
                .collect::<Vec<_>>(),
        )
    }

    fn cached_context() -> Context {
        assert_eq!(
            websocket_accept("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
        // Dropping this private runtime also stops Serenity's unused shard queuer.
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let handshake = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let mut chunk = [0u8; 1024];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = stream.read(&mut chunk).await.unwrap();
                    assert!(n > 0 && request.len() + n <= 8192);
                    request.extend_from_slice(&chunk[..n]);
                }
                let request = std::str::from_utf8(&request).unwrap();
                let key = request.lines().find_map(|line| {
                    line.split_once(':').filter(|(name, _)| name.eq_ignore_ascii_case("sec-websocket-key")).map(|(_, value)| value.trim())
                }).unwrap();
                let response = format!("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n", websocket_accept(key));
                stream.write_all(response.as_bytes()).await.unwrap();
            });
            let cache = Arc::new(Cache::new());
            let http = Arc::new(Http::new("unit-test"));
            let data = Arc::new(tokio::sync::RwLock::new(Default::default()));
            let ws_url = Arc::new(tokio::sync::Mutex::new(format!("ws://{address}")));
            let shard = tokio::time::timeout(Duration::from_secs(5), Shard::new(
                ws_url.clone(), "unit-test", ShardInfo { id: ShardId(0), total: 1 }, GatewayIntents::empty(), None,
            )).await.unwrap().unwrap();
            handshake.await.unwrap();
            let (manager, _) = ShardManager::new(ShardManagerOptions {
                data: data.clone(),
                event_handlers: vec![],
                raw_event_handlers: vec![],
                shard_index: 0,
                shard_init: 0,
                shard_total: 1,
                ws_url,
                cache: cache.clone(),
                http: http.clone(),
                intents: GatewayIntents::empty(),
                presence: None,
            });
            let runner = ShardRunner::new(ShardRunnerOptions {
                data: data.clone(),
                event_handlers: vec![],
                raw_event_handlers: vec![],
                manager,
                shard,
                cache: cache.clone(),
                http: http.clone(),
            });
            Context { data, shard: ShardMessenger::new(&runner), shard_id: ShardId(0), http, cache }
        })
    }

    fn cache_guild(ctx: &Context, guild: &Guild) {
        let mut event: GuildCreateEvent =
            serde_json::from_value(serde_json::to_value(guild).unwrap()).unwrap();
        ctx.cache.update(&mut event);
    }

    #[test]
    fn cached_guild_permissions_choose_channel_or_thread_send_permission() {
        let ctx = cached_context();
        let bot_id = ctx.cache.current_user().id;
        let mut guild = Guild::default();
        guild.id = GuildId::new(900);
        guild.owner_id = UserId::new(999);
        let mut member = Member::default();
        member.user.id = bot_id;
        member.guild_id = guild.id;
        guild.members.insert(bot_id, member.clone());
        let mut role = Role::default();
        role.id = RoleId::new(900);
        role.guild_id = guild.id;
        let mut channel = GuildChannel::default();
        channel.id = ChannelId::new(777);
        channel.guild_id = guild.id;
        let mut message = Message::default();
        message.guild_id = Some(guild.id);
        message.channel_id = channel.id;

        for (kind, permissions, in_threads, expected) in [
            (
                ChannelType::Text,
                Permissions::VIEW_CHANNEL | Permissions::SEND_MESSAGES,
                false,
                true,
            ),
            (ChannelType::Text, Permissions::VIEW_CHANNEL, false, false),
            (
                ChannelType::Text,
                Permissions::VIEW_CHANNEL | Permissions::SEND_MESSAGES_IN_THREADS,
                false,
                false,
            ),
            (
                ChannelType::PublicThread,
                Permissions::VIEW_CHANNEL | Permissions::SEND_MESSAGES,
                true,
                false,
            ),
            (
                ChannelType::PublicThread,
                Permissions::VIEW_CHANNEL | Permissions::SEND_MESSAGES_IN_THREADS,
                true,
                true,
            ),
            (
                ChannelType::PrivateThread,
                Permissions::VIEW_CHANNEL | Permissions::SEND_MESSAGES_IN_THREADS,
                true,
                true,
            ),
            (
                ChannelType::NewsThread,
                Permissions::VIEW_CHANNEL | Permissions::SEND_MESSAGES_IN_THREADS,
                true,
                true,
            ),
        ] {
            channel.kind = kind;
            role.permissions = permissions;
            guild.roles.insert(role.id, role.clone());
            guild.channels.clear();
            guild.threads.clear();
            if in_threads {
                guild.threads.push(channel.clone());
            } else {
                guild.channels.insert(channel.id, channel.clone());
            }
            cache_guild(&ctx, &guild);
            assert_eq!(
                can_reply(&ctx, &message),
                expected,
                "{kind:?}: {permissions:?}"
            );
        }

        channel.kind = ChannelType::Text;
        channel.permission_overwrites = serde_json::from_value(json!([{"id":bot_id.to_string(),"type":1,"allow":"0","deny":Permissions::SEND_MESSAGES.bits().to_string()}])).unwrap();
        role.permissions = Permissions::VIEW_CHANNEL | Permissions::SEND_MESSAGES;
        guild.roles.insert(role.id, role);
        guild.threads.clear();
        guild.channels.insert(channel.id, channel);
        cache_guild(&ctx, &guild);
        assert!(
            !can_reply(&ctx, &message),
            "channel overwrite must override the role"
        );

        guild.members.clear();
        cache_guild(&ctx, &guild);
        assert!(
            can_reply(&ctx, &message),
            "missing cached member falls back to Discord"
        );
        guild.members.insert(bot_id, member);
        guild.channels.clear();
        cache_guild(&ctx, &guild);
        assert!(
            can_reply(&ctx, &message),
            "missing cached channel falls back to Discord"
        );
        message.guild_id = Some(GuildId::new(901));
        assert!(
            can_reply(&ctx, &message),
            "missing cached guild falls back to Discord"
        );
        message.guild_id = None;
        assert!(can_reply(&ctx, &message), "DMs need no guild permissions");
    }

    #[test]
    fn cached_guild_tiers_determine_upload_limits_with_dm_and_unknown_guild_fallbacks() {
        let ctx = cached_context();
        let mut guild = Guild::default();
        guild.id = GuildId::new(900);
        for (tier, expected) in [
            (PremiumTier::Tier0, 10 * 1024 * 1024),
            (PremiumTier::Tier1, 10 * 1024 * 1024),
            (PremiumTier::Tier2, 50 * 1024 * 1024),
            (PremiumTier::Tier3, 100 * 1024 * 1024),
        ] {
            guild.premium_tier = tier;
            cache_guild(&ctx, &guild);
            assert_eq!(
                guild_upload_limit(&ctx, Some(guild.id)),
                expected,
                "{tier:?}"
            );
        }
        assert_eq!(guild_upload_limit(&ctx, None), 10 * 1024 * 1024);
        assert_eq!(
            guild_upload_limit(&ctx, Some(GuildId::new(901))),
            10 * 1024 * 1024
        );
    }
}

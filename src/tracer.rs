//! Fake-Roblox runtime assembly, byte protocol and evidence-based trace reports.
use crate::error::{Error, Result};
use indexmap::IndexMap;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashSet},
    time::Duration,
};

const RUNTIME: [&[u8]; 11] = [
    include_bytes!("lua/00_prelude.lua"),
    include_bytes!("lua/10_log.lua"),
    include_bytes!("lua/20_sched.lua"),
    include_bytes!("lua/30_types.lua"),
    include_bytes!("lua/32_math.lua"),
    include_bytes!("lua/40_inst.lua"),
    include_bytes!("lua/42_world.lua"),
    include_bytes!("lua/44_services.lua"),
    include_bytes!("lua/50_exec.lua"),
    include_bytes!("lua/60_env.lua"),
    include_bytes!("lua/90_run.lua"),
];

pub fn lua_str(data: &[u8]) -> String {
    use std::fmt::Write;
    let mut out = String::from("\"");
    for &byte in data {
        match byte {
            b'"' => out.push_str("\\\""),
            b'\\' => out.push_str("\\\\"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b'\t' => out.push_str("\\t"),
            32..=126 => out.push(char::from(byte)),
            _ => {
                let _ = write!(out, "\\{byte:03}");
            }
        }
    }
    out.push('"');
    out
}

pub fn lit(value: &Value) -> Result<String> {
    Ok(match value {
        Value::Null => "nil".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => lua_str(s.as_bytes()),
        Value::Array(items) => format!(
            "{{{}}}",
            items
                .iter()
                .map(lit)
                .collect::<Result<Vec<_>>>()?
                .join(", ")
        ),
        Value::Object(items) => format!(
            "{{{}}}",
            items
                .iter()
                .map(|(k, v)| Ok(format!("[{}] = {}", lua_str(k.as_bytes()), lit(v)?)))
                .collect::<Result<Vec<_>>>()?
                .join(", ")
        ),
    })
}

pub fn default_options() -> Value {
    json!({"place_id":0,"game_id":null,"job_id":null,"user_id":null,"username":null,"executor":null,"executor_version":null,
        "game_name":null,"responses":null,"files":null,"seed":null,"max_time":30.0,"settle_time":10.0,"max_steps":200000,
        "max_events":8000,"explore":false,"other_players":0,"timeout":30,"mem_mb":512,"max_output_mb":64,"max_bytes":3*1024*1024})
}

fn options(opts: &Value) -> Result<Value> {
    let mut out = default_options();
    if let Some(opts) = opts.as_object() {
        for (key, value) in opts {
            if out.get(key).is_some() {
                out[key] = value.clone();
            }
        }
    }
    for key in ["max_time", "settle_time", "timeout"] {
        if !out[key]
            .as_f64()
            .is_some_and(|v| v.is_finite() && v >= 0.0 && (key != "timeout" || v > 0.0))
        {
            return Err(Error::new(
                "internal",
                format!("invalid sandbox option {key}"),
            ));
        }
    }
    for key in [
        "max_steps",
        "max_events",
        "other_players",
        "mem_mb",
        "max_output_mb",
        "max_bytes",
    ] {
        if out[key].as_u64().is_none() {
            return Err(Error::new(
                "internal",
                format!("invalid sandbox option {key}"),
            ));
        }
    }
    Ok(out)
}

pub fn build_script(source: &[u8], opts: &Value, live: bool) -> Result<Vec<u8>> {
    let opts = options(opts)?;
    let mut cfg = Map::new();
    cfg.insert("chunkname".into(), json!("script"));
    for key in [
        "place_id",
        "game_id",
        "job_id",
        "user_id",
        "username",
        "executor",
        "executor_version",
        "game_name",
        "responses",
        "files",
        "seed",
        "max_time",
        "settle_time",
        "max_steps",
        "max_events",
        "other_players",
    ] {
        if !opts[key].is_null() {
            cfg.insert(key.into(), opts[key].clone());
        }
    }
    if opts["explore"] == true {
        cfg.insert("explore".into(), json!(true));
    }
    if !live {
        cfg.insert("pad".into(), json!(4096));
    }
    let mut cfg_text = lit(&Value::Object(cfg))?;
    cfg_text.pop();
    cfg_text.push_str(&format!(", [\"source\"] = {}}}", lua_str(source)));
    let mut out = format!("local CONFIG = {cfg_text}\n").into_bytes();
    for part in RUNTIME {
        out.extend_from_slice(part);
        out.push(b'\n');
    }
    Ok(out)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Event {
    pub time: f64,
    pub line: Option<String>,
    pub kind: String,
    pub text: String,
    pub a: Option<String>,
    pub b: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Payload {
    pub id: u64,
    pub kind: String,
    pub label: String,
    pub length: usize,
    pub data: Vec<u8>,
}
impl Payload {
    pub fn truncated(&self) -> bool {
        self.data.len() < self.length
    }
    pub fn sha256(&self) -> String {
        format!("{:x}", Sha256::digest(&self.data))
    }
    pub fn ext(&self) -> &'static str {
        if matches!(self.kind.as_str(), "loadstring" | "queue_on_teleport") {
            "lua"
        } else {
            "txt"
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Repeat {
    pub times: u64,
    pub kind: String,
    pub text: String,
    pub first: f64,
    pub last: f64,
}
pub type ParsedLog = (
    Vec<Event>,
    Vec<Payload>,
    Vec<Repeat>,
    Option<BTreeMap<String, String>>,
);

fn unesc_bytes(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < data.len() {
        if data[i] == b'\\' {
            if data.get(i + 1) == Some(&b'x')
                && i + 3 < data.len()
                && data[i + 2..i + 4]
                    .iter()
                    .all(|c| c.is_ascii_digit() || (b'A'..=b'F').contains(c))
            {
                if let Ok(n) =
                    u8::from_str_radix(std::str::from_utf8(&data[i + 2..i + 4]).unwrap_or(""), 16)
                {
                    out.push(n);
                    i += 4;
                    continue;
                }
            }
            if let Some(c) = data.get(i + 1) {
                let decoded = match c {
                    b'\\' => Some(b'\\'),
                    b'n' => Some(b'\n'),
                    b'r' => Some(b'\r'),
                    b't' => Some(b'\t'),
                    _ => None,
                };
                if let Some(c) = decoded {
                    out.push(c);
                    i += 2;
                    continue;
                }
            }
        }
        out.push(data[i]);
        i += 1;
    }
    out
}
fn unesc(data: &[u8]) -> String {
    String::from_utf8_lossy(&unesc_bytes(data)).into_owned()
}
fn latin(data: &[u8]) -> String {
    data.iter().map(|b| char::from(*b)).collect()
}
fn number(data: &[u8]) -> f64 {
    std::str::from_utf8(data)
        .ok()
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|n| n.is_finite())
        .unwrap_or(0.0)
}

pub fn parse_log(data: &[u8]) -> ParsedLog {
    let (mut events, mut payloads, mut repeats, mut end) =
        (Vec::new(), Vec::new(), Vec::new(), None);
    let end_pair = Regex::new(r"(\w+)=(\S+)").unwrap();
    for raw in data.split_inclusive(|b| *b == b'\n') {
        if !raw.ends_with(b"\n") || raw.first() != Some(&1) {
            continue;
        }
        let f: Vec<_> = raw[1..raw.len() - 1].split(|b| *b == b'\t').collect();
        match f.first().copied() {
            Some(b"T") if f.len() >= 4 => {
                let mut stamp = f[1].splitn(2, |b| *b == b'@');
                let time = number(stamp.next().unwrap_or_default());
                let line = stamp.next().filter(|s| !s.is_empty()).map(latin);
                events.push(Event {
                    time,
                    line,
                    kind: latin(f[2]),
                    text: unesc(f[3]),
                    a: f.get(4).map(|x| unesc(x)),
                    b: f.get(5).map(|x| unesc(x)),
                });
            }
            Some(b"P") if f.len() >= 6 => {
                if let (Ok(id), Ok(length)) =
                    (latin(f[1]).parse::<u64>(), latin(f[4]).parse::<usize>())
                {
                    payloads.push(Payload {
                        id,
                        kind: latin(f[2]),
                        label: unesc(f[3]),
                        length,
                        data: unesc_bytes(&f[5..].join(&b'\t')),
                    });
                }
            }
            Some(b"S") if f.len() >= 6 => {
                if let Ok(times) = latin(f[1]).parse::<u64>() {
                    repeats.push(Repeat {
                        times,
                        kind: latin(f[2]),
                        text: unesc(f[3]),
                        first: number(f[4]),
                        last: number(f[5]),
                    });
                }
            }
            Some(b"X") if f.len() >= 2 => {
                let mut fields = BTreeMap::from([("reason".into(), unesc(f[1]))]);
                for part in &f[2..] {
                    for m in end_pair.captures_iter(&unesc(part)) {
                        fields.insert(m[1].into(), m[2].into());
                    }
                }
                end = Some(fields);
            }
            _ => {}
        }
    }
    (events, payloads, repeats, end)
}

pub struct TraceResult {
    pub events: Vec<Event>,
    pub payloads: Vec<Payload>,
    pub repeats: Vec<Repeat>,
    pub end: Option<BTreeMap<String, String>>,
    pub status: String,
    pub stderr: String,
    pub seconds: f64,
    pub source: Vec<u8>,
    pub max_time: f64,
}
impl TraceResult {
    pub fn reason(&self) -> String {
        if let Some(end) = &self.end {
            return end.get("reason").cloned().unwrap_or_default();
        }
        match self.status.as_str() {
            "timeout" => "the real-time limit was reached and the script was stopped (it may be stuck in a loop that never waits)",
            "output" => "the script produced more output than allowed and was stopped",
            "killed" => "the sandbox process was killed (out of memory or out of CPU time)",
            "crashed" => "the sandbox process stopped unexpectedly",
            _ => "the sandbox process ended without a final line",
        }.into()
    }
}

pub fn trace(data: &[u8], opts: &Value) -> Result<TraceResult> {
    let opts = options(opts)?;
    let source = data.strip_prefix(b"\xef\xbb\xbf").unwrap_or(data);
    if crate::astutil::is_blank_source(source) {
        return Err(Error::new("empty", "there is no script to run"));
    }
    if source.len() as u64 > opts["max_bytes"].as_u64().unwrap() {
        return Err(Error::new(
            "too_big",
            format!("the script is larger than {} bytes", opts["max_bytes"]),
        ));
    }
    let program = build_script(source, &opts, crate::sandbox::live_supported())?;
    let lr = crate::sandbox::run_luau_live(
        &program,
        Duration::from_secs_f64(opts["timeout"].as_f64().unwrap()),
        opts["mem_mb"].as_u64().unwrap(),
        opts["max_output_mb"].as_u64().unwrap(),
    )?;
    let (events, payloads, repeats, end) = parse_log(&lr.out);
    Ok(TraceResult {
        events,
        payloads,
        repeats,
        end,
        status: lr.status,
        stderr: String::from_utf8_lossy(&lr.err).into(),
        seconds: lr.seconds,
        source: source.to_vec(),
        max_time: opts["max_time"].as_f64().unwrap(),
    })
}

pub fn run(data: &[u8], opts: &Value) -> Result<Value> {
    let result = trace(data, opts)?;
    let payloads: Vec<_> = result.payloads.iter().map(|p| json!({"id":p.id,"kind":p.kind,"label":p.label,"length":p.length,"data":p.data,"ext":p.ext()})).collect();
    Ok(
        json!({"report":render_report(&result,"@@NAME@@",opts.get("lines_of").and_then(Value::as_str).unwrap_or("the script as given"),Some("@@STEM@@")),"payloads":payloads,"summary":summary_info(&result)}),
    )
}

pub fn vis(text: &str) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    for c in text.chars() {
        match c {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{0}'..='\u{1f}' | '\u{7f}'..='\u{9f}' => {
                let _ = write!(out, "\\x{:02x}", c as u32);
            }
            '\u{2028}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{feff}' => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            _ => out.push(c),
        }
    }
    out
}
fn short(text: &str, n: usize) -> String {
    let text = text.replace('\n', " ");
    if text.chars().count() <= n {
        text
    } else {
        text.chars().take(n.saturating_sub(3)).collect::<String>() + "..."
    }
}
fn location(ev: &Event) -> String {
    ev.line
        .as_ref()
        .map(|s| {
            if s.contains(':') {
                s.clone()
            } else {
                format!("L{s}")
            }
        })
        .unwrap_or_default()
}
fn function(text: &str) -> String {
    Regex::new(r"^(?:local \w+ = )?([A-Za-z_][\w\.]*)\(")
        .unwrap()
        .captures(text)
        .map(|m| m[1].into())
        .unwrap_or_else(|| text.split('(').next().unwrap_or("").into())
}
fn remote_key(text: &str) -> (String, String) {
    Regex::new(r"^(.*?):(FireServer|InvokeServer|FireClient|FireAllClients|InvokeClient)\(")
        .unwrap()
        .captures(text)
        .map(|m| (m[1].into(), m[2].into()))
        .unwrap_or_else(|| (text.split('(').next().unwrap_or("").into(), String::new()))
}
fn connection(text: &str) -> String {
    Regex::new(r"\.(\w+):(?:Connect|Once)\(")
        .unwrap()
        .captures(text)
        .map(|m| m[1].into())
        .unwrap_or_else(|| "?".into())
}

#[derive(Clone)]
pub struct Finding {
    pub title: String,
    pub detail: String,
    pub event: Event,
    pub count: u64,
}
struct Remote {
    count: u64,
    first: Option<Event>,
    example: String,
}
pub struct Summary<'a> {
    result: &'a TraceResult,
    requests: Vec<(&'a Event, String, String)>,
    remotes: IndexMap<(String, String), Remote>,
    loads: Vec<(&'a Event, Option<&'a Payload>)>,
    files_written: Vec<&'a Event>,
    files_read: usize,
    executor_calls: IndexMap<String, u64>,
    kicks: Vec<&'a Event>,
    teleports: Vec<&'a Event>,
    errors: Vec<&'a Event>,
    probes: Vec<&'a Event>,
    notes: Vec<&'a Event>,
    created: IndexMap<String, u64>,
    connections: IndexMap<String, u64>,
    prints: u64,
    clipboard: Vec<&'a Event>,
    queued: Vec<&'a Event>,
    payload_urls: Vec<String>,
}
impl<'a> Summary<'a> {
    pub fn new(result: &'a TraceResult) -> Self {
        let mut s = Self {
            result,
            requests: vec![],
            remotes: IndexMap::new(),
            loads: vec![],
            files_written: vec![],
            files_read: 0,
            executor_calls: IndexMap::new(),
            kicks: vec![],
            teleports: vec![],
            errors: vec![],
            probes: vec![],
            notes: vec![],
            created: IndexMap::new(),
            connections: IndexMap::new(),
            prints: 0,
            clipboard: vec![],
            queued: vec![],
            payload_urls: vec![],
        };
        let new_instance = Regex::new(r#"Instance\.new\("([A-Za-z0-9_]+)""#).unwrap();
        for ev in &result.events {
            match ev.kind.as_str() {
                "net" => s.requests.push((
                    ev,
                    ev.b.clone().unwrap_or_default(),
                    ev.a.clone().unwrap_or_default(),
                )),
                "remote" => {
                    let r = s
                        .remotes
                        .entry(remote_key(&ev.text))
                        .or_insert_with(|| Remote {
                            count: 0,
                            first: Some(ev.clone()),
                            example: ev.text.clone(),
                        });
                    r.count += 1;
                }
                "load" => s.loads.push((
                    ev,
                    ev.a.as_ref()
                        .and_then(|a| a.parse::<u64>().ok())
                        .and_then(|id| result.payloads.iter().rev().find(|p| p.id == id)),
                )),
                "file" => {
                    if [
                        "writefile",
                        "appendfile",
                        "makefolder",
                        "delfile",
                        "delfolder",
                    ]
                    .contains(&function(&ev.text).as_str())
                    {
                        s.files_written.push(ev);
                    } else {
                        s.files_read += 1;
                    }
                }
                "exec" => {
                    let f = function(&ev.text);
                    *s.executor_calls.entry(f.clone()).or_default() += 1;
                    if ["setclipboard", "toclipboard"].contains(&f.as_str()) {
                        s.clipboard.push(ev);
                    } else if f == "queue_on_teleport" {
                        s.queued.push(ev);
                    }
                }
                "kick" => s.kicks.push(ev),
                "tp" => s.teleports.push(ev),
                "err" => s.errors.push(ev),
                "probe" => s.probes.push(ev),
                "note" => s.notes.push(ev),
                "def" => {
                    if let Some(m) = new_instance.captures(&ev.text) {
                        *s.created.entry(m[1].into()).or_default() += 1;
                    }
                }
                "conn" => *s.connections.entry(connection(&ev.text)).or_default() += 1,
                "out" => s.prints += 1,
                _ => {}
            }
        }
        for rep in &result.repeats {
            match rep.kind.as_str() {
                "remote" => {
                    let r = s
                        .remotes
                        .entry(remote_key(&rep.text))
                        .or_insert_with(|| Remote {
                            count: 0,
                            first: None,
                            example: rep.text.clone(),
                        });
                    r.count += rep.times;
                }
                "out" => s.prints += rep.times,
                "conn" => *s.connections.entry(connection(&rep.text)).or_default() += rep.times,
                "exec" => *s.executor_calls.entry(function(&rep.text)).or_default() += rep.times,
                _ => {}
            }
        }
        let url = Regex::new(r#"(?i)https?://[^\s"'<>\\)\]}]{4,200}"#).unwrap();
        for p in &result.payloads {
            if ["loadstring", "queue_on_teleport", "clipboard"].contains(&p.kind.as_str()) {
                for m in url.find_iter(&latin(&p.data[..p.data.len().min(200000)])) {
                    let u = m.as_str().trim_end_matches(['.', ',', ';']).to_string();
                    if !s.payload_urls.contains(&u) {
                        s.payload_urls.push(u);
                    }
                }
            }
        }
        s
    }
    fn instances(&self) -> u64 {
        self.created.values().sum()
    }
    fn remote_count(&self) -> u64 {
        self.remotes.values().map(|r| r.count).sum()
    }
    pub fn domains(&self) -> Vec<String> {
        let re = Regex::new(r"(?i)^[a-z][a-z0-9+.-]*://([^/:?#\s]+)").unwrap();
        let mut out = Vec::new();
        for (_, _, url) in &self.requests {
            if let Some(m) = re.captures(url) {
                let domain = m[1].to_lowercase();
                if !out.contains(&domain) {
                    out.push(domain);
                }
            }
        }
        out
    }
    pub fn findings(&self) -> Vec<Finding> {
        let mut found: Vec<Finding> = vec![];
        let mut add = |title: &str, detail: String, ev: &Event| {
            if let Some(f) = found
                .iter_mut()
                .find(|f| f.title == title && f.detail == detail)
            {
                f.count += 1;
            } else {
                found.push(Finding {
                    title: title.into(),
                    detail,
                    event: ev.clone(),
                    count: 1,
                });
            }
        };
        let mut used = HashSet::new();
        for (lev, p) in &self.loads {
            let Some(line) = lev.line.as_ref().and_then(|s| s.parse::<u64>().ok()) else {
                continue;
            };
            if p.is_some_and(|p| {
                !p.truncated()
                    && !p.data.is_empty()
                    && self
                        .result
                        .source
                        .windows(p.data.len())
                        .any(|w| w == p.data)
            }) {
                continue;
            }
            if let Some((i, (ev, _, url))) =
                self.requests
                    .iter()
                    .enumerate()
                    .rev()
                    .find(|(i, (ev, method, _))| {
                        method == "GET"
                            && !used.contains(i)
                            && ev.time <= lev.time
                            && ev
                                .line
                                .as_ref()
                                .and_then(|s| s.parse::<u64>().ok())
                                .is_some_and(|n| line >= n && line - n <= 5)
                    })
            {
                used.insert(i);
                add(
                    "Downloads code and runs it",
                    format!(
                        "{} is fetched, then loadstring runs the answer (L{}){}",
                        short(url, 120),
                        line,
                        p.map(|p| format!(", {} bytes captured", p.length))
                            .unwrap_or_default()
                    ),
                    ev,
                );
            }
        }
        let webhook =
            Regex::new(r"(?i)^https?://(?:[\w-]+\.)*discord(?:app)?\.com/api(?:/v\d+)?/webhooks/")
                .unwrap();
        for (ev, method, url) in &self.requests {
            if method == "POST" && webhook.is_match(url) {
                add("Sends data to a Discord webhook", short(url, 120), ev);
            }
        }
        let autoexec = Regex::new(r"(?i)(^|/)auto-?exec(?:ute)?(/|$)").unwrap();
        for ev in &self.files_written {
            let path = ev.a.as_deref().unwrap_or("");
            if autoexec.is_match(path) {
                add("Writes into an auto-execute folder", short(path, 120), ev);
            }
        }
        for ev in &self.queued {
            add(
                "Queues code for the next teleport (queue_on_teleport)",
                short(&ev.text, 120),
                ev,
            );
        }
        for ev in &self.clipboard {
            add("Copies text to the clipboard", short(&ev.text, 120), ev);
        }
        for ev in &self.kicks {
            add("Kicks the player", short(&ev.text, 140), ev);
        }
        for ev in &self.teleports {
            add("Teleports the player", short(&ev.text, 140), ev);
        }
        for ev in &self.probes {
            let low = ev.text.to_lowercase();
            if [
                "identifyexecutor",
                "getexecutorname",
                "gethwid",
                "get_hwid",
                "getthreadidentity",
                "getidentity",
                "checkcaller",
            ]
            .iter()
            .any(|p| low.contains(p))
            {
                add(
                    "Asks which executor / device this is",
                    short(&ev.text, 140),
                    ev,
                );
            }
        }
        for f in &mut found {
            let function = match f.title.as_str() {
                "Queues code for the next teleport (queue_on_teleport)" => "queue_on_teleport",
                "Copies text to the clipboard" => "setclipboard",
                _ => "",
            };
            f.count = f
                .count
                .max(self.executor_calls.get(function).copied().unwrap_or(0));
        }
        found
    }
}

pub fn summary_info(r: &TraceResult) -> Value {
    let s = Summary::new(r);
    json!({"events":r.events.len(),"repeated":r.repeats.iter().map(|p| p.times).sum::<u64>(),"requests":s.requests.len(),"domains":s.domains(),"remotes":s.remote_count(),
        "payloads":r.payloads.len(),"loadstrings":s.loads.len(),"errors":s.errors.len(),"files_written":s.files_written.len(),"instances":s.instances(),
        "findings":s.findings().iter().map(|f| f.title.clone()).collect::<Vec<_>>(),"finished":r.end.is_some(),"reason":r.reason(),
        "simulated":r.end.as_ref().and_then(|e| e.get("time")),"real_seconds":(r.seconds*100.0).round()/100.0,"status":r.status})
}

pub fn limits(r: &TraceResult) -> Vec<String> {
    let mut out = vec!["This is a pretend Roblox: services answer with made-up values and the world has no real game in it. Code that waits for the real game (a server event, a particular window, another player) may not have run.".into(),
        "Nothing was downloaded: every request was answered by the sandbox (an empty text unless --responses gave one), so code that a real server would have sent was not seen.".into(),
        format!("Only the first {} simulated seconds are shown; later behavior (long timers, things that start on a button press) is not in this log. Use --explore to press every button the script made.",r.max_time)];
    if r.end.is_none() {
        out.push(format!(
            "The run did not finish normally ({}), so the end of the log is missing.",
            r.reason()
        ));
        if r.status == "crashed" && !r.stderr.is_empty() {
            out.push(format!(
                "First line the sandbox process wrote to its error output: {}",
                short(r.stderr.lines().next().unwrap_or(""), 200)
            ));
        }
    }
    let internal = r
        .events
        .iter()
        .filter(|e| e.kind == "err" && e.b.as_deref() == Some("internal"))
        .count();
    if internal > 0 {
        out.push(format!("{internal} error(s) came from the fake environment itself. The real game may not raise them."));
    } else if r.events.iter().any(|e| e.kind == "err") {
        out.push("The script stopped with an error here. In the real game it might have gone on (the sandbox may lack something it needs).".into());
    }
    if r.payloads.iter().any(Payload::truncated) {
        out.push("At least one captured payload is longer than the sandbox keeps; only the beginning is saved.".into());
    }
    if r.end
        .as_ref()
        .and_then(|e| e.get("cut"))
        .is_some_and(|s| s == "1")
    {
        out.push("The log was cut at its size limit; later events are missing.".into());
    }
    out
}

pub fn render_report(r: &TraceResult, name: &str, lines_of: &str, stem: Option<&str>) -> String {
    let s = Summary::new(r);
    let mut out = Vec::new();
    let mut add = |line: String| out.push(vis(&line));
    let dash = "-".repeat(72);
    add(format!("FAKE ROBLOX SANDBOX TRACE  -  {name}"));
    add("=".repeat(72));
    let lines = r.source.iter().filter(|b| **b == b'\n').count()
        + usize::from(!r.source.is_empty() && !r.source.ends_with(b"\n"));
    add(format!(
        "Script: {} bytes, {} lines, sha256 {:x}",
        r.source.len(),
        lines,
        Sha256::digest(&r.source)
    ));
    add(format!("Ended: {}", r.reason()));
    if let Some(end) = &r.end {
        add(format!(
            "Run: {} simulated seconds, {} scheduler steps, {:.2} s real time",
            end.get("time").map(String::as_str).unwrap_or("?"),
            end.get("steps").map(String::as_str).unwrap_or("?"),
            r.seconds
        ));
    } else {
        add(format!("Run: stopped after {:.1} s real time; the log below is what had been written until then",r.seconds));
    }
    add(format!("How to read it: the first column is simulated time in seconds since the script started, L<number> is the line of {lines_of} that did it."));
    add(String::new());
    add("SUMMARY".into());
    add(dash.clone());
    add(format!(
        "Events: {} logged, {} more were left out because they repeated",
        r.events.len(),
        r.repeats.iter().map(|p| p.times).sum::<u64>()
    ));
    if s.requests.is_empty() {
        add("Network: no requests".into());
    } else {
        let domains = s.domains();
        add(format!(
            "Network (nothing was sent, the sandbox answered): {} requests to {}",
            s.requests.len(),
            if domains.is_empty() {
                "unknown hosts".into()
            } else {
                domains.join(", ")
            }
        ));
        let mut seen = HashSet::new();
        for (ev, method, url) in &s.requests {
            if seen.insert((method, url)) {
                add(format!(
                    "  {:<4} {}   [{:.3} s {}]",
                    method.split_whitespace().next().unwrap_or("?"),
                    short(url, 150),
                    ev.time,
                    location(ev)
                ));
                if seen.len() >= 25 {
                    add("  ... and more (see the log)".into());
                    break;
                }
            }
        }
    }
    if !s.loads.is_empty() {
        add(format!("loadstring: {} call(s)", s.loads.len()));
    }
    if !r.payloads.is_empty() {
        add(format!("Captured texts: {} (what loadstring ran, request bodies, files written, code queued for a teleport, long clipboard texts)",r.payloads.len()));
    }
    for p in r.payloads.iter().take(20) {
        add(format!(
            "  payload {} ({}, {}): {} bytes{}, sha256 {}...{}",
            p.id,
            p.kind,
            short(&p.label, 60),
            p.length,
            if p.truncated() {
                format!(" (first {} kept)", p.data.len())
            } else {
                String::new()
            },
            &p.sha256()[..16],
            stem.map(|s| format!("  -> {s}_payload{}.{}", p.id, p.ext()))
                .unwrap_or_default()
        ));
        add(format!(
            "    starts with: {}",
            short(
                &String::from_utf8_lossy(&p.data[..p.data.len().min(300)])
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" "),
                160
            )
        ));
    }
    if r.payloads.len() > 20 {
        add(format!(
            "  ... and {} more payloads (all are saved as files)",
            r.payloads.len() - 20
        ));
    }
    if !s.payload_urls.is_empty() {
        add("Addresses found inside the captured payloads:".into());
        for url in s.payload_urls.iter().take(15) {
            add(format!("  {url}"));
        }
    }
    if s.remotes.is_empty() {
        add("Remotes: none fired".into());
    } else {
        add(format!(
            "Remotes ({} different, {} calls):",
            s.remotes.len(),
            s.remote_count()
        ));
        let mut rows: Vec<_> = s.remotes.iter().collect();
        rows.sort_by_key(|(_, r)| std::cmp::Reverse(r.count));
        for ((path, method), row) in rows.into_iter().take(25) {
            add(format!(
                "  x{:<4} {}:{}{}",
                row.count,
                path,
                method,
                row.first
                    .as_ref()
                    .map(|e| format!("  [first {:.3} s {}]", e.time, location(e)))
                    .unwrap_or_default()
            ));
            add(format!("        e.g. {}", short(&row.example, 150)));
        }
    }
    if !s.files_written.is_empty() || s.files_read > 0 {
        add(format!("Files (a virtual folder, the real disk was not touched): {} writes/changes, {} reads/checks",s.files_written.len(),s.files_read));
        for ev in s.files_written.iter().take(15) {
            add(format!(
                "  {}   [{:.3} s {}]",
                short(&ev.text, 150),
                ev.time,
                location(ev)
            ));
        }
    }
    fn ranked(map: &IndexMap<String, u64>, cap: usize) -> Vec<(&str, u64)> {
        let mut v: Vec<_> = map.iter().map(|(k, n)| (k.as_str(), *n)).collect();
        v.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
        v.truncate(cap);
        v
    }
    if !s.executor_calls.is_empty() {
        add(format!(
            "Executor functions: {}",
            ranked(&s.executor_calls, 20)
                .iter()
                .map(|(k, n)| format!("{k} x{n}"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !s.created.is_empty() {
        add(format!(
            "Instances created: {} ({})",
            s.instances(),
            ranked(&s.created, 8)
                .iter()
                .map(|(k, n)| format!("{k} {n}"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !s.connections.is_empty() {
        add(format!(
            "Event handlers connected: {} ({})",
            s.connections.values().sum::<u64>(),
            ranked(&s.connections, 8)
                .iter()
                .map(|(k, n)| format!("{k} {n}"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    add(format!("Console output: {} print/warn calls", s.prints));
    if !s.probes.is_empty() {
        add("Questions the script asked about its environment (answered with defaults):".into());
        for ev in s.probes.iter().take(15) {
            add(format!(
                "  {}   [{:.3} s {}]",
                short(&ev.text, 150),
                ev.time,
                location(ev)
            ));
        }
    }
    if !s.errors.is_empty() {
        add("Errors:".into());
        for ev in s.errors.iter().take(10) {
            add(format!(
                "  {}   [{:.3} s {}]{}",
                short(&ev.text, 200),
                ev.time,
                location(ev),
                if ev.b.as_deref() == Some("internal") {
                    "   <- raised inside the fake environment, not by the script itself"
                } else {
                    ""
                }
            ));
        }
    }
    for ev in s
        .notes
        .iter()
        .filter(|e| !e.text.starts_with("explore:"))
        .take(5)
    {
        add(format!("Note: {}", short(&ev.text, 200)));
    }
    add(String::new());
    add("WORTH A LOOK FIRST (what the script did in this run; not a verdict)".into());
    add(dash.clone());
    let finds = s.findings();
    if finds.is_empty() {
        add("(nothing of the kinds this tool looks for)".into());
    }
    for f in finds.iter().take(30) {
        add(format!(
            "* {}: {}   [{:.3} s {}]{}",
            f.title,
            f.detail,
            f.event.time,
            location(&f.event),
            if f.count > 1 {
                format!(" ({} times)", f.count)
            } else {
                String::new()
            }
        ));
    }
    add(String::new());
    add("LOG".into());
    add(dash.clone());
    for ev in &r.events {
        add(format!(
            "{:9.3}  {:<10} {:<7} {}",
            ev.time,
            location(ev),
            ev.kind,
            ev.text
        ));
        if ev.kind == "err" {
            if let Some(stack) = &ev.a {
                if !stack.is_empty() {
                    add(format!("{:29}stack: {}", "", short(stack, 400)));
                }
            }
        }
    }
    if r.events.is_empty() {
        add("(no events: the script did nothing that the sandbox writes down)".into());
    }
    if !r.repeats.is_empty() {
        add(String::new());
        add("LEFT OUT BECAUSE THEY KEPT REPEATING (the same line again and again; only the first few are in the log)".into());
        add(dash.clone());
        for rep in &r.repeats {
            add(format!(
                "x{:<6} {:<7} {}   [{:.3} s .. {:.3} s]",
                rep.times,
                rep.kind,
                short(&rep.text, 170),
                rep.first,
                rep.last
            ));
        }
    }
    add(String::new());
    add("LIMITS OF THIS TRACE".into());
    add(dash);
    for line in limits(r) {
        add(format!("* {line}"));
    }
    out.join("\n") + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn protocol_parses_complete_lines_and_byte_escapes() {
        let (e,p,s,end)=parse_log(b"noise\n\x01T\t1.500@12\tnet\tfetch\tu\tGET\n\x01P\t1\tloadstring\tlbl\t5\tab\\x0Acd\n\x01S\t40\tout\tprint(#)\t0.100\t2.000\n\x01X\tdone\tevents=3\tsteps=7 time=2.000\n\x01T\t2.0\tout\tcut");
        assert_eq!(
            (e[0].time, e[0].line.as_deref(), e[0].a.as_deref()),
            (1.5, Some("12"), Some("u"))
        );
        assert_eq!(p[0].data, b"ab\ncd");
        assert_eq!(s[0].times, 40);
        assert_eq!(end.unwrap()["steps"], "7");
        assert_eq!(unesc(b"a\\x0Ab\\\\n\\t\\x01\\xFF"), "a\nb\\n\t\x01\u{fffd}");
    }
    #[test]
    fn literals_and_report_text_cannot_change_layout() {
        assert!(lua_str(&(0..=255).collect::<Vec<_>>())
            .bytes()
            .all(|b| (32..127).contains(&b)));
        assert_eq!(lit(&json!({"k":false})).unwrap(), "{[\"k\"] = false}");
        assert_eq!(
            vis("a\nb\r\tc\x1b\u{202e}\u{2066}\u{2028}\u{feff}"),
            "a\\nb\\r\\tc\\x1b\\u202e\\u2066\\u2028\\ufeff"
        );
    }
}

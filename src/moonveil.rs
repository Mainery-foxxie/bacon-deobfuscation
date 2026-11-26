//! MoonVeil's observed behaviour reconstruction. Never represented as recovered source.
use crate::{
    error::{Error, Result},
    model::{Options, Output},
};
use regex::Regex;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    time::{Duration, Instant},
};

const TEMPLATE: &str = include_str!("lua/moonveil.lua.tpl");
const CFG: &str = "{far=1000000, maxcb=60, maxdepth=6, maxevents=8000, maxprobe=40, maxruns=400, maxstr=300, waitcap=40}";
const RESERVED: &str = "and break do else elseif end false for function if in local nil not or repeat return then true until while continue game workspace script task Instance Enum math string table os debug coroutine utf8 buffer bit32 print warn error pcall type typeof tostring tonumber next pairs ipairs select require loadstring setclipboard inst enabled dt value text choice key getgenv _G shared wait spawn delay tick time";
const EXECUTOR: &[&str] = &[
    "setclipboard",
    "toclipboard",
    "fireproximityprompt",
    "firetouchinterest",
    "firesignal",
    "fireclickdetector",
    "loadstring",
    "request",
    "http_request",
    "getgenv",
    "hookfunction",
    "hookmetamethod",
    "getrawmetatable",
    "queue_on_teleport",
    "writefile",
    "readfile",
    "getconnections",
    "getgc",
    "identifyexecutor",
];
const HTTP: &[&str] = &[
    "HttpGet",
    "HttpGetAsync",
    "HttpPost",
    "HttpPostAsync",
    "request",
    "http_request",
];
const KNOWN_GLOBALS: &str = "game workspace script Instance Enum Vector2 Vector3 CFrame Color3 UDim UDim2 BrickColor TweenInfo Ray Random Rect Region3 NumberRange NumberSequence ColorSequence NumberSequenceKeypoint ColorSequenceKeypoint Font Axes Faces OverlapParams RaycastParams DateTime Drawing task wait spawn delay tick time warn print typeof settings stats UserSettings plugin PhysicalProperties Workspace elapsedTime version printidentity require getgenv getrenv getreg getgc getinstances getnilinstances getconnections getscripts getloadedmodules getsenv getfenv setfenv hookfunction hookmetamethod newcclosure checkcaller cloneref clonefunction iscclosure islclosure isexecutorclosure getrawmetatable setrawmetatable setreadonly isreadonly make_writeable make_readonly getnamecallmethod setnamecallmethod fireproximityprompt firetouchinterest firesignal fireclickdetector setclipboard toclipboard loadstring request http_request http syn fluxus KRNL_LOADED identifyexecutor getexecutorname queue_on_teleport isfile readfile writefile appendfile delfile listfiles makefolder isfolder delfolder getcustomasset gethui getthreadidentity setthreadidentity setidentity decompile saveinstance mouse1click mouse1press mouse1release mousemoveabs mousemoverel keypress keyrelease rconsoleprint rconsoleclear messagebox setfpscap getcallingscript lz4compress lz4decompress crypt base64 base64_encode base64_decode";

fn regex(pattern: &str) -> Regex {
    Regex::new(pattern).expect("constant MoonVeil regex")
}
pub fn detect(text: &str) -> Option<Value> {
    let head: String = text.chars().take(1500).collect();
    if let Some(m) =
        regex(r"--\s*This script was generated using MoonVeil\s+(v?[0-9][0-9A-Za-z.\-]*)")
            .captures(&head)
    {
        Some(json!({"version":m[1].trim_start_matches('v')}))
    } else if regex(r"(?i)moonveil\.cc").is_match(&head) {
        Some(json!({"version":"?"}))
    } else {
        None
    }
}

fn run_script(source: &[u8], timeout: Duration, mem_mb: u64) -> Result<String> {
    let template = TEMPLATE.replace("@@CFG@@", CFG);
    let (before, after) = template
        .split_once("@@SRC@@")
        .ok_or_else(|| Error::new("setup", "invalid MoonVeil recording template"))?;
    let mut code = before.as_bytes().to_vec();
    code.extend_from_slice(source);
    code.extend_from_slice(after.as_bytes());
    match crate::sandbox::run_luau(&code, timeout, mem_mb, 128) {
        Ok(out) => Ok(String::from_utf8_lossy(&out).into()),
        Err(e) if e.code == "timeout" => Err(Error::new(
            "timeout",
            format!(
                "the script kept running for more than {} s without finishing (a busy loop?)",
                timeout.as_secs_f64()
            ),
        )),
        Err(e) if e.code == "setup" => Err(e),
        Err(e)
            if regex(r"Expected |Malformed|Incomplete statement|Unexpected")
                .is_match(&e.message) =>
        {
            Err(Error::new("syntax", e.message))
        }
        Err(e) => Err(Error::new("sandbox", e.message)),
    }
}

#[derive(Clone, Debug)]
pub struct Body {
    pub bid: String,
    pub labels: BTreeMap<String, String>,
    pub probe: Option<String>,
    pub events: Vec<Vec<String>>,
    pub status: String,
}
impl Body {
    fn fid(&self) -> Option<u64> {
        self.bid.strip_prefix('F').and_then(|s| s.parse().ok())
    }
}
#[derive(Default)]
pub struct Trace {
    pub bodies: Vec<Body>,
    pub main: Option<usize>,
    by_fn: HashMap<u64, Vec<usize>>,
    pub tags: HashSet<String>,
    pub globals: Vec<String>,
}

pub fn unquote(s: &str) -> String {
    let Some(m) = regex(r#"^"((?:[^"\\]|\\.)*)""#).captures(s) else {
        return s.into();
    };
    regex(r"\\(\d{3}|.)")
        .replace_all(&m[1], |m: &regex::Captures<'_>| {
            let c = &m[1];
            if c.bytes().all(|b| b.is_ascii_digit()) {
                c.parse::<u32>()
                    .ok()
                    .and_then(char::from_u32)
                    .unwrap_or('\u{fffd}')
                    .to_string()
            } else {
                match c {
                    "n" => "\n",
                    "r" => "\r",
                    "t" => "\t",
                    _ => c,
                }
                .into()
            }
        })
        .into_owned()
}

pub fn parse(log: &str) -> Trace {
    let mut t = Trace::default();
    let mut stack = Vec::new();
    for line in log.split('\n') {
        let mut f: Vec<String> = line.split('\t').map(str::to_owned).collect();
        match f[0].as_str() {
            "B" if f.len() >= 3 => {
                let labels = f[2]
                    .split(';')
                    .filter_map(|s| s.split_once('=').map(|(k, v)| (k.into(), v.into())))
                    .collect();
                let body = Body {
                    bid: f[1].clone(),
                    labels,
                    probe: f.get(3).filter(|s| !s.is_empty()).map(|s| unquote(s)),
                    events: vec![],
                    status: "cut".into(),
                };
                let i = t.bodies.len();
                if body.bid == "M" {
                    t.main = Some(i);
                } else if let Some(fid) = body.fid() {
                    t.by_fn.entry(fid).or_default().push(i);
                } else {
                    continue;
                }
                t.bodies.push(body);
                stack.push(i);
            }
            "E" if f.len() >= 3 => {
                if let Some(i) = stack.pop() {
                    t.bodies[i].status = f[2].clone();
                }
            }
            "C" | "S" | "N" | "I" | "X" if !stack.is_empty() => {
                if f[0] == "C" {
                    while f.len() < 4 {
                        f.push(String::new());
                    }
                }
                let required = match f[0].as_str() {
                    "C" => 4,
                    "S" | "I" => 3,
                    _ => 2,
                };
                if f.len() >= required {
                    t.bodies[*stack.last().unwrap()].events.push(f);
                }
            }
            "T" if f.len() >= 2 => {
                t.tags.insert(f[1].clone());
            }
            "G" if f.len() >= 2 => t.globals.push(f[1].clone()),
            _ => {}
        }
    }
    t
}

fn lower_camel(s: &str) -> String {
    let words: Vec<_> = regex(r"[A-Za-z0-9]+")
        .find_iter(s)
        .map(|m| m.as_str().to_owned())
        .collect();
    let Some(first) = words.first() else {
        return String::new();
    };
    let mut out = if first
        .chars()
        .filter(|c| c.is_ascii_alphabetic())
        .all(|c| c.is_uppercase())
    {
        first.to_lowercase()
    } else {
        first[..1].to_lowercase() + &first[1..]
    };
    for word in &words[1..] {
        out.push_str(&(word[..1].to_uppercase() + &word[1..]));
    }
    out
}
fn singular(s: &str) -> String {
    if s.to_lowercase() == "children" {
        return s[..s.len() - 3].into();
    }
    for (suffix, replacement) in [("ies", "y"), ("ses", "s"), ("s", "")] {
        if s.ends_with(suffix) && s.len() > suffix.len() + 1 {
            return s[..s.len() - suffix.len()].to_owned() + replacement;
        }
    }
    s.into()
}

struct Namer {
    by_key: HashMap<String, String>,
    used: HashSet<String>,
}
impl Namer {
    fn new() -> Self {
        Self {
            by_key: HashMap::new(),
            used: RESERVED.split_whitespace().map(str::to_owned).collect(),
        }
    }
    fn pick(&mut self, key: &str, base: &str) -> String {
        if let Some(s) = self.by_key.get(key) {
            return s.clone();
        }
        let mut base: String = base
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
            .take(28)
            .collect();
        if base.is_empty() {
            base = "v".into();
        }
        if base.starts_with(|c: char| c.is_ascii_digit()) {
            base.insert(0, '_');
        }
        let mut name = base.clone();
        let mut n = 1;
        while self.used.contains(&name) {
            n += 1;
            name = format!("{base}{n}");
        }
        self.used.insert(name.clone());
        self.by_key.insert(key.into(), name.clone());
        name
    }
    fn base_for(callee: &str, args: &str) -> String {
        let method = callee.rsplit([':', '.']).next().unwrap_or("");
        let first = regex(r#"^\s*("(?:[^"\\]|\\.)*")"#)
            .captures(args)
            .map(|m| unquote(&m[1]))
            .unwrap_or_default();
        if method == "new" && callee.contains('.') {
            let parts: Vec<_> = callee.split('.').collect();
            let s = parts[parts.len() - 2];
            return s
                .chars()
                .next()
                .map(|c| c.to_lowercase().collect::<String>() + &s[c.len_utf8()..])
                .unwrap_or_default();
        }
        if ["GetService", "FindService"].contains(&method) && !first.is_empty() {
            return first;
        }
        if [
            "FindFirstChild",
            "WaitForChild",
            "FindFirstDescendant",
            "FindFirstAncestor",
            "FindFirstChildOfClass",
            "FindFirstChildWhichIsA",
        ]
        .contains(&method)
            && !first.is_empty()
        {
            let s = lower_camel(&first);
            return if s.is_empty() { "child".into() } else { s };
        }
        if callee == "Instance.new" && !first.is_empty() {
            let s = lower_camel(&first);
            return if s.is_empty() { "instance".into() } else { s };
        }
        if method == "CreateWindow" {
            return "Window".into();
        }
        if ["AddFolder", "AddTab", "AddSection", "AddPage", "AddGroup"].contains(&method)
            && !first.is_empty()
        {
            let mut s = lower_camel(&first);
            if s.is_empty() {
                s = "main".into();
            }
            return s + &method[3..];
        }
        if method.starts_with("Add") && method.len() > 3 {
            return lower_camel(&method[3..]);
        }
        if callee.starts_with("loadstring(") || callee == "loadstring" {
            return "lib".into();
        }
        if ["Connect", "Once", "ConnectParallel"].contains(&method) {
            return "connection".into();
        }
        if method.starts_with("Get") && method.len() > 3 {
            return lower_camel(&method[3..]);
        }
        let s = lower_camel(method);
        if s.is_empty() {
            "v".into()
        } else {
            s
        }
    }
}

#[derive(Clone)]
enum Node {
    Text(String),
    Group(char, char, Vec<Vec<Node>>),
    Function(u64),
}
fn string_end(text: &str, pos: usize) -> usize {
    let bytes = text.as_bytes();
    let mut i = pos + 1;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            i += 2;
        } else if bytes[i] == b'"' {
            return i + 1;
        } else {
            i += 1;
        }
    }
    bytes.len()
}
fn parse_nodes(text: &str) -> Vec<Node> {
    fn seq(text: &str, pos: &mut usize, closer: Option<u8>) -> Vec<Vec<Node>> {
        let bytes = text.as_bytes();
        let mut items = Vec::new();
        let mut cur = Vec::new();
        let mut buf = String::new();
        fn flush(buf: &mut String, cur: &mut Vec<Node>) {
            if !buf.is_empty() {
                cur.push(Node::Text(std::mem::take(buf)));
            }
        }
        while *pos < bytes.len() {
            let c = bytes[*pos];
            if Some(c) == closer {
                break;
            }
            if c == b'"' {
                let j = string_end(text, *pos);
                buf.push_str(&text[*pos..j]);
                *pos = j;
            } else if text[*pos..].starts_with("--[[") {
                let j = text[*pos..]
                    .find("]]")
                    .map(|j| *pos + j + 2)
                    .unwrap_or(text.len());
                buf.push_str(&text[*pos..j]);
                *pos = j;
            } else if b"({[".contains(&c) {
                flush(&mut buf, &mut cur);
                *pos += 1;
                let close = match c {
                    b'(' => b')',
                    b'{' => b'}',
                    _ => b']',
                };
                let inner = seq(text, pos, Some(close));
                if *pos < bytes.len() {
                    *pos += 1;
                }
                cur.push(Node::Group(c as char, close as char, inner));
            } else if c == 2 {
                flush(&mut buf, &mut cur);
                if let Some(j) = text[*pos + 1..].find('\x03') {
                    let j = *pos + 1 + j;
                    if let Ok(id) = text[*pos + 1..j].parse() {
                        cur.push(Node::Function(id));
                    }
                    *pos = j + 1;
                } else {
                    *pos += 1;
                }
            } else if c == b',' && closer.is_some() {
                flush(&mut buf, &mut cur);
                items.push(std::mem::take(&mut cur));
                *pos += 1;
                while bytes.get(*pos) == Some(&b' ') {
                    *pos += 1;
                }
            } else {
                let ch = text[*pos..].chars().next().unwrap();
                buf.push(ch);
                *pos += ch.len_utf8();
            }
        }
        flush(&mut buf, &mut cur);
        items.push(cur);
        items
    }
    seq(text, &mut 0, None).pop().unwrap_or_default()
}
fn has_fn(n: &Node) -> bool {
    match n {
        Node::Function(_) => true,
        Node::Group(_, _, items) => items.iter().flatten().any(has_fn),
        _ => false,
    }
}
fn indent(text: &str) -> String {
    text.split('\n')
        .map(|s| {
            if s.is_empty() {
                String::new()
            } else {
                format!("\t{s}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}
#[derive(Clone)]
enum Statement {
    Text(String),
    For(String, Vec<Statement>),
}
struct Loop {
    id: Option<u64>,
    head: String,
    items: Vec<Statement>,
    var: String,
}

pub struct Renderer {
    trace: Trace,
    refs: HashMap<u64, usize>,
    calls: HashMap<u64, (String, String)>,
    rid_text: HashMap<u64, String>,
    evar: HashMap<u64, String>,
    namer: Namer,
    active: Vec<u64>,
}
impl Renderer {
    pub fn new(trace: Trace) -> Self {
        let mut refs = HashMap::new();
        let mut calls = HashMap::new();
        let r = regex(r"@R(\d+)");
        for body in &trace.bodies {
            for e in &body.events {
                if e[0] == "C" {
                    if let Ok(id) = e[1].parse() {
                        calls.insert(id, (e[2].clone(), e[3].clone()));
                    }
                }
                for x in &e[1..] {
                    for m in r.captures_iter(x) {
                        if let Ok(id) = m[1].parse() {
                            *refs.entry(id).or_default() += 1;
                        }
                    }
                }
            }
        }
        Self {
            trace,
            refs,
            calls,
            rid_text: HashMap::new(),
            evar: HashMap::new(),
            namer: Namer::new(),
            active: vec![],
        }
    }
    fn subst(&self, text: &str, note: bool) -> String {
        regex(r#"("(?:[^"\\]|\\.)*")|@([REF])(\d+)"#)
            .replace_all(text, |m: &regex::Captures<'_>| {
                if let Some(lit) = m.get(1) {
                    return regex(r"@R\d+")
                        .replace_all(lit.as_str(), |r: &regex::Captures<'_>| {
                            self.fullexpr(&r[0], 0)
                                .replace('\\', "\\\\")
                                .replace('"', "\\\"")
                        })
                        .into_owned();
                }
                let id = m[3].parse::<u64>().unwrap_or(0);
                match &m[2] {
                    "R" => self.rid_text.get(&id).cloned().unwrap_or_else(|| {
                        if self.calls.contains_key(&id) {
                            self.fullexpr(&format!("@R{id}"), 0)
                        } else {
                            "nil --[[unknown value]]".into()
                        }
                    }),
                    "E" => self.evar.get(&id).cloned().unwrap_or_else(|| "item".into()),
                    _ => {
                        if note {
                            "function() end".into()
                        } else {
                            format!("\x02{id}\x03")
                        }
                    }
                }
            })
            .into_owned()
    }
    fn fullexpr(&self, text: &str, depth: usize) -> String {
        regex(r#"("(?:[^"\\]|\\.)*")|@([REF])(\d+)"#)
            .replace_all(text, |m: &regex::Captures<'_>| {
                if let Some(lit) = m.get(1) {
                    return lit.as_str().to_owned();
                }
                let id = m[3].parse::<u64>().unwrap_or(0);
                if &m[2] != "R" || depth > 6 || !self.calls.contains_key(&id) {
                    return if &m[2] == "R" {
                        "?".into()
                    } else {
                        "function() end".into()
                    };
                }
                let (callee, args) = &self.calls[&id];
                format!(
                    "{}({})",
                    self.fullexpr(callee, depth + 1),
                    self.fullexpr(args, depth + 1)
                )
            })
            .into_owned()
    }
    fn fn_mentions(&self, fid: u64, rid: u64, seen: &mut HashSet<u64>) -> bool {
        if !seen.insert(fid) {
            return false;
        }
        for i in self.trace.by_fn.get(&fid).into_iter().flatten() {
            for e in &self.trace.bodies[*i].events {
                for x in &e[1..] {
                    if mentions(x, rid) {
                        return true;
                    }
                    for m in regex(r"@F(\d+)").captures_iter(x) {
                        if self.fn_mentions(m[1].parse().unwrap_or(0), rid, seen) {
                            return true;
                        }
                    }
                }
            }
        }
        false
    }
    fn tag(text: String) -> String {
        if text.contains("loadstring(") && text.contains("HttpGet") {
            text + " -- NOT FETCHED: the downloaded code is not part of this trace"
        } else {
            text
        }
    }
    fn items(&mut self, body: &Body) -> Vec<Statement> {
        let mut stack = vec![Loop {
            id: None,
            head: String::new(),
            items: vec![],
            var: String::new(),
        }];
        for (i, e) in body.events.iter().enumerate() {
            match e[0].as_str() {
                "C" => {
                    let Ok(rid) = e[1].parse::<u64>() else {
                        continue;
                    };
                    let callee = self.subst(&e[2], false);
                    let args = self.subst(&e[3], false);
                    let text = format!("{callee}({args})");
                    let n = self.refs.get(&rid).copied().unwrap_or(0);
                    if n == 1
                        && body.events.get(i + 1).is_some_and(|next| {
                            ["C", "S", "I"].contains(&next[0].as_str())
                                && next[1..].iter().any(|x| mentions(x, rid))
                        })
                    {
                        self.rid_text.insert(rid, text);
                        continue;
                    }
                    if n > 0 {
                        let key = regex("\x02\\d+\x03")
                            .replace_all(&text, "\x02\x03")
                            .into_owned();
                        let name = self.namer.pick(&key, &Namer::base_for(&callee, &args));
                        self.rid_text.insert(rid, name.clone());
                        let self_ref = regex("\x02(\\d+)\x03").captures_iter(&args).any(|m| {
                            self.fn_mentions(m[1].parse().unwrap_or(0), rid, &mut HashSet::new())
                        });
                        let cur = &mut stack.last_mut().unwrap().items;
                        if self_ref {
                            cur.push(Statement::Text(format!("local {name}")));
                            cur.push(Statement::Text(format!("{name} = {}", Self::tag(text))));
                        } else {
                            cur.push(Statement::Text(format!(
                                "local {name} = {}",
                                Self::tag(text)
                            )));
                        }
                    } else {
                        stack
                            .last_mut()
                            .unwrap()
                            .items
                            .push(Statement::Text(Self::tag(text)));
                    }
                }
                "S" => stack
                    .last_mut()
                    .unwrap()
                    .items
                    .push(Statement::Text(format!(
                        "{} = {}",
                        self.subst(&e[1], false),
                        self.subst(&e[2], false)
                    ))),
                "N" => stack
                    .last_mut()
                    .unwrap()
                    .items
                    .push(Statement::Text(format!(
                        "-- {}",
                        self.subst(&e[1], true).replace('\n', " ")
                    ))),
                "I" => {
                    let Ok(id) = e[1].parse::<u64>() else {
                        continue;
                    };
                    let expr = self.subst(&e[2], false);
                    let method = regex(r"[:.](\w+)\([^()]*\)$")
                        .captures(&expr)
                        .map(|m| m[1].to_owned())
                        .unwrap_or_default();
                    let mut var = if method.starts_with("Get") && method.len() > 3 {
                        lower_camel(&singular(&method[3..]))
                    } else {
                        "item".into()
                    };
                    if var.is_empty() {
                        var = "item".into();
                    }
                    while self.namer.used.contains(&var) || stack.iter().any(|l| l.var == var) {
                        var.push('_');
                    }
                    self.evar.insert(id, var.clone());
                    stack.push(Loop {
                        id: Some(id),
                        head: format!("for _, {var} in ipairs({expr}) do"),
                        items: vec![],
                        var,
                    });
                }
                "X" if stack.len() > 1 && e[1].parse::<u64>().ok() == stack.last().unwrap().id => {
                    let top = stack.pop().unwrap();
                    stack
                        .last_mut()
                        .unwrap()
                        .items
                        .push(Statement::For(top.head, top.items));
                }
                _ => {}
            }
        }
        while stack.len() > 1 {
            let top = stack.pop().unwrap();
            stack
                .last_mut()
                .unwrap()
                .items
                .push(Statement::For(top.head, top.items));
        }
        stack.pop().unwrap().items
    }
    fn finish(&mut self, items: &[Statement], cut: bool) -> Vec<String> {
        let mut texts = Vec::new();
        for item in items {
            match item {
                Statement::Text(s) => texts.push(self.expand(s)),
                Statement::For(head, items) => {
                    let mut inner = self.finish(items, false);
                    if inner.is_empty() {
                        inner.push("-- UNKNOWN: nothing observable was done with each item".into());
                    }
                    texts.push(format!("{head}\n{}\nend", indent(&inner.join("\n"))));
                }
            }
        }
        Self::collapse(texts, cut)
    }
    fn collapse(texts: Vec<String>, cut: bool) -> Vec<String> {
        let mut out = Vec::new();
        let mut i = 0;
        let n = texts.len();
        while i < n {
            let mut hit = None;
            for p in 1..=8 {
                let mut k = 1;
                while i + (k + 1) * p <= n && texts[i + k * p..i + (k + 1) * p] == texts[i..i + p] {
                    k += 1;
                }
                if k >= 4 || (k >= 3 && p >= 2) {
                    hit = Some((p, k));
                    break;
                }
            }
            let Some((p, k)) = hit else {
                out.push(texts[i].clone());
                i += 1;
                continue;
            };
            let inner = indent(&texts[i..i + p].join("\n"));
            let rest = &texts[i + k * p..];
            if cut && rest.len() < p && rest == &texts[i..i + rest.len()] {
                out.push(format!("while true do -- UNKNOWN stop condition (the trace saw {k} rounds, then stopped it)\n{inner}\nend"));
                break;
            }
            out.push(format!("for _ = 1, {k} do -- the same {p} step{} happened {k} times in a row\n{inner}\nend",if p==1{""}else{"s"}));
            i += k * p;
        }
        out
    }
    fn body_lines(&mut self, body: &Body, with_status: bool) -> Vec<String> {
        let items = self.items(body);
        let cut = body.status == "cut";
        let mut lines = self.finish(&items, cut);
        if with_status {
            if let Some(error) = body.status.strip_prefix("err:") {
                lines.push(format!(
                    "-- STOPPED in the sandbox: {error} (usually because of the fake objects)"
                ));
            } else if cut && !lines.last().is_some_and(|l| l.starts_with("while true do")) {
                lines.push(
                    "-- (trace cut here: the sandbox stopped waiting; the real script continues)"
                        .into(),
                );
            }
        }
        lines
    }
    fn expand(&mut self, text: &str) -> String {
        if !text.contains('\x02') {
            text.into()
        } else {
            self.nodes_text(&parse_nodes(text))
        }
    }
    fn nodes_text(&mut self, nodes: &[Node]) -> String {
        let mut out = String::new();
        for node in nodes {
            match node {
                Node::Text(s) => out.push_str(s),
                Node::Function(id) => out.push_str(&self.fn_text(*id)),
                Node::Group(open, close, items) => {
                    let items: Vec<_> = items.iter().filter(|i| !i.is_empty()).collect();
                    let with_fn: Vec<_> = items.iter().filter(|i| i.iter().any(has_fn)).collect();
                    if with_fn.is_empty()
                        || (with_fn.len() == 1
                            && with_fn[0].len() == 1
                            && !matches!(with_fn[0][0], Node::Text(_)))
                    {
                        let rows = items.iter().map(|i| self.nodes_text(i)).collect::<Vec<_>>();
                        out.push_str(&format!("{open}{}{close}", rows.join(", ")));
                    } else {
                        let rows = items
                            .iter()
                            .map(|i| indent(&self.nodes_text(i)))
                            .collect::<Vec<_>>();
                        out.push_str(&format!(
                            "{open}\n{}{}\n{close}",
                            rows.join(",\n"),
                            if *open == '{' { "," } else { "" }
                        ));
                    }
                }
            }
        }
        out
    }
    fn fn_text(&mut self, fid: u64) -> String {
        let Some(indices) = self.trace.by_fn.get(&fid) else {
            return "function() end --[[not run in the trace]]".into();
        };
        if self.active.contains(&fid) || self.active.len() > 12 {
            return "function() end".into();
        }
        let bodies: Vec<_> = indices
            .iter()
            .map(|i| self.trace.bodies[*i].clone())
            .collect();
        self.active.push(fid);
        let (params, lines) = self.fn_body(&bodies);
        self.active.pop();
        if lines.is_empty() {
            format!("function({params}) end")
        } else {
            format!("function({params})\n{}\nend", indent(&lines.join("\n")))
        }
    }
    fn fn_body(&mut self, bodies: &[Body]) -> (String, Vec<String>) {
        let first = &bodies[0];
        let kind = first
            .labels
            .get("kind")
            .map(String::as_str)
            .unwrap_or("spawn");
        let param = first.labels.get("param").cloned().unwrap_or_default();
        if kind == "spawn" {
            return (String::new(), self.body_lines(first, true));
        }
        let plain: Vec<_> = bodies
            .iter()
            .filter(|b| b.probe.is_none())
            .cloned()
            .collect();
        let probes: Vec<_> = bodies
            .iter()
            .filter(|b| b.probe.is_some())
            .cloned()
            .collect();
        if kind == "toggle" {
            let on = self.dist_group(
                &plain
                    .iter()
                    .filter(|b| b.labels.get("val").is_some_and(|v| v == "true"))
                    .cloned()
                    .collect::<Vec<_>>(),
            );
            let off = self.dist_group(
                &plain
                    .iter()
                    .filter(|b| b.labels.get("val").is_some_and(|v| v == "false"))
                    .cloned()
                    .collect::<Vec<_>>(),
            );
            let mut lines = if on == off {
                on
            } else if !on.is_empty() && !off.is_empty() {
                vec![
                    format!("if {param} then"),
                    indent(&on.join("\n")),
                    "else".into(),
                    indent(&off.join("\n")),
                    "end".into(),
                ]
            } else if !on.is_empty() {
                vec![
                    format!("if {param} then"),
                    indent(&on.join("\n")),
                    "end".into(),
                ]
            } else {
                vec![
                    format!("if not {param} then"),
                    indent(&off.join("\n")),
                    "end".into(),
                ]
            };
            if lines.is_empty() {
                lines.push("-- (nothing observable happened in the sandbox)".into());
            }
            return (param, lines);
        }
        let mut lines = self.dist_group(&plain);
        if lines.is_empty() && !probes.is_empty() {
            let mut groups: Vec<(String, Vec<String>)> = vec![];
            for b in &probes {
                let text = self.body_lines(b, false).join("\n");
                if !text.trim().is_empty() {
                    if let Some((_, names)) = groups.iter_mut().find(|(t, _)| *t == text) {
                        names.push(b.probe.clone().unwrap());
                    } else {
                        groups.push((text, vec![b.probe.clone().unwrap()]));
                    }
                }
            }
            for (text, names) in groups {
                let param = if param.is_empty() { "inst" } else { &param };
                let mut cond = names
                    .iter()
                    .take(6)
                    .map(|n| format!("{param}.Name == \"{}\"", Self::esc(n)))
                    .collect::<Vec<_>>()
                    .join(" or ");
                if names.len() > 6 {
                    cond.push_str(&format!(" or ... --[[{} more names]]", names.len() - 6));
                }
                lines.extend([
                    format!(
                        "if {cond} then -- found by trying the names this script used elsewhere"
                    ),
                    indent(&text),
                    "end".into(),
                ]);
            }
        }
        if lines.is_empty() {
            lines.push("-- (nothing observable happened in the sandbox: the handler reacts to something that was not tried) -- UNKNOWN".into());
        }
        (param, lines)
    }
    fn esc(s: &str) -> String {
        s.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
    }
    fn dist_group(&mut self, bodies: &[Body]) -> Vec<String> {
        if bodies.is_empty() {
            return vec![];
        }
        let runs: Vec<_> = bodies
            .iter()
            .map(|b| {
                (
                    b.labels
                        .get("dist")
                        .cloned()
                        .unwrap_or_else(|| "near".into()),
                    self.body_lines(b, true),
                )
            })
            .collect();
        if runs.iter().all(|(_, lines)| *lines == runs[0].1) {
            return runs[0].1.clone();
        }
        let mut out = vec![];
        for (dist, lines) in runs {
            out.extend([format!("-- {}:",if dist=="near"{"when the script measured a short distance"}else{"when the script measured a long distance (the real limit is not observable)"}),"do".into(),if lines.is_empty(){"\t-- (nothing)".into()}else{indent(&lines.join("\n"))},"end".into()]);
        }
        out
    }
    pub fn render(&mut self) -> String {
        self.trace
            .main
            .map(|i| self.trace.bodies[i].clone())
            .map(|body| self.body_lines(&body, true).join("\n"))
            .unwrap_or_default()
    }
    pub fn summary(&self) -> Value {
        let (mut downloads, mut remotes, mut clipboard, mut executor, mut links) =
            (vec![], Vec::<(String, u64)>::new(), vec![], vec![], vec![]);
        let url = regex(r#"https?://[^\s"\\)]+"#);
        for b in &self.trace.bodies {
            for e in &b.events {
                if e[0] != "C" {
                    continue;
                }
                let callee = self.fullexpr(&e[2], 0);
                let args = self.fullexpr(&e[3], 0);
                let method = callee.rsplit([':', '.']).next().unwrap_or("");
                if HTTP.contains(&method) {
                    for u in url.find_iter(&args) {
                        if !downloads.contains(&u.as_str().to_owned()) {
                            downloads.push(u.as_str().to_owned());
                        }
                    }
                }
                if ["FireServer", "InvokeServer"].contains(&method) {
                    let key = regex(r":(FireServer|InvokeServer)$")
                        .replace(&callee, "")
                        .into_owned();
                    let stripped = regex(r#"^.*GetService\("ReplicatedStorage"\)\??\(?\)?\.?"#)
                        .replace(&key, "")
                        .into_owned();
                    let key: String = if stripped.is_empty() { key } else { stripped }
                        .chars()
                        .take(90)
                        .collect();
                    if let Some((_, n)) = remotes.iter_mut().find(|(k, _)| *k == key) {
                        *n += 1;
                    } else {
                        remotes.push((key, 1));
                    }
                }
                if ["setclipboard", "toclipboard"].contains(&callee.as_str()) {
                    if let Some(m) = regex(r#"^\s*"((?:[^"\\]|\\.)*)""#).captures(&args) {
                        if !clipboard.contains(&m[1].to_owned()) {
                            clipboard.push(m[1].to_owned());
                        }
                    }
                }
                if EXECUTOR.contains(&callee.as_str()) && !executor.contains(&callee) {
                    executor.push(callee);
                }
            }
        }
        for b in &self.trace.bodies {
            for e in &b.events {
                for u in url.find_iter(&self.fullexpr(&e[1..].join(" "), 0)) {
                    let u = u.as_str().to_owned();
                    if !downloads.contains(&u) && !links.contains(&u) {
                        links.push(u);
                    }
                }
            }
        }
        let unset: Vec<_> = self
            .trace
            .globals
            .iter()
            .filter(|g| !KNOWN_GLOBALS.split_whitespace().any(|k| k == g.as_str()))
            .cloned()
            .collect();
        json!({"downloads":downloads,"remotes":remotes,"clipboard":clipboard,"executor":executor,"links":links,"unset":unset})
    }
}
fn mentions(text: &str, rid: u64) -> bool {
    regex(r"@R(\d+)")
        .captures_iter(text)
        .any(|m| m[1].parse::<u64>().ok() == Some(rid))
}

fn header(mv: &Value, summ: &Value, calls: usize, threads: usize) -> String {
    let version = mv["version"].as_str().unwrap_or("?");
    let assumed = mv["assumed"] == true;
    let mut out = vec![
        "--[[".into(),
        "  BEHAVIOUR RECONSTRUCTION - THIS IS NOT THE ORIGINAL SOURCE.".into(),
    ];
    if assumed {
        out.extend(["  The script has no MoonVeil banner; it was handled as a MoonVeil script because that was asked for.".into(),"  MoonVeil turns a script into a private VM program, so the original code (its structure, conditions and".into(),"  variable names) cannot be recovered by any tool.".into()]);
    } else {
        let who = if version.is_empty() || version == "?" {
            "MoonVeil (version not known)".into()
        } else {
            format!("MoonVeil {version}")
        };
        out.extend([format!("  The script is protected with {who}. MoonVeil turns a script into a private VM program, so the original"),"  code (its structure, conditions and variable names) cannot be recovered by any tool.".into()]);
    }
    out.extend(["".into(),"  What this file is: the protected script was run once in a sandbox against fake Roblox objects (no network,".into(),"  no files) and every call it made was written down, in order, with the real names, strings and numbers.".into(),"  - Observed: the calls, their order, strings, numbers and the delays between them.".into(),"  - Not observed: conditions, other branches, results of the network and everything downloaded at run time.".into(),"    Such places are marked UNKNOWN. Loops were cut after 40 waits; real runs go on longer.".into(),"  - Threads and callbacks ran one after another, so the real timing between them is not preserved.".into(),"  - Local names (Players, gui, ...) were chosen by this tool. Not tested in a real game.".into(),"".into()]);
    fn wrap(out: &mut Vec<String>, label: &str, items: Vec<String>) {
        if items.is_empty() {
            return;
        }
        out.push(format!("  - {label}:"));
        for item in items {
            out.push(format!(
                "      {}",
                crate::tracer::vis(&item)
                    .chars()
                    .take(100)
                    .collect::<String>()
            ));
        }
    }
    let strings = |key: &str| {
        summ[key]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    wrap(
        &mut out,
        "Downloads and runs other code (NOT fetched, so not included)",
        strings("downloads"),
    );
    wrap(
        &mut out,
        "Remote events it fires",
        summ["remotes"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|r| {
                let key = r[0].as_str().unwrap_or("");
                let n = r[1].as_u64().unwrap_or(1);
                if n > 1 {
                    format!("{key} ({n} x)")
                } else {
                    key.into()
                }
            })
            .collect(),
    );
    wrap(&mut out, "Copies to the clipboard", strings("clipboard"));
    for (key, label, cap) in [
        ("executor", "Executor-only functions used", usize::MAX),
        (
            "unset",
            "Names it reads but never defines (probably set by the downloaded code)",
            12,
        ),
    ] {
        let items = strings(key).into_iter().take(cap).collect::<Vec<_>>();
        if !items.is_empty() {
            wrap(&mut out, label, vec![items.join(", ")]);
        }
    }
    let clipboard = strings("clipboard");
    wrap(
        &mut out,
        "Other links in strings",
        strings("links")
            .into_iter()
            .filter(|s| !clipboard.contains(s))
            .take(6)
            .collect(),
    );
    out.extend([
        format!("  - Seen: {calls} calls in {threads} threads/callbacks."),
        "]]".into(),
        "".into(),
        "".into(),
    ]);
    out.join("\n")
}

pub fn deobfuscate(data: &[u8], detected: &Value, opts: &Options) -> Result<Output> {
    let start = Instant::now();
    let log = run_script(
        data,
        Duration::from_secs(opts.sandbox_timeout),
        opts.sandbox_mem_mb,
    )?;
    let trace_seconds = start.elapsed().as_secs_f64();
    let trace = parse(&log);
    let main = trace.main.ok_or_else(|| {
        Error::new(
            "unsupported",
            "the sandbox produced no trace for this script",
        )
    })?;
    let calls = trace
        .bodies
        .iter()
        .flat_map(|b| &b.events)
        .filter(|e| ["C", "S"].contains(&e[0].as_str()))
        .count();
    let status = trace.bodies[main].status.clone();
    if calls == 0 {
        let why = status
            .strip_prefix("err:")
            .unwrap_or("it did not call anything");
        return Err(Error::new(
            "unsupported",
            format!(
                "the MoonVeil script did nothing observable in the sandbox ({})",
                crate::messages::scrub(why)
            ),
        ));
    }
    let threads = trace
        .bodies
        .iter()
        .map(|b| &b.bid)
        .collect::<HashSet<_>>()
        .len();
    let capped = trace.tags.contains("cap");
    let mut renderer = Renderer::new(trace);
    let body = renderer.render();
    let summary = renderer.summary();
    let text = header(detected, &summary, calls, threads) + body.trim_end() + "\n";
    let mut warnings = vec![];
    if capped {
        warnings.push("the script made so many calls that the trace was cut (the end of the script is missing)".to_owned());
    }
    if let Some(error) = status.strip_prefix("err:") {
        warnings.push(format!("the script stopped with an error in the sandbox ({}); what came after is not in the trace",crate::messages::scrub(error)));
    } else if status == "cut" {
        warnings.push("the script was still waiting when the trace was stopped".into());
    }
    let mut info = json!({"kind":"moonveil","trace_only":true,"moonveil":detected,"lines":text.bytes().filter(|b|*b==b'\n').count(),"calls":calls,"threads":threads,"downloads":summary["downloads"],"warnings":warnings});
    if opts.verify {
        let checked = crate::engine::compile_check(&text)?;
        if checked["ok"] == true {
            info["syntax_ok"] = json!(true);
        } else if checked["ok"] == false {
            info["warnings"].as_array_mut().unwrap().push(json!(format!(
                "the reconstruction has a syntax problem: {}",
                crate::messages::scrub(checked["message"].as_str().unwrap_or(""))
            )));
        }
    }
    info["timings"] = json!({"trace":(trace_seconds*100.0).round()/100.0,"total":(start.elapsed().as_secs_f64()*100.0).round()/100.0});
    Ok(Output {
        text: Some(text),
        info,
        trace: None,
        raw: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn render(log: &str) -> String {
        Renderer::new(parse(&log.replace("\\t", "\t"))).render()
    }
    #[test]
    fn single_use_values_inline_and_reused_values_get_names() {
        let out=render("B\\tM\\tkind=main\nC\\t1\\tgame:GetService\\t\"Players\"\nC\\t2\\t@R1.LocalPlayer:WaitForChild\\t\"PlayerGui\"\nC\\t3\\t@R2:FindFirstChild\\t\"X\"\nC\\t4\\t@R3:Destroy\\t\nC\\t5\\t@R2:FindFirstChild\\t\"Y\"\nE\\tM\\tok");
        assert_eq!(out,"local playerGui = game:GetService(\"Players\").LocalPlayer:WaitForChild(\"PlayerGui\")\nplayerGui:FindFirstChild(\"X\"):Destroy()\nplayerGui:FindFirstChild(\"Y\")");
    }
    #[test]
    fn callback_self_reference_is_declared_before_assignment() {
        let out=render("B\\tM\\tkind=main\nC\\t1\\tgame:GetService(\"RunService\").Heartbeat:Connect\\t@F1\nC\\t2\\t@R1:Disconnect\\t\nE\\tM\\tok\nB\\tF1\\tkind=frame;param=dt;dist=near\nC\\t3\\t@R1:Disconnect\\t\nE\\tF1\\tok");
        assert!(out.starts_with("local connection\nconnection = game:GetService(\"RunService\").Heartbeat:Connect(function(dt)"));
        assert!(out.contains("\tconnection:Disconnect()\nend)"));
    }
    #[test]
    fn loops_and_unobserved_work_have_explicit_labels() {
        assert_eq!(render("B\\tM\\tkind=main\nC\\t1\\tworkspace:GetChildren\\t\nI\\t1\\t@R1\nX\\t1\nE\\tM\\tok"),"for _, child in ipairs(workspace:GetChildren()) do\n\t-- UNKNOWN: nothing observable was done with each item\nend");
        assert!(
            render("B\\tM\\tkind=main\nC\\t1\\tprint\\t\"a\"\nE\\tM\\tcut").ends_with(
                "-- (trace cut here: the sandbox stopped waiting; the real script continues)"
            )
        );
        let texts = vec!["ping()".to_owned(); 40];
        assert!(Renderer::collapse(texts, true)[0]
            .starts_with("while true do -- UNKNOWN stop condition (the trace saw 40 rounds"));
    }
}

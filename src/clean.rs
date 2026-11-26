//! Bounded evaluation of closed, harmless protector expressions and sampled predicates.
use crate::astutil::parse_ast;
use crate::error::Result;
use crate::sandbox::run_luau;
use crate::source_cleanup::{arr, id, kind, lkey, lua_str, text, walk, Bindings, Src};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

const CLOCK_VALUE: &[f64] = &[0.37, 54321.5];
const CLOCK_PREDICATE: &[f64] = &[
    0.0,
    0.0004,
    0.5,
    1.0,
    1.999,
    7.77,
    100.25,
    4096.0,
    12345.678,
    999999.999,
    3000000.5,
    8000000.125,
];
const LITERALS: &[&str] = &[
    "AstExprConstantNil",
    "AstExprConstantBool",
    "AstExprConstantNumber",
    "AstExprConstantString",
];
const VALUE_TYPES: &[&str] = &[
    "AstExprCall",
    "AstExprBinary",
    "AstExprUnary",
    "AstExprGroup",
    "AstExprIfElse",
    "AstExprIndexExpr",
];
fn safe_func(name: &str) -> bool {
    [
        "type", "tostring", "tonumber", "select", "ipairs", "pairs", "next", "rawequal", "rawlen",
    ]
    .contains(&name)
}
fn safe_member(base: &str, name: &str) -> bool {
    let members=match base{
    "buffer"=>"create fromstring tostring len copy fill readstring writestring readu8 readu16 readu32 readi8 readi16 readi32 readf32 readf64 writeu8 writeu16 writeu32 writei8 writei16 writei32 writef32 writef64",
    "bit32"=>"band bor bxor bnot lshift rshift arshift lrotate rrotate extract replace countlz countrz byteswap",
    "math"=>"floor ceil abs max min sqrt fmod pow sign clamp round huge pi exp log log10 sin cos tan modf",
    "string"=>"char byte sub len reverse rep lower upper format","table"=>"concat unpack pack","os"=>"clock",_=>""};
    members.split_whitespace().any(|m| m == name)
}
#[derive(Clone, Default)]
struct Info {
    ok: bool,
    clock: bool,
    ops: usize,
    decls: HashSet<String>,
    refs: HashSet<String>,
    obf: bool,
}
impl Info {
    fn good() -> Self {
        Self {
            ok: true,
            ..Self::default()
        }
    }
    fn closed(&self) -> bool {
        self.ok && self.refs.is_subset(&self.decls)
    }
    fn join(infos: Vec<Info>, ops: usize, decls: HashSet<String>, obf: bool) -> Self {
        let mut out = Self {
            ops,
            decls,
            obf,
            ..Self::good()
        };
        for i in infos {
            out.ok &= i.ok;
            out.clock |= i.clock;
            out.obf |= i.obf;
            out.ops += i.ops;
            out.decls.extend(i.decls);
            out.refs.extend(i.refs);
        }
        out
    }
}
fn kids_of(n: &Value) -> Vec<&Value> {
    let mut out = vec![];
    if let Some(obj) = n.as_object() {
        for v in obj.values() {
            if v.get("type").is_some() && kind(v) != "AstLocal" {
                out.push(v);
            } else if let Some(xs) = v.as_array() {
                for x in xs {
                    if x.get("type").is_some() && kind(x) != "AstLocal" {
                        out.push(x);
                    }
                }
            }
        }
    }
    out
}
struct Analyzer<'a> {
    info: HashMap<usize, Info>,
    bindings: &'a Bindings<'a>,
}
impl Analyzer<'_> {
    fn visit(&mut self, n: &Value) -> Info {
        if n.is_null() {
            return Info::good();
        }
        let i = self.compute(n);
        self.info.insert(id(n), i.clone());
        i
    }
    fn compute(&mut self, n: &Value) -> Info {
        let empty = HashSet::new();
        match kind(n) {
            k if LITERALS.contains(&k) => Info::good(),
            "AstExprGroup" => self.visit(&n["expr"]),
            "AstExprLocal" => Info {
                refs: HashSet::from([lkey(&n["local"])]),
                ..Info::good()
            },
            "AstExprGlobal" | "AstExprVarargs" => Info::default(),
            "AstExprIndexName" => {
                let base = &n["expr"];
                if kind(base) == "AstExprGlobal" {
                    self.info.insert(id(base), Info::default());
                    let namespace = text(base, "global");
                    if safe_member(namespace, text(n, "index"))
                        && !self.bindings.global_writes.contains(namespace)
                    {
                        Info {
                            clock: namespace == "os",
                            ops: 1,
                            obf: ["bit32", "buffer"].contains(&namespace),
                            ..Info::good()
                        }
                    } else {
                        Info::default()
                    }
                } else {
                    Info::join(vec![self.visit(base)], 1, empty, false)
                }
            }
            "AstExprIndexExpr" => Info::join(
                vec![self.visit(&n["expr"]), self.visit(&n["index"])],
                1,
                empty,
                false,
            ),
            "AstExprCall" => {
                let f = &n["func"];
                let mut infos = vec![];
                if kind(f) == "AstExprGlobal" {
                    self.info.insert(id(f), Info::default());
                    if !safe_func(text(f, "global"))
                        || self.bindings.global_writes.contains(text(f, "global"))
                    {
                        infos.push(Info::default());
                    }
                } else {
                    infos.push(self.visit(f));
                }
                for a in arr(n, "args") {
                    infos.push(self.visit(a));
                }
                let iife = kind(f) == "AstExprFunction"
                    || kind(f) == "AstExprGroup" && kind(&f["expr"]) == "AstExprFunction";
                Info::join(infos, 1, empty, iife)
            }
            "AstExprFunction" => Info::join(
                vec![self.visit(&n["body"])],
                1,
                arr(n, "args").iter().map(lkey).collect(),
                false,
            ),
            "AstExprTable" => {
                let mut infos = vec![];
                for it in arr(n, "items") {
                    self.info.insert(id(it), Info::good());
                    if !it["key"].is_null() {
                        infos.push(self.visit(&it["key"]));
                    }
                    infos.push(self.visit(&it["value"]));
                }
                Info::join(infos, 0, empty, false)
            }
            "AstExprUnary" => {
                let e = self.visit(&n["expr"]);
                if !["Not", "Minus", "Len"].contains(&text(n, "op")) {
                    return Info::default();
                }
                let lit = text(n, "op") == "Minus" && kind(&n["expr"]) == "AstExprConstantNumber";
                Info::join(vec![e], if lit { 0 } else { 1 }, empty, false)
            }
            "AstExprBinary" => Info::join(
                vec![self.visit(&n["left"]), self.visit(&n["right"])],
                1,
                empty,
                false,
            ),
            "AstExprIfElse" => Info::join(
                vec![
                    self.visit(&n["condition"]),
                    self.visit(&n["trueExpr"]),
                    self.visit(&n["falseExpr"]),
                ],
                1,
                empty,
                false,
            ),
            "AstStatBlock" => {
                let infos = arr(n, "body").iter().map(|s| self.visit(s)).collect();
                Info::join(infos, 0, empty, false)
            }
            "AstStatLocal" => {
                let infos = arr(n, "values").iter().map(|s| self.visit(s)).collect();
                Info::join(infos, 0, arr(n, "vars").iter().map(lkey).collect(), false)
            }
            "AstStatLocalFunction" => Info::join(
                vec![self.visit(&n["func"])],
                0,
                HashSet::from([lkey(&n["name"])]),
                false,
            ),
            "AstStatAssign" | "AstStatCompoundAssign" => {
                let targets = if kind(n) == "AstStatAssign" {
                    arr(n, "vars").iter().collect::<Vec<_>>()
                } else {
                    vec![&n["var"]]
                };
                let values = if kind(n) == "AstStatAssign" {
                    arr(n, "values").iter().collect::<Vec<_>>()
                } else {
                    vec![&n["value"]]
                };
                let mut infos = vec![];
                for x in targets {
                    infos.push(self.visit(x));
                    let mut root = x;
                    while matches!(
                        kind(root),
                        "AstExprIndexName" | "AstExprIndexExpr" | "AstExprGroup"
                    ) {
                        root = &root["expr"];
                    }
                    if kind(root) != "AstExprLocal" {
                        infos.push(Info::default());
                    }
                }
                for v in values {
                    infos.push(self.visit(v));
                }
                Info::join(infos, 1, empty, false)
            }
            "AstStatIf" => Info::join(
                vec![
                    self.visit(&n["condition"]),
                    self.visit(&n["thenbody"]),
                    self.visit(&n["elsebody"]),
                ],
                0,
                empty,
                false,
            ),
            "AstStatWhile" | "AstStatRepeat" => Info::join(
                vec![self.visit(&n["condition"]), self.visit(&n["body"])],
                0,
                empty,
                false,
            ),
            "AstStatFor" => Info::join(
                vec![
                    self.visit(&n["from"]),
                    self.visit(&n["to"]),
                    self.visit(&n["step"]),
                    self.visit(&n["body"]),
                ],
                0,
                HashSet::from([lkey(&n["var"])]),
                false,
            ),
            "AstStatForIn" => {
                let mut infos = arr(n, "values")
                    .iter()
                    .map(|v| self.visit(v))
                    .collect::<Vec<_>>();
                infos.push(self.visit(&n["body"]));
                Info::join(infos, 0, arr(n, "vars").iter().map(lkey).collect(), false)
            }
            "AstStatReturn" => {
                let infos = arr(n, "list").iter().map(|v| self.visit(v)).collect();
                Info::join(infos, 0, empty, false)
            }
            "AstStatBreak" | "AstStatContinue" => Info::good(),
            "AstStatExpr" => self.visit(&n["expr"]),
            _ => {
                for child in kids_of(n) {
                    self.visit(child);
                }
                Info::default()
            }
        }
    }
}
const HARNESS: &str = r#"local os = {clock = function() return CLK end}
local W = {}
local function hx(s)
 local t = {}
 for i = 1, #s do t[i] = string.format("%02x", string.byte(s,i)) end
 return table.concat(t)
end
local function put(i,ok,...)
 local n=select('#',...)
 local v=...
 if not ok or n~=1 then W[#W+1]=i..'\tE'
 elseif type(v)=='string' then W[#W+1]=i..'\tS\t'..hx(v)
 elseif type(v)=='number' then W[#W+1]=i..'\tN\t'..string.format('%.17g',v)
 elseif type(v)=='boolean' then W[#W+1]=i..'\tB\t'..tostring(v)
 elseif v==nil then W[#W+1]=i..'\tZ'
 else W[#W+1]=i..'\tE' end
end
local function truth(i,ok,v)
 if not ok then W[#W+1]=i..'\tE' else W[#W+1]=i..'\t'..((v and 'T') or 'F') end
end
"#;
#[derive(Clone, Debug, PartialEq)]
enum Literal {
    String(Vec<u8>),
    Number(f64),
    Bool(bool),
    Nil,
    Truth(bool),
}
pub(crate) fn decode_result(result: &str) -> Option<DecodedResult> {
    let parts: Vec<_> = result.split('\t').collect();
    let lit = match *parts.first()? {
        "S" => {
            let hex = parts.get(1).copied().unwrap_or("");
            if hex.len() % 2 != 0 || hex.len() > 400000 || !hex.is_ascii() {
                return None;
            }
            let bytes = (0..hex.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
                .collect::<Option<Vec<_>>>()?;
            Literal::String(bytes)
        }
        "N" => {
            let value = parts.get(1)?.parse::<f64>().ok()?;
            if !value.is_finite() {
                return None;
            }
            Literal::Number(value)
        }
        "B" => Literal::Bool(*parts.get(1)? == "true"),
        "Z" => Literal::Nil,
        "T" => Literal::Truth(true),
        "F" => Literal::Truth(false),
        _ => return None,
    };
    Some(DecodedResult(lit))
}
pub(crate) struct DecodedResult(Literal);
fn parse_batch_results(out: &str) -> HashMap<usize, String> {
    let mut res = HashMap::new();
    for line in out.lines() {
        let Some((index, r)) = line.trim_end_matches('\r').split_once('\t') else {
            continue;
        };
        if let Ok(i) = index.parse() {
            res.insert(i, r.to_owned());
        }
    }
    res
}
struct Evaluator {
    timeout: Duration,
    mem_mb: u64,
    spent: Duration,
    cache: HashMap<(bool, String), Option<Literal>>,
}
impl Evaluator {
    fn batch(
        &self,
        items: &[(usize, bool, &str)],
        clock: f64,
        timeout: Duration,
    ) -> Result<HashMap<usize, String>> {
        let mut source = format!("local CLK={clock:?}\n{HARNESS}");
        for (i, truth, expr) in items {
            if *truth {
                source.push_str(&format!(
                    "truth({i},pcall(function() return ({expr}) end))\n"
                ));
            } else {
                source.push_str(&format!("put({i},pcall(function() return {expr} end))\n"));
            }
        }
        source.push_str("print(table.concat(W,'\\n'))\n");
        let bytes = run_luau(source.as_bytes(), timeout, self.mem_mb, 128)?;
        let out = String::from_utf8_lossy(&bytes);
        Ok(parse_batch_results(&out))
    }
    fn evaluate(&mut self, wants: &[(bool, String)]) -> Vec<Option<Literal>> {
        let mut seen = HashSet::new();
        let keys: Vec<_> = wants
            .iter()
            .filter(|k| !self.cache.contains_key(*k) && seen.insert((*k).clone()))
            .cloned()
            .collect();
        for truth in [false, true] {
            let group: Vec<_> = keys.iter().filter(|k| k.0 == truth).collect();
            let samples = if truth { CLOCK_PREDICATE } else { CLOCK_VALUE };
            let mut per_sample = vec![];
            for &clock in samples {
                let mut got = HashMap::new();
                for (start, chunk) in group.chunks(40).enumerate() {
                    let items: Vec<_> = chunk
                        .iter()
                        .enumerate()
                        .map(|(i, k)| (start * 40 + i, truth, k.1.as_str()))
                        .collect();
                    let t0 = Instant::now();
                    let batch = if self.spent < Duration::from_secs(90) {
                        self.batch(&items, clock, self.timeout).ok()
                    } else {
                        None
                    };
                    if let Some(res) = batch {
                        got.extend(res);
                    } else {
                        for item in &items {
                            if self.spent + t0.elapsed() > Duration::from_secs(90) {
                                break;
                            }
                            if let Ok(res) = self.batch(
                                &[*item],
                                clock,
                                Duration::from_secs(5).min(self.timeout),
                            ) {
                                got.extend(res);
                            }
                        }
                    }
                    self.spent += t0.elapsed();
                }
                per_sample.push(got);
            }
            for (i, key) in group.iter().enumerate() {
                let values: Vec<_> = per_sample
                    .iter()
                    .map(|g| g.get(&i).and_then(|s| decode_result(s)).map(|r| r.0))
                    .collect();
                let v = values
                    .first()
                    .cloned()
                    .flatten()
                    .filter(|v| values.iter().all(|x| x.as_ref() == Some(v)));
                self.cache.insert((*key).clone(), v);
            }
        }
        wants
            .iter()
            .map(|k| self.cache.get(k).cloned().flatten())
            .collect()
    }
}
fn literal_text(v: &Literal) -> String {
    match v {
        Literal::String(s) => lua_str(s),
        Literal::Number(n) => {
            if n.fract() == 0.0 && n.abs() < 1e15 {
                format!("{n:.0}")
            } else {
                n.to_string()
            }
        }
        Literal::Bool(v) => v.to_string(),
        _ => "nil".to_string(),
    }
}
fn wrap_literal(s: String, v: &Literal, ctx: &str) -> String {
    if ctx == "recv"
        || ["bin", "un"].contains(&ctx) && matches!(v, Literal::Number(_)) && s.starts_with('-')
        || ctx == "cat" && matches!(v, Literal::Number(_))
    {
        format!("({s})")
    } else {
        s
    }
}
struct Arm<'a> {
    condition: &'a Value,
    body: &'a Value,
    static_: Option<bool>,
    evaluation: Option<String>,
}
struct Chain<'a> {
    node: &'a Value,
    arms: Vec<Arm<'a>>,
    else_body: Option<&'a Value>,
    block: Option<&'a Value>,
    index: usize,
}
struct Collector<'a> {
    info: &'a HashMap<usize, Info>,
    src: &'a Src<'a>,
    values: Vec<(&'a Value, &'static str)>,
    chains: Vec<Chain<'a>>,
}
impl<'a> Collector<'a> {
    fn block(&mut self, block: &'a Value) {
        for (i, st) in arr(block, "body").iter().enumerate() {
            self.stmt(st, Some(block), i);
        }
    }
    fn stmt(&mut self, st: &'a Value, block: Option<&'a Value>, index: usize) {
        if kind(st) == "AstStatIf" {
            let mut arms = vec![];
            let mut current = st;
            let else_body = loop {
                let cond = &current["condition"];
                let (static_, evaluation) = match kind(cond) {
                    "AstExprConstantBool" => (cond["value"].as_bool(), None),
                    "AstExprConstantNil" => (Some(false), None),
                    "AstExprConstantNumber" | "AstExprConstantString" => (Some(true), None),
                    _ => {
                        let e = self
                            .info
                            .get(&id(cond))
                            .filter(|i| i.closed() && (i.clock || i.obf))
                            .and_then(|_| self.src.slice(cond).ok())
                            .map(str::to_owned);
                        (None, e)
                    }
                };
                arms.push(Arm {
                    condition: cond,
                    body: &current["thenbody"],
                    static_,
                    evaluation,
                });
                let e = &current["elsebody"];
                if kind(e) == "AstStatIf" {
                    current = e;
                } else {
                    break (!e.is_null()).then_some(e);
                }
            };
            let has_chain = arms
                .iter()
                .any(|a| a.static_.is_some() || a.evaluation.is_some());
            for a in &arms {
                self.expr(a.condition, "free", has_chain);
                self.block(a.body);
            }
            if let Some(e) = else_body {
                self.block(e);
            }
            if has_chain {
                self.chains.push(Chain {
                    node: st,
                    arms,
                    else_body,
                    block,
                    index,
                });
            }
            return;
        }
        if kind(st) == "AstStatBlock" {
            self.block(st);
            return;
        }
        for child in kids_of(st) {
            self.any(child, "free");
        }
    }
    fn any(&mut self, n: &'a Value, ctx: &'static str) {
        if kind(n) == "AstStatBlock" {
            self.block(n);
        } else if kind(n).starts_with("AstStat") {
            self.stmt(n, None, 0);
        } else if kind(n) != "AstLocal" {
            self.expr(n, ctx, false);
        }
    }
    fn expr(&mut self, n: &'a Value, ctx: &'static str, skip: bool) {
        let t = kind(n);
        let mut inner = n;
        while kind(inner) == "AstExprGroup" {
            inner = &inner["expr"];
        }
        let trivial = t == "AstExprGroup" && LITERALS.contains(&kind(inner)) && ctx == "recv";
        if !skip
            && VALUE_TYPES.contains(&t)
            && self
                .info
                .get(&id(n))
                .is_some_and(|i| i.closed() && i.ops > 0 && i.obf)
            && !trivial
        {
            self.values.push((n, ctx));
            if t == "AstExprCall" && matches!(kind(&n["func"]), "AstExprFunction" | "AstExprGroup")
            {
                return;
            }
        }
        match t {
            "AstExprFunction" => self.block(&n["body"]),
            "AstExprCall" => {
                self.expr(&n["func"], "recv", false);
                for a in arr(n, "args") {
                    self.expr(a, "free", false);
                }
            }
            "AstExprIndexName" => self.expr(&n["expr"], "recv", false),
            "AstExprIndexExpr" => {
                self.expr(&n["expr"], "recv", false);
                self.expr(&n["index"], "free", false);
            }
            "AstExprBinary" => {
                let c = if text(n, "op") == "Concat" {
                    "cat"
                } else {
                    "bin"
                };
                self.expr(&n["left"], c, false);
                self.expr(&n["right"], c, false);
            }
            "AstExprUnary" => self.expr(&n["expr"], "un", false),
            "AstExprGroup" => self.expr(&n["expr"], ctx, false),
            "AstExprTable" => {
                for it in arr(n, "items") {
                    if !it["key"].is_null() {
                        self.expr(&it["key"], "free", false);
                    }
                    self.expr(&it["value"], "free", false);
                }
            }
            "AstExprIfElse" => {
                self.expr(&n["condition"], "free", false);
                self.expr(&n["trueExpr"], "free", false);
                if !n["falseExpr"].is_null() {
                    self.expr(&n["falseExpr"], "free", false);
                }
            }
            _ => {}
        }
    }
}
type Span = (usize, usize);
enum Build {
    Literal(String, &'static str),
    Chain {
        keep: Vec<(Span, Option<Span>)>,
        final_: Option<Span>,
        wrap: bool,
        indent: String,
        dropped: usize,
    },
}
struct Edit {
    start: usize,
    end: usize,
    build: Build,
}
fn body_lines(body: &Value, header_line: usize, src: &Src<'_>) -> Result<Option<Option<Span>>> {
    let sts = arr(body, "body");
    if sts.is_empty() {
        return Ok(Some(None));
    }
    let [l1, _, _, _] = src.loc(&sts[0])?;
    let [_, _, l2, _] = src.loc(sts.last().unwrap())?;
    let [_, _, closer, _] = src.loc(body)?;
    if l1 <= header_line || l2 >= closer {
        return Ok(None);
    }
    let a = src.starts[l1];
    let b = src.starts.get(l2 + 1).map_or(src.text.len(), |s| s - 1);
    Ok(Some(Some((a, b))))
}
fn names_in(n: &Value) -> HashSet<String> {
    walk(n)
        .into_iter()
        .filter_map(|(x, _)| match kind(x) {
            "AstExprGlobal" => Some(text(x, "global").to_owned()),
            "AstExprLocal" => Some(text(&x["local"], "name").to_owned()),
            "AstLocal" => Some(text(x, "name").to_owned()),
            _ => None,
        })
        .collect()
}
fn chain_edit(chain: &Chain<'_>, src: &Src<'_>) -> Result<Option<Edit>> {
    if chain.arms.iter().all(|a| a.static_.is_none()) {
        return Ok(None);
    }
    let [l1, _, _, _] = src.loc(chain.node)?;
    let (start, mut end) = src.span(chain.node)?;
    let indent = &src.text[src.starts[l1]..start];
    if !indent.trim().is_empty() {
        return Ok(None);
    }
    let eol = src.text[end..]
        .find('\n')
        .map_or(src.text.len(), |i| end + i);
    if !src.text[end..eol].trim().is_empty() {
        return Ok(None);
    }
    let mut keep = vec![];
    let mut always = None;
    let mut final_ = chain.else_body;
    for a in &chain.arms {
        match a.static_ {
            Some(false) => continue,
            Some(true) => {
                always = Some(a);
                final_ = Some(a.body);
                break;
            }
            None => keep.push(a),
        }
    }
    if keep.len() == chain.arms.len() && always.is_none() {
        return Ok(None);
    }
    let mut kept = vec![];
    for a in &keep {
        let [_, _, hl, _] = src.loc(a.condition)?;
        let Some(span) = body_lines(a.body, hl, src)? else {
            return Ok(None);
        };
        kept.push((src.span(a.condition)?, span));
    }
    let final_span = if let Some(body) = final_ {
        let hl = if let Some(a) = always {
            src.loc(a.condition)?[2]
        } else {
            src.loc(body)?[0]
        };
        let Some(span) = body_lines(body, hl, src)? else {
            return Ok(None);
        };
        span
    } else {
        None
    };
    let mut declared = HashSet::new();
    if keep.is_empty() {
        if let Some(block) = final_ {
            for st in arr(block, "body") {
                if kind(st) == "AstStatLocal" {
                    declared.extend(arr(st, "vars").iter().map(|v| text(v, "name").to_owned()));
                } else if kind(st) == "AstStatLocalFunction" {
                    declared.insert(text(&st["name"], "name").to_owned());
                }
            }
        }
    }
    let safe = chain.block.is_some_and(|block| {
        arr(block, "body")[chain.index + 1..]
            .iter()
            .all(|st| declared.is_disjoint(&names_in(st)))
    });
    let wrap = !declared.is_empty() && !safe;
    if keep.is_empty()
        && !wrap
        && final_.is_some_and(|b| {
            arr(b, "body").last().is_some_and(|n| {
                matches!(
                    kind(n),
                    "AstStatReturn" | "AstStatBreak" | "AstStatContinue"
                )
            })
        })
    {
        if let Some(block) = chain.block {
            if let Some(last) = arr(block, "body")
                .get(chain.index + 1..)
                .and_then(|rest| rest.last())
            {
                end = src.span(last)?.1;
                let eol = src.text[end..]
                    .find('\n')
                    .map_or(src.text.len(), |i| end + i);
                if !src.text[end..eol].trim().is_empty() {
                    return Ok(None);
                }
            }
        }
    }
    let mut dropped = chain.arms.len() - keep.len() - usize::from(always.is_some());
    if always.is_some() && chain.else_body.is_some() {
        dropped += 1;
    }
    Ok(Some(Edit {
        start,
        end,
        build: Build::Chain {
            keep: kept,
            final_: final_span,
            wrap,
            indent: indent.to_owned(),
            dropped,
        },
    }))
}
fn inc(stats: &mut Value, key: &str, count: usize) {
    stats[key] = json!(stats[key].as_u64().unwrap_or(0) + count as u64);
}
fn render(source: &str, edits: &[Edit], a: usize, b: usize, stats: &mut Value) -> String {
    let mut out = String::new();
    let mut pos = a;
    for e in edits {
        if e.start < a || e.end > b || e.start < pos {
            continue;
        }
        out.push_str(&source[pos..e.start]);
        match &e.build {
            Build::Literal(s, key) => {
                out.push_str(s);
                inc(stats, key, 1);
            }
            Build::Chain {
                keep,
                final_,
                wrap,
                indent,
                dropped,
            } => {
                inc(stats, "checks", 1);
                inc(stats, "arms", *dropped);
                let body = final_.map(|(a, b)| render(source, edits, a, b, stats));
                if keep.is_empty() {
                    if let Some(body) = body {
                        if *wrap {
                            out.push_str(&format!("do\n{body}\n{indent}end"));
                        } else {
                            out.push_str(&unindent(&body, indent));
                        }
                    } else {
                        inc(stats, "statements", 1);
                    }
                } else {
                    let mut lines = vec![];
                    for (i, (cond, span)) in keep.iter().enumerate() {
                        lines.push(format!(
                            "{}{} then",
                            if i == 0 {
                                "if ".to_owned()
                            } else {
                                format!("{indent}elseif ")
                            },
                            render(source, edits, cond.0, cond.1, stats)
                        ));
                        if let Some((a, b)) = span {
                            lines.push(render(source, edits, *a, *b, stats));
                        }
                    }
                    if let Some(body) = body {
                        lines.push(format!("{indent}else"));
                        lines.push(body);
                    }
                    lines.push(format!("{indent}end"));
                    out.push_str(&lines.join("\n"));
                }
            }
        }
        pos = e.end;
    }
    out.push_str(&source[pos..b]);
    out
}
fn unindent(source: &str, indent: &str) -> String {
    let lines: Vec<_> = source.split('\n').collect();
    let Some(first) = lines.iter().find(|l| !l.trim().is_empty()) else {
        return String::new();
    };
    let first = &first[..first.len() - first.trim_start().len()];
    let unit = first.strip_prefix(indent).unwrap_or("\t");
    let out = lines
        .iter()
        .map(|l| {
            if l.starts_with(first) {
                &l[unit.len()..]
            } else {
                l
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    if out.starts_with(indent) {
        out.trim_start_matches([' ', '\t']).to_owned()
    } else {
        out
    }
}
pub fn opens_block(line: &str) -> bool {
    static OPENER: OnceLock<regex::Regex> = OnceLock::new();
    static END: OnceLock<regex::Regex> = OnceLock::new();
    let line = line.trim_end();
    let opener = OPENER.get_or_init(|| {
        regex::Regex::new(r"(?:\b(?:then|do|else|repeat)|\bfunction\b[^\n]*\))$").unwrap()
    });
    if !opener.is_match(line) {
        return false;
    }
    let tail = line.rfind("function").map(|i| &line[i..]).unwrap_or("");
    tail.is_empty()
        || !END
            .get_or_init(|| regex::Regex::new(r"\bend\b").unwrap())
            .is_match(tail)
}
pub fn plain_statement(line: &str) -> bool {
    static BLOCK_START: OnceLock<regex::Regex> = OnceLock::new();
    let st = line.trim();
    if st.is_empty()||st.starts_with("--")||BLOCK_START.get_or_init(||regex::Regex::new(r"^(?:if|for|while|repeat|do|function|local\s+function|end|else|elseif|until|return|break|continue)\b").unwrap()).is_match(st)||opens_block(st){return false;}
    let (mut depth, mut quote, mut escape) = (0, None, false);
    for c in st.chars() {
        if escape {
            escape = false;
            continue;
        }
        if let Some(q) = quote {
            if c == '\\' {
                escape = true;
            } else if c == q {
                quote = None;
            }
        } else {
            match c {
                '"' | '\'' => quote = Some(c),
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' => depth -= 1,
                _ => {}
            }
        }
    }
    depth == 0
        && quote.is_none()
        && ![",", "=", "and", "or", "..", "+", "-", "*", "/"]
            .iter()
            .any(|s| st.ends_with(s))
}
pub fn tidy(source: &str) -> String {
    if source.contains("[[") || source.contains("[=") {
        return source.to_owned();
    }
    static TRAILING: OnceLock<regex::Regex> = OnceLock::new();
    static BLANKS: OnceLock<regex::Regex> = OnceLock::new();
    static CLOSER: OnceLock<regex::Regex> = OnceLock::new();
    let trim = TRAILING
        .get_or_init(|| regex::Regex::new(r"[ \t]+\n").unwrap())
        .replace_all(source, "\n");
    let text = BLANKS
        .get_or_init(|| regex::Regex::new(r"\n[ \t]*\n(?:[ \t]*\n)+").unwrap())
        .replace_all(&trim, "\n\n")
        .into_owned();
    let closer =
        CLOSER.get_or_init(|| regex::Regex::new(r"^[ \t]*(?:end|else|elseif|until)\b").unwrap());
    let lines: Vec<_> = text.split('\n').collect();
    let mut out: Vec<&str> = vec![];
    for (i, line) in lines.iter().enumerate() {
        if line.trim().is_empty() {
            let prev = out.last().copied().unwrap_or("");
            let next = lines[i + 1..]
                .iter()
                .find(|x| !x.trim().is_empty())
                .copied()
                .unwrap_or("");
            if !prev.trim().is_empty() && opens_block(prev) || closer.is_match(next) {
                continue;
            }
            let indent = |s: &str| s.len() - s.trim_start().len();
            if plain_statement(prev)
                && plain_statement(next)
                && lines.get(i + 1).is_some_and(|s| s.trim() == next.trim())
                && indent(prev) == indent(next)
            {
                continue;
            }
        }
        out.push(line);
    }
    out.join("\n")
}
pub fn clean_source(
    source: &str,
    timeout: Duration,
    mem_mb: u64,
    verified: bool,
) -> Result<(String, Value)> {
    let mut raw = source.to_owned();
    let mut stats =
        json!({"strings":0,"numbers":0,"values":0,"checks":0,"arms":0,"statements":0,"states":0});
    let mut evaluator = Evaluator {
        timeout,
        mem_mb,
        spent: Duration::ZERO,
        cache: HashMap::new(),
    };
    for _ in 0..12 {
        let root = parse_ast(raw.as_bytes())?;
        let (helper, counts) = crate::source_cleanup::cleanup_helpers(&raw, &root)?;
        if helper != raw {
            for (key, count) in counts.as_object().unwrap() {
                inc(&mut stats, key, count.as_u64().unwrap_or(0) as usize);
            }
            raw = helper;
            continue;
        }
        let (flat, states) = crate::unflatten::flatten_dispatchers(&raw, &root)?;
        if flat != raw {
            inc(&mut stats, "states", states);
            raw = flat;
            continue;
        }
        let src = Src::new(&raw);
        let bindings = Bindings::new(&root);
        let mut analyzer = Analyzer {
            info: HashMap::new(),
            bindings: &bindings,
        };
        analyzer.visit(&root);
        let mut collector = Collector {
            info: &analyzer.info,
            src: &src,
            values: vec![],
            chains: vec![],
        };
        collector.block(&root);
        collector.values.truncate(6000);
        let mut wants = vec![];
        for (n, _) in &collector.values {
            wants.push((false, src.slice(n)?.to_owned()));
        }
        let nval = wants.len();
        let mut chain_wants = vec![];
        for (ci, chain) in collector.chains.iter().enumerate() {
            for (ai, arm) in chain.arms.iter().enumerate() {
                if arm.static_.is_none() {
                    if let Some(text) = &arm.evaluation {
                        wants.push((true, text.clone()));
                        chain_wants.push((ci, ai));
                    }
                }
            }
        }
        let results = evaluator.evaluate(&wants);
        let mut picked = vec![];
        for ((n, ctx), res) in collector.values.iter().zip(&results[..nval]) {
            if let Some(res) = res.as_ref().filter(|r| !matches!(r, Literal::Truth(_))) {
                let (a, z) = src.span(n)?;
                picked.push((a, z, *n, *ctx, res));
            }
        }
        picked.sort_by_key(|p| (p.0, std::cmp::Reverse(p.1)));
        let mut edits = vec![];
        let mut end = 0;
        for (a, z, n, ctx, res) in picked {
            if a < end {
                continue;
            }
            end = z;
            let literal = wrap_literal(literal_text(res), res, ctx);
            if raw[a..z] == literal {
                continue;
            }
            let key = if matches!(res, Literal::String(_)) && kind(n) == "AstExprCall" {
                "strings"
            } else if matches!(res, Literal::Number(_)) {
                "numbers"
            } else {
                "values"
            };
            edits.push(Edit {
                start: a,
                end: z,
                build: Build::Literal(literal, key),
            });
        }
        for ((ci, ai), res) in chain_wants.into_iter().zip(&results[nval..]) {
            if let Some(Literal::Truth(value)) = res {
                collector.chains[ci].arms[ai].static_ = Some(*value);
            }
        }
        for chain in &collector.chains {
            if let Some(edit) = chain_edit(chain, &src)? {
                edits.push(edit);
            }
        }
        if edits.is_empty() {
            break;
        }
        edits.sort_by_key(|e| (e.start, std::cmp::Reverse(e.end)));
        let out = render(&raw, &edits, 0, raw.len(), &mut stats);
        if out == raw {
            break;
        }
        raw = out;
    }
    let _ = verified;
    Ok((if raw == source { raw } else { tidy(&raw) }, stats))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn batch_result_streams_preserve_lf_crlf_maps_and_decoded_values() {
        let lf = "0\tT\n1\tF\n2\tZ\n3\tS\t484900ff\n4\tN\t-12.5\n5\tB\ttrue\n6\tB\tfalse\n7\tS\t\n8\tE\n";
        let expected = HashMap::from([
            (0, "T".to_owned()),
            (1, "F".to_owned()),
            (2, "Z".to_owned()),
            (3, "S\t484900ff".to_owned()),
            (4, "N\t-12.5".to_owned()),
            (5, "B\ttrue".to_owned()),
            (6, "B\tfalse".to_owned()),
            (7, "S\t".to_owned()),
            (8, "E".to_owned()),
        ]);
        let expected_decoded = HashMap::from([
            (0, Some(Literal::Truth(true))),
            (1, Some(Literal::Truth(false))),
            (2, Some(Literal::Nil)),
            (3, Some(Literal::String(vec![b'H', b'I', 0, 255]))),
            (4, Some(Literal::Number(-12.5))),
            (5, Some(Literal::Bool(true))),
            (6, Some(Literal::Bool(false))),
            (7, Some(Literal::String(vec![]))),
            (8, None),
        ]);
        let parsed_lf = parse_batch_results(lf);
        let parsed_crlf = parse_batch_results(&lf.replace('\n', "\r\n"));
        assert_eq!(parsed_lf, expected);
        assert_eq!(parsed_crlf, parsed_lf);
        for parsed in [&parsed_lf, &parsed_crlf] {
            let decoded = parsed
                .iter()
                .map(|(&index, result)| (index, decode_result(result).map(|r| r.0)))
                .collect::<HashMap<_, _>>();
            assert_eq!(decoded, expected_decoded);
        }
    }
    #[test]
    fn windows_results_and_nonfinite_values() {
        assert_eq!(decode_result("Z").unwrap().0, Literal::Nil);
        assert_eq!(decode_result("T").unwrap().0, Literal::Truth(true));
        assert!(decode_result("N\tinf").is_none());
        assert!(decode_result("S\tzz").is_none());
        assert_eq!(
            decode_result("S\tff00").unwrap().0,
            Literal::String(vec![255, 0])
        );
    }
    #[test]
    fn printer_blank_lines() {
        assert_eq!(tidy("a()\n\n\n\nb()\n"), "a()\nb()\n");
        assert_eq!(
            tidy("if x then\n\n\ta()\n\nend\n"),
            "if x then\n\ta()\nend\n"
        );
        assert_eq!(tidy("x=[[a\n\n\nb]]\n"), "x=[[a\n\n\nb]]\n");
        assert!(plain_statement("\tfoo(1,\"a)\")"));
        assert!(!plain_statement("\tfoo(function()"));
    }
}

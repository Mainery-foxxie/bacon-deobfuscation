//! Register-independent labelled CFG comparison against caller-supplied Luau compiler text.
use crate::error::{Error, Result};
use crate::model::decode_number;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::OnceLock;

#[path = "fidelity_proofs.rs"]
pub(crate) mod proofs;
pub use proofs::compare_for_source;

/// An external report is evidence only if its strict fields and witnesses can be replayed
/// from the same IR and compiler listing. A source digest is not an attestation.
pub fn semantic_ids(program: &Value, listing: &str, report: &Value) -> Option<BTreeSet<u64>> {
    if report.get("semantic_equivalent").is_none() && report.get("semantic_unresolved").is_none() {
        return None;
    }
    if &proofs::compare_scoped(program, listing, proofs::CANDIDATES).ok()? != report {
        return None;
    }
    fn ids(value: &Value) -> Option<BTreeSet<u64>> {
        let list = value.as_array()?;
        let ids: BTreeSet<_> = list.iter().map(Value::as_u64).collect::<Option<_>>()?;
        (list.len() == ids.len()).then_some(ids)
    }
    let n = report["functions_orig"].as_u64()?;
    let map = report["match"].as_object()?;
    let orig: BTreeSet<u64> = map.keys().map(|s| s.parse().ok()).collect::<Option<_>>()?;
    let targets: BTreeSet<_> = map.values().map(Value::as_u64).collect::<Option<_>>()?;
    let bad = ids(&report["differing"])?;
    let cosmetic = ids(&report["cosmetic"])?;
    let moved = ids(&report["reordered_protos"])?;
    if n == 0
        || n != report["functions_new"].as_u64()?
        || orig.len() != map.len()
        || n != orig.len() as u64
        || targets.len() != orig.len()
        || !bad.is_subset(&orig)
        || !cosmetic.is_subset(&bad)
        || !moved.is_subset(&orig)
        || !bad.is_disjoint(&moved)
        || moved.len() as u64 != report["reordered"].as_u64()?
        || report["identical"]
            .as_u64()?
            .checked_add((bad.len() + moved.len()) as u64)
            != Some(n)
    {
        return None;
    }
    let mut accepted = BTreeSet::new();
    for pair in report["semantic_equivalent"].as_array()? {
        let id = pair["prototype"].as_u64()?;
        if !proofs::CANDIDATES.contains(&id)
            || pair["function"].as_u64() != Some(id)
            || map.get(&id.to_string())?.as_u64() != Some(id)
            || !bad.contains(&id)
            || cosmetic.contains(&id)
            || !accepted.insert(id)
        {
            return None;
        }
        let witness = pair["proofs"].as_array()?;
        let [w] = witness.as_slice() else {
            return None;
        };
        if w["kind"]
            != if id == 216 {
                "false-seed-short-circuit"
            } else {
                "closure-allocation"
            }
            || w["record"].as_u64().is_none()
            || w["compiler_pc"].as_u64().is_none()
            || w["evidence"].as_object()?.is_empty()
        {
            return None;
        }
    }
    let mut unresolved = BTreeSet::new();
    for item in report["semantic_unresolved"].as_array()? {
        let id = item["prototype"].as_u64()?;
        if !proofs::CANDIDATES.contains(&id)
            || item["function"] != *map.get(&id.to_string())?
            || item["reason"].as_str()?.trim().is_empty()
            || !bad.contains(&id)
            || cosmetic.contains(&id)
            || accepted.contains(&id)
            || !unresolved.insert(id)
        {
            return None;
        }
    }
    if bad.difference(&cosmetic).any(|id| {
        proofs::CANDIDATES.contains(id) && !accepted.contains(id) && !unresolved.contains(id)
    }) {
        return None;
    }
    Some(accepted)
}

const NONE_ID: u64 = 4294967295;
const PURE: &[&str] = &[
    "GU", "IMP:", "K:", "LB:", "LN", "GT:", "GT[]", "GTN:", "OP:", "OPR:", "NOT", "UNM", "LEN",
    "CONCAT:",
];
fn fail(message: impl Into<String>) -> Error {
    Error::new("fidelity", message)
}
fn a(v: &Value, k: &str) -> i64 {
    v[k].as_i64()
        .or_else(|| v[k].as_f64().map(|v| v as i64))
        .unwrap_or(0)
}
fn field_id(v: &Value) -> Option<u64> {
    v.as_u64()
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        .filter(|v| *v != NONE_ID)
}
fn arith(name: &str) -> Option<&'static str> {
    match name {
        "ADD" => Some("+"),
        "SUB" => Some("-"),
        "MUL" => Some("*"),
        "DIV" => Some("/"),
        "MOD" => Some("%"),
        "POW" => Some("^"),
        "IDIV" => Some("//"),
        _ => None,
    }
}
pub fn num(v: f64) -> String {
    if v.is_nan() {
        return "nan".into();
    }
    if v.is_infinite() {
        return if v.is_sign_negative() { "-inf" } else { "inf" }.into();
    }
    if v == 0.0 {
        return if v.is_sign_negative() { "-0" } else { "0" }.into();
    }
    let exponent = v.abs().log10().floor() as i32;
    let raw = format!("{v:.11e}");
    let (mant, e) = raw.split_once('e').unwrap();
    let e = e.parse::<i32>().unwrap_or(exponent);
    if !(-4..12).contains(&e) {
        let mant = mant.trim_end_matches('0').trim_end_matches('.');
        format!(
            "{mant}e{}{e:02}",
            if e < 0 { "-" } else { "+" },
            e = e.abs()
        )
    } else {
        let precision = (11 - e).max(0) as usize;
        let fixed = format!("{v:.precision$}");
        if fixed.contains('.') {
            fixed.trim_end_matches('0').trim_end_matches('.').to_owned()
        } else {
            fixed
        }
    }
}
pub fn sstr(bytes: &[u8]) -> String {
    let mut out = String::new();
    for &b in bytes {
        if b < 32 {
            out.push_str(&format!("\\x{b:02X}"));
        } else {
            out.push(b as char);
        }
    }
    out.chars().take(24).collect()
}
fn constant(p: &Value, index: i64) -> Option<&Value> {
    p["consts"].get(index.to_string())
}
fn kname(p: &Value, index: i64) -> String {
    constant(p, index)
        .filter(|c| c[0] == "s")
        .and_then(|c| c[1].as_array())
        .and_then(|v| {
            v.iter()
                .map(|x| x.as_u64().and_then(|n| u8::try_from(n).ok()))
                .collect::<Option<Vec<_>>>()
        })
        .map(|v| sstr(&v))
        .unwrap_or_else(|| "?".into())
}
fn kconst(p: &Value, index: i64) -> Result<String> {
    let Some(c) = constant(p, index) else {
        return Ok("?".into());
    };
    Ok(match c[0].as_str() {
        Some("s") => format!("S{}", kname(p, index)),
        Some("n") => format!("N{}", num(decode_number(&c[1])?)),
        Some("b") => format!(
            "B{}",
            if let Some(v) = c[1].as_bool() {
                if v {
                    "True".into()
                } else {
                    "False".into()
                }
            } else {
                c[1].to_string()
            }
        ),
        _ => "Z".into(),
    })
}
fn import_path(p: &Value, aux: i64) -> String {
    let aux = aux as u32;
    let count = (aux >> 30) as usize;
    [(aux >> 20) & 1023, (aux >> 10) & 1023, aux & 1023]
        .iter()
        .take(count)
        .map(|i| kname(p, *i as i64))
        .collect::<Vec<_>>()
        .join(".")
}
fn otok(p: &Value, name: &str, v: &Value) -> Result<Option<String>> {
    let (b, c, rf, aux) = (a(v, "B"), a(v, "C"), a(v, "rf"), a(v, "aux"));
    let tok = match name {
        "MOVE" | "CLOSEUPVALS" | "NOP" | "JUNK" => return Ok(None),
        "GETIMPORT" => {
            let path = import_path(p, aux);
            if path == "pcall" {
                return Ok(None);
            }
            format!("IMP:{path}")
        }
        "GETTABLEKS" => format!("GT:{}", kname(p, aux)),
        "SETTABLEKS" => format!("ST:{}", kname(p, aux)),
        "SELF" => format!("NC:{}", kname(p, aux)),
        "GETTABLE" => "GT[]".into(),
        "SETTABLE" => "ST[]".into(),
        "GETTABLEN" => format!("GTN:{}", c + 1),
        "SETTABLEN" => format!("STN:{}", c + 1),
        "LOADK" => format!("K:{}", kconst(p, rf)?),
        "LOADN" => format!("K:N{}", num(v["rf"].as_f64().unwrap_or(0.0))),
        "LOADB" => format!("LB:{b}"),
        "LOADNIL" => "LN".into(),
        "GETUPVAL" => "GU".into(),
        "SETUPVAL" => "SU".into(),
        "CALL" => format!("CALL:{}:{}", b - 1, c - 1),
        "CONCAT" => format!("CONCAT:{}", c - b + 1),
        "NOT" => "NOT".into(),
        "MINUS" => "UNM".into(),
        "LENGTH" => "LEN".into(),
        "OR" => "OR".into(),
        "AND" => "AND".into(),
        "ORK" => format!("ORK:{}", kconst(p, c)?),
        "ANDK" => format!("ANDK:{}", kconst(p, c)?),
        "NEWTABLE" => "NEWT".into(),
        "DUPTABLE" => "DUPT".into(),
        "SETLIST" => format!("SETLIST:{}", c - 1),
        "NEWCLOSURE" => "NCLO".into(),
        "DUPCLOSURE" => "DCLO".into(),
        "CAPTURE" => format!("CAP:{}", a(v, "A")),
        "FORGPREP" => "FORGPREP".into(),
        "GETVARARGS" => format!("GVA:{}", b - 1),
        _ => {
            if let Some(op) = arith(name) {
                format!("OP:{op}")
            } else if let Some(op) = name.strip_suffix('K').and_then(arith) {
                format!("OP:{op}:K:{}", kconst(p, c)?)
            } else if let Some(op) = name.strip_suffix("RK").and_then(arith) {
                format!("OPR:{op}:K:{}", kconst(p, b)?)
            } else {
                format!("UNK:{name}")
            }
        }
    };
    Ok(Some(tok))
}
fn ctrl_token(p: &Value, name: &str, v: &Value) -> Result<String> {
    let aux = a(v, "aux");
    let inv = if (aux >> 31) & 1 != 0 { "!" } else { "" };
    Ok(match name {
        "RETURN" => format!("RET:{}", a(v, "B") - 1),
        "JUMPIF" => "JIF".into(),
        "JUMPIFNOT" => "JIFN".into(),
        "JUMPIFEQ" => "JEQ".into(),
        "JUMPIFNOTEQ" => "JNEQ".into(),
        "JUMPIFLE" => "JLE".into(),
        "JUMPIFNOTLE" => "JNLE".into(),
        "JUMPIFLT" => "JLT".into(),
        "JUMPIFNOTLT" => "JNLT".into(),
        "JUMPXEQK" => format!("JXK:{}{inv}", kconst(p, aux & 0xffffff)?),
        "JUMPXEQKB" => format!("JXB:{}{inv}", aux & 1),
        "JUMPXEQKNIL" => format!("JXNIL{inv}"),
        "FORNPREP" => "FORNPREP".into(),
        "FORNLOOP" => "FORNLOOP".into(),
        "FORGLOOP" => format!("FORGLOOP:{}", aux & 255),
        _ => format!("UNKCTRL:{name}"),
    })
}
#[derive(Clone, Debug)]
struct Node {
    tok: Option<String>,
    next: Option<usize>,
    alt: Option<usize>,
    cond: bool,
}
#[derive(Clone, Debug)]
struct Graph {
    nodes: Vec<Node>,
    entry: Option<usize>,
}
fn ins(v: &Value) -> Result<(&str, &Value)> {
    let name = v[0]
        .as_str()
        .ok_or_else(|| fail("malformed instruction name"))?;
    if !v[1].is_object() {
        return Err(fail("malformed instruction operands"));
    }
    Ok((name, &v[1]))
}
fn orig_nodes(p: &Value, dead: &HashSet<(u64, usize)>) -> Result<Graph> {
    let records = p["decoded"]
        .as_object()
        .ok_or_else(|| fail("prototype is missing decoded records"))?;
    let mut nodes = vec![];
    let mut starts = HashMap::new();
    let mut edges = vec![];
    for (key, d) in records {
        let rid = key
            .parse::<u64>()
            .map_err(|_| fail("invalid decoded record id"))?;
        starts.insert(rid, nodes.len());
        let mut toks = vec![];
        let instructions = d["ins"]
            .as_array()
            .ok_or_else(|| fail("record has no instruction list"))?;
        for (ii, item) in instructions.iter().enumerate() {
            if dead.contains(&(rid, ii)) {
                continue;
            }
            let (name, v) = ins(item)?;
            if let Some(t) = otok(p, name, v)? {
                toks.push(t);
            }
        }
        let ctrl = &d["ctrl"];
        let mut edge = "seq";
        if !ctrl.is_null() {
            let (name, v) = ins(ctrl)?;
            edge = match name {
                "JUMP" => "jump",
                "RETURN" => "ret",
                _ => "cond",
            };
            if name != "JUMP" {
                toks.push(ctrl_token(p, name, v)?);
            }
        }
        let length = toks.len().max(1);
        for (i, tok) in if toks.is_empty() {
            vec![None]
        } else {
            toks.into_iter().map(Some).collect()
        }
        .into_iter()
        .enumerate()
        {
            let index = nodes.len();
            nodes.push(Node {
                tok,
                next: if i + 1 < length {
                    Some(index + 1)
                } else {
                    None
                },
                alt: None,
                cond: false,
            });
            if i + 1 == length {
                edges.push((index, edge, field_id(&d["nxt"]), field_id(&d["alt"])));
            }
        }
    }
    for (index, edge, next, alt) in edges {
        let next = next.and_then(|n| starts.get(&n).copied());
        let alt = alt.and_then(|n| starts.get(&n).copied());
        match edge {
            "ret" => {}
            "jump" => nodes[index].next = alt,
            "cond" => {
                nodes[index].next = next;
                nodes[index].alt = alt;
            }
            _ => nodes[index].next = next,
        }
    }
    let entry = field_id(&p["entry"]).and_then(|n| starts.get(&n).copied());
    if entry.is_none() {
        return Err(fail("prototype entry does not name a decoded record"));
    }
    Ok(Graph { nodes, entry })
}
pub fn ann(line: &str) -> Option<String> {
    static TRAILING: OnceLock<regex::Regex> = OnceLock::new();
    static REVERSE: OnceLock<regex::Regex> = OnceLock::new();
    static FIRST: OnceLock<regex::Regex> = OnceLock::new();
    let trailing = TRAILING
        .get_or_init(|| regex::Regex::new(r"\[(.*)\]\s*$").unwrap())
        .captures(line)
        .map(|c| c[1].to_owned());
    let reverse = REVERSE
        .get_or_init(|| regex::Regex::new(r"^(?:L\d+: )?(SUBRK|DIVRK)\b").unwrap())
        .is_match(line);
    if trailing.is_none() || reverse {
        if let Some(c) = FIRST
            .get_or_init(|| regex::Regex::new(r"\[(.*?)\]").unwrap())
            .captures(line)
        {
            return Some(c[1].to_owned());
        }
    }
    trailing
}
pub fn unq(s: Option<&str>) -> String {
    let Some(s) = s else {
        return "?".into();
    };
    if s.starts_with('\'') {
        let last = if s.ends_with("'...") {
            s.len() - 4
        } else if s.ends_with('\'') {
            s.len() - 1
        } else {
            return s.to_owned();
        };
        return s[1..last].chars().take(24).collect();
    }
    s.to_owned()
}
fn parse_luau(text: &str) -> Result<BTreeMap<u64, Vec<String>>> {
    let mut functions = BTreeMap::new();
    let mut current = None;
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(rest) = line.strip_prefix("Function ") {
            let Some(id) = rest
                .split_whitespace()
                .next()
                .and_then(|v| v.parse::<u64>().ok())
            else {
                return Err(fail("invalid compiler function header"));
            };
            if functions.insert(id, vec![]).is_some() {
                return Err(fail("duplicate compiler function id"));
            }
            current = Some(id);
            continue;
        }
        if let Some(id) = current {
            if line.is_empty()
                || line.starts_with("REMARK")
                || line.starts_with("slot ")
                || line.strip_prefix('K').is_some_and(|s| {
                    s.split_once(':').is_some_and(|(n, _)| {
                        !n.is_empty() && n.chars().all(|c| c.is_ascii_digit())
                    })
                })
            {
                continue;
            }
            functions.get_mut(&id).unwrap().push(line.to_owned());
        }
    }
    Ok(functions)
}
fn compiler_nodes(lines: &[String]) -> Result<Graph> {
    static INSTRUCTION: OnceLock<regex::Regex> = OnceLock::new();
    let regex = INSTRUCTION
        .get_or_init(|| regex::Regex::new(r"^(?:(L\d+): )?([A-Z_0-9]+)\b ?(.*)$").unwrap());
    let mut insts = vec![];
    let mut labels = HashMap::new();
    for line in lines {
        let captures = regex
            .captures(line)
            .ok_or_else(|| fail(format!("unparsed compiler instruction: {line}")))?;
        if let Some(label) = captures.get(1) {
            labels.insert(label.as_str().to_owned(), insts.len());
        }
        insts.push((captures[2].to_owned(), captures[3].to_owned(), line));
    }
    let mut nodes = vec![];
    for (i, (op, rest, line)) in insts.iter().enumerate() {
        let annotation = ann(line);
        let toks: Vec<_> = rest.split_whitespace().collect();
        let operand = |j: usize| -> Result<&str> {
            toks.get(j)
                .copied()
                .ok_or_else(|| fail(format!("missing operand in {line}")))
        };
        let an = annotation.as_deref();
        let con = || -> String {
            if an.is_some_and(|s| s.starts_with('\'')) {
                format!("S{}", unq(an))
            } else {
                format!(
                    "N{}",
                    an.and_then(|v| v.parse::<f64>().ok())
                        .map(num)
                        .unwrap_or_else(|| an.unwrap_or("None").to_owned())
                )
            }
        };
        let mut target = toks.iter().find_map(|t| labels.get(*t).copied());
        let next = (i + 1 < insts.len()).then_some(i + 1);
        let mut edge = "seq";
        let tok = match op.as_str() {
            "MOVE" | "NOP" | "CLOSEUPVALS" | "PREPVARARGS" | "COVERAGE" | "FASTCALL"
            | "FASTCALL1" | "FASTCALL2" | "FASTCALL2K" | "FASTCALL3" | "FASTPCALL"
            | "DEP_JUMPIFEQK" | "DEP_JUMPIFNOTEQK" => None,
            "GETIMPORT" => {
                if an == Some("pcall") {
                    None
                } else {
                    Some(format!("IMP:{}", an.unwrap_or("?")))
                }
            }
            "GETTABLEKS" => Some(format!("GT:{}", unq(an))),
            "SETTABLEKS" => Some(format!("ST:{}", unq(an))),
            "NAMECALL" => Some(format!("NC:{}", unq(an))),
            "GETTABLE" => Some("GT[]".into()),
            "SETTABLE" => Some("ST[]".into()),
            "GETTABLEN" => Some(format!("GTN:{}", operand(2)?)),
            "SETTABLEN" => Some(format!("STN:{}", operand(2)?)),
            "LOADK" => Some(format!("K:{}", con())),
            "LOADN" => Some(format!(
                "K:N{}",
                num(operand(1)?
                    .parse()
                    .map_err(|_| fail("invalid LOADN value"))?)
            )),
            "LOADB" => {
                if let Some(offset) = toks
                    .get(2)
                    .and_then(|s| s.strip_prefix('+'))
                    .and_then(|s| s.parse::<usize>().ok())
                {
                    target = i.checked_add(1 + offset).filter(|v| *v < insts.len());
                    edge = "jump";
                }
                Some(format!("LB:{}", operand(1)?))
            }
            "LOADNIL" => Some("LN".into()),
            "GETUPVAL" => Some("GU".into()),
            "SETUPVAL" => Some("SU".into()),
            "CALL" | "CALLFB" => Some(format!("CALL:{}:{}", operand(1)?, operand(2)?)),
            "CONCAT" => {
                let b = operand(1)?
                    .trim_start_matches('R')
                    .parse::<i64>()
                    .map_err(|_| fail("invalid CONCAT register"))?;
                let c = operand(2)?
                    .trim_start_matches('R')
                    .parse::<i64>()
                    .map_err(|_| fail("invalid CONCAT register"))?;
                Some(format!("CONCAT:{}", c - b + 1))
            }
            "NOT" => Some("NOT".into()),
            "MINUS" => Some("UNM".into()),
            "LENGTH" => Some("LEN".into()),
            "OR" => Some("OR".into()),
            "AND" => Some("AND".into()),
            "ORK" => Some(format!("ORK:{}", con())),
            "ANDK" => Some(format!("ANDK:{}", con())),
            "NEWTABLE" => Some("NEWT".into()),
            "DUPTABLE" => Some("DUPT".into()),
            "SETLIST" => Some(format!("SETLIST:{}", operand(2)?)),
            "NEWCLOSURE" => Some("NCLO".into()),
            "DUPCLOSURE" => Some("DCLO".into()),
            "GETVARARGS" => Some(format!("GVA:{}", operand(1)?)),
            "CAPTURE" => Some(format!(
                "CAP:{}",
                match operand(0)? {
                    "VAL" => 0,
                    "REF" => 1,
                    "UPVAL" => 2,
                    _ => return Err(fail("invalid CAPTURE mode")),
                }
            )),
            "RETURN" => {
                edge = "ret";
                Some(format!("RET:{}", operand(1)?))
            }
            "JUMP" | "JUMPBACK" | "JUMPX" => {
                edge = "jump";
                None
            }
            "JUMPIF" | "JUMPIFNOT" | "JUMPIFEQ" | "JUMPIFNOTEQ" | "JUMPIFLE" | "JUMPIFNOTLE"
            | "JUMPIFLT" | "JUMPIFNOTLT" => {
                edge = "cond";
                Some(
                    match op.as_str() {
                        "JUMPIF" => "JIF",
                        "JUMPIFNOT" => "JIFN",
                        "JUMPIFEQ" => "JEQ",
                        "JUMPIFNOTEQ" => "JNEQ",
                        "JUMPIFLE" => "JLE",
                        "JUMPIFNOTLE" => "JNLE",
                        "JUMPIFLT" => "JLT",
                        _ => "JNLT",
                    }
                    .into(),
                )
            }
            "JUMPXEQKS" | "JUMPXEQKN" => {
                edge = "cond";
                Some(format!(
                    "JXK:{}{}",
                    if op == "JUMPXEQKS" {
                        format!("S{}", unq(an))
                    } else {
                        con()
                    },
                    if toks.contains(&"NOT") { "!" } else { "" }
                ))
            }
            "JUMPXEQKB" => {
                edge = "cond";
                Some(format!(
                    "JXB:{}{}",
                    operand(1)?,
                    if toks.contains(&"NOT") { "!" } else { "" }
                ))
            }
            "JUMPXEQKNIL" => {
                edge = "cond";
                Some(format!(
                    "JXNIL{}",
                    if toks.contains(&"NOT") { "!" } else { "" }
                ))
            }
            "FORNPREP" | "FORNLOOP" => {
                edge = "cond";
                Some(op.clone())
            }
            "FORGLOOP" => {
                edge = "cond";
                Some(format!("FORGLOOP:{}", operand(2)?))
            }
            "FORGPREP" | "FORGPREP_INEXT" | "FORGPREP_NEXT" => {
                edge = "jump";
                Some("FORGPREP".into())
            }
            _ => {
                if let Some(arith) = arith(op) {
                    Some(format!("OP:{arith}"))
                } else if let Some(arith) = op.strip_suffix('K').and_then(arith) {
                    Some(format!("OP:{arith}:K:{}", con()))
                } else if let Some(arith) = op.strip_suffix("RK").and_then(arith) {
                    Some(format!("OPR:{arith}:K:{}", con()))
                } else {
                    Some(format!("UNK:{op}"))
                }
            }
        };
        nodes.push(Node {
            tok,
            next: match edge {
                "ret" => None,
                "jump" => target,
                _ => next,
            },
            alt: if edge == "cond" { target } else { None },
            cond: false,
        });
    }
    Ok(Graph {
        entry: (!nodes.is_empty()).then_some(0),
        nodes,
    })
}
fn expand_or(g: &mut Graph) {
    let n = g.nodes.len();
    for i in 0..n {
        let Some(t) = g.nodes[i].tok.clone() else {
            continue;
        };
        if ["OR", "AND"].contains(&t.as_str()) || t.starts_with("ORK:") || t.starts_with("ANDK:") {
            let after = g.nodes[i].next;
            if let Some((_, k)) = t.split_once(':') {
                let index = g.nodes.len();
                g.nodes.push(Node {
                    tok: Some(format!("K:{k}")),
                    next: after,
                    alt: None,
                    cond: false,
                });
                g.nodes[i].next = Some(index);
            }
            g.nodes[i].tok = Some(if t.starts_with("OR") { "JIF" } else { "JIFN" }.into());
            g.nodes[i].alt = after;
        }
    }
}
fn condition(token: &str) -> Option<(String, bool)> {
    match token {
        "JIF" => Some(("JIF".into(), true)),
        "JIFN" => Some(("JIF".into(), false)),
        "JEQ" => Some(("JEQ".into(), true)),
        "JNEQ" => Some(("JEQ".into(), false)),
        "JLE" => Some(("JLE".into(), true)),
        "JNLE" => Some(("JLE".into(), false)),
        "JLT" => Some(("JLT".into(), true)),
        "JNLT" => Some(("JLT".into(), false)),
        _ if token.starts_with("JXK:")
            || token.starts_with("JXB:")
            || token.starts_with("JXNIL") =>
        {
            Some((
                token.trim_end_matches('!').to_owned(),
                !token.ends_with('!'),
            ))
        }
        _ => None,
    }
}
fn resolve(nodes: &[Node], mut index: Option<usize>) -> Option<usize> {
    let mut seen = HashSet::new();
    while let Some(i) = index {
        let node = nodes.get(i)?;
        if node.tok.is_some() {
            return Some(i);
        }
        if !seen.insert(i) {
            return None;
        }
        index = node.next;
    }
    None
}
type Canon = Vec<(String, Option<usize>, Option<usize>)>;
fn canon(graph: &Graph, relax: &[&str]) -> Canon {
    canon_with_order(graph, relax).0
}
fn canon_with_order(graph: &Graph, relax: &[&str]) -> (Canon, Vec<usize>) {
    let mut g = graph.clone();
    expand_or(&mut g);
    for n in &mut g.nodes {
        if n.tok
            .as_ref()
            .is_some_and(|t| relax.iter().any(|p| t.starts_with(p)))
        {
            n.tok = None;
        }
        if let Some((base, true_alt)) = n.tok.as_deref().and_then(condition) {
            if true_alt {
                std::mem::swap(&mut n.next, &mut n.alt);
            }
            n.tok = Some(base);
            n.cond = true;
        }
    }
    loop {
        let mut changed = false;
        for i in 0..g.nodes.len() {
            if g.nodes[i].tok.is_some() && g.nodes[i].cond {
                let a = resolve(&g.nodes, g.nodes[i].next);
                let b = resolve(&g.nodes, g.nodes[i].alt);
                if a == b && a != Some(i) {
                    g.nodes[i].tok = None;
                    g.nodes[i].next = a;
                    g.nodes[i].alt = None;
                    g.nodes[i].cond = false;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    let Some(start) = resolve(&g.nodes, g.entry) else {
        return (vec![], vec![]);
    };
    let mut order = vec![start];
    let mut indices = HashMap::from([(start, 0)]);
    let mut stack = vec![(start, 0)];
    while let Some((n, st)) = stack.last_mut() {
        if *st >= 2 {
            stack.pop();
            continue;
        }
        let target = if *st == 0 {
            g.nodes[*n].next
        } else {
            g.nodes[*n].alt
        };
        *st += 1;
        if let Some(s) = resolve(&g.nodes, target) {
            if let std::collections::hash_map::Entry::Vacant(entry) = indices.entry(s) {
                entry.insert(order.len());
                order.push(s);
                stack.push((s, 0));
            }
        }
    }
    let canonical = order
        .iter()
        .map(|&i| {
            let n = &g.nodes[i];
            (
                n.tok.clone().unwrap(),
                resolve(&g.nodes, n.next).map(|x| indices[&x]),
                resolve(&g.nodes, n.alt).map(|x| indices[&x]),
            )
        })
        .collect();
    (canonical, order)
}
fn reachable(g: &Graph) -> HashSet<usize> {
    let mut seen = HashSet::new();
    let mut todo = g.entry.into_iter().collect::<Vec<_>>();
    while let Some(i) = todo.pop() {
        if i >= g.nodes.len() || !seen.insert(i) {
            continue;
        }
        todo.extend(g.nodes[i].next);
        todo.extend(g.nodes[i].alt);
    }
    seen
}
fn pure_multiset(graph: &Graph) -> BTreeMap<String, usize> {
    let mut g = graph.clone();
    expand_or(&mut g);
    let live = reachable(&g);
    let mut counts = BTreeMap::new();
    for i in live {
        if let Some(t) = &g.nodes[i].tok {
            if PURE.iter().any(|p| t.starts_with(p)) {
                *counts.entry(t.clone()).or_default() += 1;
            }
        }
    }
    counts
}
fn matching_blocks(a: &[String], b: &[String]) -> Vec<(usize, usize, usize)> {
    let mut positions: HashMap<&str, Vec<usize>> = HashMap::new();
    for (j, s) in b.iter().enumerate() {
        positions.entry(s).or_default().push(j);
    }
    let mut todo = vec![(0, a.len(), 0, b.len())];
    let mut blocks = vec![];
    while let Some((alo, ahi, blo, bhi)) = todo.pop() {
        let (mut besti, mut bestj, mut best) = (alo, blo, 0);
        let mut j2len: HashMap<usize, usize> = HashMap::new();
        for (i, s) in a.iter().enumerate().take(ahi).skip(alo) {
            let mut next = HashMap::new();
            for &j in positions.get(s.as_str()).into_iter().flatten() {
                if j < blo {
                    continue;
                }
                if j >= bhi {
                    break;
                }
                let size = j
                    .checked_sub(1)
                    .and_then(|x| j2len.get(&x).copied())
                    .unwrap_or(0)
                    + 1;
                next.insert(j, size);
                if size > best {
                    besti = i + 1 - size;
                    bestj = j + 1 - size;
                    best = size;
                }
            }
            j2len = next;
        }
        if best > 0 {
            blocks.push((besti, bestj, best));
            if alo < besti && blo < bestj {
                todo.push((alo, besti, blo, bestj));
            }
            if besti + best < ahi && bestj + best < bhi {
                todo.push((besti + best, ahi, bestj + best, bhi));
            }
        }
    }
    blocks.sort();
    let mut merged: Vec<(usize, usize, usize)> = vec![];
    for (i, j, k) in blocks {
        if let Some(last) = merged.last_mut() {
            if last.0 + last.2 == i && last.1 + last.2 == j {
                last.2 += k;
                continue;
            }
        }
        merged.push((i, j, k));
    }
    merged.push((a.len(), b.len(), 0));
    merged
}
fn similarity(a: &Canon, b: &Canon) -> f64 {
    let ta: Vec<_> = a.iter().map(|x| x.0.clone()).collect();
    let tb: Vec<_> = b.iter().map(|x| x.0.clone()).collect();
    if ta.is_empty() && tb.is_empty() {
        return 1.0;
    }
    2.0 * matching_blocks(&ta, &tb).iter().map(|x| x.2).sum::<usize>() as f64
        / (ta.len() + tb.len()) as f64
}
pub fn only_constant_loads_differ(a: &[String], b: &[String]) -> bool {
    let movable = |t: &&String| t.as_str() == "LN" || t.starts_with("K:N");
    let fixed_a: Vec<_> = a.iter().filter(|t| !movable(t)).collect();
    let fixed_b: Vec<_> = b.iter().filter(|t| !movable(t)).collect();
    let mut na: Vec<_> = a.iter().filter(|t| t.starts_with("K:N")).collect();
    let mut nb: Vec<_> = b.iter().filter(|t| t.starts_with("K:N")).collect();
    na.sort();
    nb.sort();
    a != b && fixed_a == fixed_b && na == nb
}
fn py_list(items: &[String]) -> String {
    format!(
        "[{}]",
        items
            .iter()
            .map(|s| format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'")))
            .collect::<Vec<_>>()
            .join(", ")
    )
}
fn show_diff(a: &Canon, b: &Canon, out: &mut Vec<String>) {
    let ta: Vec<_> = a.iter().map(|x| x.0.clone()).collect();
    let tb: Vec<_> = b.iter().map(|x| x.0.clone()).collect();
    let (mut i, mut j, mut edge) = (0, 0, 0);
    for (ai, bj, size) in matching_blocks(&ta, &tb) {
        if i < ai || j < bj {
            let tag = if i < ai && j < bj {
                "replace"
            } else if i < ai {
                "delete"
            } else {
                "insert"
            };
            out.push(format!(
                "   {tag} orig[{i}:{ai}] {}",
                py_list(&ta[i.saturating_sub(3)..(ai + 3).min(ta.len())])
            ));
            out.push(format!(
                "      new[{j}:{bj}]  {}",
                py_list(&tb[j.saturating_sub(3)..(bj + 3).min(tb.len())])
            ));
        }
        for k in 0..size {
            let left = &a[ai + k];
            let right = &b[bj + k];
            let left_fallthrough = left.1 == Some(ai + k + 1);
            let right_fallthrough = right.1 == Some(bj + k + 1);
            let same_next = if left_fallthrough || right_fallthrough {
                left_fallthrough == right_fallthrough
            } else {
                left.1 == right.1
            };
            if !same_next || left.2 != right.2 {
                edge += 1;
            }
        }
        i = ai + size;
        j = bj + size;
    }
    out.push(format!(
        "   (edge-only mismatches in matching tokens: {edge})"
    ));
}
#[derive(Clone)]
struct ANode {
    op: String,
    args: Value,
    rid: u64,
    ii: usize,
    succ: Vec<usize>,
    preds: Vec<usize>,
    reads: Vec<i64>,
    writes: Vec<i64>,
    caps: Vec<(i64, i64)>,
}
fn conditional(name: &str) -> bool {
    [
        "JUMPIFNOT",
        "JUMPIF",
        "JUMPIFEQ",
        "JUMPIFNOTEQ",
        "JUMPIFLE",
        "JUMPIFLT",
        "JUMPIFNOTLT",
        "JUMPIFNOTLE",
        "JUMPXEQK",
        "JUMPXEQKB",
        "JUMPXEQKNIL",
        "FORNPREP",
        "FORNLOOP",
        "FORGLOOP",
    ]
    .contains(&name)
}
fn terminal(name: &str) -> bool {
    conditional(name) || ["RETURN", "FORGPREP"].contains(&name)
}
fn dead_loads(p: &Value, protos: &serde_json::Map<String, Value>) -> Result<HashSet<(u64, usize)>> {
    let decoded = p["decoded"]
        .as_object()
        .ok_or_else(|| fail("prototype has no decoded instruction map"))?;
    let records: BTreeMap<u64, &Value> = decoded
        .iter()
        .map(|(k, v)| {
            k.parse()
                .map(|k| (k, v))
                .map_err(|_| fail("invalid record id"))
        })
        .collect::<Result<_>>()?;
    let mut nodes = vec![];
    let mut byrid: BTreeMap<u64, Vec<usize>> = BTreeMap::new();
    let mut following = HashMap::new();
    for (&rid, d) in &records {
        if d["kind"] == "CAPTURE" {
            continue;
        }
        let list = byrid.entry(rid).or_default();
        for (ii, item) in d["ins"]
            .as_array()
            .ok_or_else(|| fail("record has no instruction array"))?
            .iter()
            .enumerate()
        {
            let (op, args) = ins(item)?;
            if op == "NOP" {
                continue;
            }
            list.push(nodes.len());
            nodes.push(ANode {
                op: op.into(),
                args: args.clone(),
                rid,
                ii,
                succ: vec![],
                preds: vec![],
                reads: vec![],
                writes: vec![],
                caps: vec![],
            });
        }
        if !d["ctrl"].is_null() {
            let (op, args) = ins(&d["ctrl"])?;
            if !["LOADBSKIP", "JUMP"].contains(&op) {
                list.push(nodes.len());
                nodes.push(ANode {
                    op: op.into(),
                    args: args.clone(),
                    rid,
                    ii: usize::MAX,
                    succ: vec![],
                    preds: vec![],
                    reads: vec![],
                    writes: vec![],
                    caps: vec![],
                });
            }
        }
        following.insert(rid, field_id(&d["nxt"]));
    }
    for n in &mut nodes {
        if !["NEWCLOSURE", "DUPCLOSURE"].contains(&n.op.as_str()) {
            continue;
        }
        let rf = a(&n.args, "rf");
        let child = if n.op == "NEWCLOSURE" {
            if let Some(kids) = p["kids"].as_array() {
                kids.get(rf as usize).and_then(field_id)
            } else {
                p["kids"].get(rf.to_string()).and_then(field_id)
            }
        } else {
            constant(p, rf)
                .and_then(|c| crate::ir::integer(&c[1], "closure prototype").ok())
                .and_then(|id| u64::try_from(id).ok())
        }
        .ok_or_else(|| fail("closure references an unknown child"))?;
        let child = protos
            .get(&child.to_string())
            .ok_or_else(|| fail("missing child prototype metadata"))?;
        let nup = child["nup"]
            .as_u64()
            .ok_or_else(|| fail("missing child upvalue count"))?;
        if nup > 255 {
            return Err(fail("invalid child upvalue count"));
        }
        let mut current = field_id(&records[&n.rid]["nxt"]);
        for _ in 0..nup {
            let record = current
                .and_then(|i| records.get(&i).copied())
                .ok_or_else(|| fail("truncated capture records"))?;
            if record["kind"] != "CAPTURE" {
                return Err(fail("expected closure CAPTURE record"));
            }
            let (op, args) = ins(&record["ins"][0])?;
            if op != "CAPTURE" {
                return Err(fail("invalid closure capture instruction"));
            }
            n.caps.push((a(args, "A"), a(args, "B")));
            current = field_id(&record["nxt"]);
        }
        following.insert(n.rid, current);
    }
    fn first(
        mut rid: Option<u64>,
        records: &BTreeMap<u64, &Value>,
        byrid: &BTreeMap<u64, Vec<usize>>,
        following: &HashMap<u64, Option<u64>>,
    ) -> Result<usize> {
        let mut seen = HashSet::new();
        loop {
            let key = rid.ok_or_else(|| fail("instruction edge has no target"))?;
            if !seen.insert(key) {
                return Err(fail("empty instruction cycle"));
            }
            let d = records
                .get(&key)
                .ok_or_else(|| fail("instruction edge names a missing record"))?;
            if let Some(n) = byrid.get(&key).and_then(|xs| xs.first()) {
                return Ok(*n);
            }
            rid = if d["ctrl"][0] == "JUMP" {
                field_id(&d["alt"])
            } else {
                following
                    .get(&key)
                    .copied()
                    .flatten()
                    .or_else(|| field_id(&d["nxt"]))
            };
        }
    }
    for (&rid, list) in &byrid {
        let d = records[&rid];
        for (i, &idx) in list.iter().enumerate() {
            nodes[idx].succ = if i + 1 < list.len() {
                vec![list[i + 1]]
            } else if nodes[idx].op == "RETURN" {
                vec![]
            } else if conditional(&nodes[idx].op) {
                vec![
                    first(field_id(&d["nxt"]), &records, &byrid, &following)?,
                    first(field_id(&d["alt"]), &records, &byrid, &following)?,
                ]
            } else {
                vec![first(
                    if d["ctrl"][0] == "JUMP" {
                        field_id(&d["alt"])
                    } else {
                        following[&rid]
                    },
                    &records,
                    &byrid,
                    &following,
                )?]
            };
        }
    }
    let entry = first(field_id(&p["entry"]), &records, &byrid, &following)?;
    let mut order = vec![];
    let mut live = HashSet::new();
    let mut stack = vec![entry];
    while let Some(i) = stack.pop() {
        if !live.insert(i) {
            continue;
        }
        order.push(i);
        stack.extend(nodes[i].succ.iter().rev().copied());
    }
    for &i in &order {
        for s in nodes[i].succ.clone() {
            nodes[s].preds.push(i);
        }
    }
    for &i in &order {
        let n = &mut nodes[i];
        let (x, b, c, aux) = (
            a(&n.args, "A"),
            a(&n.args, "B"),
            a(&n.args, "C"),
            a(&n.args, "aux"),
        );
        if [x, b, c].iter().any(|r| *r < 0 || *r > 65535) {
            return Err(fail("invalid register operands"));
        }
        match n.op.as_str() {
            "LOADK" | "LOADN" | "LOADB" | "LOADNIL" | "GETIMPORT" | "GETUPVAL" | "NEWTABLE"
            | "DUPTABLE" => n.writes = vec![x],
            "MOVE" => {
                n.reads = vec![b];
                n.writes = vec![x];
            }
            "GETTABLEKS" => {
                n.reads = vec![b];
                n.writes = vec![x];
            }
            "SETTABLEKS" | "SETTABLEN" => n.reads = vec![x, b],
            "SELF" => {
                n.reads = vec![b];
                n.writes = vec![x + 1, x];
            }
            "SETUPVAL" => n.reads = vec![x],
            "LENGTH" | "NOT" | "MINUS" | "GETTABLEN" | "ORK" | "ANDK" => {
                n.reads = vec![b];
                n.writes = vec![x];
            }
            "GETVARARGS" => n.writes = (x..x + (b - 1).max(0)).collect(),
            "CONCAT" => {
                n.reads = (b..=c).collect();
                n.writes = vec![x];
            }
            "SETTABLE" => n.reads = vec![x, b, c],
            "FORGPREP" => n.reads = vec![x, x + 1, x + 2],
            "FORGLOOP" => n.writes = (x + 3..x + 3 + (aux & 255)).collect(),
            "SETLIST" => {
                n.reads = vec![x];
                n.writes = vec![x];
            }
            "NEWCLOSURE" | "DUPCLOSURE" => {
                n.reads = n
                    .caps
                    .iter()
                    .filter(|(mode, index)| [0, 1].contains(mode) && *index != x)
                    .map(|(_, index)| *index)
                    .collect();
                n.writes = vec![x];
            }
            "CALL" | "RETURN" | "CLOSEUPVALS" | "JUNK" => {}
            "JUMPIFNOT" | "JUMPIF" | "JUMPXEQK" | "JUMPXEQKB" | "JUMPXEQKNIL" => n.reads = vec![x],
            "JUMPIFEQ" | "JUMPIFNOTEQ" | "JUMPIFLE" | "JUMPIFLT" | "JUMPIFNOTLT"
            | "JUMPIFNOTLE" => n.reads = vec![x, aux],
            "FORNPREP" => {
                n.reads = vec![x, x + 1, x + 2];
                n.writes = vec![x + 2];
            }
            "FORNLOOP" => n.writes = vec![x + 2],
            op if arith(op).is_some() || ["OR", "AND", "GETTABLE"].contains(&op) => {
                n.reads = vec![b, c];
                n.writes = vec![x];
            }
            op if op.strip_suffix('K').and_then(arith).is_some() => {
                n.reads = vec![b];
                n.writes = vec![x];
            }
            "SUBRK" | "DIVRK" => {
                n.reads = vec![c];
                n.writes = vec![x];
            }
            op => return Err(fail(format!("unknown register semantics for {op}"))),
        }
    }
    for &i in &order {
        if !["CALL", "RETURN", "SETLIST"].contains(&nodes[i].op.as_str()) {
            continue;
        }
        let (x, b, c) = (
            a(&nodes[i].args, "A"),
            a(&nodes[i].args, "B"),
            a(&nodes[i].args, "C"),
        );
        let variable = if nodes[i].op == "SETLIST" {
            c == 0
        } else {
            b == 0
        };
        let mut top = None;
        if variable {
            let mut cur = i;
            let mut seen = HashSet::new();
            loop {
                if !seen.insert(cur) || nodes[cur].preds.len() != 1 {
                    return Err(fail("multret consumer without single predecessor chain"));
                }
                cur = nodes[cur].preds[0];
                if nodes[cur].op == "CALL" && a(&nodes[cur].args, "C") == 0
                    || nodes[cur].op == "GETVARARGS" && a(&nodes[cur].args, "B") == 0
                {
                    top = Some(a(&nodes[cur].args, "A"));
                    break;
                }
                if terminal(&nodes[cur].op) {
                    return Err(fail("multret producer not found"));
                }
            }
        }
        match nodes[i].op.as_str() {
            "CALL" => {
                let count = if b > 0 { b - 1 } else { top.unwrap() - x - 1 };
                nodes[i].reads = (x..=x + count).collect();
                if c >= 2 {
                    nodes[i].writes = (x..x + c - 1).collect();
                }
            }
            "RETURN" => {
                let count = if b > 0 { b - 1 } else { top.unwrap() - x };
                nodes[i].reads = (x..x + count).collect();
            }
            _ => {
                let count = if c > 0 { c - 1 } else { top.unwrap() - b };
                nodes[i].reads = std::iter::once(x).chain(b..b + count).collect();
                nodes[i].writes = vec![x];
            }
        }
    }
    let np = p["np"]
        .as_u64()
        .ok_or_else(|| fail("missing parameter count"))? as usize;
    if np > 255 {
        return Err(fail("invalid parameter count"));
    }
    let mut defs: Vec<(Option<usize>, i64)> = (0..np).map(|r| (None, r as i64)).collect();
    let mut node_defs = HashMap::new();
    for &i in &order {
        for &r in &nodes[i].writes {
            node_defs.insert((i, r), defs.len());
            defs.push((Some(i), r));
        }
    }
    type Reaching = BTreeMap<i64, BTreeSet<usize>>;
    let init: Reaching = (0..np).map(|r| (r as i64, BTreeSet::from([r]))).collect();
    let mut incoming = vec![Reaching::new(); nodes.len()];
    let mut outgoing = incoming.clone();
    let mut work: VecDeque<_> = order.iter().copied().collect();
    let mut queued = live.clone();
    while let Some(i) = work.pop_front() {
        queued.remove(&i);
        let mut cur = if i == entry {
            init.clone()
        } else {
            Reaching::new()
        };
        for &pred in &nodes[i].preds {
            for (&r, ds) in &outgoing[pred] {
                cur.entry(r).or_default().extend(ds);
            }
        }
        incoming[i] = cur.clone();
        for &r in &nodes[i].writes {
            cur.insert(r, BTreeSet::from([node_defs[&(i, r)]]));
        }
        if outgoing[i] != cur {
            outgoing[i] = cur;
            for &s in &nodes[i].succ {
                if queued.insert(s) {
                    work.push_back(s);
                }
            }
        }
    }
    let mut uf: Vec<_> = (0..defs.len()).collect();
    fn find(uf: &mut [usize], mut x: usize) -> usize {
        while uf[x] != x {
            uf[x] = uf[uf[x]];
            x = uf[x];
        }
        x
    }
    fn union(uf: &mut [usize], x: usize, y: usize) {
        let a = find(uf, x);
        let b = find(uf, y);
        if a != b {
            uf[b] = a;
        }
    }
    let mut loop_base: HashMap<usize, i64> = HashMap::new();
    for &i in &order {
        if !["FORNPREP", "FORGPREP"].contains(&nodes[i].op.as_str()) || nodes[i].succ.is_empty() {
            continue;
        }
        let (body, leave) = if nodes[i].op == "FORNPREP" {
            if nodes[i].succ.len() != 2 {
                continue;
            }
            (nodes[i].succ[0], nodes[i].succ[1])
        } else {
            let lp = nodes[i].succ[0];
            if nodes[lp].op != "FORGLOOP" || nodes[lp].succ.len() != 2 {
                continue;
            }
            (nodes[lp].succ[1], nodes[lp].succ[0])
        };
        let mut todo = vec![body];
        let mut seen = HashSet::new();
        while let Some(j) = todo.pop() {
            if j == leave || !seen.insert(j) {
                continue;
            }
            loop_base
                .entry(j)
                .and_modify(|b| *b = (*b).max(a(&nodes[i].args, "A")))
                .or_insert(a(&nodes[i].args, "A"));
            todo.extend(nodes[j].succ.iter().copied());
        }
    }
    let mut usedefs: HashMap<(usize, i64), BTreeSet<usize>> = HashMap::new();
    let mut movers = vec![];
    let mut assigns = vec![];
    for &i in &order {
        let n = &nodes[i];
        for &r in &n.reads {
            let ds = incoming[i]
                .get(&r)
                .filter(|ds| !ds.is_empty())
                .ok_or_else(|| {
                    fail(format!(
                        "read of undefined register R{r} in {} record {}",
                        n.op, n.rid
                    ))
                })?;
            let first = *ds.first().unwrap();
            for &d in ds.iter().skip(1) {
                union(&mut uf, first, d);
            }
            usedefs.insert((i, r), ds.clone());
        }
        if n.op == "MOVE" && a(&n.args, "A") < a(&n.args, "B") {
            if let Some(ds) = incoming[i].get(&a(&n.args, "A")) {
                movers.push((i, ds.clone()));
            }
        }
        if let Some(base) = loop_base.get(&i) {
            for &r in &n.writes {
                if r < *base {
                    if let Some(ds) = incoming[i].get(&r) {
                        assigns.push((i, r, ds.clone()));
                    }
                }
            }
        }
        if ["NEWCLOSURE", "DUPCLOSURE"].contains(&n.op.as_str()) {
            for (mode, index) in &n.caps {
                if [0, 1].contains(mode) && *index == a(&n.args, "A") {
                    usedefs.insert((i, *index), BTreeSet::from([node_defs[&(i, *index)]]));
                }
            }
        }
    }
    let mut readers: HashMap<usize, Vec<usize>> = HashMap::new();
    for (&(i, _), ds) in &usedefs {
        for &d in ds {
            readers.entry(d).or_default().push(i);
        }
    }
    for (i, prev) in movers {
        let r = a(&nodes[i].args, "A");
        let d = node_defs[&(i, r)];
        let rd = readers.get(&d).map(Vec::as_slice).unwrap_or(&[]);
        if !rd.is_empty()
            && (rd.iter().all(|&j| {
                nodes[j].op == "CONCAT"
                    && a(&nodes[j].args, "B") <= r
                    && r <= a(&nodes[j].args, "C")
            }) || rd.iter().all(|&j| {
                nodes[j].op == "FORNPREP"
                    && a(&nodes[j].args, "A") <= r
                    && r <= a(&nodes[j].args, "A") + 2
            }))
        {
            continue;
        }
        for old in prev {
            union(&mut uf, d, old);
        }
    }
    for (i, r, prev) in assigns {
        for old in prev {
            union(&mut uf, node_defs[&(i, r)], old);
        }
    }
    for &i in &order {
        let n = &nodes[i];
        if !["NEWCLOSURE", "DUPCLOSURE"].contains(&n.op.as_str()) {
            continue;
        }
        for (mode, index) in &n.caps {
            if *mode != 1 {
                continue;
            }
            let d0 = *usedefs
                .get(&(i, *index))
                .and_then(|ds| ds.first())
                .ok_or_else(|| fail("undefined captured register"))?;
            let mut seen = HashSet::new();
            let mut todo = n.succ.clone();
            while let Some(j) = todo.pop() {
                if !seen.insert(j) {
                    continue;
                }
                let m = &nodes[j];
                if m.op == "CLOSEUPVALS" && a(&m.args, "A") <= *index {
                    continue;
                }
                if let Some(d) = node_defs.get(&(j, *index)) {
                    union(&mut uf, d0, *d);
                }
                todo.extend(m.succ.iter().copied());
            }
        }
    }
    for r in 0..np {
        for (d, (_, reg)) in defs.iter().enumerate() {
            if *reg == r as i64 {
                union(&mut uf, r, d);
            }
        }
    }
    let used: HashSet<_> = usedefs.values().flat_map(|ds| ds.iter().copied()).collect();
    let mut used_webs = HashSet::new();
    for &d in &used {
        used_webs.insert(find(&mut uf, d));
    }
    let mut captured = HashSet::new();
    for &i in &order {
        let n = &nodes[i];
        if ["NEWCLOSURE", "DUPCLOSURE"].contains(&n.op.as_str()) {
            for (mode, index) in &n.caps {
                if [0, 1].contains(mode) {
                    if let Some(ds) = usedefs.get(&(i, *index)) {
                        if let Some(&d) = ds.first() {
                            captured.insert(find(&mut uf, d));
                        }
                    }
                }
            }
        }
    }
    let mut dead = HashSet::new();
    for &i in &order {
        let n = &nodes[i];
        if !["LOADK", "LOADN", "LOADB", "LOADNIL"].contains(&n.op.as_str()) {
            continue;
        }
        let d = node_defs[&(i, a(&n.args, "A"))];
        let root = find(&mut uf, d);
        if !used_webs.contains(&root) || !used.contains(&d) && !captured.contains(&root) {
            dead.insert((n.rid, n.ii));
        }
    }
    Ok(dead)
}
pub fn compare(program: &Value, bytecode_text: &str) -> Result<Value> {
    let protos = program["protos"]
        .as_object()
        .ok_or_else(|| fail("normalized program has no prototypes"))?;
    if protos.is_empty() {
        return Err(fail("normalized program is empty"));
    }
    let compiled = parse_luau(bytecode_text)?;
    let (mut original, mut new, mut relaxed_original, mut relaxed_new, mut ma, mut mb) = (
        BTreeMap::new(),
        BTreeMap::new(),
        BTreeMap::new(),
        BTreeMap::new(),
        BTreeMap::new(),
        BTreeMap::new(),
    );
    let mut ndead = 0;
    for (key, p) in protos {
        let pid = key
            .parse::<u64>()
            .map_err(|_| fail("invalid prototype id"))?;
        let dead = dead_loads(p, protos)?;
        ndead += dead.len();
        let graph = orig_nodes(p, &dead)?;
        original.insert(pid, canon(&graph, &["LN"]));
        relaxed_original.insert(pid, canon(&graph, PURE));
        ma.insert(pid, pure_multiset(&graph));
    }
    for (&fid, lines) in &compiled {
        let graph = compiler_nodes(lines)?;
        new.insert(fid, canon(&graph, &["LN"]));
        relaxed_new.insert(fid, canon(&graph, PURE));
        mb.insert(fid, pure_multiset(&graph));
    }
    let mut matches: BTreeMap<u64, Option<u64>> = BTreeMap::new();
    let mut levels = HashMap::new();
    let mut used = HashSet::new();
    for stage in 0..2 {
        for &pid in original.keys() {
            if matches.contains_key(&pid) {
                continue;
            }
            let mut candidates = vec![];
            for &fid in new.keys() {
                if used.contains(&fid) {
                    continue;
                }
                if if stage == 0 {
                    original[&pid] == new[&fid]
                } else {
                    relaxed_original[&pid] == relaxed_new[&fid] && ma[&pid] == mb[&fid]
                } {
                    candidates.push(fid);
                }
            }
            if let Some(fid) = candidates
                .iter()
                .find(|&&id| id == pid)
                .or_else(|| candidates.first())
                .copied()
            {
                matches.insert(pid, Some(fid));
                levels.insert(pid, stage);
                used.insert(fid);
            }
        }
    }
    let rest_a: Vec<_> = original
        .keys()
        .filter(|p| !matches.contains_key(*p))
        .copied()
        .collect();
    let rest_b: Vec<_> = new.keys().filter(|f| !used.contains(*f)).copied().collect();
    let mut sims = vec![];
    for &pid in &rest_a {
        for &fid in &rest_b {
            let (la, lb) = (original[&pid].len(), new[&fid].len());
            if la.min(lb) * 2 + 6 < la.max(lb) {
                continue;
            }
            sims.push((similarity(&original[&pid], &new[&fid]), pid, fid));
        }
    }
    sims.sort_by(|x, y| {
        y.0.total_cmp(&x.0)
            .then_with(|| y.1.cmp(&x.1))
            .then_with(|| y.2.cmp(&x.2))
    });
    let (mut done_a, mut done_b) = (HashSet::new(), HashSet::new());
    for (_, pid, fid) in sims {
        if done_a.contains(&pid) || done_b.contains(&fid) {
            continue;
        }
        matches.insert(pid, Some(fid));
        levels.insert(pid, 2);
        done_a.insert(pid);
        done_b.insert(fid);
    }
    for pid in rest_a {
        if let std::collections::btree_map::Entry::Vacant(entry) = matches.entry(pid) {
            entry.insert(None);
            levels.insert(pid, 3);
        }
    }
    let identical = levels.values().filter(|&&v| v == 0).count();
    let reordered = levels.values().filter(|&&v| v == 1).count();
    let mut report = vec![];
    let mut bad = vec![];
    let mut cosmetic = vec![];
    let reord: Vec<_> = original
        .keys()
        .filter(|p| levels[p] == 1)
        .copied()
        .collect();
    for (&pid, ac) in &original {
        if levels[&pid] <= 1 {
            continue;
        }
        bad.push(pid);
        let Some(fid) = matches[&pid] else {
            report.push(format!("proto {pid}: NO MATCH"));
            continue;
        };
        let bc = &new[&fid];
        let ta: Vec<_> = ac.iter().map(|x| x.0.clone()).collect();
        let tb: Vec<_> = bc.iter().map(|x| x.0.clone()).collect();
        report.push(format!(
            "=== orig proto {pid} ~ new function {fid}  (nodes {} vs {}, token-seq {}, sim {:.3})",
            ac.len(),
            bc.len(),
            if ta == tb { "equal" } else { "differs" },
            similarity(ac, bc)
        ));
        show_diff(ac, bc, &mut report);
        let subtract =
            |a: &BTreeMap<String, usize>, b: &BTreeMap<String, usize>| -> BTreeMap<String, usize> {
                a.iter()
                    .filter_map(|(k, v)| {
                        v.checked_sub(*b.get(k).unwrap_or(&0))
                            .filter(|v| *v > 0)
                            .map(|v| (k.clone(), v))
                    })
                    .collect()
            };
        let extra_a = subtract(&ma[&pid], &mb[&fid]);
        let extra_b = subtract(&mb[&fid], &ma[&pid]);
        if !extra_a.is_empty() || !extra_b.is_empty() {
            let repr = |m: &BTreeMap<String, usize>| {
                format!(
                    "{{{}}}",
                    m.iter()
                        .map(|(k, v)| format!("'{k}': {v}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            };
            report.push(format!(
                "   pure-op multiset: only-orig {} | only-new {}",
                repr(&extra_a),
                repr(&extra_b)
            ));
        }
        let ra = ta.iter().filter(|t| t.as_str() == "RET:0").count();
        let rb = tb.iter().filter(|t| t.as_str() == "RET:0").count();
        if ra > rb
            && rb >= 1
            && ta
                .iter()
                .filter(|t| t.as_str() != "RET:0")
                .eq(tb.iter().filter(|t| t.as_str() != "RET:0"))
            || only_constant_loads_differ(&ta, &tb)
        {
            cosmetic.push(pid);
        }
    }
    Ok(
        json!({"functions_orig":original.len(),"functions_new":new.len(),"identical":identical,"reordered":reordered,"differing":bad,"reordered_protos":reord,"cosmetic":cosmetic,"dead_loads":ndead,"report":report,"match":matches}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn control_bytes_and_annotations() {
        assert_eq!(sstr(b"a\x01\x1fb\n\t"), "a\\x01\\x1Fb\\x0A\\x09");
        assert_eq!(sstr(&[1; 10]), "\\x01".repeat(6));
        assert_eq!(sstr(&[127, 200]), "\u{7f}È");
        assert_eq!(ann("SUBRK R0 [3] R1 ['ignored']"), Some("3".into()));
        assert_eq!(unq(Some("'abc'...")), "abc");
    }
    #[test]
    fn movable_loads_are_not_strings_or_changed_numbers() {
        let s = |v: &[&str]| v.iter().map(|v| v.to_string()).collect::<Vec<_>>();
        assert!(only_constant_loads_differ(
            &s(&["LN", "K:N1", "RET:0"]),
            &s(&["K:N1", "RET:0"])
        ));
        assert!(!only_constant_loads_differ(
            &s(&["K:Sa", "IMP:print", "RET:0"]),
            &s(&["IMP:print", "K:Sa", "RET:0"])
        ));
        assert!(!only_constant_loads_differ(
            &s(&["K:N0", "RET:0"]),
            &s(&["K:N1", "RET:0"])
        ));
    }
    #[test]
    fn numbers_preserve_negative_zero_and_nonfinite() {
        assert_eq!(num(-0.0), "-0");
        assert_eq!(num(1.23456789123456), "1.23456789123");
        assert_eq!(num(1e12), "1e+12");
        assert_eq!(num(1e-5), "1e-05");
        assert_eq!(num(f64::INFINITY), "inf");
    }
    #[test]
    fn edge_diagnostics_treat_fallthrough_as_relative_to_each_matched_node() {
        let before = vec![
            ("K:N1".into(), Some(1), None),
            ("CALL:0:0".into(), Some(2), None),
            ("RET:0".into(), None, None),
        ];
        let after = vec![
            ("CALL:0:0".into(), Some(1), None),
            ("RET:0".into(), None, None),
        ];
        let mut report = vec![];
        show_diff(&before, &after, &mut report);
        assert_eq!(
            report.last().unwrap(),
            "   (edge-only mismatches in matching tokens: 0)"
        );
    }
}

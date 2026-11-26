//! Binding-aware source passes. Source spans are byte offsets into the original UTF-8 source.
use crate::astutil::{parse_ast, unlatin1};
use crate::error::{Error, Result};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::OnceLock;

pub(crate) fn kind(n: &Value) -> &str {
    n["type"].as_str().unwrap_or("")
}
pub(crate) fn arr<'a>(n: &'a Value, key: &str) -> &'a [Value] {
    n[key].as_array().map(Vec::as_slice).unwrap_or(&[])
}
pub(crate) fn text<'a>(n: &'a Value, key: &str) -> &'a str {
    n[key].as_str().unwrap_or("")
}
pub(crate) fn id(n: &Value) -> usize {
    n as *const Value as usize
}
pub(crate) fn unparen(mut n: &Value) -> &Value {
    while kind(n) == "AstExprGroup" {
        n = &n["expr"];
    }
    n
}
pub(crate) fn lkey(n: &Value) -> String {
    text(n, "location").to_owned()
}
pub(crate) fn local_key(n: &Value) -> Option<String> {
    let n = unparen(n);
    (kind(n) == "AstExprLocal").then(|| lkey(&n["local"]))
}
pub(crate) fn walk(n: &Value) -> Vec<(&Value, Option<&Value>)> {
    let mut out = Vec::new();
    let mut stack = vec![(n, None)];
    while let Some((node, parent)) = stack.pop() {
        out.push((node, parent));
        let mut children = Vec::new();
        if let Some(obj) = node.as_object() {
            for v in obj.values() {
                if v.get("type").is_some() {
                    children.push(v);
                } else if let Some(items) = v.as_array() {
                    for x in items {
                        if x.get("type").is_some() {
                            children.push(x);
                        } else if let Some(item) = x.as_object() {
                            for vv in item.values() {
                                if vv.get("type").is_some() {
                                    children.push(vv);
                                }
                            }
                        }
                    }
                }
            }
        }
        for c in children.into_iter().rev() {
            stack.push((c, Some(node)));
        }
    }
    out
}
pub(crate) struct Src<'a> {
    pub text: &'a str,
    pub starts: Vec<usize>,
}
impl<'a> Src<'a> {
    pub fn new(text: &'a str) -> Self {
        let mut starts = vec![0];
        starts.extend(
            text.bytes()
                .enumerate()
                .filter_map(|(i, b)| (b == b'\n').then_some(i + 1)),
        );
        Self { text, starts }
    }
    pub fn loc(&self, n: &Value) -> Result<[usize; 4]> {
        let p: Vec<_> = text(n, "location")
            .split(|c: char| !c.is_ascii_digit())
            .filter(|x| !x.is_empty())
            .filter_map(|x| x.parse().ok())
            .collect();
        if p.len() != 4 {
            return Err(Error::new("cleanup", "invalid AST location"));
        }
        Ok([p[0], p[1], p[2], p[3]])
    }
    pub fn span(&self, n: &Value) -> Result<(usize, usize)> {
        let [l, c, ll, cc] = self.loc(n)?;
        let a = self.starts.get(l).and_then(|v| v.checked_add(c));
        let b = self.starts.get(ll).and_then(|v| v.checked_add(cc));
        match (a, b) {
            (Some(a), Some(b))
                if a <= b
                    && b <= self.text.len()
                    && self.text.is_char_boundary(a)
                    && self.text.is_char_boundary(b) =>
            {
                Ok((a, b))
            }
            _ => Err(Error::new("cleanup", "AST byte span outside source")),
        }
    }
    pub fn slice(&self, n: &Value) -> Result<&'a str> {
        let (a, b) = self.span(n)?;
        Ok(&self.text[a..b])
    }
}
pub(crate) type Edit = (usize, usize, String);
pub(crate) fn apply(text: &str, mut edits: Vec<Edit>) -> Result<String> {
    edits.sort();
    edits.dedup();
    let mut out = text.to_owned();
    let mut previous = text.len() + 1;
    for (a, b, replacement) in edits.into_iter().rev() {
        if a > b
            || b > text.len()
            || b > previous
            || !out.is_char_boundary(a)
            || !out.is_char_boundary(b)
        {
            return Err(Error::new("cleanup", "overlapping or invalid source edits"));
        }
        out.replace_range(a..b, &replacement);
        previous = a;
    }
    Ok(out)
}
pub(crate) fn outer(mut edits: Vec<Edit>) -> Vec<Edit> {
    edits.sort_by_key(|e| (e.0, std::cmp::Reverse(e.1)));
    edits.dedup();
    let mut kept: Vec<Edit> = vec![];
    for e in edits {
        if kept.last().is_none_or(|p| e.0 >= p.1) {
            kept.push(e);
        }
    }
    kept
}
pub(crate) fn lua_str(bytes: &[u8]) -> String {
    let mut out = String::from("\"");
    fn put(out: &mut String, c: char) {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 32 || c == '\u{7f}' => out.push_str(&format!("\\{:03}", c as u32)),
            _ => out.push(c),
        }
    }
    if let Ok(s) = std::str::from_utf8(bytes) {
        for c in s.chars() {
            put(&mut out, c);
        }
    } else {
        for &b in bytes {
            if (32..127).contains(&b) || [9, 10, 13].contains(&b) {
                put(&mut out, b as char)
            } else {
                out.push_str(&format!("\\{b:03}"));
            }
        }
    }
    out.push('"');
    out
}
pub(crate) struct Bindings<'a> {
    pub nodes: Vec<(&'a Value, Option<&'a Value>)>,
    pub parents: HashMap<usize, Option<&'a Value>>,
    pub defs: BTreeMap<String, (&'a Value, &'a Value)>,
    pub values: HashMap<String, &'a Value>,
    pub refs: HashMap<String, Vec<&'a Value>>,
    pub writes: HashSet<String>,
    pub global_writes: HashSet<String>,
}
pub(crate) fn targets(n: &Value) -> Vec<&Value> {
    match kind(n) {
        "AstStatAssign" => arr(n, "vars").iter().collect(),
        "AstStatCompoundAssign" => vec![&n["var"]],
        "AstStatFunction" => vec![&n["name"]],
        _ => vec![],
    }
}
impl<'a> Bindings<'a> {
    pub fn new(root: &'a Value) -> Self {
        let nodes = walk(root);
        let parents = nodes.iter().map(|(n, p)| (id(n), *p)).collect();
        let mut b = Self {
            nodes,
            parents,
            defs: BTreeMap::new(),
            values: HashMap::new(),
            refs: HashMap::new(),
            writes: HashSet::new(),
            global_writes: HashSet::new(),
        };
        for (n, _) in &b.nodes {
            match kind(n) {
                "AstExprLocal" => b.refs.entry(lkey(&n["local"])).or_default().push(n),
                "AstStatLocal" if arr(n, "vars").len() == 1 && arr(n, "values").len() == 1 => {
                    b.values.insert(lkey(&n["vars"][0]), &n["values"][0]);
                }
                _ => {
                    for target in targets(n) {
                        if let Some(key) = local_key(target) {
                            b.writes.insert(key);
                        }
                    }
                }
            }
        }
        for n in arr(root, "body") {
            match kind(n) {
                "AstStatLocal" if arr(n, "vars").len() == 1 && arr(n, "values").len() == 1 => {
                    b.defs.insert(lkey(&n["vars"][0]), (n, &n["values"][0]));
                }
                "AstStatLocalFunction" => {
                    b.defs.insert(lkey(&n["name"]), (n, &n["func"]));
                }
                _ => {}
            }
        }
        let mut globals = HashSet::new();
        for (n, _) in &b.nodes {
            for target in targets(n) {
                let target = unparen(target);
                let base = if matches!(kind(target), "AstExprIndexExpr" | "AstExprIndexName") {
                    &target["expr"]
                } else {
                    target
                };
                if let Some(path) = b.path(base) {
                    if let Some(first) = path.first() {
                        globals.insert(first.clone());
                    }
                }
            }
        }
        b.global_writes = globals;
        b
    }
    pub fn path(&self, n: &Value) -> Option<Vec<String>> {
        self.path_seen(n, &mut HashSet::new())
    }
    fn path_seen(&self, n: &Value, seen: &mut HashSet<String>) -> Option<Vec<String>> {
        let n = unparen(n);
        match kind(n) {
            "AstExprGlobal" => {
                let name = text(n, "global");
                (!self.global_writes.contains(name)).then(|| vec![name.to_owned()])
            }
            "AstExprIndexName" => {
                let mut base = self.path_seen(&n["expr"], seen)?;
                base.push(text(n, "index").to_owned());
                Some(base)
            }
            "AstExprBinary" if text(n, "op") == "And" => {
                let base = self.path_seen(&n["left"], &mut seen.clone())?;
                let member = self.path_seen(&n["right"], &mut seen.clone())?;
                (base.len() == 1 && member.first() == base.first()).then_some(member)
            }
            _ => {
                let key = local_key(n)?;
                if self.writes.contains(&key) || !seen.insert(key.clone()) {
                    return None;
                }
                self.path_seen(self.values.get(&key)?, seen)
            }
        }
    }
    pub fn is_path(&self, n: &Value, path: &[&str]) -> bool {
        self.path(n)
            .is_some_and(|p| p.iter().map(String::as_str).eq(path.iter().copied()))
    }
}
fn primitive(p: &[String]) -> bool {
    p.len() == 2
        && (p[0] == "bit32" && p[1] == "bxor"
            || p[0] == "buffer"
                && [
                    "len",
                    "create",
                    "readu8",
                    "writeu8",
                    "tostring",
                    "fromstring",
                ]
                .contains(&p[1].as_str()))
}
pub(crate) fn integer(n: &Value) -> Option<i64> {
    let n = unparen(n);
    if kind(n) != "AstExprConstantNumber" {
        return None;
    }
    let v = n["value"].as_f64()?;
    (v.is_finite() && v.fract() == 0.0 && v.abs() <= 9007199254740992.0).then_some(v as i64)
}

fn normalized(
    fn_: &Value,
    bindings: Option<&Bindings<'_>>,
    cache: Option<&str>,
    constants: &HashMap<usize, &str>,
) -> Value {
    let mut slots: HashMap<String, Value> = arr(fn_, "args")
        .iter()
        .enumerate()
        .map(|(i, p)| (lkey(p), json!(["arg", i])))
        .collect();
    fn slot(n: &Value, slots: &mut HashMap<String, Value>) -> Value {
        let key = lkey(n);
        let len = slots.len();
        slots
            .entry(key)
            .or_insert_with(|| json!(["local", len]))
            .clone()
    }
    fn norm(
        n: &Value,
        b: Option<&Bindings<'_>>,
        cache: Option<&str>,
        constants: &HashMap<usize, &str>,
        slots: &mut HashMap<String, Value>,
    ) -> Value {
        if n.is_null() {
            return Value::Null;
        }
        let n = unparen(n);
        if let Some(label) = constants.get(&id(n)) {
            return json!(["constant", label]);
        }
        let list = |key: &str, slots: &mut HashMap<String, Value>| -> Vec<Value> {
            arr(n, key)
                .iter()
                .map(|v| norm(v, b, cache, constants, slots))
                .collect()
        };
        match kind(n) {
            "AstExprLocal" => {
                let key = lkey(&n["local"]);
                if cache == Some(&key) {
                    return json!(["cache"]);
                }
                if let Some(path) = b.and_then(|bb| bb.path(n)) {
                    if primitive(&path) {
                        return json!(["path", path]);
                    }
                }
                slots
                    .get(&key)
                    .cloned()
                    .unwrap_or_else(|| json!(["capture", key]))
            }
            "AstExprGlobal" => {
                if text(n, "global") == "_CACHE" {
                    json!(["cache"])
                } else {
                    json!(["path", [n["global"]]])
                }
            }
            "AstExprIndexName" => {
                let base = norm(&n["expr"], b, cache, constants, slots);
                if base[0] == "path" {
                    let mut p = base[1].as_array().cloned().unwrap_or_default();
                    p.push(n["index"].clone());
                    json!(["path", p])
                } else {
                    json!(["field", base, n["index"]])
                }
            }
            "AstExprIndexExpr" => json!([
                "index",
                norm(&n["expr"], b, cache, constants, slots),
                norm(&n["index"], b, cache, constants, slots)
            ]),
            k if k.starts_with("AstExprConstant") => json!([k, n["value"]]),
            "AstExprBinary" => json!([
                "binary",
                n["op"],
                norm(&n["left"], b, cache, constants, slots),
                norm(&n["right"], b, cache, constants, slots)
            ]),
            "AstExprCall" => json!([
                "call",
                norm(&n["func"], b, cache, constants, slots),
                list("args", slots),
                n["self"].as_bool().unwrap_or(false)
            ]),
            "AstStatExpr" => json!(["expr", norm(&n["expr"], b, cache, constants, slots)]),
            "AstStatLocal" => {
                let vars: Vec<_> = arr(n, "vars").iter().map(|v| slot(v, slots)).collect();
                json!(["local", vars, list("values", slots)])
            }
            "AstStatAssign" => json!(["assign", list("vars", slots), list("values", slots)]),
            "AstStatBlock" => json!(list("body", slots)),
            "AstStatIf" => json!([
                "if",
                norm(&n["condition"], b, cache, constants, slots),
                norm(&n["thenbody"], b, cache, constants, slots),
                norm(&n["elsebody"], b, cache, constants, slots)
            ]),
            "AstStatFor" => {
                let var = slot(&n["var"], slots);
                json!([
                    "for",
                    var,
                    norm(&n["from"], b, cache, constants, slots),
                    norm(&n["to"], b, cache, constants, slots),
                    norm(&n["step"], b, cache, constants, slots),
                    norm(&n["body"], b, cache, constants, slots)
                ])
            }
            "AstStatReturn" => json!(["return", list("list", slots)]),
            _ => json!(["unsupported", kind(n)]),
        }
    }
    json!([
        arr(fn_, "args").len(),
        fn_["vararg"],
        norm(&fn_["body"], bindings, cache, constants, &mut slots)
    ])
}
const REFERENCE:&str="local function decode(input,key,id)\nif id and _CACHE[id] then return _CACHE[id] end\nlocal size=buffer.len(input)\nlocal output=buffer.create(size)\nfor offset=0,size-1 do\nbuffer.writeu8(output,offset,bit32.bxor(buffer.readu8(input,offset),key,offset%256))\nend\nlocal text=buffer.tostring(output)\nif id then _CACHE[id]=text end\nreturn text\nend";
const INLINE_REFERENCE:&str="return function()\nlocal input=buffer.fromstring('ab')\nlocal output=buffer.create(2)\nfor offset=0,1 do\nbuffer.writeu8(output,offset,bit32.bxor(buffer.readu8(input,offset),73,offset%256))\nend\nreturn buffer.tostring(output)\nend";
fn empty_cache(n: &Value, b: &Bindings<'_>) -> bool {
    let n = unparen(n);
    if kind(n) == "AstExprTable" {
        return arr(n, "items").is_empty();
    }
    if kind(n) != "AstExprCall"
        || !b.is_path(&n["func"], &["setmetatable"])
        || arr(n, "args").len() != 2
    {
        return false;
    }
    let table = unparen(&n["args"][0]);
    let mt = unparen(&n["args"][1]);
    kind(table) == "AstExprTable"
        && arr(table, "items").is_empty()
        && kind(mt) == "AstExprTable"
        && arr(mt, "items").len() == 1
        && arr(mt, "items").iter().all(|it| {
            kind(&it["key"]) == "AstExprConstantString"
                && kind(&it["value"]) == "AstExprConstantString"
                && it["key"]["value"] == "__mode"
                && it["value"]["value"] == "v"
        })
}
fn constant_call(n: &Value, b: &Bindings<'_>) -> Option<(Vec<u8>, u8, i64)> {
    if arr(n, "args").len() != 3 {
        return None;
    }
    let arg = unparen(&n["args"][0]);
    let key = integer(&n["args"][1])?;
    let cache_id = integer(&n["args"][2])?;
    if !(0..=255).contains(&key)
        || kind(arg) != "AstExprCall"
        || arg["self"] == true
        || !b.is_path(&arg["func"], &["buffer", "fromstring"])
        || arr(arg, "args").len() != 1
    {
        return None;
    }
    let value = unparen(&arg["args"][0]);
    if kind(value) != "AstExprConstantString" {
        return None;
    }
    let raw = unlatin1(text(value, "value")).ok()?;
    (raw.len() <= 200000).then_some((raw, key as u8, cache_id))
}
fn decoded_literal(call: &Value, b: &Bindings<'_>, bytes: &[u8]) -> String {
    let literal = lua_str(bytes);
    if b.parents
        .get(&id(call))
        .copied()
        .flatten()
        .is_some_and(|p| {
            matches!(
                kind(p),
                "AstExprIndexName" | "AstExprIndexExpr" | "AstExprCall"
            ) && (id(&p["expr"]) == id(call) || id(&p["func"]) == id(call))
        })
    {
        format!("({literal})")
    } else {
        literal
    }
}
pub(crate) fn shared_edits(
    root: &Value,
    b: &Bindings<'_>,
    src: &Src<'_>,
) -> Result<(Vec<Edit>, Value)> {
    let mut edits = vec![];
    let (mut strings, mut statements) = (0, 0);
    if ["buffer", "bit32"]
        .iter()
        .any(|name| b.global_writes.contains(*name))
    {
        return Ok((edits, json!({"strings":0,"statements":0})));
    }
    let mut reference = None;
    for (key, (decl, fn_)) in &b.defs {
        let fn_ = unparen(fn_);
        if kind(fn_) != "AstExprFunction"
            || b.writes.contains(key)
            || arr(fn_, "args").len() != 3
            || fn_["vararg"] == true
        {
            continue;
        }
        let (nodes, declared, _) = local_sets(fn_, Some(b));
        let inside: HashSet<_> = nodes.iter().map(|n| id(n)).collect();
        let captures: HashSet<_> = nodes
            .iter()
            .filter_map(|n| local_key(n))
            .filter(|k| !declared.contains(k))
            .collect();
        let caches: Vec<_> = captures
            .iter()
            .filter(|k| {
                b.defs
                    .get(*k)
                    .is_some_and(|(_, v)| !b.writes.contains(*k) && empty_cache(v, b))
            })
            .collect();
        if caches.len() != 1 {
            continue;
        }
        let cache = caches[0];
        if b.refs
            .get(cache)
            .is_some_and(|rs| rs.iter().any(|r| !inside.contains(&id(r))))
        {
            continue;
        }
        if reference.is_none() {
            let r = parse_ast(REFERENCE.as_bytes())?;
            reference = Some(normalized(
                &r["body"][0]["func"],
                None,
                None,
                &HashMap::new(),
            ));
        }
        if normalized(fn_, Some(b), Some(cache), &HashMap::new()) != *reference.as_ref().unwrap() {
            continue;
        }
        let mut calls = vec![];
        let mut valid = true;
        for r in b.refs.get(key).into_iter().flatten() {
            let mut parent = b.parents.get(&id(r)).copied().flatten();
            while parent.is_some_and(|p| kind(p) == "AstExprGroup") {
                parent = parent.and_then(|p| b.parents.get(&id(p)).copied().flatten());
            }
            let Some(p) = parent else {
                valid = false;
                break;
            };
            if kind(p) != "AstExprCall" || local_key(&p["func"]).as_ref() != Some(key) {
                valid = false;
                break;
            }
            let Some(value) = constant_call(p, b) else {
                valid = false;
                break;
            };
            calls.push((p, value));
        }
        if !valid || calls.is_empty() {
            continue;
        }
        let mut ids = HashMap::new();
        for (_, (cipher, key, cid)) in &calls {
            if ids
                .insert(*cid, (cipher, key))
                .is_some_and(|old| old != (cipher, key))
            {
                valid = false;
                break;
            }
        }
        if !valid {
            continue;
        }
        for (call, (cipher, key, _)) in &calls {
            let decoded: Vec<_> = cipher
                .iter()
                .enumerate()
                .map(|(i, x)| x ^ key ^ (i as u8))
                .collect();
            let (a, z) = src.span(call)?;
            edits.push((a, z, decoded_literal(call, b, &decoded)));
        }
        let mut remove = HashSet::from([key.clone(), cache.clone()]);
        for k in &captures {
            if b.defs
                .get(k)
                .and_then(|(_, v)| b.path(v))
                .is_some_and(|p| primitive(&p))
                && b.refs
                    .get(k)
                    .is_none_or(|rs| rs.iter().all(|r| inside.contains(&id(r))))
            {
                remove.insert(k.clone());
            }
        }
        for k in &remove {
            let n = if k == key { *decl } else { b.defs[k].0 };
            let (a, z) = src.span(n)?;
            edits.push((a, z, String::new()));
        }
        strings += calls.len();
        statements += remove.len();
    }
    let _ = root;
    Ok((edits, json!({"strings":strings,"statements":statements})))
}
fn inline_parts(fn_: &Value, b: Option<&Bindings<'_>>) -> Option<(Value, Vec<u8>, u8)> {
    let body = arr(&fn_["body"], "body");
    if !arr(fn_, "args").is_empty() || fn_["vararg"] == true || body.len() != 4 {
        return None;
    }
    let cipher = unparen(&unparen(&body[0]["values"][0])["args"][0]);
    let size = unparen(&unparen(&body[1]["values"][0])["args"][0]);
    let last = unparen(&body[2]["to"]);
    let expr = unparen(&body[2]["body"]["body"][0]["expr"]);
    let xor = unparen(&expr["args"][2]);
    let key = unparen(&xor["args"][1]);
    if kind(cipher) != "AstExprConstantString" {
        return None;
    }
    let raw = unlatin1(text(cipher, "value")).ok()?;
    let xor_key = integer(key)?;
    let stop = integer(last).or_else(|| {
        (kind(last) == "AstExprUnary" && text(last, "op") == "Minus")
            .then(|| integer(&last["expr"]).and_then(i64::checked_neg))
            .flatten()
    })?;
    if raw.len() > 200000
        || integer(size)? != raw.len() as i64
        || stop != raw.len() as i64 - 1
        || !(0..=255).contains(&xor_key)
    {
        return None;
    }
    let constants = HashMap::from([
        (id(cipher), "bytes"),
        (id(size), "size"),
        (id(last), "last"),
        (id(key), "key"),
    ]);
    Some((normalized(fn_, b, None, &constants), raw, xor_key as u8))
}
fn decoder_edits(root: &Value, b: &Bindings<'_>, src: &Src<'_>) -> Result<(Vec<Edit>, Value)> {
    let (mut edits, mut counts) = shared_edits(root, b, src)?;
    if ["buffer", "bit32"]
        .iter()
        .any(|x| b.global_writes.contains(*x))
    {
        return Ok((edits, counts));
    }
    let mut reference = None;
    for (call, _) in &b.nodes {
        if kind(call) != "AstExprCall" || !arr(call, "args").is_empty() || call["self"] == true {
            continue;
        }
        let fn_ = unparen(&call["func"]);
        if kind(fn_) != "AstExprFunction" {
            continue;
        }
        let Some((shape, cipher, key)) = inline_parts(fn_, Some(b)) else {
            continue;
        };
        if reference.is_none() {
            let r = parse_ast(INLINE_REFERENCE.as_bytes())?;
            reference = inline_parts(&r["body"][0]["list"][0], None).map(|x| x.0);
        }
        if reference.as_ref() != Some(&shape) {
            continue;
        }
        let decoded: Vec<_> = cipher
            .iter()
            .enumerate()
            .map(|(i, x)| x ^ key ^ (i as u8))
            .collect();
        let (a, z) = src.span(call)?;
        edits.push((a, z, decoded_literal(call, b, &decoded)));
        counts["strings"] = json!(counts["strings"].as_u64().unwrap_or(0) + 1);
    }
    Ok((outer(edits), counts))
}
pub(crate) fn local_sets<'a>(
    fn_: &'a Value,
    b: Option<&Bindings<'_>>,
) -> (Vec<&'a Value>, HashSet<String>, HashSet<String>) {
    let nodes: Vec<_> = walk(fn_).into_iter().map(|x| x.0).collect();
    let (mut declared, mut private) = (HashSet::new(), HashSet::new());
    for n in &nodes {
        match kind(n) {
            "AstExprFunction" => declared.extend(arr(n, "args").iter().map(lkey)),
            "AstStatLocal" | "AstStatForIn" => {
                declared.extend(arr(n, "vars").iter().map(lkey));
                if kind(n) == "AstStatLocal" {
                    for (v, value) in arr(n, "vars").iter().zip(arr(n, "values")) {
                        if kind(unparen(value)) == "AstExprTable" {
                            private.insert(lkey(v));
                        }
                    }
                    if let Some(b) = b {
                        if arr(n, "values").len() == 1 {
                            let call = unparen(&n["values"][0]);
                            if kind(call) == "AstExprCall" {
                                if b.is_path(&call["func"], &["buffer", "create"])
                                    && arr(n, "vars").len() == 1
                                {
                                    private.insert(lkey(&n["vars"][0]));
                                } else if b.is_path(&call["func"], &["pcall"])
                                    && !arr(call, "args").is_empty()
                                    && b.is_path(&call["args"][0], &["buffer", "create"])
                                    && arr(n, "vars").len() == 2
                                {
                                    private.insert(lkey(&n["vars"][1]));
                                }
                            }
                        }
                    }
                }
            }
            "AstStatLocalFunction" => {
                declared.insert(lkey(&n["name"]));
            }
            "AstStatFor" => {
                declared.insert(lkey(&n["var"]));
            }
            _ => {}
        }
    }
    (nodes, declared, private)
}
pub(crate) fn state_integer(n: &Value, b: &Bindings<'_>, depth: usize) -> Option<i64> {
    if depth > 32 {
        return None;
    }
    let n = unparen(n);
    match kind(n) {
        "AstExprConstantNumber" => integer(n),
        "AstExprLocal" => {
            let key = local_key(n)?;
            if b.writes.contains(&key) {
                return None;
            }
            let value = unparen(b.values.get(&key)?);
            // Repeated expression aliases can expand exponentially; fold them in passes.
            if !matches!(kind(value), "AstExprConstantNumber" | "AstExprLocal") {
                return None;
            }
            state_integer(value, b, depth + 1)
        }
        "AstExprUnary" if text(n, "op") == "Minus" => {
            state_integer(&n["expr"], b, depth + 1)?.checked_neg()
        }
        "AstExprBinary" if ["Add", "Sub", "Mul"].contains(&text(n, "op")) => {
            let a = state_integer(&n["left"], b, depth + 1)?;
            let z = state_integer(&n["right"], b, depth + 1)?;
            let v = match text(n, "op") {
                "Add" => a.checked_add(z),
                "Sub" => a.checked_sub(z),
                _ => a.checked_mul(z),
            }?;
            (v.unsigned_abs() <= 9007199254740992).then_some(v)
        }
        "AstExprCall" if n["self"] != true => {
            let p = b.path(&n["func"])?;
            if p.len() != 2
                || p[0] != "bit32"
                || !["bxor", "bor", "band", "bnot"].contains(&p[1].as_str())
            {
                return None;
            }
            let vals: Vec<_> = arr(n, "args")
                .iter()
                .map(|a| state_integer(a, b, depth + 1).and_then(|v| u32::try_from(v).ok()))
                .collect::<Option<_>>()?;
            let first = *vals.first()?;
            if p[1] == "bnot" {
                return (vals.len() == 1).then_some((!first) as i64);
            }
            Some(vals[1..].iter().fold(first, |a, z| match p[1].as_str() {
                "bxor" => a ^ z,
                "bor" => a | z,
                _ => a & z,
            }) as i64)
        }
        _ => None,
    }
}
fn state_number_edits(b: &Bindings<'_>, src: &Src<'_>) -> Result<Vec<Edit>> {
    let selectors: HashSet<_> = b
        .nodes
        .iter()
        .filter(|(n, _)| kind(n) == "AstStatWhile")
        .filter_map(|(n, _)| local_key(&n["condition"]))
        .collect();
    let mut edits = vec![];
    for (n, _) in &b.nodes {
        let v = match kind(n) {
            "AstStatLocal"
                if arr(n, "vars").len() == 1
                    && arr(n, "values").len() == 1
                    && selectors.contains(&lkey(&n["vars"][0])) =>
            {
                Some(&n["values"][0])
            }
            "AstStatAssign"
                if arr(n, "vars").len() == 1
                    && arr(n, "values").len() == 1
                    && local_key(&n["vars"][0]).is_some_and(|k| selectors.contains(&k)) =>
            {
                Some(&n["values"][0])
            }
            "AstExprBinary" if text(n, "op") == "CompareEq" => {
                if local_key(&n["left"]).is_some_and(|k| selectors.contains(&k)) {
                    Some(&n["right"])
                } else if local_key(&n["right"]).is_some_and(|k| selectors.contains(&k)) {
                    Some(&n["left"])
                } else {
                    None
                }
            }
            _ => None,
        };
        if let Some(v) = v {
            if kind(unparen(v)) != "AstExprConstantNumber" {
                if let Some(number) = state_integer(v, b, 0) {
                    let (a, z) = src.span(v)?;
                    edits.push((
                        a,
                        z,
                        if number < 0 {
                            format!("({number})")
                        } else {
                            number.to_string()
                        },
                    ));
                }
            }
        }
    }
    Ok(outer(edits))
}
fn features(
    fn_: &Value,
    b: &Bindings<'_>,
) -> (HashSet<String>, HashSet<Vec<String>>, HashSet<String>) {
    let mut globals = HashSet::new();
    let mut paths = HashSet::new();
    let mut strings = HashSet::new();
    for (n, _) in walk(fn_) {
        match kind(n) {
            "AstExprGlobal" => {
                globals.insert(text(n, "global").to_owned());
                if let Some(p) = b.path(n) {
                    paths.insert(p);
                }
            }
            "AstExprIndexName" => {
                if let Some(p) = b.path(n) {
                    paths.insert(p);
                }
            }
            "AstExprConstantString" => {
                strings.insert(text(n, "value").to_owned());
            }
            _ => {}
        }
    }
    (globals, paths, strings)
}
fn has_path(paths: &HashSet<Vec<String>>, p: &[&str]) -> bool {
    paths.contains(&p.iter().map(|x| x.to_string()).collect::<Vec<_>>())
}
fn verifier(fn_: &Value, b: &Bindings<'_>) -> bool {
    let (_, paths, strings) = features(fn_, b);
    strings.contains("[C]")
        && (has_path(&paths, &["debug", "info"]) || has_path(&paths, &["string", "dump"]))
}
fn literal_number_edits(b: &Bindings<'_>, src: &Src<'_>) -> Result<Vec<Edit>> {
    let mut protected = HashSet::new();
    for (n, _) in &b.nodes {
        if kind(n) == "AstExprFunction" && verifier(n, b)
            || kind(n) == "AstExprBinary"
                && ["CompareEq", "CompareNe"].contains(&text(n, "op"))
                && ["left", "right"]
                    .iter()
                    .all(|k| state_integer(&n[*k], b, 0).is_some())
        {
            protected.extend(walk(n).iter().map(|(v, _)| id(v)));
        }
    }
    let mut edits = vec![];
    for (n, parent) in &b.nodes {
        if protected.contains(&id(n))
            || !["AstExprCall", "AstExprBinary", "AstExprUnary"].contains(&kind(n))
        {
            continue;
        }
        let Some(v) = state_integer(n, b, 0) else {
            continue;
        };
        if !walk(n).iter().any(|(x, _)| kind(x) == "AstExprCall") {
            continue;
        }
        let receiver = parent.is_some_and(|p| {
            matches!(kind(p), "AstExprIndexName" | "AstExprIndexExpr") && id(&p["expr"]) == id(n)
                || kind(p) == "AstExprCall" && id(&p["func"]) == id(n)
        });
        let concat =
            parent.is_some_and(|p| kind(p) == "AstExprBinary" && text(p, "op") == "Concat");
        let (a, z) = src.span(n)?;
        edits.push((
            a,
            z,
            if v < 0 || receiver || concat {
                format!("({v})")
            } else {
                v.to_string()
            },
        ));
    }
    Ok(outer(edits))
}
#[allow(dead_code)] // Reference opt-in helper; public cleanup intentionally retains application helpers.
pub(crate) fn inert(n: &Value, b: &Bindings<'_>) -> bool {
    let n = unparen(n);
    match kind(n) {
        "AstExprFunction" | "AstExprGlobal" | "AstExprLocal" => true,
        k if k.starts_with("AstExprConstant") => true,
        "AstExprTable" => arr(n, "items")
            .iter()
            .all(|it| (it["key"].is_null() || inert(&it["key"], b)) && inert(&it["value"], b)),
        "AstExprUnary" => text(n, "op") == "Not" && inert(&n["expr"], b),
        "AstExprBinary" => {
            ["And", "Or", "CompareEq", "CompareNe"].contains(&text(n, "op"))
                && inert(&n["left"], b)
                && inert(&n["right"], b)
        }
        "AstExprCall" if n["self"] != true => {
            let args = arr(n, "args");
            if ["type", "typeof", "getfenv"]
                .iter()
                .any(|p| b.is_path(&n["func"], &[p]))
            {
                return args.iter().all(|a| inert(a, b));
            }
            b.is_path(&n["func"], &["setmetatable"])
                && args.len() == 2
                && kind(unparen(&args[0])) == "AstExprTable"
                && arr(unparen(&args[0]), "items").is_empty()
                && inert(&args[1], b)
        }
        _ => false,
    }
}

#[allow(dead_code)] // Complete reference pass, not part of ordinary source cleanup.
pub(crate) fn dead_helper_edits(
    root: &Value,
    b: &Bindings<'_>,
    src: &Src<'_>,
) -> Result<Vec<Edit>> {
    let body = arr(root, "body");
    let starts: Vec<_> = body
        .iter()
        .map(|n| src.span(n).map(|s| s.0))
        .collect::<Result<_>>()?;
    let mut candidates = HashMap::new();
    for (i, n) in body.iter().enumerate() {
        let key = match kind(n) {
            "AstStatLocalFunction" => Some(lkey(&n["name"])),
            "AstStatLocal"
                if arr(n, "vars").len() == 1
                    && arr(n, "values").len() == 1
                    && inert(&n["values"][0], b) =>
            {
                Some(lkey(&n["vars"][0]))
            }
            _ => None,
        };
        if let Some(k) = key {
            if !b.writes.contains(&k) {
                candidates.insert(k, i);
            }
        }
    }
    let mut mentions = vec![HashSet::new(); body.len()];
    for (k, refs) in &b.refs {
        for r in refs {
            let a = src.span(r)?.0;
            let index = starts.partition_point(|s| *s <= a);
            if index > 0 {
                mentions[index - 1].insert(k.clone());
            }
        }
    }
    let eligible: HashSet<_> = candidates
        .iter()
        .filter(|(k, i)| {
            kind(&body[**i]) == "AstStatLocalFunction"
                || kind(unparen(&body[**i]["values"][0])) == "AstExprFunction"
                || b.refs.get(*k).is_some_and(|rs| !rs.is_empty())
        })
        .map(|(k, _)| k.clone())
        .collect();
    let owner: HashMap<_, _> = candidates.iter().map(|(k, i)| (*i, k)).collect();
    let mut todo = vec![];
    for (i, keys) in mentions.iter().enumerate() {
        if owner.get(&i).is_none_or(|k| !eligible.contains(*k)) {
            todo.extend(keys.iter().filter(|k| eligible.contains(*k)).cloned());
        }
    }
    let mut live = HashSet::new();
    while let Some(key) = todo.pop() {
        if live.insert(key.clone()) {
            todo.extend(
                mentions[candidates[&key]]
                    .iter()
                    .filter(|k| eligible.contains(*k) && !live.contains(*k))
                    .cloned(),
            );
        }
    }
    let mut edits = vec![];
    for key in eligible.difference(&live) {
        let (a, z) = src.span(&body[candidates[key]])?;
        edits.push((a, z, String::new()));
    }
    Ok(edits)
}

fn merged_declarations(b: &Bindings<'_>, src: &Src<'_>) -> Result<Vec<Edit>> {
    let mut edits = vec![];
    for (block, parent) in &b.nodes {
        if kind(block) != "AstStatBlock" || parent.is_some_and(|p| kind(p) == "AstStatRepeat") {
            continue;
        }
        let body = arr(block, "body");
        let mut bare = HashMap::new();
        for (i, n) in body.iter().enumerate() {
            if kind(n) == "AstStatLocal" && arr(n, "values").is_empty() && arr(n, "vars").len() == 1
            {
                let var = &n["vars"][0];
                if var["luauType"].is_null() && var["isConst"] != true {
                    bare.insert(lkey(var), i);
                }
            }
        }
        for (where_, target) in body.iter().enumerate() {
            if bare.is_empty() || kind(target) != "AstStatAssign" {
                continue;
            }
            let Some(keys) = arr(target, "vars")
                .iter()
                .map(local_key)
                .collect::<Option<Vec<_>>>()
            else {
                continue;
            };
            let unique: HashSet<_> = keys.iter().collect();
            if unique.len() != keys.len()
                || keys
                    .iter()
                    .any(|k| bare.get(k).is_none_or(|i| *i >= where_))
            {
                continue;
            }
            let (first, last) = src.span(target)?;
            let mut valid = true;
            for k in &keys {
                let refs = b.refs.get(k).map(Vec::as_slice).unwrap_or(&[]);
                let positions = refs
                    .iter()
                    .map(|r| src.span(r).map(|s| s.0))
                    .collect::<Result<Vec<_>>>()?;
                if positions.iter().min().is_none_or(|p| *p < first)
                    || positions
                        .iter()
                        .filter(|p| first <= **p && **p < last)
                        .count()
                        != 1
                {
                    valid = false;
                    break;
                }
            }
            if valid {
                for k in &keys {
                    let (a, z) = src.span(&body[bare[k]])?;
                    edits.push((a, z, String::new()));
                }
                edits.push((first, first, "local ".to_owned()));
            }
        }
    }
    Ok(edits)
}
pub(crate) fn cleanup_helpers(source: &str, root: &Value) -> Result<(String, Value)> {
    let src = Src::new(source);
    let b = Bindings::new(root);
    let (mut edits, mut counts) = shared_edits(root, &b, &src)?;
    if edits.is_empty() {
        edits = merged_declarations(&b, &src)?;
        counts["statements"] = json!(
            counts["statements"].as_u64().unwrap_or(0)
                + edits.iter().filter(|e| e.2.is_empty()).count() as u64
        );
    }
    Ok((apply(source, edits)?, counts))
}
fn predicate_shape(n: &Value, b: &Bindings<'_>) -> Value {
    let n = unparen(n);
    match kind(n) {
        "AstExprGlobal" | "AstExprLocal" | "AstExprIndexName" => json!(["path", b.path(n)]),
        "AstExprConstantNumber" => json!(["number", integer(n)]),
        "AstExprBinary" => json!([
            n["op"],
            predicate_shape(&n["left"], b),
            predicate_shape(&n["right"], b)
        ]),
        "AstExprCall" if n["self"] != true => json!([
            "call",
            predicate_shape(&n["func"], b),
            arr(n, "args")
                .iter()
                .map(|a| predicate_shape(a, b))
                .collect::<Vec<_>>()
        ]),
        _ => Value::Null,
    }
}
const CLOCK_PATTERNS: [(&str, bool); 7] = [
    ("bit32.band(math.floor(os.clock()*100),0)==0", true),
    ("math.abs(math.floor(os.clock()*10)-999999)<0", false),
    (
        "bit32.bor(math.floor(os.clock()*50),4294967295)==4294967295",
        true,
    ),
    ("math.floor(math.abs(os.clock())*10)%1==0", true),
    ("math.floor(os.clock())*2%2==0", true),
    ("math.abs(os.clock())+1>0", true),
    ("bit32.bxor(1337,7331)+math.floor(os.clock()*50)<0", false),
];
const PREDICATE_BODIES: [&str; 5] = [
    "return bit32.bor(argument,13)-bit32.band(argument,13)==bit32.bxor(argument,13)",
    "local masked=bit32.band(argument,65535)\nreturn masked*(masked+1)%2==0",
    "local masked=bit32.band(argument,65535)\nreturn (masked*masked-masked)%2==0",
    "return bit32.band(bit32.bor(argument,6),6)==6",
    "return bit32.bxor(argument,bit32.bnot(argument))==4294967295",
];
fn private_predicate_checks(
    b: &Bindings<'_>,
    src: &Src<'_>,
    templates: &[Value],
    clock_shape: &Value,
) -> Result<HashMap<usize, String>> {
    let mut out = HashMap::new();
    // shortcut: any local bit32 defers inlining; check destination scopes if this blocks useful helpers.
    if ["bit32", "math", "os"]
        .iter()
        .any(|name| b.global_writes.contains(*name))
        || b.nodes
            .iter()
            .any(|(node, _)| kind(node) == "AstLocal" && text(node, "name") == "bit32")
    {
        return Ok(out);
    }
    let names: HashSet<_> = b
        .nodes
        .iter()
        .filter_map(|(n, _)| match kind(n) {
            "AstLocal" => Some(text(n, "name")),
            "AstExprGlobal" => Some(text(n, "global")),
            _ => None,
        })
        .collect();
    let mut suffix = 0;
    let temporary = loop {
        let candidate = format!("predicateValue{suffix}");
        if !names.contains(candidate.as_str()) {
            break candidate;
        }
        suffix += 1;
    };
    for (declaration, _) in &b.nodes {
        if kind(declaration) != "AstStatLocalFunction" {
            continue;
        }
        let key = lkey(&declaration["name"]);
        let function = &declaration["func"];
        if arr(function, "args").len() != 1
            || arr(&function["body"], "body").len() > 2
            || b.writes.contains(&key)
            || !templates.contains(&normalized(function, None, None, &HashMap::new()))
        {
            continue;
        }
        let parameter = lkey(&function["args"][0]);
        let mut calls = vec![];
        let mut valid = true;
        for reference in b.refs.get(&key).into_iter().flatten() {
            let Some(call) = b.parents.get(&id(reference)).copied().flatten() else {
                valid = false;
                break;
            };
            let Some(condition) = b.parents.get(&id(call)).copied().flatten() else {
                valid = false;
                break;
            };
            if kind(call) != "AstExprCall"
                || call["self"] == true
                || id(&call["func"]) != id(reference)
                || kind(condition) != "AstStatIf"
                || id(&condition["condition"]) != id(call)
                || arr(call, "args").len() != 1
            {
                valid = false;
                break;
            }
            let argument = unparen(&call["args"][0]);
            if kind(argument) != "AstExprBinary"
                || text(argument, "op") != "Add"
                || predicate_shape(&argument["left"], b) != *clock_shape
                || integer(&argument["right"]).is_none()
            {
                valid = false;
                break;
            }
            calls.push((call, argument));
        }
        if !valid || calls.is_empty() {
            continue;
        }
        let body = arr(&function["body"], "body");
        let first = src.span(&body[0])?.0;
        let last = src.span(body.last().unwrap())?.1;
        let mut replacements = vec![];
        for (node, _) in walk(&function["body"]) {
            if kind(node) == "AstExprLocal" && local_key(node).as_deref() == Some(&parameter) {
                let (a, z) = src.span(node)?;
                replacements.push((a - first, z - first, temporary.clone()));
            }
        }
        let return_start = src.span(body.last().unwrap())?.0;
        replacements.push((
            return_start - first,
            return_start - first + "return".len(),
            format!("local {temporary} ="),
        ));
        let body =
            crate::unflatten::dedent(&apply(&src.text[first..last], replacements)?).join("\n");
        for (call, argument) in calls {
            // Keep the argument and native operations even when their boolean result is known.
            out.insert(
                id(call),
                format!(
                    "do\n\tlocal {temporary} = ({})\n{}\nend",
                    src.slice(argument)?,
                    body.lines()
                        .map(|line| format!("\t{line}"))
                        .collect::<Vec<_>>()
                        .join("\n")
                ),
            );
        }
    }
    Ok(out)
}
fn clock_check_edits(source: &str, root: &Value) -> Result<(String, Value)> {
    let pattern_source = format!(
        "return {},{},math.floor(os.clock()*100)",
        CLOCK_PATTERNS
            .iter()
            .map(|p| p.0)
            .collect::<Vec<_>>()
            .join(","),
        PREDICATE_BODIES
            .iter()
            .map(|body| format!("function(argument) {body} end"))
            .collect::<Vec<_>>()
            .join(",")
    );
    let patterns = parse_ast(pattern_source.as_bytes())?;
    let pb = Bindings::new(&patterns);
    let shapes: Vec<_> = arr(&patterns["body"][0], "list")
        .iter()
        .zip(CLOCK_PATTERNS)
        .map(|(n, (_, v))| (predicate_shape(n, &pb), v))
        .collect();
    let src = Src::new(source);
    let b = Bindings::new(root);
    let expressions = arr(&patterns["body"][0], "list");
    let templates: Vec<_> = expressions[CLOCK_PATTERNS.len()..expressions.len() - 1]
        .iter()
        .map(|function| normalized(function, Some(&pb), None, &HashMap::new()))
        .collect();
    let private = private_predicate_checks(
        &b,
        &src,
        &templates,
        &predicate_shape(expressions.last().unwrap(), &pb),
    )?;
    let mut edits = vec![];
    let mut arms = HashMap::new();
    for (n, parent) in &b.nodes {
        if kind(n) != "AstStatIf" {
            continue;
        }
        let shape = predicate_shape(&n["condition"], &b);
        let prelude = private.get(&id(&n["condition"]));
        let known = prelude.map(|_| true).or_else(|| {
            shapes
                .iter()
                .find(|(s, _)| *s == shape)
                .map(|(_, known)| *known)
        });
        let Some(known) = known else {
            continue;
        };
        if walk(n).iter().any(|(x, _)| {
            kind(x) == "AstExprConstantString" && src.slice(x).is_ok_and(|s| s.contains('\n'))
        }) {
            continue;
        }
        let (first, mut last) = src.span(n)?;
        let line = source[..first].rfind('\n').map_or(0, |i| i + 1);
        let prefix = &source[line..first];
        let indent = &prefix[..prefix.len() - prefix.trim_start_matches([' ', '\t']).len()];
        let is_elseif =
            parent.is_some_and(|p| kind(p) == "AstStatIf" && id(&p["elsebody"]) == id(n));
        let selected = if known {
            &n["thenbody"]
        } else {
            &n["elsebody"]
        };
        let replacement = if kind(selected) == "AstStatIf" {
            let s = src.slice(selected)?;
            if !is_elseif && s.starts_with("elseif") {
                format!("if{}", &s[6..])
            } else {
                s.to_owned()
            }
        } else {
            let body = arr(selected, "body");
            let content = if body.is_empty() {
                ""
            } else {
                &source[src.span(&body[0])?.0..src.span(body.last().unwrap())?.1]
            };
            let mut lines = if content.is_empty() {
                vec![]
            } else {
                crate::unflatten::dedent(content)
            };
            if let Some(prelude) = prelude {
                lines.splice(0..0, prelude.lines().map(str::to_owned));
            }
            let scoped = body
                .iter()
                .any(|s| matches!(kind(s), "AstStatLocal" | "AstStatLocalFunction"));
            let replacement = if is_elseif || scoped {
                if lines.is_empty() && is_elseif {
                    "end".to_owned()
                } else {
                    format!(
                        "{}\n{}\n{indent}end",
                        if is_elseif { "else" } else { "do" },
                        lines
                            .iter()
                            .map(|l| if l.trim().is_empty() {
                                String::new()
                            } else {
                                format!("{indent}\t{l}")
                            })
                            .collect::<Vec<_>>()
                            .join("\n")
                    )
                }
            } else {
                lines
                    .iter()
                    .enumerate()
                    .map(|(i, l)| {
                        if i == 0 {
                            l.clone()
                        } else if l.trim().is_empty() {
                            String::new()
                        } else {
                            format!("{indent}{l}")
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            if !is_elseif
                && body.last().is_some_and(|s| {
                    matches!(
                        kind(s),
                        "AstStatReturn" | "AstStatBreak" | "AstStatContinue"
                    )
                })
                && parent.is_some_and(|p| kind(p) == "AstStatBlock")
            {
                if let Some(st) = parent.and_then(|p| arr(p, "body").last()) {
                    last = src.span(st)?.1;
                }
            }
            replacement
        };
        arms.insert((first, last), (!known || !n["elsebody"].is_null()) as u64);
        edits.push((first, last, replacement));
    }
    let kept = outer(edits);
    let count = kept.len();
    let dropped: u64 = kept.iter().map(|(a, z, _)| arms[&(*a, *z)]).sum();
    let cleaned = apply(source, kept)?;
    if private.is_empty() || count == 0 {
        return Ok((cleaned, json!({"checks":count,"arms":dropped})));
    }
    let root = parse_ast(cleaned.as_bytes())?;
    let b = Bindings::new(&root);
    let src = Src::new(&cleaned);
    let mut declarations = vec![];
    for (node, _) in &b.nodes {
        if kind(node) == "AstStatLocalFunction"
            && !b.writes.contains(&lkey(&node["name"]))
            && b.refs.get(&lkey(&node["name"])).is_none_or(Vec::is_empty)
            && templates.contains(&normalized(&node["func"], None, None, &HashMap::new()))
        {
            let (a, z) = src.span(node)?;
            declarations.push((a, z, String::new()));
        }
    }
    let statements = declarations.len();
    Ok((
        apply(&cleaned, declarations)?,
        json!({"checks":count,"arms":dropped,"statements":statements}),
    ))
}
const SERVICES:&str="Players ReplicatedStorage ReplicatedFirst Workspace RunService VirtualUser VirtualInputManager TweenService UserInputService CollectionService BadgeService CoreGui HttpService Lighting StarterGui StarterPack StarterPlayer TeleportService MarketplaceService SoundService ContextActionService GuiService Debris ServerStorage ServerScriptService TextService TextChatService PathfindingService ProximityPromptService Stats GroupService LocalizationService PhysicsService NetworkClient ScriptContext SocialService UserService VoiceChatService AssetService ContentProvider InsertService KeyframeSequenceProvider LogService MaterialService Teams TestService";
const KEYWORDS:&str="and break do else elseif end false for function if in local nil not or repeat return then true until while continue type export";
fn generic(name: &str) -> bool {
    let base = name.trim_end_matches(|c: char| c.is_ascii_digit());
    [
        "value",
        "require",
        "items",
        "clone",
        "flag",
        "number",
        "text",
        "waitForChild",
        "findFirstChild",
        "frame",
        "screenGui",
        "scrollingFrame",
        "textLabel",
        "textButton",
        "textBox",
        "imageLabel",
        "imageButton",
    ]
    .contains(&base)
}
fn ident(name: &str) -> bool {
    let mut c = name.chars();
    c.next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && c.all(|c| c.is_ascii_alphanumeric() || c == '_')
}
fn camel(s: &str) -> String {
    let caps = s.bytes().take_while(u8::is_ascii_uppercase).count();
    if caps == 0 {
        return s.to_owned();
    }
    let down = if caps == 1 || caps == s.len() {
        caps
    } else {
        caps - 1
    };
    format!("{}{}", s[..down].to_ascii_lowercase(), &s[down..])
}
fn label(value: &str) -> Option<String> {
    if value.len() > 48 {
        return None;
    }
    static LABEL: OnceLock<regex::Regex> = OnceLock::new();
    let valid = LABEL.get_or_init(|| {
        regex::Regex::new(r"^[A-Za-z][A-Za-z0-9]*(?:[ _-]+[A-Za-z0-9]+){0,4}$").unwrap()
    });
    if !valid.is_match(value) {
        return None;
    }
    let words: Vec<_> = value
        .split([' ', '_', '-'])
        .filter(|w| !w.is_empty())
        .collect();
    let mut out = camel(words[0]);
    for word in &words[1..] {
        out.push_str(&word[..1].to_ascii_uppercase());
        if word.bytes().all(|c| !c.is_ascii_lowercase()) {
            out.push_str(&word[1..].to_ascii_lowercase());
        } else {
            out.push_str(&word[1..]);
        }
    }
    Some(out)
}
fn file_role(n: &Value) -> Option<&'static str> {
    let n = unparen(n);
    if kind(n) != "AstExprConstantString" {
        return None;
    }
    let v = text(n, "value").to_ascii_lowercase();
    if v.ends_with("settings.json") {
        Some("settings")
    } else if v.ends_with("config.json") {
        Some("config")
    } else {
        None
    }
}
fn initializer_hint(n: &Value, b: &Bindings<'_>) -> Option<String> {
    let n = unparen(n);
    match kind(n) {
        "AstExprBinary" if text(n, "op") == "Or" => initializer_hint(&n["left"], b),
        "AstExprIndexName" => label(text(n, "index")),
        "AstExprIndexExpr" => {
            let index = unparen(&n["index"]);
            if kind(index) == "AstExprConstantString" {
                label(text(index, "value"))
            } else {
                None
            }
        }
        "AstExprConstantString" => file_role(n).map(|r| format!("{r}File")),
        "AstExprCall" if !arr(n, "args").is_empty() => {
            let arg = unparen(&n["args"][0]);
            let value = text(arg, "value");
            if arr(n, "args").len() == 1
                && kind(arg) == "AstExprConstantString"
                && SERVICES.split_whitespace().any(|s| s == value)
            {
                return Some(value.to_owned());
            }
            let func = unparen(&n["func"]);
            if n["self"] == true
                && kind(func) == "AstExprIndexName"
                && [
                    "GetService",
                    "WaitForChild",
                    "FindFirstChild",
                    "FindFirstChildOfClass",
                    "FindFirstChildWhichIsA",
                ]
                .contains(&text(func, "index"))
            {
                return if kind(arg) == "AstExprConstantString" && ident(value) {
                    label(value)
                } else {
                    None
                };
            }
            if b.is_path(func, &["require"]) && arr(n, "args").len() == 1 {
                return initializer_hint(arg, b);
            }
            None
        }
        _ => None,
    }
}
fn inferred_name_edits(b: &Bindings<'_>, src: &Src<'_>) -> Result<(Vec<Edit>, usize)> {
    let mut reserved: HashSet<String> = KEYWORDS.split_whitespace().map(str::to_owned).collect();
    let mut protected = HashSet::new();
    let mut type_prefixes = HashSet::new();
    let mut labels: HashMap<(String, String), BTreeSet<Option<String>>> = HashMap::new();
    let mut roles: HashMap<String, BTreeSet<String>> = HashMap::new();
    for (n, _) in &b.nodes {
        if kind(n) == "AstTypeReference" && !n["prefixLocal"].is_null() {
            type_prefixes.insert(lkey(&n["prefixLocal"]));
        }
        match kind(n) {
            "AstLocal" => {
                reserved.insert(text(n, "name").to_owned());
            }
            "AstExprGlobal" => {
                reserved.insert(text(n, "global").to_owned());
            }
            "AstExprFunction" if verifier(n, b) => {
                protected.extend(walk(n).iter().map(|(x, _)| id(x)))
            }
            _ => {}
        }
        for (index, target) in targets(n).iter().enumerate() {
            let target = unparen(target);
            if !matches!(kind(target), "AstExprIndexName" | "AstExprIndexExpr") {
                continue;
            }
            let Some(key) = local_key(&target["expr"]) else {
                continue;
            };
            let member = if kind(target) == "AstExprIndexName" {
                Some(text(target, "index"))
            } else {
                let idx = unparen(&target["index"]);
                (kind(idx) == "AstExprConstantString").then(|| text(idx, "value"))
            };
            let value = if kind(n) == "AstStatAssign" {
                arr(n, "values").get(index).map(unparen)
            } else {
                None
            };
            let value = value
                .filter(|v| kind(v) == "AstExprConstantString")
                .map(|v| text(v, "value").to_owned());
            for field in ["Name", "Text"] {
                if member.is_none() || member == Some(field) {
                    labels
                        .entry((key.clone(), field.to_owned()))
                        .or_default()
                        .insert(if member.is_some() {
                            value.clone()
                        } else {
                            None
                        });
                }
            }
        }
        if kind(n) == "AstExprCall"
            && b.is_path(&n["func"], &["writefile"])
            && arr(n, "args").len() == 2
        {
            let file = unparen(&n["args"][0]);
            let data = unparen(&n["args"][1]);
            let file_value = local_key(file)
                .and_then(|k| b.values.get(&k).copied())
                .unwrap_or(file);
            if let Some(role) = file_role(file_value) {
                if kind(data) == "AstExprCall" && arr(data, "args").len() == 1 {
                    let func = unparen(&data["func"]);
                    if kind(func) == "AstExprIndexName" && text(func, "index") == "JSONEncode" {
                        if let Some(key) = local_key(&data["args"][0]) {
                            roles
                                .entry(key.clone())
                                .or_default()
                                .insert(role.to_owned());
                            if let Some(value) = b.values.get(&key) {
                                let value = unparen(value);
                                if kind(value) == "AstExprCall"
                                    && arr(value, "args").len() == 1
                                    && b.is_path(&value["func"], &["table", "clone"])
                                {
                                    if let Some(k) = local_key(&value["args"][0]) {
                                        roles.entry(k).or_default().insert(format!(
                                            "default{}{}",
                                            role[..1].to_ascii_uppercase(),
                                            &role[1..]
                                        ));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    let mut edits = vec![];
    let mut renamed = 0;
    for (n, _) in &b.nodes {
        if protected.contains(&id(n))
            || kind(n) != "AstStatLocal"
            || arr(n, "vars").len() != 1
            || arr(n, "values").len() != 1
        {
            continue;
        }
        let variable = &n["vars"][0];
        let value = unparen(&n["values"][0]);
        let key = lkey(variable);
        if b.writes.contains(&key)
            || type_prefixes.contains(&key)
            || b.refs
                .get(&key)
                .is_some_and(|rs| rs.iter().any(|r| protected.contains(&id(r))))
            || !generic(text(variable, "name"))
        {
            continue;
        }
        let stable = |field: &str| {
            labels
                .get(&(key.clone(), field.to_owned()))
                .filter(|s| s.len() == 1)
                .and_then(|s| s.first())
                .and_then(|v| v.as_deref())
        };
        let mut name = stable("Name").and_then(label);
        if name.is_none()
            && kind(value) == "AstExprCall"
            && b.is_path(&value["func"], &["Instance", "new"])
            && !arr(value, "args").is_empty()
        {
            let class = unparen(&value["args"][0]);
            let suffix = match text(class, "value") {
                "TextButton" | "ImageButton" => Some("Button"),
                "TextLabel" => Some("Label"),
                "TextBox" => Some("Input"),
                "ImageLabel" => Some("Image"),
                _ => None,
            };
            if kind(class) == "AstExprConstantString" {
                if let (Some(suffix), Some(text)) = (suffix, stable("Text")) {
                    name = label(text).map(|s| format!("{s}{suffix}"));
                }
            }
        }
        if name.is_none() {
            name = roles
                .get(&key)
                .filter(|s| s.len() == 1)
                .and_then(|s| s.first())
                .cloned();
        }
        let Some(name) = name.or_else(|| initializer_hint(value, b)) else {
            continue;
        };
        if !ident(&name) || name.len() > 48 || generic(&name) {
            continue;
        }
        let (mut candidate, mut number) = (name.clone(), 2);
        while reserved.contains(&candidate) {
            candidate = format!("{name}{number}");
            number += 1;
        }
        reserved.insert(candidate.clone());
        let (a, z) = src.span(variable)?;
        edits.push((a, z, candidate.clone()));
        for r in b.refs.get(&key).into_iter().flatten() {
            let (a, z) = src.span(r)?;
            edits.push((a, z, candidate.clone()));
        }
        renamed += 1;
    }
    Ok((edits, renamed))
}
pub fn rename_locals_source(source: &str) -> Result<(String, usize)> {
    let root = parse_ast(source.as_bytes())?;
    let (edits, names) = inferred_name_edits(&Bindings::new(&root), &Src::new(source))?;
    Ok((apply(source, edits)?, names))
}
fn add_counts(total: &mut Value, delta: &Value) {
    if let Some(obj) = delta.as_object() {
        for (key, count) in obj {
            total[key] = json!(total[key].as_u64().unwrap_or(0) + count.as_u64().unwrap_or(0));
        }
    }
}
pub fn clean_helpers_source(source: &str, infer_names: bool) -> Result<(String, Value)> {
    let mut raw = source.to_owned();
    let mut counts = json!({"strings":0,"numbers":0,"statements":0,"states":0,"checks":0,"arms":0});
    for _ in 0..12 {
        let root = parse_ast(raw.as_bytes())?;
        let b = Bindings::new(&root);
        let src = Src::new(&raw);
        let (edits, folded) = decoder_edits(&root, &b, &src)?;
        if !edits.is_empty() {
            raw = apply(&raw, edits)?;
            add_counts(&mut counts, &folded);
            continue;
        }
        let edits = state_number_edits(&b, &src)?;
        if !edits.is_empty() {
            counts["numbers"] = json!(counts["numbers"].as_u64().unwrap() + edits.len() as u64);
            raw = apply(&raw, edits)?;
            continue;
        }
        let (out, states) = crate::unflatten::flatten_dispatchers(&raw, &root)?;
        if states > 0 {
            raw = out;
            counts["states"] = json!(counts["states"].as_u64().unwrap() + states as u64);
            continue;
        }
        let (out, checks) = clock_check_edits(&raw, &root)?;
        if out != raw {
            raw = out;
            add_counts(&mut counts, &checks);
            continue;
        }
        let edits = literal_number_edits(&b, &src)?;
        if !edits.is_empty() {
            counts["numbers"] = json!(counts["numbers"].as_u64().unwrap() + edits.len() as u64);
            raw = apply(&raw, edits)?;
            continue;
        }
        if !infer_names {
            break;
        }
        let (edits, names) = inferred_name_edits(&b, &src)?;
        if edits.is_empty() {
            break;
        }
        raw = apply(&raw, edits)?;
        counts["names"] = json!(counts["names"].as_u64().unwrap_or(0) + names as u64);
    }
    Ok((raw, counts))
}
fn cyclic_decoy(fn_: &Value, b: &Bindings<'_>) -> bool {
    let (globals, paths, _) = features(fn_, b);
    if globals.iter().any(|g| g != "setmetatable")
        || !arr(fn_, "args").is_empty()
        || fn_["vararg"] == true
    {
        return false;
    }
    let nodes = walk(fn_);
    let cycle = nodes.iter().any(|(n, _)| {
        if kind(n) != "AstStatAssign" || arr(n, "vars").len() != 1 || arr(n, "values").len() != 1 {
            return false;
        }
        let target = unparen(&n["vars"][0]);
        let value = local_key(&n["values"][0]);
        kind(target) == "AstExprIndexExpr"
            && value.is_some()
            && local_key(&target["expr"]) == value
            && local_key(&target["index"]) == value
    });
    cycle
        && has_path(&paths, &["setmetatable"])
        && nodes.iter().any(|(n, _)| {
            kind(n) == "AstExprIndexName" && text(n, "index") == "__tostring"
                || kind(n) == "AstExprConstantString" && text(n, "value") == "__tostring"
        })
}
fn constant_decoys(fn_: &Value) -> bool {
    let body = arr(&fn_["body"], "body");
    arr(fn_, "args").is_empty()
        && fn_["vararg"] != true
        && body.len() == 1
        && kind(&body[0]) == "AstStatReturn"
        && arr(&body[0], "list").len() >= 4
        && arr(&body[0], "list")
            .iter()
            .all(|n| kind(unparen(n)) == "AstExprConstantString")
}
const INSPECTORS: [&str; 6] = [
    "getconstants",
    "getprotos",
    "getupvalues",
    "getconstant",
    "getproto",
    "getupvalue",
];
fn inspector_effects_safe(
    group: &HashSet<String>,
    functions: &BTreeMap<String, &Value>,
    b: &Bindings<'_>,
) -> bool {
    let (mut indexed, mut receivers, mut name_tables) =
        (HashSet::new(), HashSet::new(), HashSet::new());
    for key in group {
        for (node, _) in walk(functions[key]) {
            if kind(node) != "AstStatForIn" || arr(node, "values").len() != 1 {
                continue;
            }
            let call = unparen(&node["values"][0]);
            if kind(call) != "AstExprCall"
                || !b.is_path(&call["func"], &["ipairs"])
                || arr(call, "args").len() != 1
            {
                continue;
            }
            let Some(iterable) = local_key(&call["args"][0]) else {
                continue;
            };
            let Some(table) = b
                .values
                .get(&iterable)
                .filter(|_| !b.writes.contains(&iterable))
                .map(|v| unparen(v))
            else {
                continue;
            };
            if kind(table) != "AstExprTable" {
                continue;
            }
            receivers.extend(arr(node, "vars").iter().map(lkey));
            let values: Vec<_> = arr(table, "items")
                .iter()
                .map(|it| unparen(&it["value"]))
                .collect();
            if values.iter().any(|v| {
                kind(v) == "AstExprConstantString" && INSPECTORS.contains(&text(v, "value"))
            }) {
                if values.is_empty()
                    || values.iter().any(|v| {
                        kind(v) != "AstExprConstantString"
                            || !INSPECTORS.contains(&text(v, "value"))
                    })
                    || values
                        .iter()
                        .map(|v| text(v, "value"))
                        .collect::<HashSet<_>>()
                        .len()
                        != values.len()
                {
                    return false;
                }
                let Some(var) = arr(node, "vars").last() else {
                    return false;
                };
                indexed.insert(lkey(var));
                name_tables.insert(iterable);
            }
        }
    }
    if indexed.is_empty() {
        return false;
    }
    for key in group {
        let fn_ = functions[key];
        let (_, declared, _) = local_sets(fn_, Some(b));
        let cycle = cyclic_decoy(fn_, b);
        for (node, _) in walk(fn_) {
            for target in targets(node) {
                let target = unparen(target);
                if kind(target) == "AstExprLocal" {
                    if !local_key(target).is_some_and(|k| declared.contains(&k)) {
                        return false;
                    }
                    continue;
                }
                if kind(target) != "AstExprIndexExpr" {
                    return false;
                }
                let Some(base) = local_key(&target["expr"]) else {
                    return false;
                };
                let value = b.values.get(&base).map(|v| unparen(v));
                if cycle
                    && declared.contains(&base)
                    && value.is_some_and(|v| kind(v) == "AstExprTable")
                {
                    continue;
                }
                let base_target = value.is_some_and(|v| {
                    kind(v) == "AstExprCall"
                        && arr(v, "args").is_empty()
                        && b.is_path(&v["func"], &["getgenv"])
                });
                if !local_key(&target["index"]).is_some_and(|k| indexed.contains(&k))
                    || !(base_target || receivers.contains(&base))
                {
                    return false;
                }
            }
            if kind(node) == "AstExprCall"
                && b.path(&node["func"])
                    .is_some_and(|p| p.first().is_some_and(|x| x == "table"))
                && !arr(node, "args").is_empty()
            {
                let k = local_key(&node["args"][0]);
                if k.as_ref().is_some_and(|k| name_tables.contains(k))
                    || k.as_ref()
                        .and_then(|k| b.values.get(k))
                        .is_none_or(|v| kind(unparen(v)) != "AstExprTable")
                {
                    return false;
                }
            }
        }
    }
    true
}
fn inspector_group<'a>(
    root: &'a Value,
    b: &Bindings<'a>,
    functions: &BTreeMap<String, &'a Value>,
) -> (HashSet<String>, Vec<&'a Value>) {
    for (key, fn_) in functions {
        let (globals, _, strings) = features(fn_, b);
        if !INSPECTORS[..3].iter().all(|s| strings.contains(*s))
            || !["getgenv", "debug"].iter().any(|g| globals.contains(*g))
        {
            continue;
        }
        let allowed = [
            "type",
            "typeof",
            "pcall",
            "getgenv",
            "debug",
            "table",
            "ipairs",
            "pairs",
            "setreadonly",
            "isreadonly",
            "make_writeable",
            "make_readonly",
            "newcclosure",
        ];
        if globals.iter().any(|g| !allowed.contains(&g.as_str())) || b.writes.contains(key) {
            continue;
        }
        if !walk(fn_).iter().any(|(n, _)| {
            kind(n) == "AstStatAssign"
                && arr(n, "vars")
                    .iter()
                    .any(|v| kind(unparen(v)) == "AstExprIndexExpr")
        }) {
            continue;
        }
        let mut group = HashSet::new();
        let mut todo = vec![key.clone()];
        while let Some(k) = todo.pop() {
            if group.insert(k.clone()) {
                for (n, _) in walk(functions[&k]) {
                    if let Some(dep) = local_key(n) {
                        if functions.contains_key(&dep) && !group.contains(&dep) {
                            todo.push(dep);
                        }
                    }
                }
            }
        }
        if !group.iter().any(|k| cyclic_decoy(functions[k], b))
            || !group.iter().any(|k| constant_decoys(functions[k]))
            || group.iter().any(|k| {
                k != key && !cyclic_decoy(functions[k], b) && !constant_decoys(functions[k])
            })
        {
            continue;
        }
        let external = group.iter().any(|k| {
            let (nodes, declared, _) = local_sets(functions[k], None);
            nodes
                .iter()
                .filter_map(|n| local_key(n))
                .any(|k| !declared.contains(&k) && !group.contains(&k))
        });
        if external || !inspector_effects_safe(&group, functions, b) {
            continue;
        }
        let calls: Vec<_> = arr(root, "body")
            .iter()
            .filter(|n| {
                let expr = unparen(&n["expr"]);
                kind(n) == "AstStatExpr"
                    && kind(expr) == "AstExprCall"
                    && local_key(&expr["func"]).as_ref() == Some(key)
                    && arr(expr, "args").is_empty()
            })
            .collect();
        if calls.is_empty() {
            continue;
        }
        let mut inside = HashSet::new();
        for k in &group {
            inside.extend(walk(functions[k]).iter().map(|(n, _)| id(n)));
        }
        for call in &calls {
            inside.extend(walk(call).iter().map(|(n, _)| id(n)));
        }
        if group.iter().any(|k| {
            b.refs
                .get(k)
                .is_some_and(|rs| rs.iter().any(|r| !inside.contains(&id(r))))
        }) {
            continue;
        }
        return (group, calls);
    }
    (HashSet::new(), vec![])
}
const GUARD_CALLS: &[&[&str]] = &[
    &["type"],
    &["typeof"],
    &["pcall"],
    &["iscclosure"],
    &["error"],
    &["os", "clock"],
    &["getrawmetatable"],
    &["debug", "info"],
    &["debug", "getmetatable"],
    &["string", "dump"],
    &["table", "freeze"],
    &["table", "isfrozen"],
    &["buffer", "create"],
    &["buffer", "readu8"],
    &["buffer", "writeu8"],
    &["bit32", "bxor"],
    &["bit32", "band"],
];
fn safe_guard_target(n: &Value, b: &Bindings<'_>, seen: &mut HashSet<String>) -> bool {
    let n = unparen(n);
    if kind(n) == "AstExprFunction"
        || GUARD_CALLS
            .iter()
            .any(|p| *p != ["pcall"] && b.is_path(n, p))
    {
        return true;
    }
    if matches!(kind(n), "AstExprConstantNil" | "AstExprConstantBool") {
        return n["value"].is_null() || n["value"] == false;
    }
    if kind(n) == "AstExprBinary" && ["And", "Or"].contains(&text(n, "op")) {
        return safe_guard_target(&n["right"], b, &mut seen.clone())
            && (text(n, "op") == "And" || safe_guard_target(&n["left"], b, &mut seen.clone()));
    }
    let Some(key) = local_key(n) else {
        return false;
    };
    !b.writes.contains(&key)
        && seen.insert(key.clone())
        && b.values
            .get(&key)
            .is_some_and(|v| safe_guard_target(v, b, seen))
}
fn readonly_guard(fn_: &Value, b: &Bindings<'_>) -> bool {
    let (nodes, declared, private) = local_sets(fn_, Some(b));
    for n in nodes {
        if kind(n) == "AstExprLocal" && !local_key(n).is_some_and(|k| declared.contains(&k)) {
            return false;
        }
        if kind(n) == "AstExprCall" {
            if !GUARD_CALLS.iter().any(|p| b.is_path(&n["func"], p)) {
                return false;
            }
            let pcall = b.is_path(&n["func"], &["pcall"]);
            if pcall
                && (arr(n, "args").is_empty()
                    || !safe_guard_target(&n["args"][0], b, &mut HashSet::new()))
            {
                return false;
            }
            let write = b.is_path(&n["func"], &["buffer", "writeu8"])
                || pcall && b.is_path(&n["args"][0], &["buffer", "writeu8"]);
            if write {
                let receiver = if pcall { 1 } else { 0 };
                if arr(n, "args")
                    .get(receiver)
                    .and_then(local_key)
                    .is_none_or(|k| !private.contains(&k))
                {
                    return false;
                }
            }
        }
        if matches!(
            kind(n),
            "AstStatAssign" | "AstStatCompoundAssign" | "AstStatFunction"
        ) {
            for target in targets(n) {
                let target = unparen(target);
                if kind(target) == "AstExprLocal"
                    && local_key(target).is_some_and(|k| declared.contains(&k))
                {
                    continue;
                }
                if matches!(kind(target), "AstExprIndexExpr" | "AstExprIndexName")
                    && local_key(&target["expr"]).is_some_and(|k| private.contains(&k))
                {
                    continue;
                }
                return false;
            }
        }
    }
    true
}
fn guard_kind(fn_: &Value, b: &Bindings<'_>) -> Option<&'static str> {
    let (globals, paths, strings) = features(fn_, b);
    if !readonly_guard(fn_, b) {
        return None;
    }
    let nodes = walk(fn_);
    let returns: Vec<_> = nodes
        .iter()
        .filter(|(n, _)| kind(n) == "AstStatReturn")
        .collect();
    if returns.is_empty()
        || returns.iter().any(|(n, _)| {
            arr(n, "list").len() != 1 || kind(unparen(&n["list"][0])) != "AstExprConstantBool"
        })
    {
        return None;
    }
    let subset = |allow: &[&str]| globals.iter().all(|g| allow.contains(&g.as_str()));
    let intro = has_path(&paths, &["debug", "info"])
        && has_path(&paths, &["string", "dump"])
        && strings.contains("[C]");
    if arr(fn_, "args").len() == 1
        && fn_["vararg"] != true
        && intro
        && subset(&["type", "debug", "pcall", "string"])
    {
        return Some("primitive");
    }
    if !arr(fn_, "args").is_empty() || fn_["vararg"] == true {
        return None;
    }
    if intro
        && [
            &["buffer", "create"][..],
            &["buffer", "readu8"],
            &["buffer", "writeu8"],
            &["bit32", "bxor"],
        ]
        .iter()
        .all(|p| has_path(&paths, p))
        && subset(&[
            "type",
            "typeof",
            "buffer",
            "bit32",
            "pcall",
            "iscclosure",
            "string",
            "debug",
            "table",
        ])
    {
        return Some("environment");
    }
    if has_path(&paths, &["debug", "info"])
        && strings.contains("[C]")
        && nodes
            .iter()
            .any(|(n, _)| kind(n) == "AstExprIndexName" && text(n, "index") == "__namecall")
        && subset(&[
            "type",
            "typeof",
            "pcall",
            "game",
            "getrawmetatable",
            "debug",
        ])
    {
        return Some("remote");
    }
    if subset(&["os", "error"])
        && has_path(&paths, &["os", "clock"])
        && has_path(&paths, &["error"])
    {
        let clocks: HashSet<_> = b
            .values
            .iter()
            .filter(|(_, v)| {
                kind(unparen(v)) == "AstExprCall"
                    && b.is_path(&unparen(v)["func"], &["os", "clock"])
            })
            .map(|(k, _)| k.clone())
            .collect();
        for (n, _) in &nodes {
            if kind(n) != "AstExprBinary" || text(n, "op") != "CompareGt" {
                continue;
            }
            let left = unparen(&n["left"]);
            let limit = unparen(&n["right"]);
            if kind(left) == "AstExprBinary"
                && text(left, "op") == "Sub"
                && local_key(&left["right"]).is_some_and(|k| clocks.contains(&k))
                && kind(unparen(&left["left"])) == "AstExprCall"
                && b.is_path(&unparen(&left["left"])["func"], &["os", "clock"])
                && kind(limit) == "AstExprConstantNumber"
                && limit["value"].as_f64().is_some_and(|v| v > 0.0 && v <= 1.0)
                && nodes.iter().any(|(q, _)| {
                    kind(q) == "AstStatFor"
                        && integer(&q["from"]) == Some(1)
                        && integer(&q["to"]).is_some_and(|v| (1..=1000).contains(&v))
                })
            {
                return Some("timing");
            }
        }
    }
    None
}
fn error_branch(block: &Value, b: &Bindings<'_>) -> bool {
    let body = arr(block, "body");
    if body.len() != 1 || kind(&body[0]) != "AstStatExpr" {
        return false;
    }
    let call = unparen(&body[0]["expr"]);
    let args = arr(call, "args");
    kind(call) == "AstExprCall"
        && b.is_path(&call["func"], &["error"])
        && (args.len() == 1 || args.len() == 2)
        && kind(unparen(&args[0])) == "AstExprConstantString"
        && (args.len() == 1 || integer(&args[1]).is_some())
}
fn primitive_condition(n: &Value, b: &Bindings<'_>, kinds: &HashMap<String, &str>) -> bool {
    let n = unparen(n);
    if kind(n) == "AstExprBinary" && text(n, "op") == "And" {
        return primitive_condition(&n["left"], b, kinds)
            && primitive_condition(&n["right"], b, kinds);
    }
    kind(n) == "AstExprCall"
        && arr(n, "args").len() == 1
        && local_key(&n["func"]).and_then(|k| kinds.get(&k).copied()) == Some("primitive")
        && b.path(&n["args"][0]).is_some_and(|p| primitive(&p))
}
fn primitive_availability(n: &Value, b: &Bindings<'_>) -> bool {
    let n = unparen(n);
    kind(n) == "AstExprBinary"
        && text(n, "op") == "And"
        && ((b.is_path(&n["left"], &["buffer"]) && b.is_path(&n["right"], &["bit32"]))
            || (b.is_path(&n["left"], &["bit32"]) && b.is_path(&n["right"], &["buffer"])))
}
fn forwarding_function(n: &Value, b: &Bindings<'_>) -> bool {
    let mut p = b.parents.get(&id(n)).copied().flatten();
    while p.is_some_and(|p| kind(p) != "AstExprFunction") {
        p = p.and_then(|p| b.parents.get(&id(p)).copied().flatten());
    }
    let Some(fn_) = p else {
        return false;
    };
    let Some(arg) = arr(fn_, "args").first() else {
        return false;
    };
    let first = lkey(arg);
    walk(fn_).iter().any(|(n, _)| {
        let f = unparen(&n["func"]);
        kind(n) == "AstExprCall"
            && n["self"] == true
            && kind(f) == "AstExprIndexName"
            && ["FireServer", "InvokeServer"].contains(&text(f, "index"))
            && local_key(&f["expr"]).as_ref() == Some(&first)
    })
}
fn discarded_remote_branch(block: &Value, b: &Bindings<'_>) -> bool {
    let nodes = walk(block);
    let returns: Vec<_> = nodes
        .iter()
        .filter(|(n, _)| kind(n) == "AstStatReturn")
        .collect();
    if returns.is_empty()
        || returns.iter().any(|(n, _)| {
            arr(n, "list").len() != 1 || kind(unparen(&n["list"][0])) != "AstExprConstantNil"
        })
    {
        return false;
    }
    if arr(block, "body").len() == 1 && kind(&block["body"][0]) == "AstStatReturn" {
        return true;
    }
    if nodes
        .iter()
        .any(|(n, _)| kind(n) == "AstExprGlobal" && text(n, "global") != "Instance")
    {
        return false;
    }
    let (_, declared, _) = local_sets(block, None);
    if nodes.iter().any(|(n, _)| {
        kind(n) == "AstExprLocal"
            && !local_key(n).is_some_and(|k| declared.contains(&k))
            && !b.is_path(n, &["pcall"])
    }) {
        return false;
    }
    nodes.iter().any(|(n, _)| {
        kind(n) == "AstExprCall"
            && b.is_path(&n["func"], &["Instance", "new"])
            && arr(n, "args").len() == 1
            && kind(unparen(&n["args"][0])) == "AstExprConstantString"
            && unparen(&n["args"][0])["value"] == "BindableEvent"
    })
}
fn protection_edits(
    root: &Value,
    b: &Bindings<'_>,
    src: &Src<'_>,
    protected: bool,
) -> Result<(Vec<Edit>, Value)> {
    let functions: BTreeMap<_, _> = b
        .defs
        .iter()
        .filter_map(|(k, (_, v))| {
            let v = unparen(v);
            (kind(v) == "AstExprFunction").then_some((k.clone(), v))
        })
        .collect();
    let (group, calls) = inspector_group(root, b, &functions);
    if group.is_empty() && !protected {
        return Ok((vec![], json!({"checks":0,"statements":0,"scaffold":0})));
    }
    let mut remove: Vec<_> = group.iter().map(|k| b.defs[k].0).chain(calls).collect();
    let kinds: HashMap<_, _> = functions
        .iter()
        .filter(|(k, _)| !b.writes.contains(*k))
        .filter_map(|(k, f)| guard_kind(f, b).map(|v| (k.clone(), v)))
        .collect();
    let mut checks = 0;
    for (n, _) in &b.nodes {
        if kind(n) != "AstStatIf" || !n["elsebody"].is_null() {
            continue;
        }
        let cond = unparen(&n["condition"]);
        if kind(cond) != "AstExprUnary" || text(cond, "op") != "Not" {
            continue;
        }
        let expr = unparen(&cond["expr"]);
        if kind(expr) == "AstExprCall" && arr(expr, "args").is_empty() {
            match local_key(&expr["func"]).and_then(|k| kinds.get(&k).copied()) {
                Some("environment") if error_branch(&n["thenbody"], b) => {
                    remove.push(*n);
                    checks += 1;
                }
                Some("remote")
                    if forwarding_function(n, b) && discarded_remote_branch(&n["thenbody"], b) =>
                {
                    remove.push(*n);
                    checks += 1;
                }
                _ => {}
            }
        } else if error_branch(&n["thenbody"], b) && primitive_condition(expr, b, &kinds) {
            let validators: Vec<_> = walk(expr)
                .into_iter()
                .filter(|(q, _)| kind(q) == "AstExprCall")
                .collect();
            if validators.is_empty()
                || validators.iter().any(|(q, _)| {
                    local_key(&q["func"]).and_then(|k| kinds.get(&k).copied()) != Some("primitive")
                        || arr(q, "args").len() != 1
                        || !b.path(&q["args"][0]).is_some_and(|p| primitive(&p))
                })
            {
                continue;
            }
            let parent = b.parents.get(&id(n)).copied().flatten();
            let outer = parent.and_then(|p| b.parents.get(&id(p)).copied().flatten());
            if let Some(outer) = outer.filter(|o| {
                kind(o) == "AstStatIf"
                    && o["elsebody"].is_null()
                    && primitive_availability(&o["condition"], b)
                    && parent.is_some_and(|p| {
                        arr(p, "body").iter().all(|s| {
                            id(s) == id(n)
                                || kind(s) == "AstStatLocal"
                                    && arr(s, "values")
                                        .iter()
                                        .all(|v| b.path(v).is_some_and(|p| primitive(&p)))
                        })
                    })
            }) {
                remove.push(outer);
            } else {
                remove.push(*n);
            }
            checks += 1;
        }
    }
    for n in arr(root, "body") {
        let expr = unparen(&n["expr"]);
        if kind(n) == "AstStatExpr"
            && kind(expr) == "AstExprCall"
            && local_key(&expr["func"]).and_then(|k| kinds.get(&k).copied()) == Some("timing")
            && arr(expr, "args").is_empty()
        {
            remove.push(n);
            checks += 1;
        }
    }
    let spans = remove
        .iter()
        .map(|n| src.span(n))
        .collect::<Result<Vec<_>>>()?;
    for key in functions.keys() {
        if group.contains(key) || !kinds.contains_key(key) {
            continue;
        }
        let covered = b
            .refs
            .get(key)
            .into_iter()
            .flatten()
            .map(|r| src.span(r))
            .collect::<Result<Vec<_>>>()?
            .iter()
            .all(|(a, _)| spans.iter().any(|(s, e)| s <= a && a < e));
        if covered {
            remove.push(b.defs[key].0);
        }
    }
    let edits = outer(
        remove
            .iter()
            .map(|n| src.span(n).map(|(a, z)| (a, z, String::new())))
            .collect::<Result<_>>()?,
    );
    let count = edits.len();
    Ok((
        edits,
        json!({"checks":checks,"statements":count,"scaffold":if group.is_empty(){0}else{1}}),
    ))
}
pub fn strip_protection_source(source: &str, verified: bool) -> Result<(String, Value)> {
    let mut counts = json!({"checks":0,"statements":0});
    if !verified {
        return Ok((source.to_owned(), counts));
    }
    let mut raw = source.to_owned();
    let mut protected = false;
    for _ in 0..8 {
        let root = parse_ast(raw.as_bytes())?;
        let (edits, mut stats) =
            protection_edits(&root, &Bindings::new(&root), &Src::new(&raw), protected)?;
        protected |= stats["scaffold"].as_u64().unwrap_or(0) > 0;
        if edits.is_empty() {
            break;
        }
        raw = apply(&raw, edits)?;
        stats.as_object_mut().unwrap().remove("scaffold");
        add_counts(&mut counts, &stats);
    }
    Ok((raw, counts))
}

#[cfg(test)]
mod reference_helper_tests {
    use super::*;
    #[test]
    fn dead_helper_graph_retains_live_values_effects_and_unread_application_constants() {
        let source="local constant=17\nlocal privateValue=18\nlocal function dead() return privateValue end\nlocal effect=applicationEvent()\nlocal untouched=19\nreturn constant\n";
        let root = parse_ast(source.as_bytes()).unwrap();
        let bindings = Bindings::new(&root);
        let edits = dead_helper_edits(&root, &bindings, &Src::new(source)).unwrap();
        let out = apply(source, edits).unwrap();
        assert!(!out.contains("privateValue") && !out.contains("function dead"));
        assert!(
            out.contains("constant=17")
                && out.contains("applicationEvent()")
                && out.contains("untouched=19")
        );
    }
}

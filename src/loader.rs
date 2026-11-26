//! Structural FORGE loader analysis, independent of identifier spelling.
use crate::{
    astutil::{self, lkey, unparen, walk, Src},
    error::{Error, Result},
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};
static NIL: Value = Value::Null;
fn err(message: impl Into<String>) -> Error {
    Error::new("unsupported", message)
}
pub fn ty(v: &Value) -> &str {
    v["type"].as_str().unwrap_or("")
}
pub fn array(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or(&[])
}
fn first(v: &Value) -> &Value {
    array(v).first().unwrap_or(&NIL)
}
fn nth(v: &Value, i: usize) -> &Value {
    array(v).get(i).unwrap_or(&NIL)
}
fn body(v: &Value) -> &[Value] {
    array(&v["body"]["body"])
}
fn same(a: &Value, b: &Value) -> bool {
    std::ptr::eq(a, b)
}
pub fn is_in(v: &Value, name: &str) -> bool {
    ty(v) == "AstExprIndexName" && v["index"] == name
}
pub fn loc_of(v: &Value) -> Option<&Value> {
    let v = unparen(v);
    if ty(v) == "AstExprLocal" {
        Some(&v["local"])
    } else {
        None
    }
}
pub fn const_num(v: &Value) -> Option<f64> {
    let v = unparen(v);
    if ty(v) == "AstExprConstantNumber" {
        v["value"].as_f64()
    } else {
        None
    }
}
fn int(v: f64) -> Option<i64> {
    if v.is_finite() && v.fract() == 0.0 && v >= i64::MIN as f64 && v < (i64::MAX as f64) {
        Some(v as i64)
    } else {
        None
    }
}
pub fn numeric_member(v: &Value, owner: &Value) -> Option<i64> {
    let v = unparen(v);
    if ty(v) != "AstExprIndexExpr" || lkey(loc_of(&v["expr"])?) != lkey(owner) {
        return None;
    }
    int(const_num(&v["index"])?)
}
pub fn eq_info(v: &Value) -> Option<(&Value, f64)> {
    let c = unparen(v);
    if ty(c) != "AstExprBinary" || c["op"] != "CompareEq" {
        return None;
    }
    let (l, r) = (unparen(&c["left"]), unparen(&c["right"]));
    loc_of(l)
        .zip(const_num(r))
        .or_else(|| loc_of(r).zip(const_num(l)))
}
fn nil_eq(v: &Value) -> Option<&Value> {
    let c = unparen(v);
    if ty(c) != "AstExprBinary" || c["op"] != "CompareEq" {
        return None;
    }
    let (l, r) = (unparen(&c["left"]), unparen(&c["right"]));
    if ty(r) == "AstExprConstantNil" {
        loc_of(l)
    } else if ty(l) == "AstExprConstantNil" {
        loc_of(r)
    } else {
        None
    }
}
pub fn if_chain(mut v: &Value) -> (Vec<(&Value, &Value, &Value)>, Option<&Value>) {
    let mut out = vec![];
    let mut tail = None;
    while ty(v) == "AstStatIf" {
        out.push((&v["condition"], &v["thenbody"], v));
        let e = &v["elsebody"];
        if e.is_null() {
            break;
        }
        if ty(e) == "AstStatIf" {
            v = e;
        } else {
            tail = Some(e);
            break;
        }
    }
    (out, tail)
}
fn or_terms(c: &Value) -> Vec<&Value> {
    let c = unparen(c);
    if ty(c) == "AstExprBinary" && c["op"] == "Or" {
        let mut v = or_terms(&c["left"]);
        v.extend(or_terms(&c["right"]));
        v
    } else {
        vec![c]
    }
}
pub fn handler_chain(statement: &Value, opcode: &Value) -> Result<Vec<Value>> {
    let mut out = vec![];
    let mut seen = BTreeSet::new();
    let mut pending = vec![(statement, f64::NEG_INFINITY, f64::INFINITY)];
    while let Some((mut node, lower, upper)) = pending.pop() {
        if node.is_null() {
            return Err(err("handler partition is incomplete"));
        }
        if ty(node) == "AstStatBlock" {
            if array(&node["body"]).len() != 1 {
                return Err(err("handler partition contains extra statements"));
            }
            node = first(&node["body"]);
        }
        if ty(node) != "AstStatIf" {
            return Err(err("handler partition is not a dispatcher"));
        }
        let c = unparen(&node["condition"]);
        if let Some((local, value)) = eq_info(c) {
            if lkey(local) == lkey(opcode) {
                let n = int(value).ok_or_else(|| err("handler opcode is not an integer"))?;
                if !(lower < value && value <= upper) || !seen.insert(n) {
                    return Err(err("handler opcode is duplicate or outside its partition"));
                }
                out.push(json!([n, node["thenbody"]]));
                let tail = &node["elsebody"];
                if ty(tail) == "AstStatIf" {
                    pending.push((tail, lower, upper));
                }
                continue;
            }
        }
        if ty(c) != "AstExprBinary" || c["op"] != "CompareLe" {
            return Err(err("handler condition not recognised"));
        }
        let local = loc_of(&c["left"]);
        let pivot = const_num(&c["right"]);
        if local.map(lkey) != Some(lkey(opcode)) || pivot.and_then(int).is_none() {
            return Err(err(
                "handler partition does not compare the opcode to an integer",
            ));
        }
        let pivot = pivot.unwrap();
        if !(lower < pivot && pivot < upper) {
            return Err(err("handler partition is contradictory"));
        }
        pending.push((&node["elsebody"], pivot, upper));
        pending.push((&node["thenbody"], lower, pivot));
    }
    Ok(out)
}
pub fn stmt_indices(st: &Value) -> Result<Vec<i64>> {
    Ok(stmt_fields(st, None, None)?
        .into_iter()
        .map(|(index, _)| index)
        .collect())
}
fn stmt_fields(
    st: &Value,
    owner: Option<&Value>,
    record: Option<&Value>,
) -> Result<Vec<(i64, Option<i64>)>> {
    let mut values: Vec<&Value> = match ty(st) {
        "AstStatAssign" => array(&st["values"]).iter().collect(),
        "AstStatReturn" => array(&st["list"]).iter().collect(),
        _ => return Err(err("dispatchFormat body")),
    };
    if ty(st) == "AstStatReturn" && values.len() == 1 && ty(unparen(values[0])) == "AstExprTable" {
        values = array(&unparen(values[0])["items"])
            .iter()
            .map(|i| &i["value"])
            .collect();
    }
    values
        .into_iter()
        .map(|v| {
            let mut v = unparen(v);
            let mut mask = None;
            if ty(v) == "AstExprCall" {
                let f = unparen(&v["func"]);
                if !is_in(f, "bxor")
                    || ty(unparen(&f["expr"])) != "AstExprGlobal"
                    || unparen(&f["expr"])["global"] != "bit32"
                    || array(&v["args"]).len() != 2
                {
                    return Err(err("dispatchFormat mask"));
                }
                mask = Some(
                    owner
                        .and_then(|m| numeric_member(nth(&v["args"], 1), m))
                        .ok_or_else(|| err("dispatchFormat mask field"))?,
                );
                v = unparen(first(&v["args"]));
            }
            if ty(v) != "AstExprIndexExpr" {
                return Err(err("dispatchFormat index"));
            }
            if record.is_some_and(|record| loc_of(&v["expr"]).map(lkey) != Some(lkey(record))) {
                return Err(err("dispatchFormat record"));
            }
            let index = const_num(&v["index"])
                .and_then(int)
                .ok_or_else(|| err("dispatchFormat index"))?;
            if mask.is_some() && !(1..=10).contains(&index) {
                return Err(err("dispatchFormat masked index"));
            }
            Ok((index, mask))
        })
        .collect()
}
pub fn dispatch_perm(st: &Value, owner: Option<&Value>, format: Option<i64>) -> Result<Value> {
    dispatch_perm_with_record(st, owner, None, format)
}
fn dispatch_perm_with_record(
    st: &Value,
    owner: Option<&Value>,
    record: Option<&Value>,
    format: Option<i64>,
) -> Result<Value> {
    let mut result = json!({});
    let indices = |s| -> Result<Vec<i64>> {
        Ok(stmt_fields(s, owner, record)?
            .into_iter()
            .map(|(index, _)| index)
            .collect())
    };
    if ty(st) != "AstStatIf" {
        let ix = indices(st)?;
        for k in 0..4 {
            result[k.to_string()] = json!(ix);
        }
        return Ok(result);
    }
    for (c, b, _) in if_chain(st).0 {
        let c = unparen(c);
        let numeric = owner
            .zip(format)
            .map(|(m, f)| numeric_member(&c["left"], m) == Some(f))
            .unwrap_or(false);
        if ty(c) != "AstExprBinary"
            || c["op"] != "CompareEq"
            || !(is_in(unparen(&c["left"]), "dispatchFormat") || numeric)
        {
            return Err(err("dispatchFormat ladder expected"));
        }
        let k = const_num(&c["right"])
            .and_then(int)
            .ok_or_else(|| err("dispatchFormat integer expected"))?;
        if result.get(k.to_string()).is_some() {
            return Err(err("duplicate dispatchFormat"));
        }
        result[k.to_string()] = json!(indices(first(&b["body"]))?);
    }
    Ok(result)
}
type FunctionIndex<'a> = HashMap<String, &'a Value>;
type DeclarationIndex<'a> = HashMap<String, (&'a Value, &'static str)>;
fn index(root: &Value) -> (FunctionIndex<'_>, DeclarationIndex<'_>) {
    let (mut funcs, mut decl) = (HashMap::new(), HashMap::new());
    for (n, _) in walk(root) {
        match ty(n) {
            "AstStatLocalFunction" => {
                funcs.insert(lkey(&n["name"]).into(), &n["func"]);
                decl.insert(lkey(&n["name"]).into(), (n, "localfn"));
            }
            "AstStatLocal" => {
                for (i, v) in array(&n["vars"]).iter().enumerate() {
                    decl.insert(lkey(v).into(), (n, "local"));
                    let val = unparen(nth(&n["values"], i));
                    if array(&n["vars"]).len() == array(&n["values"]).len()
                        && ty(val) == "AstExprFunction"
                    {
                        funcs.insert(lkey(v).into(), val);
                    }
                }
            }
            _ => {}
        }
    }
    (funcs, decl)
}
pub fn index_source(data: &[u8]) -> Result<Value> {
    let text = astutil::normalize_source(data);
    let root = astutil::parse_ast(&text)?;
    let (funcs, decl) = index(&root);
    let fs: serde_json::Map<_, _> = funcs.into_iter().map(|(k, v)| (k, v.clone())).collect();
    let ds: serde_json::Map<_, _> = decl
        .into_iter()
        .map(|(k, (n, t))| (k, json!([n, t])))
        .collect();
    Ok(
        json!({"text":astutil::latin1(&text),"root":root,"funcs":fs,"local_decl":ds,"roles":{},"role_local":{}}),
    )
}
struct Analyzer<'a> {
    root: &'a Value,
    src: Src,
    parents: HashMap<usize, &'a Value>,
    funcs: FunctionIndex<'a>,
    decl: DeclarationIndex<'a>,
    out: Value,
    frame: &'a Value,
    mk: &'a Value,
    module: &'a Value,
    loop_node: &'a Value,
    ctrl: &'a Value,
    branches: HashMap<String, &'a Value>,
    hot: Vec<i64>,
    previous_simple_roles: BTreeMap<String, Value>,
}
impl<'a> Analyzer<'a> {
    fn new(root: &'a Value, text: &str) -> Result<Self> {
        let (funcs, decl) = index(root);
        let parents = walk(root)
            .into_iter()
            .filter_map(|(n, p)| p.map(|p| (n as *const Value as usize, p)))
            .collect();
        let fs: serde_json::Map<_, _> = funcs
            .iter()
            .map(|(k, v)| (k.clone(), (*v).clone()))
            .collect();
        let ds: serde_json::Map<_, _> = decl
            .iter()
            .map(|(k, (n, t))| (k.clone(), json!([n, t])))
            .collect();
        Ok(Self {
            root,
            src: Src::new(text)?,
            parents,
            funcs,
            decl,
            out: json!({"text":text,"root":root,"funcs":fs,"local_decl":ds,"roles":{},"role_local":{},"numeric_fields":{},"layout":"named"}),
            frame: &NIL,
            mk: &NIL,
            module: &NIL,
            loop_node: &NIL,
            ctrl: &NIL,
            branches: HashMap::new(),
            hot: vec![],
            previous_simple_roles: BTreeMap::new(),
        })
    }
    fn parent(&self, n: &Value) -> Option<&'a Value> {
        self.parents.get(&(n as *const Value as usize)).copied()
    }
    fn enclosing(&self, n: &Value) -> Option<&'a Value> {
        let mut p = self.parent(n);
        while let Some(v) = p {
            if ty(v) == "AstExprFunction" {
                return Some(v);
            }
            p = self.parent(v);
        }
        None
    }
    fn set(&mut self, local: &Value, name: &str) -> Result<()> {
        if local.is_null() || lkey(local).is_empty() {
            return Err(err(format!("role {name}: local not found")));
        }
        self.out["roles"][lkey(local)] = json!(name);
        self.out["role_local"][name] = local.clone();
        Ok(())
    }
    fn set_opt(&mut self, local: Option<&Value>, name: &str) -> Result<()> {
        self.set(local.unwrap_or(&NIL), name)
    }
    fn role(&self, e: &Value) -> Option<&str> {
        loc_of(e).and_then(|l| self.out["roles"][lkey(l)].as_str())
    }
    fn local(&self, name: &str) -> &Value {
        &self.out["role_local"][name]
    }
    fn need(&self, names: &[&str]) -> Result<()> {
        for n in names {
            if self.local(n).is_null() {
                return Err(err(format!("loader role not found: {n}")));
            }
        }
        Ok(())
    }
    fn num(&self, name: &str) -> Option<i64> {
        self.out["numeric_fields"][name].as_i64()
    }
    fn field(&mut self, name: &str, n: i64) {
        self.out["numeric_fields"][name] = json!(n);
    }
    fn perm(&self, s: &Value, record: Option<&Value>) -> Result<Value> {
        dispatch_perm_with_record(
            s,
            if self.local("PROTO").is_null() {
                None
            } else {
                Some(self.local("PROTO"))
            },
            record,
            self.num("dispatch_format"),
        )
    }
    fn setup(&mut self, frame: &'a Value, mk: &'a Value, module: &'a Value) -> Result<()> {
        self.frame = frame;
        self.mk = mk;
        self.module = module;
        self.out["FRAME"] = frame.clone();
        self.out["MK"] = mk.clone();
        self.out["MOD"] = module.clone();
        self.out["frame_body"] = json!(body(frame));
        self.set(nth(&mk["args"], 0), "PROTO")?;
        self.set(nth(&mk["args"], 1), "UP")?;
        if array(&module["args"]).len() >= 2 {
            self.set(nth(&module["args"], 1), "ENV")?;
        }
        let p = self
            .parent(mk)
            .ok_or_else(|| err("proto factory name not found"))?;
        let name = if ty(p) == "AstStatLocalFunction" {
            &p["name"]
        } else if ty(p) == "AstStatLocal" {
            array(&p["vars"])
                .iter()
                .zip(array(&p["values"]))
                .find(|(_, v)| same(unparen(v), mk))
                .map(|(v, _)| v)
                .unwrap_or(&NIL)
        } else {
            &NIL
        };
        self.set(name, "MK")?;
        self.out["mk_stmt"] = p.clone();
        Ok(())
    }
    fn analyze_named(&mut self) -> Result<()> {
        let mut frames: Vec<&'a Value> = vec![];
        for (n, _) in walk(self.root) {
            if is_in(n, "maxStack") {
                if let Some(f) = self.enclosing(n) {
                    if f["vararg"] == true && !frames.iter().any(|g| same(f, g)) {
                        frames.push(f);
                    }
                }
            }
        }
        let frame = frames
            .iter()
            .copied()
            .find(|f| {
                walk(f)
                    .into_iter()
                    .any(|(n, _)| is_in(n, "fetchInstruction"))
            })
            .or_else(|| frames.first().copied())
            .ok_or_else(|| err("unexpected frame function"))?;
        let mk = self
            .enclosing(frame)
            .ok_or_else(|| err("unexpected proto-closure factory"))?;
        let module = self
            .enclosing(mk)
            .ok_or_else(|| err("unexpected proto-closure factory"))?;
        if array(&mk["args"]).len() != 2 {
            return Err(err("unexpected proto-closure factory"));
        }
        self.setup(frame, mk, module)?;
        let m = first(&mk["args"]);
        let based =
            |v: &Value, name: &str| is_in(v, name) && loc_of(&v["expr"]).map(lkey) == Some(lkey(m));
        for s in body(mk) {
            if ty(s) == "AstStatLocal"
                && array(&s["vars"]).len() == 1
                && array(&s["values"]).len() == 1
                && based(unparen(first(&s["values"])), "constantReader")
            {
                self.set(first(&s["vars"]), "KGET")?;
            }
        }
        let mut indices = BTreeMap::new();
        for (i, s) in body(frame).iter().enumerate() {
            if ty(s) != "AstStatLocal"
                || array(&s["vars"]).len() != 1
                || array(&s["values"]).len() != 1
            {
                continue;
            }
            let v = unparen(first(&s["values"]));
            let var = first(&s["vars"]);
            if ty(v) == "AstExprBinary"
                && v["op"] == "Or"
                && ty(unparen(&v["right"])) == "AstExprCall"
            {
                let call = unparen(&v["right"]);
                if based(unparen(first(&call["args"])), "maxStack") && loc_of(&v["left"]).is_some()
                {
                    self.set(var, "REGS")?;
                    self.set_opt(loc_of(&v["left"]), "RECYCLE")?;
                    indices.insert("REGS", i);
                }
            } else if let Some(role) = [
                ("instructionReader", "READER"),
                ("constantReader", "KGET"),
                ("simpleFields", "SIMPLEF"),
                ("entry", "PC"),
                ("fetchInstruction", "FETCH"),
            ]
            .iter()
            .find_map(|(field, role)| if based(v, field) { Some(*role) } else { None })
            {
                self.set(var, role)?;
                indices.insert(role, i);
            } else if ty(v) == "AstExprIndexExpr"
                && based(unparen(&v["index"]), "index")
                && loc_of(&v["expr"]).is_some()
            {
                self.set(var, "PROF")?;
                self.set_opt(loc_of(&v["expr"]), "PROFTAB")?;
                indices.insert("PROF", i);
            } else if ty(v) == "AstExprUnary"
                && v["op"] == "Minus"
                && const_num(&v["expr"]) == Some(1.0)
            {
                self.set(var, "TOP")?;
            } else if based(v, "numParams") {
                indices.insert("NUMPARAMS", i);
            }
        }
        for (n, _) in walk(mk) {
            if ty(n) != "AstStatAssign"
                || array(&n["vars"]).len() != 1
                || array(&n["values"]).len() != 1
            {
                continue;
            }
            let target = loc_of(first(&n["vars"]));
            let v = unparen(first(&n["values"]));
            if target.is_none() {
                continue;
            }
            if let Some(role) = [
                ("importReader", "IMP"),
                ("fusedFields", "FUSED"),
                ("captureFields", "CAPF"),
            ]
            .iter()
            .find_map(|(field, role)| if based(v, field) { Some(*role) } else { None })
            {
                self.set_opt(target, role)?;
            } else if ty(v) == "AstExprFunction"
                && array(&v["args"]).len() >= 10
                && self.local("CTRL").is_null()
                && self.enclosing(n).map(|f| same(f, frame)) == Some(true)
            {
                self.set_opt(target, "CTRL")?;
                self.ctrl = v;
            }
        }
        self.need(&[
            "REGS", "KGET", "IMP", "FUSED", "SIMPLEF", "PROF", "CTRL", "TOP", "PC", "FETCH",
            "READER", "CAPF",
        ])?;
        let ip = *indices
            .get("NUMPARAMS")
            .ok_or_else(|| err("loader role not found: NUMPARAMS"))?;
        self.varargs(&body(frame)[ip + 1..body(frame).len().min(ip + 10)])?;
        let sf = *indices
            .get("SIMPLEF")
            .ok_or_else(|| err("simple-op set not found"))?;
        self.simple_set(&body(frame)[sf + 1..body(frame).len().min(sf + 5)])?;
        fn loops<'b>(stmts: &'b [Value], out: &mut Vec<&'b Value>) {
            for s in stmts {
                match ty(s) {
                    "AstStatWhile" | "AstStatRepeat" => out.push(s),
                    "AstStatBlock" => loops(array(&s["body"]), out),
                    _ => {}
                }
            }
        }
        let mut all = vec![];
        loops(body(frame), &mut all);
        self.loop_node = *all
            .last()
            .ok_or_else(|| err("interpreter loop not found"))?;
        Ok(())
    }
    fn varargs(&mut self, stmts: &[Value]) -> Result<()> {
        for s in stmts {
            if ty(s) != "AstStatLocal"
                || array(&s["vars"]).len() != 1
                || array(&s["values"]).len() != 1
            {
                continue;
            }
            let v = unparen(first(&s["values"]));
            if ty(v) == "AstExprLocal" && self.local("VARARGS").is_null() {
                self.set(first(&s["vars"]), "VARARGS")?;
                self.set(&v["local"], "NOVARARGS")?;
            } else if const_num(v) == Some(0.0) && self.local("NVAR").is_null() {
                self.set(first(&s["vars"]), "NVAR")?;
            }
        }
        Ok(())
    }
    fn simple_set(&mut self, stmts: &[Value]) -> Result<()> {
        for s in stmts {
            let v = unparen(first(&s["values"]));
            if ty(s) == "AstStatLocal"
                && array(&s["vars"]).len() == 1
                && array(&s["values"]).len() == 1
                && ty(v) == "AstExprLocal"
            {
                self.set(first(&s["vars"]), "SIMPLESETV")?;
                self.set(&v["local"], "SIMPLESET")?;
                return Ok(());
            }
        }
        Err(err("simple-op set not found"))
    }
    fn numeric_record_loop(loop_node: &Value) -> bool {
        for w in body(loop_node).windows(9) {
            let (fetch, fields, perm, sem) = (&w[0], &w[1], &w[2], &w[3]);
            if ty(fetch) != "AstStatLocal"
                || array(&fetch["vars"]).len() != 2
                || array(&fetch["values"]).len() != 1
                || ty(unparen(first(&fetch["values"]))) != "AstExprCall"
                || ty(fields) != "AstStatLocal"
                || array(&fields["vars"]).len() != 10
                || !array(&fields["values"]).is_empty()
                || ty(perm) != "AstStatIf"
                || ty(sem) != "AstStatLocal"
                || array(&sem["vars"]).len() != 1
                || array(&sem["values"]).len() != 1
            {
                continue;
            }
            let value = unparen(first(&sem["values"]));
            if ty(value) != "AstExprIndexExpr"
                || loc_of(&value["expr"]).map(lkey) != Some(lkey(first(&fetch["vars"])))
                || const_num(&value["index"]).is_none()
            {
                continue;
            }
            if w[4..9].iter().all(|s| {
                let v = unparen(first(&s["values"]));
                ty(s) == "AstStatLocal"
                    && array(&s["vars"]).len() == 1
                    && array(&s["values"]).len() == 1
                    && ty(v) == "AstExprIndexExpr"
                    && loc_of(&v["index"]).map(lkey) == Some(lkey(first(&sem["vars"])))
            }) {
                return true;
            }
        }
        false
    }
    fn lazy_member(fn_node: &'a Value, m: &Value, record: bool) -> Option<(&'a Value, i64)> {
        let mut found = vec![];
        for w in body(fn_node).windows(2) {
            let (s, init) = (&w[0], &w[1]);
            if ty(s) != "AstStatLocal"
                || array(&s["vars"]).len() != 1
                || array(&s["values"]).len() != 1
                || ty(init) != "AstStatIf"
            {
                continue;
            }
            let field = match numeric_member(first(&s["values"]), m) {
                Some(f) => f,
                None => continue,
            };
            let local = first(&s["vars"]);
            if nil_eq(&init["condition"]).map(lkey) != Some(lkey(local)) {
                continue;
            }
            let mut writes = false;
            let mut readers = vec![];
            for (n, _) in walk(init) {
                if ty(n) == "AstStatAssign"
                    && array(&n["vars"]).len() == 1
                    && array(&n["values"]).len() == 1
                    && numeric_member(first(&n["vars"]), m) == Some(field)
                    && loc_of(first(&n["values"])).map(lkey) == Some(lkey(local))
                {
                    writes = true;
                }
                if record
                    && ty(n) == "AstStatAssign"
                    && array(&n["vars"]).len() == 1
                    && array(&n["values"]).len() == 1
                {
                    let target = first(&n["vars"]);
                    let value = unparen(first(&n["values"]));
                    if loc_of(target).map(lkey) == Some(lkey(local))
                        && ty(value) == "AstExprFunction"
                    {
                        writes = true;
                        readers.push(value);
                    }
                }
            }
            let all_readers: Vec<_> = if record {
                walk(fn_node)
                    .into_iter()
                    .filter_map(|(n, _)| {
                        let value = unparen(first(&n["values"]));
                        (ty(n) == "AstStatAssign"
                            && array(&n["vars"]).len() == 1
                            && array(&n["values"]).len() == 1
                            && loc_of(first(&n["vars"])).map(lkey) == Some(lkey(local))
                            && ty(value) == "AstExprFunction")
                            .then_some(value)
                    })
                    .collect()
            } else {
                vec![]
            };
            let ten = record
                && walk(init).into_iter().any(|(function, _)| {
                    ty(function) == "AstExprFunction"
                        && walk(&function["body"]).into_iter().any(|(r, _)| {
                            let v = unparen(first(&r["list"]));
                            ty(r) == "AstStatReturn"
                                && array(&r["list"]).len() == 1
                                && ty(v) == "AstExprTable"
                                && array(&v["items"]).len() == 10
                        })
                });
            let unique_reader =
                readers.len() == 1 && all_readers.len() == 1 && same(readers[0], all_readers[0]);
            if writes && (!record || (unique_reader && ten)) {
                found.push((local, field));
            }
        }
        if found.len() == 1 {
            Some(found[0])
        } else {
            None
        }
    }
    fn analyze_numeric(&mut self) -> Result<()> {
        let mut candidates = vec![];
        for (f, _) in walk(self.root) {
            if ty(f) != "AstExprFunction" || f["vararg"] != true {
                continue;
            }
            let (start, end) = self.src.span(f)?;
            if end - start < 15000 {
                continue;
            }
            let mk = match self.enclosing(f) {
                Some(m) => m,
                None => continue,
            };
            let module = match self.enclosing(mk) {
                Some(m) => m,
                None => continue,
            };
            if array(&mk["args"]).len() != 2 || array(&module["args"]).len() < 2 {
                continue;
            }
            if walk(f).into_iter().any(|(n, _)| {
                matches!(ty(n), "AstStatRepeat" | "AstStatWhile") && Self::numeric_record_loop(n)
            }) {
                candidates.push((f, mk, module));
            }
        }
        if candidates.len() != 1 {
            return Err(err(
                "not a Bacon Guard loader: no unique numeric VM frame found",
            ));
        }
        let (frame, mk, module) = candidates[0];
        let m = first(&mk["args"]);
        let record = Self::lazy_member(frame, m, true)
            .ok_or_else(|| err("numeric frame readers are incomplete"))?;
        let constant = Self::lazy_member(mk, m, false)
            .ok_or_else(|| err("numeric frame readers are incomplete"))?;
        let loop_node = walk(frame)
            .into_iter()
            .map(|(n, _)| n)
            .find(|n| {
                matches!(ty(n), "AstStatRepeat" | "AstStatWhile") && Self::numeric_record_loop(n)
            })
            .ok_or_else(|| err("numeric record loop missing"))?;
        let fields: Vec<_> = walk(loop_node)
            .into_iter()
            .filter_map(|(n, _)| {
                if ty(n) != "AstExprBinary" || n["op"] != "Mul" {
                    None
                } else if const_num(&n["left"]) == Some(256.0) {
                    numeric_member(&n["right"], m)
                } else if const_num(&n["right"]) == Some(256.0) {
                    numeric_member(&n["left"], m)
                } else {
                    None
                }
            })
            .collect();
        let ids: BTreeSet<_> = fields.iter().copied().collect();
        if ids.len() != 1 || fields.len() < 2 {
            return Err(err(
                "numeric child and pattern keys do not share a validated prototype id",
            ));
        }
        self.setup(frame, mk, module)?;
        self.loop_node = loop_node;
        self.out["layout"] = json!("numeric");
        let init = body(mk).first().unwrap_or(&NIL);
        let call = unparen(&init["expr"]);
        if ty(init) == "AstStatExpr"
            && ty(call) == "AstExprCall"
            && array(&call["args"]).len() == 1
            && loc_of(first(&call["args"])).map(lkey) == Some(lkey(m))
        {
            if let Some(local) = loc_of(&call["func"]) {
                if self
                    .funcs
                    .get(lkey(local))
                    .is_some_and(|f| array(&f["args"]).len() == 1)
                {
                    self.set(local, "PROTOINIT")?;
                }
            }
        }
        self.field("record_reader", record.1);
        self.field("constant_reader", constant.1);
        self.field("proto_id", *ids.first().unwrap());
        self.set(record.0, "READER")?;
        self.set(constant.0, "KGET")?;
        self.numeric_frame_roles(m)?;
        self.numeric_format(m)?;
        self.numeric_reader_roles(m)?;
        self.numeric_other_roles(m)?;
        self.numeric_dump_fields(m)?;
        Ok(())
    }
    fn numeric_frame_roles(&mut self, m: &Value) -> Result<()> {
        let first_fetch = walk(self.frame)
            .into_iter()
            .filter(|(n, _)| {
                matches!(ty(n), "AstStatWhile" | "AstStatRepeat") && Self::numeric_record_loop(n)
            })
            .find_map(|(n, _)| {
                body(n).iter().find(|s| {
                    ty(s) == "AstStatLocal"
                        && array(&s["vars"]).len() == 2
                        && array(&s["values"]).len() == 1
                        && ty(unparen(first(&s["values"]))) == "AstExprCall"
                })
            });
        let fetch = first_fetch.ok_or_else(|| err("numeric fetch call roles not found"))?;
        let call = unparen(first(&fetch["values"]));
        self.set_opt(loc_of(&call["func"]), "FETCH")?;
        self.set_opt(loc_of(first(&call["args"])), "PC")?;
        let params = body(self.frame)
            .iter()
            .enumerate()
            .find_map(|(i, s)| {
                if ty(s) == "AstStatLocal"
                    && array(&s["vars"]).len() == 1
                    && array(&s["values"]).len() == 1
                {
                    numeric_member(first(&s["values"]), m).map(|f| (i, first(&s["vars"]), f))
                } else {
                    None
                }
            })
            .ok_or_else(|| err("numeric parameter-count role not found"))?;
        self.set(params.1, "NUMPARAMS")?;
        self.field("num_params", params.2);
        let frame = self.frame;
        self.varargs(&body(frame)[params.0 + 1..])?;
        for s in body(frame) {
            if ty(s) == "AstStatLocal"
                && array(&s["vars"]).len() == 1
                && array(&s["values"]).len() == 1
            {
                let v = unparen(first(&s["values"]));
                if ty(v) == "AstExprUnary"
                    && v["op"] == "Minus"
                    && const_num(&v["expr"]) == Some(1.0)
                {
                    self.set(first(&s["vars"]), "TOP")?;
                } else if ty(v) == "AstExprBinary"
                    && v["op"] == "Or"
                    && ty(unparen(&v["right"])) == "AstExprCall"
                {
                    let call = unparen(&v["right"]);
                    if let Some(f) = numeric_member(first(&call["args"]), m) {
                        self.set(first(&s["vars"]), "REGS")?;
                        self.set_opt(loc_of(&v["left"]), "RECYCLE")?;
                        self.field("max_stack", f);
                    }
                }
            }
            if ty(s) == "AstStatAssign"
                && array(&s["vars"]).len() == 1
                && array(&s["values"]).len() == 1
            {
                let v = unparen(first(&s["values"]));
                if loc_of(first(&s["vars"])).is_some()
                    && ty(v) == "AstExprFunction"
                    && array(&v["args"]).len() >= 10
                {
                    self.set_opt(loc_of(first(&s["vars"])), "CTRL")?;
                    self.ctrl = v;
                }
            }
        }
        self.need(&["REGS", "TOP", "PC", "FETCH", "CTRL"])
    }
    fn numeric_format(&mut self, m: &Value) -> Result<()> {
        let lb = body(self.loop_node);
        let perm = lb
            .windows(2)
            .find(|w| {
                ty(&w[0]) == "AstStatLocal"
                    && array(&w[0]["vars"]).len() == 10
                    && array(&w[0]["values"]).is_empty()
            })
            .map(|w| &w[1])
            .ok_or_else(|| err("numeric dispatch-format permutation not found"))?;
        if ty(perm) != "AstStatIf" {
            return Err(err("numeric dispatch-format permutation not found"));
        }
        let (mut fields, mut formats) = (BTreeSet::new(), BTreeSet::new());
        for (c, _, _) in if_chain(perm).0 {
            let c = unparen(c);
            if ty(c) != "AstExprBinary" || c["op"] != "CompareEq" {
                return Err(err("numeric dispatch-format ladder expected"));
            }
            if let Some(f) = numeric_member(&c["left"], m) {
                fields.insert(f);
            }
            formats.insert(const_num(&c["right"]).and_then(int));
        }
        if fields.len() != 1
            || formats != [Some(0), Some(1), Some(2), Some(3)].into_iter().collect()
        {
            return Err(err("numeric dispatch-format ladder is ambiguous"));
        }
        self.field("dispatch_format", *fields.first().unwrap());
        Ok(())
    }
    fn assigned_function(&self, local: &Value, scope: &'a Value) -> Option<&'a Value> {
        walk(scope).into_iter().find_map(|(n, _)| {
            let v = unparen(first(&n["values"]));
            if ty(n) == "AstStatAssign"
                && array(&n["vars"]).len() == 1
                && array(&n["values"]).len() == 1
                && loc_of(first(&n["vars"])).map(lkey) == Some(lkey(local))
                && ty(v) == "AstExprFunction"
            {
                Some(v)
            } else {
                None
            }
        })
    }
    fn role_function(&self, role: &str, scope: &'a Value) -> Result<&'a Value> {
        self.assigned_function(self.local(role), scope)
            .ok_or_else(|| err(format!("reader function for {role} not found")))
    }
    fn numeric_reader_roles(&mut self, m: &Value) -> Result<()> {
        let mut arities: BTreeMap<usize, Vec<(&Value, &Value)>> = BTreeMap::new();
        for (n, _) in walk(self.frame) {
            if ty(n) != "AstStatAssign"
                || array(&n["vars"]).len() != 1
                || array(&n["values"]).len() != 1
                || numeric_member(first(&n["vars"]), m).is_none()
            {
                continue;
            }
            let local = match loc_of(first(&n["values"])) {
                Some(l) => l,
                None => continue,
            };
            let f = self
                .funcs
                .get(lkey(local))
                .copied()
                .or_else(|| self.assigned_function(local, self.frame));
            let f = match f {
                Some(f) => f,
                None => continue,
            };
            let p = match self.perm(body(f).first().unwrap_or(&NIL), Some(first(&f["args"]))) {
                Ok(p) => p,
                Err(_) => continue,
            };
            let counts: BTreeSet<_> = p
                .as_object()
                .unwrap()
                .values()
                .map(|v| array(v).len())
                .collect();
            if counts.len() == 1 {
                arities
                    .entry(*counts.first().unwrap())
                    .or_default()
                    .push((local, f));
            }
        }
        for (n, role) in [(6, "FUSED"), (7, "SIMPLEF"), (4, "CAPF")] {
            if let Some(items) = arities.get(&n) {
                if items.len() == 1 {
                    self.set(items[0].0, role)?;
                }
            }
        }
        self.need(&["FUSED", "SIMPLEF", "CAPF"])
    }
    fn numeric_other_roles(&mut self, m: &Value) -> Result<()> {
        let frame = self.frame;
        for s in body(frame) {
            if ty(s) != "AstStatLocal"
                || array(&s["vars"]).len() != 1
                || array(&s["values"]).len() != 1
            {
                continue;
            }
            let v = unparen(first(&s["values"]));
            if ty(v) == "AstExprIndexExpr"
                && numeric_member(&v["index"], m) == self.num("proto_id")
                && loc_of(&v["expr"]).is_some()
            {
                self.set(first(&s["vars"]), "PROF")?;
                self.set_opt(loc_of(&v["expr"]), "PROFTAB")?;
                break;
            }
        }
        let i = body(frame)
            .iter()
            .position(|s| {
                ty(s) == "AstStatLocal"
                    && array(&s["vars"]).len() == 1
                    && self.out["roles"][lkey(first(&s["vars"]))] == "SIMPLEF"
            })
            .ok_or_else(|| err("numeric simple-op set not found"))?;
        self.simple_set(&body(frame)[i + 1..body(frame).len().min(i + 6)])?;
        for (n, _) in walk(self.mk) {
            if ty(n) != "AstStatAssign"
                || array(&n["vars"]).len() != 1
                || array(&n["values"]).len() != 1
                || numeric_member(first(&n["values"]), m).is_none()
            {
                continue;
            }
            let local = match loc_of(first(&n["vars"])) {
                Some(l) => l,
                None => continue,
            };
            if let Some(f) = self.assigned_function(local, self.mk) {
                if walk(f)
                    .into_iter()
                    .any(|(q, _)| ty(q) == "AstExprCall" && self.role(&q["func"]) == Some("KGET"))
                {
                    self.set(local, "IMP")?;
                    break;
                }
            }
        }
        self.need(&["PROF", "IMP"])
    }
    fn nested_numeric(v: &Value, m: &Value) -> Option<i64> {
        let v = unparen(v);
        if ty(v) == "AstExprIndexExpr" {
            numeric_member(&v["expr"], m)
        } else {
            None
        }
    }
    fn numeric_dump_fields(&mut self, m: &Value) -> Result<()> {
        let reader = self.role_function("READER", self.frame)?;
        let kget = self.role_function("KGET", self.mk)?;
        for (f, cache, data) in [
            (reader, "record_cache", "record_data"),
            (kget, "constant_cache", "constant_data"),
        ] {
            let mut fields = vec![];
            for (n, _) in walk(f) {
                if let Some(n) = Self::nested_numeric(n, m) {
                    if !fields.contains(&n) {
                        fields.push(n);
                    }
                }
            }
            if fields.len() < 2 {
                return Err(err("numeric data tables are incomplete"));
            }
            self.field(cache, fields[0]);
            self.field(data, fields[1]);
        }
        for s in body(self.frame) {
            if ty(s) == "AstStatLocal"
                && array(&s["vars"]).len() == 1
                && lkey(first(&s["vars"])) == lkey(self.local("PC"))
            {
                if let Some(n) = numeric_member(first(&s["values"]), m) {
                    self.field("entry", n);
                    break;
                }
            }
        }
        let va = walk(self.frame)
            .into_iter()
            .find_map(|(n, _)| {
                if ty(n) != "AstStatIf" {
                    return None;
                }
                let f = numeric_member(&n["condition"], m)?;
                if walk(&n["thenbody"]).into_iter().any(|(q, _)| {
                    ty(q) == "AstExprCall"
                        && ty(unparen(&q["func"])) == "AstExprGlobal"
                        && unparen(&q["func"])["global"] == "select"
                }) {
                    Some(f)
                } else {
                    None
                }
            })
            .ok_or_else(|| err("numeric vararg field not found"))?;
        self.field("is_vararg", va);
        let mut children = vec![];
        for (n, _) in walk(self.ctrl) {
            if let Some(f) = Self::nested_numeric(n, m) {
                if Some(f) != self.num("record_cache") && !children.contains(&f) {
                    children.push(f);
                }
            }
        }
        if children.len() != 1 {
            return Err(err("numeric child table is ambiguous"));
        }
        self.field("children", children[0]);
        let mut decoder = None;
        for (n, _) in walk(self.module) {
            if ty(n) != "AstExprFunction" || array(&n["args"]).len() != 1 {
                continue;
            }
            for (q, _) in walk(n) {
                let v = unparen(first(&q["values"]));
                if ty(q) == "AstStatLocal"
                    && array(&q["vars"]).len() == 1
                    && array(&q["values"]).len() == 1
                    && ty(v) == "AstExprTable"
                    && array(&v["items"])
                        .iter()
                        .filter(|i| !i["key"].is_null())
                        .count()
                        >= 7
                    && array(&v["items"]).iter().any(|i| {
                        ty(unparen(&i["value"])) == "AstExprBinary"
                            && unparen(&i["value"])["op"] == "CompareNe"
                    })
                {
                    decoder = Some((n, first(&q["vars"])));
                    break;
                }
            }
            if decoder.is_some() {
                break;
            }
        }
        let (decoder, work) = decoder.ok_or_else(|| err("numeric prototype decoder not found"))?;
        let mut nups = BTreeSet::new();
        for (q, _) in walk(self.ctrl) {
            let limit = unparen(&q["to"]);
            if ty(q) == "AstStatFor"
                && ty(limit) == "AstExprBinary"
                && limit["op"] == "Sub"
                && const_num(&limit["right"]) == Some(1.0)
            {
                let v = unparen(&limit["left"]);
                if ty(v) == "AstExprIndexExpr" {
                    if let Some(f) = const_num(&v["index"]).and_then(int) {
                        nups.insert(f);
                    }
                }
            }
        }
        if nups.len() != 1 {
            return Err(err("numeric upvalue-count field not found"));
        }
        self.field("num_upvalues", *nups.first().unwrap());
        let mut vm = None;
        let mut shift_detected = false;
        for (q, _) in walk(decoder) {
            if ty(q) != "AstStatAssign"
                || array(&q["vars"]).len() != 1
                || array(&q["values"]).len() != 1
            {
                continue;
            }
            let target = unparen(first(&q["vars"]));
            if ty(target) != "AstExprIndexExpr"
                || loc_of(&target["expr"]).map(lkey) != Some(lkey(work))
            {
                continue;
            }
            let slot = match const_num(&target["index"]).and_then(int) {
                Some(f) => f,
                None => continue,
            };
            for (call, _) in walk(unparen(first(&q["values"]))) {
                if ty(call) != "AstExprCall"
                    || !is_in(unparen(&call["func"]), "band")
                    || array(&call["args"]).len() != 2
                {
                    continue;
                }
                let mask = const_num(nth(&call["args"], 1));
                let shifted = unparen(first(&call["args"]));
                if mask == Some(31.0) {
                    shift_detected = true;
                }
                if mask == Some(3.0)
                    && ty(shifted) == "AstExprCall"
                    && is_in(unparen(&shifted["func"]), "rshift")
                    && array(&shifted["args"]).len() == 2
                    && const_num(nth(&shifted["args"], 1)) == Some(1.0)
                {
                    vm = Some(slot);
                }
            }
        }
        if !shift_detected || vm.is_none() {
            return Err(err("numeric prototype VM-family field is incomplete"));
        }
        self.field("vm_family", vm.unwrap());
        let rs = body(self.frame)
            .iter()
            .find_map(|s| {
                if ty(s) != "AstStatLocal"
                    || array(&s["vars"]).len() != 1
                    || array(&s["values"]).len() != 1
                {
                    return None;
                }
                let v = unparen(first(&s["values"]));
                if ty(v) == "AstExprBinary"
                    && v["op"] == "Or"
                    && const_num(&v["right"]) == Some(0.0)
                {
                    numeric_member(&v["left"], m)
                } else {
                    None
                }
            })
            .ok_or_else(|| err("numeric register-shift field not found"))?;
        self.field("reg_shift", rs);
        let mut aot = BTreeSet::new();
        for w in body(self.mk).windows(2) {
            let (s, branch) = (&w[0], &w[1]);
            if ty(s) != "AstStatLocal"
                || array(&s["vars"]).len() != 1
                || array(&s["values"]).len() != 1
                || ty(branch) != "AstStatIf"
            {
                continue;
            }
            if let Some(slot) = numeric_member(first(&s["values"]), m) {
                if walk(branch).into_iter().any(|(q, _)| {
                    ty(q) == "AstExprCall"
                        && loc_of(&q["func"]).map(lkey) == Some(lkey(first(&s["vars"])))
                }) {
                    aot.insert(slot);
                }
            }
        }
        if aot.len() != 1 {
            return Err(err("numeric native-handler field is ambiguous"));
        }
        self.field("aot_handler", *aot.first().unwrap());
        Ok(())
    }
    fn analyze_loop(&mut self) -> Result<()> {
        let lb = body(self.loop_node);
        let i = lb
            .iter()
            .position(|s| {
                ty(s) == "AstStatLocal"
                    && array(&s["vars"]).len() == 2
                    && array(&s["values"]).len() == 1
                    && ty(unparen(first(&s["values"]))) == "AstExprCall"
            })
            .ok_or_else(|| err("loop fetch not found"))?;
        let s = &lb[i];
        self.set(nth(&s["vars"], 0), "REC")?;
        self.set(nth(&s["vars"], 1), "CLR")?;
        let s10 = lb.get(i + 1).unwrap_or(&NIL);
        if ty(s10) != "AstStatLocal"
            || array(&s10["vars"]).len() != 10
            || !array(&s10["values"]).is_empty()
        {
            return Err(err("field locals not found"));
        }
        for (v, name) in array(&s10["vars"])
            .iter()
            .zip(["OP", "EF", "A", "B", "C", "RF", "HF", "AUX", "NXT", "ALT"])
        {
            self.set(v, name)?;
        }
        self.out["perm"] = self.perm(lb.get(i + 2).unwrap_or(&NIL), Some(self.local("REC")))?;
        if self.out["layout"] == "numeric" {
            let mut masks = json!({});
            for (condition, branch, _) in if_chain(lb.get(i + 2).unwrap_or(&NIL)).0 {
                let format = const_num(&unparen(condition)["right"])
                    .and_then(int)
                    .ok_or_else(|| err("dispatchFormat integer expected"))?;
                let mut fields = json!({});
                for (index, mask) in stmt_fields(
                    first(&branch["body"]),
                    Some(self.local("PROTO")),
                    Some(self.local("REC")),
                )? {
                    if let Some(mask) = mask {
                        let key = index.to_string();
                        if !fields[&key].is_null() && fields[&key] != mask {
                            return Err(err("conflicting record masks"));
                        }
                        fields[key] = json!(mask);
                    }
                }
                masks[format.to_string()] = fields;
            }
            self.out["record_masks"] = masks;
        }
        let j = i + 3;
        let sem = lb.get(j).unwrap_or(&NIL);
        let v = unparen(first(&sem["values"]));
        if ty(sem) != "AstStatLocal" || ty(v) != "AstExprIndexExpr" {
            return Err(err("semantic key not found"));
        }
        let ix = const_num(&v["index"])
            .and_then(int)
            .ok_or_else(|| err("semantic key index not constant"))?;
        self.out["sem_index"] = json!(ix);
        self.set(first(&sem["vars"]), "SEM")?;
        for k in 1..=5 {
            let s = lb.get(j + k).unwrap_or(&NIL);
            if ty(s) != "AstStatLocal" {
                return Err(err(format!("dedicated table {k} not found")));
            }
            let v = unparen(first(&s["values"]));
            self.set_opt(loc_of(&v["expr"]), &format!("D{k}"))?;
            self.set(first(&s["vars"]), &format!("D{k}V"))?;
        }
        let d6 = lb.get(j + 6).unwrap_or(&NIL);
        if ty(d6) != "AstStatIf" {
            return Err(err("dedicated table 6 not found"));
        }
        let v = unparen(first(&first(&d6["thenbody"]["body"])["values"]));
        self.set_opt(loc_of(&v["expr"]), "D6")?;
        let p1 = lb.get(j + 7).unwrap_or(&NIL);
        let v = unparen(first(&p1["values"]));
        self.set_opt(loc_of(&v["expr"]), "P1")?;
        self.set(first(&p1["vars"]), "PATV")?;
        let p2 = lb.get(j + 8).unwrap_or(&NIL);
        let v = unparen(first(&first(&p2["thenbody"]["body"])["values"]));
        self.set_opt(loc_of(&v["expr"]), "P2")?;
        self.dispatch(lb.get(j + 9).unwrap_or(&NIL))?;
        Ok(())
    }
    fn dispatch(&mut self, s: &'a Value) -> Result<()> {
        if ty(s) != "AstStatIf" {
            return Err(err("dispatch chain not found"));
        }
        let (ch, fin) = if_chain(s);
        for (c, b, n) in ch {
            let c = unparen(c);
            if ty(c) == "AstExprBinary" && c["op"] == "CompareNe" {
                if let Some(name) = self.role(&c["left"]) {
                    if name.starts_with('D') && name.ends_with('V') {
                        self.branches.insert(name.to_string(), b);
                        continue;
                    }
                    if name == "PATV" {
                        self.branches.insert("PAT".into(), b);
                        continue;
                    }
                }
            }
            if let Some((local, op)) = eq_info(c) {
                if self.out["roles"][lkey(local)] == "OP" {
                    self.out["junk_op"] =
                        json!(int(op).ok_or_else(|| err("junk opcode must be integer"))?);
                    self.branches.insert("JUNK".into(), b);
                    continue;
                }
            }
            let terms = or_terms(c);
            if terms.len() >= 2 && terms.iter().all(|t| eq_info(t).is_some()) {
                self.hot = terms
                    .into_iter()
                    .map(|t| {
                        int(eq_info(t).unwrap().1).ok_or_else(|| err("hot opcode must be integer"))
                    })
                    .collect::<Result<_>>()?;
                self.branches.insert("HOT".into(), b);
                continue;
            }
            if ty(c) == "AstExprBinary"
                && matches!(c["op"].as_str(), Some("CompareEq" | "CompareNe"))
                && ty(unparen(&c["left"])) == "AstExprIndexExpr"
            {
                self.branches.insert("SIMPLE".into(), b);
                continue;
            }
            return Err(err(format!(
                "unrecognised dispatch branch at {}",
                n["location"]
            )));
        }
        self.branches.insert(
            "CTRLCALL".into(),
            fin.ok_or_else(|| err("control fallback missing"))?,
        );
        for name in [
            "D1V", "D2V", "D3V", "D4V", "D5V", "PAT", "JUNK", "HOT", "SIMPLE",
        ] {
            if !self.branches.contains_key(name) {
                return Err(err(format!("dispatch branch missing: {name}")));
            }
        }
        Ok(())
    }
    fn free_tables(&self, f: &Value) -> Result<Vec<Value>> {
        let (fa, fb) = self.src.span(f)?;
        let mut out = vec![];
        let mut seen = BTreeSet::new();
        for (n, _) in walk(f) {
            let local = if ty(n) == "AstExprLocal" {
                &n["local"]
            } else {
                continue;
            };
            let k = lkey(local);
            if !seen.insert(k) {
                continue;
            }
            let (st, kind) = match self.decl.get(k) {
                Some(d) => *d,
                None => continue,
            };
            if kind != "local" {
                continue;
            }
            let (sa, sb) = self.src.span(st)?;
            if fa <= sa && sb <= fb {
                continue;
            }
            for (v, val) in array(&st["vars"]).iter().zip(array(&st["values"])) {
                if lkey(v) == k && ty(unparen(val)) == "AstExprTable" {
                    out.push(json!([v, unparen(val)]));
                }
            }
        }
        Ok(out)
    }
    fn analyze_module(&mut self) -> Result<()> {
        let mb = body(self.module);
        let named_simple_roles = self.out["layout"] == "named";
        let last = mb.last().unwrap_or(&NIL);
        let call = unparen(first(&last["list"]));
        if ty(last) != "AstStatReturn"
            || array(&last["list"]).len() != 1
            || ty(call) != "AstExprCall"
        {
            return Err(err("module tail not found"));
        }
        if loc_of(&call["func"]).map(lkey) != Some(lkey(self.local("MK"))) {
            return Err(err("module tail does not call the proto factory"));
        }
        let main = loc_of(first(&call["args"])).ok_or_else(|| err("main local not found"))?;
        self.set(main, "MAIN")?;
        let (st, _) = self
            .decl
            .get(lkey(main))
            .copied()
            .ok_or_else(|| err("main proto statement not found"))?;
        let value = array(&st["vars"])
            .iter()
            .zip(array(&st["values"]))
            .find(|(v, _)| lkey(v) == lkey(main))
            .map(|(_, v)| unparen(v))
            .unwrap_or(&NIL);
        if ty(st) != "AstStatLocal"
            || ty(value) != "AstExprIndexExpr"
            || !mb.iter().any(|s| same(s, st))
        {
            return Err(err("main proto statement not found"));
        }
        self.set_opt(loc_of(&value["expr"]), "PROTOS")?;
        let (main, mapper) = match unparen(&value["index"]) {
            index
                if self.out["layout"] == "numeric"
                    && ty(index) == "AstExprCall"
                    && array(&index["args"]).len() == 1
                    && loc_of(&index["func"]).is_some() =>
            {
                (loc_of(first(&index["args"])), loc_of(&index["func"]))
            }
            index => (loc_of(index), None),
        };
        self.set_opt(main, "MAINIDX")?;
        if let Some(mapper) = mapper {
            if !self
                .funcs
                .get(lkey(mapper))
                .is_some_and(|f| array(&f["args"]).len() == 1)
            {
                return Err(err("prototype map function not found"));
            }
            self.set(mapper, "PROTOMAP")?;
        }
        self.out["tail_stmt"] = last.clone();
        self.out["main_stmt"] = st.clone();
        let pat = self.branches["PAT"];
        let pat = first(&pat["body"]);
        if ty(pat) != "AstStatFor" {
            return Err(err("pattern loop not found"));
        }
        let (mut xorkey, mut icall) = (None, None);
        for (n, _) in walk(&pat["body"]) {
            if ty(n) == "AstStatLocal" && array(&n["values"]).len() == 1 {
                let value = unparen(first(&n["values"]));
                if ty(value) == "AstExprCall" {
                    if self.role(&value["func"]) == Some("FUSED") && array(&n["vars"]).len() == 6 {
                        for (local, role) in array(&n["vars"])
                            .iter()
                            .zip(["A", "B", "C", "RF", "HF", "AUX"])
                        {
                            let key = lkey(local);
                            if named_simple_roles {
                                self.previous_simple_roles
                                    .entry(key.to_owned())
                                    .or_insert_with(|| self.out["roles"][&key].clone());
                            }
                            self.out["roles"][key] = json!(role);
                        }
                    } else if is_in(unparen(&value["func"]), "bxor")
                        && array(&value["args"]).len() == 2
                        && array(&n["vars"]).len() == 1
                        && loc_of(nth(&value["args"], 1)).is_some()
                    {
                        let key = lkey(first(&n["vars"]));
                        if named_simple_roles {
                            self.previous_simple_roles
                                .entry(key.to_owned())
                                .or_insert_with(|| self.out["roles"][&key].clone());
                        }
                        self.out["roles"][key] = json!("OP");
                    }
                }
            }
            if ty(n) == "AstExprCall"
                && is_in(unparen(&n["func"]), "bxor")
                && array(&n["args"]).len() == 2
            {
                if let Some(l) = loc_of(nth(&n["args"], 1)) {
                    xorkey = Some(l);
                }
            }
            let cl = unparen(&n["expr"]);
            if ty(n) == "AstStatExpr" && ty(cl) == "AstExprCall" && loc_of(&cl["func"]).is_some() {
                icall = Some(cl);
            }
        }
        self.set_opt(xorkey, "XORKEY")?;
        let icall = icall.ok_or_else(|| err("pattern decoding not recognised"))?;
        self.set_opt(loc_of(&icall["func"]), "SIMPLEFN")?;
        self.out["pat_call"] = icall.clone();
        let junk = first(&self.branches["JUNK"]["body"]);
        let jc = walk(junk)
            .into_iter()
            .map(|(n, _)| n)
            .rfind(|n| {
                ty(n) == "AstExprCall"
                    && loc_of(&n["func"]).is_some()
                    && array(&n["args"]).len() == 6
            })
            .ok_or_else(|| err("junk mix call not found"))?;
        self.set_opt(loc_of(&jc["func"]), "MIX")?;
        let sf = self
            .funcs
            .get(lkey(self.local("SIMPLEFN")))
            .copied()
            .ok_or_else(|| err("simple dispatcher body not found"))?;
        self.out["simple_fn"] = sf.clone();
        if array(&icall["args"]).len() != 12 {
            return Err(err("simple dispatcher call arity"));
        }
        let order = array(&icall["args"])
            .iter()
            .map(|arg| {
                self.role(arg)
                    .map(str::to_owned)
                    .ok_or_else(|| err("simple dispatcher argument role not found"))
            })
            .collect::<Result<Vec<_>>>()?;
        self.out["simple_param_roles"] = json!(order
            .iter()
            .map(|role| if named_simple_roles && role == "OP" {
                "X"
            } else {
                role.as_str()
            })
            .collect::<Vec<_>>());
        self.out["roles_param_names"] = json!({});
        for (prm, nm) in array(&sf["args"]).iter().zip(order) {
            let role = if named_simple_roles && nm == "OP" {
                "X".to_owned()
            } else {
                nm.clone()
            };
            self.out["roles"][lkey(prm)] = json!(format!("S_{role}"));
            self.out["roles_param_names"][lkey(prm)] = json!(role);
        }
        let tables = self.free_tables(sf)?;
        if !tables.iter().any(|pair| {
            array(&pair[1]["items"]).iter().any(|i| {
                !i["key"].is_null()
                    && matches!(ty(unparen(&i["value"])), "AstExprFunction" | "AstExprLocal")
            })
        }) {
            return Err(err("simple handler table not found"));
        }
        self.out["simple_tables"] = json!(tables);
        self.out["xor_stmt"] = Value::Null;
        Ok(())
    }
    fn analyze_fields(&mut self) -> Result<()> {
        for (role, key) in [
            ("FUSED", "fused_perm"),
            ("SIMPLEF", "simple_perm"),
            ("CAPF", "cap_perm"),
        ] {
            // SIMPLEF deliberately uses the last assignment; the other readers use the first.
            let f = {
                let mut functions = walk(self.frame).into_iter().filter_map(|(n, _)| {
                    let v = unparen(first(&n["values"]));
                    (ty(n) == "AstStatAssign"
                        && array(&n["vars"]).len() == 1
                        && loc_of(first(&n["vars"])).map(lkey) == Some(lkey(self.local(role)))
                        && ty(v) == "AstExprFunction")
                        .then_some(v)
                });
                if role == "SIMPLEF" {
                    functions.next_back()
                } else {
                    functions.next()
                }
            }
            .ok_or_else(|| err(format!("reader function for {role} not found")))?;
            self.out[key] = self.perm(
                body(f).first().unwrap_or(&NIL),
                (self.out["layout"] == "numeric").then_some(first(&f["args"])),
            )?;
            if self.out["layout"] == "numeric" {
                let statement = body(f).first().unwrap_or(&NIL);
                let branches = if ty(statement) == "AstStatIf" {
                    if_chain(statement)
                        .0
                        .into_iter()
                        .map(|(condition, branch, _)| {
                            (
                                const_num(&unparen(condition)["right"])
                                    .and_then(int)
                                    .unwrap(),
                                first(&branch["body"]),
                            )
                        })
                        .collect::<Vec<_>>()
                } else {
                    (0..4).map(|format| (format, statement)).collect()
                };
                for (format, statement) in branches {
                    for (index, mask) in stmt_fields(
                        statement,
                        Some(self.local("PROTO")),
                        Some(first(&f["args"])),
                    )? {
                        if self.out["record_masks"][format.to_string()][index.to_string()]
                            != json!(mask)
                        {
                            return Err(err("reader masks do not match record masks"));
                        }
                    }
                }
            }
        }
        Ok(())
    }
    fn decl_table(&self, local: &Value) -> Result<&'a Value> {
        let (st, kind) = self
            .decl
            .get(lkey(local))
            .copied()
            .ok_or_else(|| err("table declaration not found"))?;
        if kind != "local" {
            return Err(err("table declaration expected"));
        }
        let val = array(&st["vars"])
            .iter()
            .zip(array(&st["values"]))
            .find(|(v, _)| lkey(v) == lkey(local))
            .map(|(_, v)| unparen(v))
            .ok_or_else(|| err("table declaration not found"))?;
        if ty(val) != "AstExprTable" {
            return Err(err("table constructor expected"));
        }
        Ok(val)
    }
    fn extend(&mut self) -> Result<()> {
        self.out["simple_table_items"] = json!(array(&self.out["simple_tables"])
            .iter()
            .map(|p| json!([p[0], table_items(&p[1])]))
            .collect::<Vec<_>>());
        self.out["ded_decl"] = json!({});
        for ti in 1..=6 {
            let tab = self.decl_table(self.local(&format!("D{ti}")))?;
            let mut items = vec![];
            for pair in table_items(tab) {
                let val = &pair[1];
                let f = if let Some(l) = loc_of(val) {
                    self.funcs.get(lkey(l)).copied()
                } else if ty(val) == "AstExprFunction" {
                    Some(val)
                } else {
                    None
                };
                let f =
                    f.ok_or_else(|| err(format!("dedicated handler {ti}/{} not found", pair[0])))?;
                items.push(json!([pair[0], f]));
            }
            self.out["ded_decl"][ti.to_string()] = json!(items);
        }
        let hot = self.branches["HOT"];
        let inner = array(&hot["body"])
            .iter()
            .find(|s| ty(s) == "AstStatIf")
            .ok_or_else(|| err("hot chain not found"))?;
        let handlers = handler_chain(inner, self.local("OP"))?;
        let ops: BTreeSet<_> = handlers.iter().map(|p| p[0].as_i64().unwrap()).collect();
        if ops != self.hot.iter().copied().collect() {
            return Err(err(
                "hot chain opcode coverage does not match its dispatch branch",
            ));
        }
        self.out["hot_chain"] = json!(handlers);
        let first_if = body(self.ctrl)
            .iter()
            .find(|s| ty(s) == "AstStatIf")
            .ok_or_else(|| err("control chain not found"))?;
        self.out["ctrl_chain"] = json!(handler_chain(first_if, first(&self.ctrl["args"]))?);
        for s in body(self.mk) {
            if ty(s) == "AstStatLocalFunction" {
                self.out["roles"][lkey(&s["name"])] = json!("MKFN");
            }
        }
        self.out["call_roles"] = json!({});
        for (role, branch) in [
            ("D1", "D1V"),
            ("D2", "D2V"),
            ("D3", "D3V"),
            ("D4", "D4V"),
            ("D5", "D5V"),
            ("CTRL", "CTRLCALL"),
        ] {
            let callee = if role == "CTRL" { "CTRL" } else { branch };
            let args = walk(self.branches[branch]).into_iter().find_map(|(n, _)| {
                if ty(n) == "AstExprCall" && self.role(&n["func"]) == Some(callee) {
                    Some(
                        array(&n["args"])
                            .iter()
                            .map(|v| self.role(v).map(str::to_owned))
                            .collect::<Vec<_>>(),
                    )
                } else {
                    None
                }
            });
            self.out["call_roles"][role] = json!(args);
        }
        self.out["call_roles"]["SIMPLE"] = json!(array(&self.out["pat_call"]["args"])
            .iter()
            .map(|v| self.role(v).map(str::to_owned))
            .collect::<Vec<_>>());
        for (key, previous) in std::mem::take(&mut self.previous_simple_roles) {
            if previous.is_null() {
                self.out["roles"].as_object_mut().unwrap().remove(&key);
            } else {
                self.out["roles"][key] = previous;
            }
        }
        self.out["branches"] = json!({});
        for (k, v) in &self.branches {
            self.out["branches"][k] = if k == "HOT" {
                json!([v, self.hot])
            } else if k == "JUNK" {
                json!([v, self.out["junk_op"]])
            } else {
                (*v).clone()
            };
        }
        Ok(())
    }
    fn run(mut self) -> Result<Value> {
        if walk(self.root)
            .into_iter()
            .any(|(n, _)| is_in(n, "maxStack"))
        {
            self.analyze_named()?;
        } else {
            self.analyze_numeric()?;
        }
        self.out["loop"] = self.loop_node.clone();
        self.out["ctrl_fn"] = self.ctrl.clone();
        self.analyze_loop()?;
        self.analyze_module()?;
        self.analyze_fields()?;
        self.extend()?;
        Ok(self.out)
    }
}
pub fn table_items(tab: &Value) -> Vec<Value> {
    array(&tab["items"])
        .iter()
        .filter_map(|it| {
            const_num(&it["key"])
                .and_then(int)
                .map(|k| json!([k, unparen(&it["value"])]))
        })
        .collect()
}
pub fn analyze(data: &[u8]) -> Result<Value> {
    let data = astutil::normalize_source(data);
    let text = astutil::latin1(&data);
    let root = astutil::parse_ast(&data)?;
    Analyzer::new(&root, &text)?.run()
}

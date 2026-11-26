//! Behavioral handler classification. Unknown and ambiguous operations remain explicit.
use crate::{
    error::{Error, Result},
    luavm::{
        array, location, ty, unparen, walk, Env, Interp, LuaValue as V, TrialHeap, VmError,
        VmResult,
    },
    refspec::{self, Fields, State},
};
use serde_json::{json, Map, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    time::{Duration, Instant},
};

pub const RAWF: [&str; 10] = ["op", "ef", "A", "B", "C", "RF", "HF", "AUX", "NXT", "ALT"];
pub const FIELDS: [&str; 6] = ["A", "B", "C", "RF", "HF", "AUX"];
pub type Domains = BTreeMap<String, Vec<Fields>>;
pub type Groups = Vec<(usize, Vec<Value>)>;
fn err(s: impl Into<String>) -> Error {
    Error::new("unsupported", s)
}
fn vmerr(e: VmError) -> Error {
    err(e.to_string())
}
fn loc(n: &Value) -> Option<String> {
    let n = unparen(n);
    if ty(n) == "AstExprLocal" {
        Some(location(&n["local"]))
    } else {
        None
    }
}
fn cn(n: &Value) -> Option<f64> {
    let n = unparen(n);
    if ty(n) == "AstExprConstantNumber" {
        n["value"].as_f64()
    } else {
        None
    }
}
pub fn indexed(v: &Value, i: i64) -> Option<&Value> {
    if let Some(m) = v.as_object() {
        m.get(&i.to_string())
    } else {
        v.as_array()
            .and_then(|a| usize::try_from(i).ok().and_then(|i| a.get(i)))
    }
}
pub fn permute(table: &Value, df: i64, raw: &Value, count: usize) -> Result<Vec<Value>> {
    let perm = indexed(table, df)
        .ok_or_else(|| err(format!("missing field permutation for format {df}")))?;
    if array(perm).len() != count {
        return Err(err(format!(
            "invalid {count}-field permutation for format {df}"
        )));
    }
    array(perm)
        .iter()
        .map(|i| {
            let index = i
                .as_u64()
                .filter(|&n| n > 0)
                .ok_or_else(|| err("invalid field permutation index"))?;
            array(raw)
                .get((index - 1) as usize)
                .cloned()
                .ok_or_else(|| err("record has too few fields for its permutation"))
        })
        .collect()
}

pub fn collect_domains(loader: &Value, dump: &Value) -> Result<Domains> {
    let mut out: Domains = BTreeMap::new();
    let mut add = |key: String, fields: Fields| {
        if !fields.is_empty() {
            let a = out.entry(key).or_default();
            if !a.contains(&fields) {
                a.push(fields);
            }
        }
    };
    let protos = dump["protos"]
        .as_object()
        .ok_or_else(|| err("dump has no prototypes"))?;
    for (pid, p) in protos {
        let pid = pid
            .parse::<i64>()
            .map_err(|_| err("invalid prototype id"))?;
        let df = p["df"]
            .as_i64()
            .ok_or_else(|| err("missing dispatch format"))?;
        let recs = p["recs"]
            .as_object()
            .ok_or_else(|| err("prototype has no records"))?;
        for rec in recs.values() {
            let mut parts = vec![];
            for raw in array(&rec["parts"]) {
                let vals = permute(&loader["fused_perm"], df, raw, 6)?;
                parts.push(
                    FIELDS
                        .iter()
                        .zip(vals)
                        .filter_map(|(k, v)| v.as_f64().map(|v| ((*k).into(), v)))
                        .collect::<Fields>(),
                );
            }
            if rec["ded"].as_str() != Some("-") && !rec["ded"].is_null() {
                let ti = as_int(&rec["ded"])?;
                let sem = as_int(&rec["sem"])?;
                let key = if ti == 6 { sem + pid * 256 } else { sem };
                for (i, f) in parts.into_iter().enumerate() {
                    add(format!("ded:{ti}:{key}:{i}"), f);
                }
                continue;
            }
            if !rec["pat"].is_null() {
                let xor = as_int(&dump["xorkey"])?;
                for (i, b) in pattern(&rec["pat"])?.into_iter().enumerate() {
                    if let Some(f) = parts.get(i) {
                        add(format!("op:{}", b ^ xor), f.clone());
                    }
                }
                continue;
            }
            let vals = permute(&loader["perm"], df, &rec["T"], 10)?;
            let f: Fields = RAWF
                .iter()
                .zip(vals)
                .filter_map(|(k, v)| v.as_f64().map(|v| ((*k).into(), v)))
                .collect();
            let op = *f
                .get("op")
                .ok_or_else(|| err("record opcode is not numeric"))?;
            add(
                format!("op:{}", op as i64),
                f.into_iter()
                    .filter(|(k, _)| FIELDS.contains(&k.as_str()))
                    .collect(),
            );
        }
    }
    Ok(out)
}
pub fn as_int(v: &Value) -> Result<i64> {
    v.as_i64()
        .or_else(|| v.as_u64().and_then(|v| i64::try_from(v).ok()))
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        .ok_or_else(|| err("expected an integer"))
}
pub fn pattern(v: &Value) -> Result<Vec<i64>> {
    if let Some(s) = v.as_str() {
        s.chars()
            .map(|c| {
                u8::try_from(c as u32)
                    .map(i64::from)
                    .map_err(|_| err("invalid pattern byte"))
            })
            .collect()
    } else {
        array(v)
            .iter()
            .map(|v| {
                let n = as_int(v)?;
                if (0..=255).contains(&n) {
                    Ok(n)
                } else {
                    Err(err("invalid pattern byte"))
                }
            })
            .collect()
    }
}
pub fn split_by_part(stmts: Vec<(Value, BTreeSet<usize>)>) -> Option<Groups> {
    let mut groups: Groups = vec![];
    let mut pending = vec![];
    for (s, ps) in stmts {
        if ps.len() > 1 {
            return None;
        }
        if ps.is_empty() {
            if let Some((_, g)) = groups.last_mut() {
                g.push(s);
            } else {
                pending.push(s);
            }
            continue;
        }
        let part = *ps.iter().next()?;
        if let Some((p, g)) = groups.last_mut() {
            if *p == part {
                g.push(s);
                continue;
            }
        }
        let mut g = std::mem::take(&mut pending);
        g.push(s);
        groups.push((part, g));
    }
    if pending.is_empty() {
        Some(groups)
    } else {
        None
    }
}
pub fn part_read(s: &Value, rec_loc: &str) -> Option<usize> {
    if ty(s) != "AstStatLocal" || array(&s["vars"]).len() != 1 || array(&s["values"]).len() != 1 {
        return None;
    }
    let v = unparen(&s["values"][0]);
    if ty(v) != "AstExprIndexExpr" || loc(&v["expr"]).as_deref() != Some(rec_loc) {
        return None;
    }
    let i = cn(&v["index"])?;
    if i < 11.0 {
        return None;
    }
    Some(i as usize - 11)
}
pub fn register_read(
    s: &Value,
    rec_loc: &str,
    var_part: &mut BTreeMap<String, usize>,
    part_of: &mut BTreeMap<String, usize>,
    reads: &mut BTreeMap<usize, Vec<Value>>,
) -> bool {
    if let Some(p) = part_read(s, rec_loc) {
        var_part.insert(location(&s["vars"][0]), p);
        reads.entry(p).or_default().push(s.clone());
        return true;
    }
    if ty(s) == "AstStatLocal" && array(&s["values"]).len() == 1 {
        let v = unparen(&s["values"][0]);
        if ty(v) == "AstExprCall" && array(&v["args"]).len() == 1 {
            if let Some(p) = loc(&v["args"][0]).and_then(|l| var_part.get(&l).copied()) {
                for v in array(&s["vars"]) {
                    part_of.insert(location(v), p);
                }
                reads.entry(p).or_default().push(s.clone());
                return true;
            }
        }
    }
    false
}
pub fn hoist_loop_reads(
    stmts: &[Value],
    rec_loc: &str,
    var_part: &mut BTreeMap<String, usize>,
    part_of: &mut BTreeMap<String, usize>,
    reads: &mut BTreeMap<usize, Vec<Value>>,
) -> Vec<Value> {
    let mut out = vec![];
    for s in stmts {
        if ty(s) == "AstStatBlock" {
            let inner = array(&s["body"]);
            let mut k = 0;
            while k < inner.len() && register_read(&inner[k], rec_loc, var_part, part_of, reads) {
                k += 1;
            }
            if k > 0 {
                out.extend_from_slice(&inner[k..]);
                continue;
            }
        } else if register_read(s, rec_loc, var_part, part_of, reads) {
            continue;
        }
        out.push(s.clone());
    }
    out
}

#[derive(Clone)]
pub enum Runner {
    Simple(i64),
    Ctrl(i64),
    Block(Value),
    Whole(i64, Value),
    Micro(i64, Value, Vec<Value>),
    Group(i64, Value, Vec<Value>, String),
}
pub struct Classifier {
    pub loader: Value,
    pub interp: Interp,
    pub domains: Domains,
    pub trials: usize,
    pub deadline: Option<Instant>,
    pub profile_tables: Vec<String>,
    pub mapping: Value,
    loc: BTreeMap<String, String>,
    nil_locs: Vec<String>,
    env_table: V,
}
impl Classifier {
    pub fn new(loader: &Value, domains: Domains, deadline: Option<Instant>) -> Self {
        let mut loc = BTreeMap::new();
        if let Some(roles) = loader["role_local"].as_object() {
            for (r, v) in roles {
                loc.insert(r.clone(), location(v));
            }
        }
        let mut nil_locs = vec![];
        for s in array(&loader["frame_body"]) {
            if ty(s) == "AstStatLocal" && array(&s["values"]).is_empty() {
                nil_locs.extend(array(&s["vars"]).iter().map(location));
            }
        }
        let mut profile_tables = vec![];
        if let Some(profile) = loc.get("PROF") {
            for statement in array(&loader["frame_body"]) {
                if ty(statement) != "AstStatLocal"
                    || array(&statement["vars"]).len() != 1
                    || array(&statement["values"]).len() != 1
                {
                    continue;
                }
                let value = unparen(&statement["values"][0]);
                if ty(value) != "AstExprTable" || array(&value["items"]).len() != 3 {
                    continue;
                }
                let mut valid = true;
                for (index, item) in array(&value["items"]).iter().enumerate() {
                    if !item["key"].is_null() {
                        valid = false;
                        break;
                    }
                    let mut e = unparen(&item["value"]);
                    if ty(e) != "AstExprBinary" || e["op"] != "Mod" || cn(&e["right"]) != Some(4.0)
                    {
                        valid = false;
                        break;
                    }
                    e = unparen(&e["left"]);
                    if index > 0 {
                        if ty(e) != "AstExprCall" || array(&e["args"]).len() != 1 {
                            valid = false;
                            break;
                        }
                        let f = unparen(&e["func"]);
                        let div = unparen(&e["args"][0]);
                        if ty(f) != "AstExprIndexName"
                            || f["index"] != "floor"
                            || unparen(&f["expr"])["global"] != "math"
                            || ty(div) != "AstExprBinary"
                            || div["op"] != "Div"
                            || cn(&div["right"]) != Some(4f64.powi(index as i32))
                        {
                            valid = false;
                            break;
                        }
                        e = unparen(&div["left"]);
                    }
                    if loc_of(e).as_ref() != Some(profile) {
                        valid = false;
                        break;
                    }
                }
                if valid {
                    profile_tables.push(location(&statement["vars"][0]));
                }
            }
        }
        let mut interp = Interp::new(60000);
        interp.deadline = deadline;
        Self {
            loader: loader.clone(),
            interp,
            domains,
            trials: 24,
            deadline,
            profile_tables,
            mapping: json!({"simple":{},"hot":{},"ctrl":{},"ded":{},"junk":loader["junk_op"],"unknown":[]}),
            loc,
            nil_locs,
            env_table: V::table(),
        }
    }
    fn check_time(&self) -> Result<()> {
        if self.deadline.is_some_and(|d| Instant::now() > d) {
            Err(Error::new("timeout", "classification took too long"))
        } else {
            Ok(())
        }
    }
    fn values(&self, st: &State, op: Option<i64>) -> BTreeMap<String, V> {
        let mut vals = BTreeMap::new();
        for (k, v) in [
            ("REGS", st.r.clone()),
            ("UP", st.up.clone()),
            ("ENV", self.env_table.clone()),
            ("NXT", st.nxt.clone()),
            ("ALT", st.alt.clone()),
            ("TOP", st.top.clone()),
            ("REC", st.rec.clone()),
            ("CLR", V::Bool(st.clr)),
            ("PROF", st.prof.clone()),
            ("VARARGS", st.varargs.clone()),
            ("NVAR", V::Num(st.nvar as f64)),
            ("PC", V::Num(1.0)),
        ] {
            vals.insert(k.into(), v);
        }
        for (k, v) in &st.f {
            vals.insert(k.to_uppercase(), V::Num(*v));
        }
        let op = V::Num(op.map(|v| v as f64).unwrap_or(st.field("op")));
        vals.insert("OP".into(), op);
        let tmpl = st.tmpl.clone();
        let rf = st.field("RF");
        let template_op = matches!(st.name.as_str(), "DUPTABLE" | "NEWTABLE");
        vals.insert(
            "KGET".into(),
            V::host_with_roots("kget", tmpl.iter().cloned().collect(), move |_, a| {
                let i = a.first().cloned().unwrap_or(V::Nil).number()?;
                Ok(vec![if template_op && i == rf {
                    tmpl.clone()
                        .unwrap_or_else(|| refspec::opaque("K", &[i as i64]))
                } else {
                    refspec::opaque("K", &[i as i64])
                }])
            }),
        );
        vals.insert(
            "IMP".into(),
            V::host("imp", |_, a| {
                Ok(vec![refspec::opaque(
                    "imp",
                    &[a.first().cloned().unwrap_or(V::Nil).integer()?],
                )])
            }),
        );
        vals.insert(
            "FUSED".into(),
            V::host("fused", |_, a| {
                let p = a.first().cloned().unwrap_or(V::Nil);
                (3..9).map(|k| p.get(&V::Num(k as f64))).collect()
            }),
        );
        vals
    }
    pub fn seed(&mut self, st: &State) -> VmResult<Env> {
        self.interp.unbound = Some(st.tag.clone());
        let mut env = Env::default();
        for l in &self.nil_locs {
            env.declare(l.clone(), V::Nil);
        }
        for (role, value) in self.values(st, None) {
            if let Some(l) = self.loc.get(&role) {
                env.declare(l.clone(), value);
            }
        }
        for l in &self.profile_tables {
            env.declare(l.clone(), st.prof.clone());
        }
        if let Some(roles) = self.loader["roles"].as_object() {
            for (k, role) in roles {
                if role == "MKFN" {
                    env.declare(k.clone(), V::host("mkfn", |_, _| Ok(vec![])));
                }
            }
        }
        for item in array(&self.loader["simple_table_items"]) {
            let local = &item[0];
            let items = array(&item[1]);
            if items.is_empty() {
                continue;
            }
            let tab = V::table();
            for item in items {
                let expr = unparen(&item[1]);
                let value = match ty(expr) {
                    "AstExprConstantNumber" | "AstExprConstantBool" | "AstExprConstantString" => {
                        self.interp.eval(expr, &mut env)?
                    }
                    "AstExprFunction" => V::function(expr.clone(), env.cells.clone()),
                    "AstExprLocal" => {
                        let f = &self.loader["funcs"][location(&expr["local"])];
                        if f.is_object() {
                            V::function(f.clone(), env.cells.clone())
                        } else {
                            V::Nil
                        }
                    }
                    _ => V::Nil,
                };
                if !value.is_nil() {
                    let key = match &item[0] {
                        Value::String(s) => V::bytes(
                            &crate::astutil::unlatin1(s)
                                .map_err(|e| crate::luavm::unsupported(e.message))?,
                        ),
                        v => V::Num(v.as_f64().unwrap_or(0.0)),
                    };
                    tab.set(key, value)?;
                }
            }
            env.declare(location(local), tab);
        }
        Ok(env)
    }
    fn args_for(&self, roles: &Value, st: &State, op: Option<i64>) -> Vec<V> {
        let vals = self.values(st, op);
        array(roles)
            .iter()
            .map(|r| {
                vals.get(r.as_str().unwrap_or(""))
                    .cloned()
                    .unwrap_or(V::Nil)
            })
            .collect()
    }
    pub fn run(&mut self, runner: &Runner, st: &mut State) -> VmResult<()> {
        let mut env = self.seed(st)?;
        let mut ret = None;
        match runner {
            Runner::Simple(op) => {
                let roles = &self.loader["call_roles"]["SIMPLE"];
                let args = self.args_for(roles, st, Some(*op));
                let f = V::function(self.loader["simple_fn"].clone(), env.cells);
                self.interp.call(f, args)?;
            }
            Runner::Ctrl(op) => {
                let args = self.args_for(&self.loader["call_roles"]["CTRL"], st, Some(*op));
                let f = V::function(self.loader["ctrl_fn"].clone(), env.cells);
                let res = self.interp.call(f, args)?;
                if let Some(v) = res.first() {
                    st.nxt = v.clone();
                }
                if let Some(v) = res.get(1) {
                    st.top = v.clone();
                }
                if let Some(v) = res.get(2).filter(|v| !v.is_nil()) {
                    if matches!(v, V::Table(_)) {
                        let count = v.get(&V::bytes(b"n"))?;
                        let count = if count.truthy() { count.integer()? } else { 0 };
                        st.ret = Some(
                            (1..=count)
                                .map(|i| v.get(&V::Num(i as f64)))
                                .collect::<VmResult<_>>()?,
                        );
                    } else {
                        st.ret = Some(vec![v.clone()]);
                    }
                }
            }
            Runner::Block(block) => {
                match self.interp.exec_block(block, &mut env) {
                    Err(VmError::Return(v)) => ret = Some(v),
                    Ok(()) => {}
                    Err(e) => return Err(e),
                }
                if let Some(l) = self.loc.get("NXT") {
                    st.nxt = env.get(l).unwrap_or(V::Nil);
                }
                if let Some(l) = self.loc.get("TOP") {
                    st.top = env.get(l).unwrap_or(V::Nil);
                }
                if let Some(v) = ret {
                    st.ret = Some(v);
                }
            }
            Runner::Whole(ti, f) => {
                let args = self.args_for(&self.loader["call_roles"][format!("D{ti}")], st, None);
                let res = self.interp.call(V::function(f.clone(), env.cells), args)?;
                if *ti == 1 {
                    st.ret = Some(res);
                }
            }
            Runner::Micro(ti, f, stmts) | Runner::Group(ti, f, stmts, _) => {
                let roles = self.loader["call_roles"]
                    [format!("D{}", if *ti == 6 { 5 } else { *ti })]
                .clone();
                let args = self.args_for(&roles, st, None);
                env = env.fork();
                for (i, p) in array(&f["args"]).iter().enumerate() {
                    env.declare(location(p), args.get(i).cloned().unwrap_or(V::Nil));
                }
                for s in stmts {
                    match self.interp.exec_stat(s, &mut env) {
                        Err(VmError::Return(v)) => {
                            ret = Some(v);
                            break;
                        }
                        Ok(()) => {}
                        Err(e) => return Err(e),
                    }
                }
                for (r, attr) in [("TOP", 0), ("NXT", 1)] {
                    if let Some(i) = array(&roles).iter().position(|v| v == r) {
                        if let Some(p) = array(&f["args"]).get(i) {
                            let v = env.get(&location(p)).unwrap_or(V::Nil);
                            if attr == 0 {
                                st.top = v;
                            } else {
                                st.nxt = v;
                            }
                        }
                    }
                }
                if let Runner::Group(_, _, _, mode) = runner {
                    if mode == "jump" || mode == "loop" && ret.is_none() {
                        st.nxt = st.alt.clone();
                    }
                } else if let Some(ret) = ret {
                    if *ti == 4 {
                        if ret.len() < 2 {
                            return Err(crate::luavm::unsupported(
                                "dedicated handler returns too few values",
                            ));
                        }
                        st.nxt = ret[0].clone();
                        st.top = ret[1].clone();
                    } else if matches!(ti, 2 | 3) {
                        st.top = ret.first().cloned().unwrap_or(V::Nil);
                    }
                }
            }
        }
        st.log = self.interp.log.clone();
        Ok(())
    }
    pub fn matches(
        &mut self,
        runner: &Runner,
        name: &str,
        domain: Option<&[Fields]>,
        variant: Option<&Fields>,
        strict: bool,
    ) -> Result<bool> {
        self.check_time()?;
        for t in 0..self.trials {
            self.check_time()?;
            let _heap = TrialHeap::new(vec![self.interp.globals.clone(), self.env_table.clone()]);
            let seed = 7919 * t as u64 + 13;
            let mut reference = State::new(name, seed, domain, variant, t).map_err(vmerr)?;
            let mut got = State::new(name, seed, domain, variant, t).map_err(vmerr)?;
            let ref_result = reference.run().and_then(|_| {
                if !strict && refspec::has_normalize(name) {
                    reference.normalize()
                } else {
                    Ok(())
                }
            });
            let a = match ref_result {
                Ok(()) => Some(reference.snapshot()),
                Err(VmError::Lua(_)) => None,
                Err(_) => return Ok(false),
            };
            self.interp.steps = 0;
            self.interp.log.clear();
            let got_result = self.run(runner, &mut got).and_then(|_| {
                if !strict && refspec::has_normalize(name) {
                    got.normalize()
                } else {
                    Ok(())
                }
            });
            let b = match got_result {
                Ok(()) => Some(got.snapshot()),
                Err(VmError::Lua(_)) => None,
                Err(_) => {
                    self.check_time()?;
                    return Ok(false);
                }
            };
            if a != b {
                return Ok(false);
            }
        }
        Ok(true)
    }
    fn find(
        &mut self,
        runner: &Runner,
        key: Option<&str>,
        skip_return: bool,
        hf: Option<i64>,
    ) -> Result<Vec<String>> {
        let dom = key.and_then(|k| self.domains.get(k)).cloned();
        let variant = hf.map(|v| BTreeMap::from([("HF".into(), v as f64)]));
        let mut passes = vec![(false, false), (false, true)];
        if dom.as_ref().is_some_and(|v| !v.is_empty()) {
            passes.extend([(true, false), (true, true)]);
        }
        for (restricted, lenient) in passes {
            let mut out = vec![];
            for name in refspec::specs() {
                if skip_return && matches!(name.as_str(), "RETURN" | "TAILCALL")
                    || lenient && !refspec::has_normalize(&name)
                {
                    continue;
                }
                let dd = if restricted {
                    let fields = refspec::domain_fields(&name);
                    if fields.is_empty() {
                        continue;
                    }
                    let dd: Vec<Fields> = dom
                        .as_ref()
                        .unwrap()
                        .iter()
                        .map(|d| {
                            d.iter()
                                .filter(|(k, _)| fields.contains(&k.as_str()))
                                .map(|(k, v)| (k.clone(), *v))
                                .collect()
                        })
                        .filter(|d: &Fields| !d.is_empty())
                        .collect();
                    if dd.is_empty() {
                        continue;
                    }
                    Some(dd)
                } else {
                    None
                };
                if self.matches(runner, &name, dd.as_deref(), variant.as_ref(), !lenient)? {
                    out.push(name);
                }
            }
            if !out.is_empty() {
                return Ok(out);
            }
        }
        Ok(vec![])
    }
    fn unknown(&mut self, where_: &str, key: i64, why: impl Into<String>) {
        self.mapping["unknown"]
            .as_array_mut()
            .unwrap()
            .push(json!([where_, key, why.into()]));
    }
    fn pick(&mut self, names: Vec<String>, where_: &str, key: i64) -> Option<String> {
        if names.is_empty() {
            self.unknown(where_, key, "no reference operation matches");
            return None;
        }
        let mut uniq: BTreeSet<String> = names.iter().map(|n| strip_fixed(n).0).collect();
        if uniq.len() > 1 {
            uniq.remove("NOP");
            if uniq.len() != 1 {
                self.unknown(where_, key, format!("ambiguous: {}", names.join(",")));
                return None;
            }
        }
        let name = uniq.into_iter().next()?;
        names.into_iter().filter(|n| strip_fixed(n).0 == name).min()
    }
    pub fn structural(&self, body: &Value) -> Option<String> {
        let stmts = array(&body["body"]);
        if stmts.len() == 1 && ty(&stmts[0]) == "AstStatExpr" {
            let e = unparen(&stmts[0]["expr"]);
            if ty(e) == "AstExprCall" && unparen(&e["func"])["global"] == "error" {
                return Some("CAPTURE".into());
            }
        }
        let (mut nup, mut children, mut kget, mut forin) = (false, false, false, false);
        let mut nodes = vec![];
        walk(body, &mut nodes);
        for node in nodes {
            if ty(node) == "AstExprIndexName" {
                nup |= node["index"] == "numUpvalues";
                children |= node["index"] == "children";
            }
            if self.loader["layout"] == "numeric" && ty(node) == "AstExprIndexExpr" {
                nup |= cn(&node["index"]) == self.loader["numeric_fields"]["num_upvalues"].as_f64();
                let base = unparen(&node["expr"]);
                if ty(base) == "AstExprIndexExpr"
                    && loc(&base["expr"]) == self.loc.get("PROTO").cloned()
                {
                    children |=
                        cn(&base["index"]) == self.loader["numeric_fields"]["children"].as_f64();
                }
            }
            if ty(node) == "AstExprCall" {
                kget |= self.loc.contains_key("KGET")
                    && loc(&node["func"]) == self.loc.get("KGET").cloned();
            }
            forin |= ty(node) == "AstStatForIn";
        }
        if nup && children {
            Some("NEWCLOSURE".into())
        } else if nup && kget {
            Some("DUPCLOSURE".into())
        } else if forin && !nup {
            Some("CLOSEUPVALS".into())
        } else {
            None
        }
    }
    fn classify_simple(&mut self, ops: &[i64]) -> Result<()> {
        if !ops.is_empty()
            && (ty(&self.loader["simple_fn"]) != "AstExprFunction"
                || array(&self.loader["call_roles"]["SIMPLE"]).len() != 12)
        {
            return Err(err("simple handler function or argument roles are missing"));
        }
        let mut ops = ops.to_vec();
        ops.sort();
        ops.dedup();
        for op in ops {
            let key = format!("op:{op}");
            let runner = Runner::Simple(op);
            let names = self.find(&runner, Some(&key), true, None)?;
            if !names.is_empty() {
                if let Some(n) = self.pick(names, "simple", op) {
                    self.mapping["simple"][op.to_string()] = json!(n);
                }
                continue;
            }
            let mut hf = Map::new();
            for h in 0..64 {
                let names = self.find(&runner, Some(&key), true, Some(h))?;
                if !names.is_empty() {
                    if let Some(n) = self.pick(names, &format!("simple/hf{h}"), op) {
                        hf.insert(h.to_string(), json!(n));
                    }
                }
            }
            if hf.is_empty() {
                self.unknown("simple", op, "handler could not be classified");
            } else {
                self.mapping["simple"][op.to_string()] = json!(["HF", hf]);
            }
        }
        Ok(())
    }
    pub fn classify_all(&mut self, ops: &[i64]) -> Result<Value> {
        self.classify_simple(ops)?;
        for family in ["hot", "ctrl"] {
            let chain = self.loader[format!("{family}_chain")].clone();
            for pair in array(&chain) {
                let op = as_int(&pair[0])?;
                let block = &pair[1];
                if family == "ctrl" {
                    if let Some(name) = self.structural(block) {
                        self.mapping[family][op.to_string()] = json!(name);
                        continue;
                    }
                }
                let runner = if family == "hot" {
                    Runner::Block(block.clone())
                } else {
                    Runner::Ctrl(op)
                };
                let names =
                    self.find(&runner, Some(&format!("op:{op}")), family == "ctrl", None)?;
                if let Some(name) = self.pick(names, family, op) {
                    self.mapping[family][op.to_string()] = json!(name);
                }
            }
        }
        for ti in 1..7 {
            let pairs = indexed(&self.loader["ded_decl"], ti)
                .cloned()
                .unwrap_or(json!([]));
            for pair in array(&pairs) {
                self.classify_ded(ti, as_int(&pair[0])?, &pair[1])?;
            }
        }
        Ok(self.mapping.clone())
    }
    fn classify_ded(&mut self, ti: i64, key: i64, f: &Value) -> Result<()> {
        let where_ = format!("ded{ti}");
        let body = array(&f["body"]["body"]);
        if ti == 1 {
            let names = self.find(&Runner::Whole(1, f.clone()), None, false, None)?;
            if let Some(name) = self.pick(names, &where_, key) {
                self.mapping["ded"][format!("{ti}:{key}")] = json!([[name, 0, {}, false]]);
            }
            return Ok(());
        }
        if body.iter().any(|s| ty(s) == "AstStatWhile") {
            return self.classify_loop_ded(ti, key, f);
        }
        let blocks: Vec<_> = body
            .iter()
            .filter(|s| ty(s) == "AstStatBlock")
            .cloned()
            .collect();
        let tail: Vec<_> = body
            .iter()
            .filter(|s| ty(s) != "AstStatBlock")
            .cloned()
            .collect();
        if blocks.is_empty() {
            self.unknown(&where_, key, "no micro-operations");
            return Ok(());
        }
        if tail.iter().any(|s| ty(s) != "AstStatReturn") {
            self.unknown(&where_, key, "unexpected statements in handler body");
            return Ok(());
        }
        let rec_loc = location(&f["args"][0]);
        let mut micro = vec![];
        for (k, block) in blocks.iter().enumerate() {
            let mut nodes = vec![];
            walk(block, &mut nodes);
            let part = nodes
                .into_iter()
                .filter(|n| {
                    ty(n) == "AstExprIndexExpr" && loc(&n["expr"]).as_deref() == Some(&rec_loc)
                })
                .filter_map(|n| cn(&n["index"]))
                .filter(|v| *v >= 11.0)
                .map(|v| v as usize - 11)
                .next_back();
            let Some(part) = part else {
                self.unknown(
                    &where_,
                    key,
                    format!("micro-operation {k} reads no record part"),
                );
                return Ok(());
            };
            let mut stmts = vec![block.clone()];
            if k + 1 == blocks.len() {
                stmts.extend(tail.clone());
            }
            let runner = Runner::Micro(ti, f.clone(), stmts);
            let names = self.find(&runner, Some(&format!("ded:{ti}:{key}:{part}")), true, None)?;
            let Some(name) = self.pick(names, &format!("{where_}/{key}.{k}"), key) else {
                return Ok(());
            };
            let (ir, fixed) = strip_fixed(&name);
            micro.push(json!([ir, part, fixed, refspec::is_ctrl(&name)]));
        }
        self.mapping["ded"][format!("{ti}:{key}")] = json!(micro);
        Ok(())
    }
    fn classify_loop_ded(&mut self, ti: i64, key: i64, f: &Value) -> Result<()> {
        let where_ = format!("ded{ti}");
        let body = array(&f["body"]["body"]);
        let Some(w) = body.last() else {
            return Ok(());
        };
        let cond = unparen(&w["condition"]);
        if ty(w) != "AstStatWhile"
            || body.iter().filter(|s| ty(s) == "AstStatWhile").count() != 1
            || ty(cond) != "AstExprConstantBool"
            || cond["value"] != true
        {
            self.unknown(&where_, key, "loop handler shape not recognised");
            return Ok(());
        }
        let rec_loc = location(&f["args"][0]);
        let (mut var_part, mut part_of, mut reads) =
            (BTreeMap::new(), BTreeMap::new(), BTreeMap::new());
        let mut others = vec![];
        for s in &body[..body.len() - 1] {
            if !register_read(s, &rec_loc, &mut var_part, &mut part_of, &mut reads) {
                others.push(s.clone());
            }
        }
        let loop_body = hoist_loop_reads(
            array(&w["body"]["body"]),
            &rec_loc,
            &mut var_part,
            &mut part_of,
            &mut reads,
        );
        let parts_in = |s: Value| {
            let mut nodes = vec![];
            walk(&s, &mut nodes);
            let ps = nodes
                .iter()
                .filter_map(|n| loc(n).and_then(|l| part_of.get(&l).copied()))
                .collect();
            (s, ps)
        };
        let pre_groups = split_by_part(others.into_iter().map(&parts_in).collect());
        let loop_groups = split_by_part(loop_body.into_iter().map(parts_in).collect());
        let (Some(pre), Some(groups)) = (pre_groups, loop_groups) else {
            self.unknown(&where_, key, "loop handler: micro-operations not separable");
            return Ok(());
        };
        if groups.is_empty() {
            self.unknown(&where_, key, "loop handler: micro-operations not separable");
            return Ok(());
        }
        let used: Vec<_> = pre.iter().chain(&groups).map(|g| g.0).collect();
        if used.iter().copied().collect::<BTreeSet<_>>().len() != used.len()
            || used.iter().any(|p| !reads.contains_key(p))
        {
            self.unknown(&where_, key, "loop handler: a record part is used twice");
            return Ok(());
        }
        let ctl: Vec<_> = groups
            .iter()
            .enumerate()
            .filter_map(|(i, (_, ss))| {
                let mut ns = vec![];
                for s in ss {
                    walk(s, &mut ns);
                }
                if ns.iter().any(|n| ty(n) == "AstStatReturn") {
                    Some(i)
                } else {
                    None
                }
            })
            .collect();
        if ctl.len() != 1 {
            self.unknown(&where_, key, "loop handler: loop control not found");
            return Ok(());
        }
        let ci = ctl[0];
        let shape = if ci == 0 && groups.len() > 1 {
            "head"
        } else if ci + 1 == groups.len() {
            "tail"
        } else {
            self.unknown(
                &where_,
                key,
                "loop handler: loop control in the middle of the body",
            );
            return Ok(());
        };
        let mut res = json!({"pre":[],"head":[],"body":[],"tail":[]});
        for (is_pre, gi, (part, ss)) in pre
            .iter()
            .enumerate()
            .map(|(i, g)| (true, i, g))
            .chain(groups.iter().enumerate().map(|(i, g)| (false, i, g)))
        {
            let modes: Vec<&str> = if is_pre {
                vec!["jump", "plain"]
            } else if gi == ci {
                vec!["loop"]
            } else {
                vec!["plain"]
            };
            let mut names = vec![];
            for mode in modes {
                let mut stmts = reads[part].clone();
                stmts.extend(ss.clone());
                names = self.find(
                    &Runner::Group(ti, f.clone(), stmts, mode.into()),
                    Some(&format!("ded:{ti}:{key}:{part}")),
                    true,
                    None,
                )?;
                if !names.is_empty() {
                    break;
                }
            }
            let Some(name) = self.pick(
                names,
                &format!(
                    "{where_}/{key}.{}{}",
                    if is_pre { "pre" } else { "loop" },
                    gi
                ),
                key,
            ) else {
                return Ok(());
            };
            let (ir, fixed) = strip_fixed(&name);
            let section = if is_pre {
                "pre"
            } else if gi == ci {
                shape
            } else {
                "body"
            };
            res[section].as_array_mut().unwrap().push(json!([
                ir,
                part,
                fixed,
                refspec::is_ctrl(&name)
            ]));
        }
        let expected = if shape == "head" {
            "FORGLOOP"
        } else {
            "FORNLOOP"
        };
        if res[shape][0][0] != expected {
            self.unknown(
                &where_,
                key,
                format!("loop handler: unexpected loop control {}", res[shape][0][0]),
            );
            return Ok(());
        }
        let prep = array(&res["pre"]);
        let check = if shape == "tail" {
            prep.len()
        } else {
            prep.len().saturating_sub(1)
        };
        if prep[..check].iter().any(|m| m[3] == true) {
            self.unknown(
                &where_,
                key,
                "loop handler: unexpected jump before the loop",
            );
            return Ok(());
        }
        self.mapping["ded"][format!("{ti}:{key}")] = json!({"loop":res});
        Ok(())
    }
}
fn loc_of(v: &Value) -> Option<String> {
    loc(v)
}
pub fn strip_fixed(name: &str) -> (String, Value) {
    if let Some((b, c)) = refspec::fixed_call(name) {
        ("CALL".into(), json!({"B":b,"C":c}))
    } else {
        (name.into(), json!({}))
    }
}
pub fn classify(loader: &Value, dump: &Value, timeout: Duration) -> Result<Value> {
    let domains = collect_domains(loader, dump)?;
    let mut ops = vec![];
    if let Some(a) = dump["simple"].as_array() {
        for op in a {
            ops.push(as_int(op)?);
        }
    } else if let Some(m) = dump["simple"].as_object() {
        for op in m.keys() {
            ops.push(op.parse().map_err(|_| err("invalid simple opcode"))?);
        }
    }
    let deadline = Instant::now()
        .checked_add(timeout)
        .ok_or_else(|| Error::new("timeout", "invalid classification timeout"))?;
    Classifier::new(loader, domains, Some(deadline)).classify_all(&ops)
}

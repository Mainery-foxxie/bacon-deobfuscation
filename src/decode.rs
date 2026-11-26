//! Decode VM records into the original operand schema and expand virtual records.
use crate::{
    classify::{as_int, indexed, pattern, permute},
    error::{Error, Result},
    luavm::array,
};
use serde_json::{json, Map, Value};
pub const NONE: u64 = 4294967295;
fn err(s: impl Into<String>) -> Error {
    Error::new("unsupported", s)
}
fn int(v: &Value) -> Result<i64> {
    as_int(v)
}
fn operand_fields(name: &str) -> Option<&'static [&'static str]> {
    Some(match name {
        "LOADK" | "LOADN" | "LOADB" | "LOADNIL" | "GETIMPORT" | "GETUPVAL" | "SETUPVAL"
        | "NEWTABLE" | "DUPTABLE" | "NEWCLOSURE" | "DUPCLOSURE" | "CALL" | "RETURN"
        | "JUMPIFNOT" | "JUMPIF" | "JUMPXEQK" | "JUMPXEQKB" | "JUMPXEQKNIL" | "FORNPREP"
        | "FORNLOOP" | "FORGPREP" | "FORGLOOP" | "CLOSEUPVALS" | "GETVARARGS" => &["A"],
        "MOVE" | "GETTABLEKS" | "SETTABLEKS" | "SELF" | "LENGTH" | "NOT" | "MINUS"
        | "GETTABLEN" | "SETTABLEN" | "ORK" | "ANDK" | "SETLIST" | "ADDK" | "SUBK" | "MULK"
        | "DIVK" | "MODK" | "POWK" | "IDIVK" => &["A", "B"],
        "ADD" | "SUB" | "MUL" | "DIV" | "MOD" | "POW" | "IDIV" | "AND" | "OR" | "GETTABLE"
        | "SETTABLE" | "CONCAT" => &["A", "B", "C"],
        "SUBRK" | "DIVRK" => &["A", "C"],
        "JUMPIFEQ" | "JUMPIFNOTEQ" | "JUMPIFLE" | "JUMPIFLT" | "JUMPIFNOTLT" | "JUMPIFNOTLE" => {
            &["A", "aux"]
        }
        "JUMP" | "NOP" => &[],
        _ => return None,
    })
}
pub fn unshift_args(name: &str, a: &Value, rs: i64) -> Result<Value> {
    let fields = if name == "CAPTURE" {
        if matches!(int(&a["A"])?, 0 | 1) {
            &["B"][..]
        } else {
            &[][..]
        }
    } else {
        operand_fields(name).ok_or_else(|| {
            err(format!(
                "instruction {name} has no register layout (needed for the register offset)"
            ))
        })?
    };
    let mut b = a.clone();
    for field in fields {
        b[*field] = json!(int(&a[*field])?
            .checked_sub(rs)
            .ok_or_else(|| err("register offset overflow"))?);
    }
    Ok(b)
}
fn args(v: &Value) -> Result<Value> {
    let mut m = Map::new();
    for field in ["A", "B", "C", "rf", "Hf", "aux"] {
        let val = v
            .get(field)
            .ok_or_else(|| err(format!("missing operand {field}")))?;
        int(val)?;
        m.insert(field.into(), val.clone());
    }
    Ok(Value::Object(m))
}
fn fixed_args(f: &Value, fixed: &Value) -> Result<Value> {
    let mut a = json!({"A":f["A"],"B":0,"C":0,"rf":0,"Hf":0,"aux":0});
    let m = fixed
        .as_object()
        .ok_or_else(|| err("invalid fixed operands"))?;
    for (k, v) in m {
        a[k] = v.clone();
    }
    Ok(a)
}
fn instruction(name: &str, f: &Value) -> Result<Value> {
    Ok(json!([name, args(f)?]))
}
fn node(
    kind: &str,
    sem: &Value,
    ins: Vec<Value>,
    ctrl: Value,
    nxt: Value,
    alt: Value,
    rid: u64,
) -> Value {
    json!({"kind":kind,"sem":sem,"ins":ins,"ctrl":ctrl,"nxt":nxt,"alt":alt,"rid":rid})
}
pub struct Decoder<'a> {
    dump: &'a Value,
    mapping: &'a Value,
    loader: &'a Value,
}
impl<'a> Decoder<'a> {
    pub fn new(dump: &'a Value, mapping: &'a Value, loader: &'a Value) -> Self {
        Self {
            dump,
            mapping,
            loader,
        }
    }
    fn logical(&self, df: i64, raw: &Value) -> Result<Value> {
        let v = permute(&self.loader["perm"], df, raw, 10)?;
        Ok(
            json!({"op":v[0],"ef":v[1],"A":v[2],"B":v[3],"C":v[4],"rf":v[5],"Hf":v[6],"aux":v[7],"nxt":v[8],"alt":v[9]}),
        )
    }
    fn fused(&self, df: i64, raw: &Value) -> Result<Value> {
        let v = permute(&self.loader["fused_perm"], df, raw, 6)?;
        Ok(json!({"A":v[0],"B":v[1],"C":v[2],"rf":v[3],"Hf":v[4],"aux":v[5]}))
    }
    fn capture(&self, df: i64, raw: &Value) -> Result<Value> {
        let v = permute(&self.loader["cap_perm"], df, raw, 4)?;
        Ok(json!({"A":v[1],"B":v[2],"C":0,"rf":0,"Hf":0,"aux":0}))
    }
    fn capperm(&self) -> bool {
        !self.loader["cap_perm"].is_null()
    }
    fn simple_op(&self, op: i64, f: &Value, where_: &str) -> Result<Value> {
        let mut m = indexed(&self.mapping["simple"], op)
            .ok_or_else(|| err(format!("{where_}: simple operation {op} is not understood")))?;
        if m.is_array() {
            let hf = int(&f["Hf"])?;
            m = indexed(&m[1], hf).ok_or_else(|| {
                err(format!(
                    "{where_}: operation {op} with selector {hf} is not understood"
                ))
            })?;
        }
        let name = m
            .as_str()
            .ok_or_else(|| err("simple handler name is not a string"))?;
        instruction(name, f)
    }
    fn plain(
        &self,
        name: &str,
        f: &Value,
        sem: &Value,
        nxt: Value,
        alt: Value,
        rid: u64,
    ) -> Result<Value> {
        let ctrl = matches!(
            name,
            "RETURN"
                | "FORNLOOP"
                | "FORGLOOP"
                | "JUMPIFNOT"
                | "JUMPIF"
                | "JUMPIFEQ"
                | "JUMPIFNOTEQ"
                | "JUMPIFLE"
                | "JUMPIFLT"
                | "JUMPIFNOTLT"
                | "JUMPIFNOTLE"
                | "JUMPXEQKB"
                | "JUMPXEQK"
                | "JUMPXEQKNIL"
                | "FORNPREP"
                | "JUMP"
        );
        let out = if ctrl {
            node(name, sem, vec![], instruction(name, f)?, nxt, alt, rid)
        } else if name == "FORGPREP" {
            node(
                name,
                sem,
                vec![instruction(name, f)?],
                instruction("JUMP", f)?,
                nxt,
                alt,
                rid,
            )
        } else if name == "LOADB" {
            node(
                name,
                sem,
                vec![instruction(name, f)?],
                if int(&f["C"])? != 0 {
                    instruction("JUMP", f)?
                } else {
                    Value::Null
                },
                nxt,
                alt,
                rid,
            )
        } else if name == "NOP" {
            node(name, sem, vec![], Value::Null, nxt, alt, rid)
        } else {
            let kind = if matches!(
                name,
                "CALL" | "SETLIST" | "NEWCLOSURE" | "DUPCLOSURE" | "CLOSEUPVALS" | "CAPTURE"
            ) {
                name
            } else {
                "SIMPLE"
            };
            node(
                kind,
                sem,
                vec![instruction(name, f)?],
                Value::Null,
                nxt,
                alt,
                rid,
            )
        };
        Ok(out)
    }
    pub fn decode(&self, pid: i64, rid: u64) -> Result<Value> {
        let proto = indexed(&self.dump["protos"], pid).ok_or_else(|| err("prototype missing"))?;
        let mut d = self.decode_raw(pid, proto, rid)?;
        let rs = proto.get("rs").map(int).transpose()?.unwrap_or(0);
        if rs != 0 {
            Self::unshift(&mut d, rs)?;
        }
        Ok(d)
    }
    fn unshift(d: &mut Value, rs: i64) -> Result<()> {
        for ins in d["ins"]
            .as_array_mut()
            .ok_or_else(|| err("decoded instructions missing"))?
        {
            let name = ins[0].as_str().ok_or_else(|| err("invalid instruction"))?;
            let a = unshift_args(name, &ins[1], rs)?;
            ins[1] = a;
        }
        if !d["ctrl"].is_null() {
            let name = d["ctrl"][0]
                .as_str()
                .ok_or_else(|| err("invalid control instruction"))?;
            let a = unshift_args(name, &d["ctrl"][1], rs)?;
            d["ctrl"][1] = a;
        }
        if let Some(extra) = d.get_mut("extra").and_then(Value::as_array_mut) {
            for pair in extra {
                Self::unshift(&mut pair[1], rs)?;
            }
        }
        Ok(())
    }
    fn decode_raw(&self, pid: i64, p: &Value, rid: u64) -> Result<Value> {
        let rec = p["recs"]
            .get(rid.to_string())
            .ok_or_else(|| err("record missing"))?;
        let df = int(&p["df"])?;
        let l = self.logical(df, &rec["T"])?;
        let sem = &rec["sem"];
        let (nxt, alt) = (l["nxt"].clone(), l["alt"].clone());
        let mut ins = vec![];
        let mut ctrl = Value::Null;
        let parts = array(&rec["parts"]);
        let ff = parts
            .iter()
            .map(|p| self.fused(df, p))
            .collect::<Result<Vec<_>>>()?;
        let where_ = format!("proto {pid} record {rid}");
        if rec["ded"].as_str() != Some("-") && !rec["ded"].is_null() {
            let ti = int(&rec["ded"])?;
            let k = int(sem)?
                .checked_add(if ti == 6 {
                    pid.checked_mul(256)
                        .ok_or_else(|| err("dedicated key overflow"))?
                } else {
                    0
                })
                .ok_or_else(|| err("dedicated key overflow"))?;
            let micro = self.mapping["ded"]
                .get(format!("{ti}:{k}"))
                .ok_or_else(|| {
                    err(format!(
                        "{where_}: dedicated handler {ti}/{k} is not understood"
                    ))
                })?;
            let mut kind = format!("D{ti}_{k}");
            if micro.is_object() {
                return self.decode_loop(&micro["loop"], &ff, rid, sem, nxt, &kind, &where_);
            }
            for m in array(micro) {
                let name = m[0]
                    .as_str()
                    .ok_or_else(|| err("invalid dedicated instruction"))?;
                let part = usize::try_from(int(&m[1])?).map_err(|_| err("negative part"))?;
                let f = ff.get(part).ok_or_else(|| {
                    err(format!(
                        "{where_}: record has too few parts for handler {ti}/{k}"
                    ))
                })?;
                let f = if name == "CAPTURE" && self.capperm() {
                    self.capture(df, &parts[part])?
                } else {
                    f.clone()
                };
                if name == "TAILCALL" {
                    kind = "TAILCALL".into();
                    ins.push(instruction(
                        "CALL",
                        &json!({"A":f["A"],"B":f["B"],"C":0,"rf":0,"Hf":0,"aux":0}),
                    )?);
                    ctrl = instruction(
                        "RETURN",
                        &json!({"A":f["A"],"B":0,"C":0,"rf":0,"Hf":0,"aux":0}),
                    )?;
                    continue;
                }
                let a = if m[2].as_object().is_some_and(|m| !m.is_empty()) {
                    fixed_args(&f, &m[2])?
                } else {
                    f
                };
                if m[3] == true {
                    ctrl = instruction(name, &a)?;
                } else {
                    ins.push(instruction(name, &a)?);
                }
            }
            return Ok(node(&kind, sem, ins, ctrl, nxt, alt, rid));
        }
        if !rec["pat"].is_null() {
            let pt = pattern(&rec["pat"])?;
            if pt.len() != parts.len() {
                return Err(err(format!("{where_}: pattern/part count mismatch")));
            }
            let xor = int(&self.dump["xorkey"])?;
            for (i, b) in pt.into_iter().enumerate() {
                ins.push(self.simple_op(b ^ xor, &ff[i], &where_)?);
            }
            return Ok(node("PAT", sem, ins, Value::Null, nxt, alt, rid));
        }
        let op = int(&l["op"])?;
        let f = args(&l)?;
        let junk = self
            .loader
            .get("junk_op")
            .or_else(|| self.mapping.get("junk"));
        if junk.is_some_and(|v| int(v).ok() == Some(op)) {
            let mix = rec.get("mix").filter(|v| !v.is_null()).ok_or_else(|| {
                err(format!(
                    "{where_}: no mixing value dumped for the junk operation"
                ))
            })?;
            return Ok(node(
                "JUNK",
                sem,
                vec![],
                Value::Null,
                json!(int(&nxt)? ^ int(mix)?),
                alt,
                rid,
            ));
        }
        if let Some(name) = indexed(&self.mapping["hot"], op)
            .or_else(|| indexed(&self.mapping["ctrl"], op))
            .and_then(Value::as_str)
        {
            let mut d = self.plain(name, &f, sem, nxt.clone(), alt, rid)?;
            if matches!(name, "NEWCLOSURE" | "DUPCLOSURE") && parts.len() >= 2 {
                self.fused_captures(&mut d, df, parts, rid, nxt)?;
            }
            return Ok(d);
        }
        if indexed(&self.mapping["simple"], op).is_some() {
            return Ok(node(
                "SIMPLE",
                sem,
                vec![self.simple_op(op, &f, &where_)?],
                Value::Null,
                nxt,
                alt,
                rid,
            ));
        }
        Err(err(format!(
            "{where_}: operation {op} is not understood (build not supported)"
        )))
    }
    fn virtual_id(rid: u64, i: usize) -> Result<u64> {
        let n = u64::try_from(i + 1).map_err(|_| err("too many virtual records"))?;
        let upper = n
            .checked_mul(1u64 << 40)
            .ok_or_else(|| err("virtual record id overflow"))?;
        Ok(upper | rid)
    }
    fn fused_captures(
        &self,
        d: &mut Value,
        df: i64,
        parts: &[Value],
        rid: u64,
        nxt: Value,
    ) -> Result<()> {
        let caps = &parts[1..];
        let ids = (0..caps.len())
            .map(|i| Self::virtual_id(rid, i))
            .collect::<Result<Vec<_>>>()?;
        let mut extra = vec![];
        for (q, part) in caps.iter().enumerate() {
            let l = if self.capperm() {
                self.capture(df, part)?
            } else {
                self.logical(df, part)?
            };
            let f = json!({"A":l["A"],"B":l["B"],"C":0,"rf":0,"Hf":0,"aux":0});
            extra.push(json!([
                ids[q],
                node(
                    "CAPTURE",
                    &d["sem"],
                    vec![instruction("CAPTURE", &f)?],
                    Value::Null,
                    if q + 1 < caps.len() {
                        json!(ids[q + 1])
                    } else {
                        nxt.clone()
                    },
                    json!(NONE),
                    ids[q]
                )
            ]));
        }
        d["nxt"] = json!(ids[0]);
        d["extra"] = json!(extra);
        Ok(())
    }
    #[allow(clippy::too_many_arguments)] // Keep the reference record fields explicit.
    fn decode_loop(
        &self,
        lp: &Value,
        ff: &[Value],
        rid: u64,
        sem: &Value,
        nxt: Value,
        kind: &str,
        where_: &str,
    ) -> Result<Value> {
        let one = |m: &Value| -> Result<Value> {
            let name = m[0]
                .as_str()
                .ok_or_else(|| err("invalid fused instruction"))?;
            let part = usize::try_from(int(&m[1])?).map_err(|_| err("negative record part"))?;
            let f = ff.get(part).ok_or_else(|| {
                err(format!(
                    "{where_}: record has too few parts for a fused loop"
                ))
            })?;
            instruction(
                name,
                &if m[2].as_object().is_some_and(|m| !m.is_empty()) {
                    fixed_args(f, &m[2])?
                } else {
                    f.clone()
                },
            )
        };
        let none = json!({"A":0,"B":0,"C":0,"rf":0,"Hf":0,"aux":0});
        let jump = instruction("JUMP", &none)?;
        let mut pre = vec![];
        for m in array(&lp["pre"]) {
            if m[3] == true && m[0] != "FORGPREP" {
                return Err(err(format!(
                    "{where_}: unexpected jump in front of a fused loop"
                )));
            }
            pre.push(one(m)?);
        }
        let body = array(&lp["body"])
            .iter()
            .map(one)
            .collect::<Result<Vec<_>>>()?;
        if !array(&lp["head"]).is_empty() {
            let ctl = one(&lp["head"][0])?;
            let head = Self::virtual_id(rid, 0)?;
            let bod = Self::virtual_id(rid, 1)?;
            let mut d = node(kind, sem, pre, jump.clone(), nxt.clone(), json!(head), rid);
            let mut extra = vec![json!([
                head,
                node(
                    kind,
                    sem,
                    vec![],
                    ctl,
                    nxt.clone(),
                    json!(if body.is_empty() { head } else { bod }),
                    head
                )
            ])];
            if !body.is_empty() {
                extra.push(json!([
                    bod,
                    node(kind, sem, body, jump, nxt, json!(head), bod)
                ]));
            }
            d["extra"] = json!(extra);
            return Ok(d);
        }
        let tail = array(&lp["tail"])
            .first()
            .ok_or_else(|| err("fused loop has no control instruction"))?;
        let ctl = one(tail)?;
        if pre.is_empty() {
            return Ok(node(kind, sem, body, ctl, nxt, json!(rid), rid));
        }
        let id = Self::virtual_id(rid, 0)?;
        let mut d = node(kind, sem, pre, Value::Null, json!(id), json!(NONE), rid);
        d["extra"] = json!([[id, node(kind, sem, body, ctl, nxt, json!(id), id)]]);
        Ok(d)
    }
}
pub fn decode_program(dump: &Value, mapping: &Value, loader: &Value) -> Result<Value> {
    let ps = dump["protos"]
        .as_object()
        .ok_or_else(|| err("dump has no prototypes"))?;
    let decoder = Decoder::new(dump, mapping, loader);
    let mut protos = Map::new();
    for (pid, p) in ps {
        let pid_num = pid.parse().map_err(|_| err("invalid prototype id"))?;
        let recs = p["recs"]
            .as_object()
            .ok_or_else(|| err("prototype has no records"))?;
        let mut decoded = Map::new();
        for rid in recs.keys() {
            let id = rid.parse().map_err(|_| err("invalid record id"))?;
            let d = decoder.decode(pid_num, id)?;
            if decoded.insert(rid.clone(), d.clone()).is_some() {
                return Err(err("virtual record id collision"));
            }
            for pair in array(&d["extra"]) {
                let id = pair[0]
                    .as_u64()
                    .ok_or_else(|| err("invalid virtual record id"))?
                    .to_string();
                if recs.contains_key(&id) || decoded.insert(id, pair[1].clone()).is_some() {
                    return Err(err("virtual record id collision"));
                }
            }
        }
        let mut p = p.clone();
        p["decoded"] = Value::Object(decoded);
        protos.insert(pid.clone(), p);
    }
    Ok(json!({"main":dump["main"],"protos":protos}))
}

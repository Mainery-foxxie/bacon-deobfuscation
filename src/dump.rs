//! Strict parser for the dump transport; no source text is evaluated here.
use crate::error::{Error, Result};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
fn bad(message: impl Into<String>) -> Error {
    Error::new("decode", message)
}
fn integer(text: &str) -> Result<i64> {
    text.parse().map_err(|_| bad("bad dump integer"))
}
fn nonnegative(text: &str) -> Result<i64> {
    let n = integer(text)?;
    if n < 0 {
        return Err(bad("negative dump identifier"));
    }
    Ok(n)
}
fn list(text: &str) -> Result<Vec<i64>> {
    if text.is_empty() {
        Ok(vec![])
    } else {
        text.split(',').map(integer).collect()
    }
}
pub fn hex(text: &str) -> Result<Vec<u8>> {
    if !text.len().is_multiple_of(2) || !text.is_ascii() {
        return Err(bad("invalid hex bytes"));
    }
    text.as_bytes()
        .chunks(2)
        .map(|p| {
            let s = std::str::from_utf8(p).unwrap();
            u8::from_str_radix(s, 16).map_err(|_| bad("invalid hex bytes"))
        })
        .collect()
}
fn insert(map: &mut Value, key: String, value: Value) -> Result<()> {
    let map = map
        .as_object_mut()
        .ok_or_else(|| bad("invalid dump container"))?;
    if map.insert(key, value).is_some() {
        return Err(bad("duplicate dump entry"));
    }
    Ok(())
}
fn proto<'a>(d: &'a mut Value, key: &str) -> Result<&'a mut Value> {
    d["protos"]
        .get_mut(key)
        .ok_or_else(|| bad("dump references undeclared prototype"))
}
fn raw(text: &str, widths: &[usize]) -> Result<Vec<Value>> {
    let values = text
        .split(',')
        .map(|x| {
            if x == "-" {
                Ok(Value::Null)
            } else {
                Ok(json!(integer(x)?))
            }
        })
        .collect::<Result<Vec<_>>>()?;
    if !widths.contains(&values.len()) {
        return Err(bad("invalid instruction record field count"));
    }
    Ok(values)
}
pub fn parse_dump(text: &str) -> Result<Value> {
    let mut d =
        json!({"protos":{},"ded":{},"pat1":{},"pat2":{},"simple":[],"xorkey":null,"main":null});
    let (mut expected, mut header, mut simple) = (None, false, false);
    let mut kids = BTreeSet::new();
    for line in text
        .split('\n')
        .map(|s| s.trim_end_matches('\r'))
        .filter(|s| !s.is_empty())
    {
        let (tag, rest) = line.split_once(' ').unwrap_or((line, ""));
        match tag {
            "N" => {
                if header {
                    return Err(bad("duplicate dump header"));
                }
                let p: Vec<_> = rest.split(' ').collect();
                if p.len() != 5 || p[1] != "xorkey" || p[3] != "main" {
                    return Err(bad("bad header line"));
                }
                expected = Some(nonnegative(p[0])? as usize);
                d["xorkey"] = json!(integer(p[2])?);
                d["main"] = json!(nonnegative(p[4])?);
                header = true;
            }
            "D" => {
                let (t, keys) = rest
                    .split_once(' ')
                    .ok_or_else(|| bad("bad dedicated table line"))?;
                let ti = nonnegative(t)?;
                if !(1..=6).contains(&ti) {
                    return Err(bad("invalid dedicated table"));
                }
                let values = list(keys)?;
                let set: BTreeSet<_> = values.into_iter().collect();
                insert(&mut d["ded"], ti.to_string(), json!(set))?;
            }
            "P1" | "P2" => {
                let (k, h) = rest
                    .split_once(' ')
                    .ok_or_else(|| bad("bad pattern line"))?;
                let key = nonnegative(k)?.to_string();
                insert(
                    &mut d[if tag == "P1" { "pat1" } else { "pat2" }],
                    key,
                    json!(hex(h)?),
                )?;
            }
            "S" => {
                if simple {
                    return Err(bad("duplicate simple set"));
                }
                simple = true;
                d["simple"] = json!(list(rest)?.into_iter().collect::<BTreeSet<_>>());
            }
            "P" => {
                let mut parts = rest.split(' ');
                let idx = nonnegative(parts.next().unwrap_or(""))?;
                let mut kv = BTreeMap::new();
                for p in parts {
                    let (k, v) = p
                        .split_once('=')
                        .ok_or_else(|| bad("bad prototype field"))?;
                    if kv.insert(k, integer(v)?).is_some() {
                        return Err(bad("duplicate prototype field"));
                    }
                }
                let mut p = json!({"idx":idx,"kids":[],"consts":{},"recs":{}});
                for key in ["np", "va", "nup", "ms", "entry", "df", "vf", "nrec"] {
                    let n = *kv
                        .get(key)
                        .ok_or_else(|| bad(format!("missing prototype field {key}")))?;
                    if n < 0 {
                        return Err(bad("negative prototype field"));
                    }
                    p[key] = json!(n);
                }
                if ![0, 1].contains(&p["va"].as_i64().unwrap()) {
                    return Err(bad("invalid vararg flag"));
                }
                if kv.keys().any(|k| {
                    !["np", "va", "nup", "ms", "entry", "df", "vf", "nrec", "rs"].contains(k)
                }) {
                    return Err(bad("unknown prototype field"));
                }
                p["rs"] = json!(kv.get("rs").copied().unwrap_or(0));
                insert(&mut d["protos"], idx.to_string(), p)?;
            }
            "H" => {
                let (idx, children) = rest.split_once(' ').ok_or_else(|| bad("bad child list"))?;
                let key = nonnegative(idx)?.to_string();
                if !kids.insert(key.clone()) {
                    return Err(bad("duplicate child list"));
                }
                proto(&mut d, &key)?["kids"] = json!(list(children)?);
            }
            "C" => {
                let mut p = rest.splitn(4, ' ');
                let idx = nonnegative(p.next().unwrap_or(""))?.to_string();
                let cid = nonnegative(p.next().unwrap_or(""))?.to_string();
                let tag = p.next().ok_or_else(|| bad("bad constant line"))?;
                let value = p.next().unwrap_or("");
                let cv = match tag {
                    "s" => json!(["s", hex(value)?]),
                    "n" => {
                        let n: f64 = value.parse().map_err(|_| bad("bad numeric constant"))?;
                        json!(["n", crate::model::encode_number(n)])
                    }
                    "z" => {
                        if !value.is_empty() {
                            return Err(bad("nil constant has data"));
                        }
                        json!(["z", null])
                    }
                    "b" => {
                        if value != "0" && value != "1" {
                            return Err(bad("bad boolean constant"));
                        }
                        json!(["b", value == "1"])
                    }
                    "i" => json!(["i", list(value)?]),
                    "t" => {
                        let mut items = vec![];
                        if !value.is_empty() {
                            for item in value.split(',') {
                                let (k, v) = item
                                    .split_once(':')
                                    .ok_or_else(|| bad("bad table constant"))?;
                                items.push(json!([integer(k)?, integer(v)?]));
                            }
                        }
                        json!(["t", items])
                    }
                    _ => json!([
                        "?",
                        if value.is_empty() {
                            Value::Null
                        } else {
                            json!(value)
                        }
                    ]),
                };
                insert(&mut proto(&mut d, &idx)?["consts"], cid, cv)?;
            }
            "R" => {
                let mut p = rest.splitn(3, ' ');
                let idx = nonnegative(p.next().unwrap_or(""))?.to_string();
                let rid = nonnegative(p.next().unwrap_or(""))?.to_string();
                let rem = p.next().ok_or_else(|| bad("bad record line"))?;
                let mut kv = BTreeMap::new();
                for field in rem.split(' ') {
                    let (k, v) = field
                        .split_once('=')
                        .ok_or_else(|| bad("bad record line"))?;
                    if kv.insert(k, v).is_some() {
                        return Err(bad("duplicate record field"));
                    }
                }
                let get = |key| {
                    kv.get(key)
                        .copied()
                        .ok_or_else(|| bad("missing record field"))
                };
                let sem = nonnegative(get("sem")?)?;
                let ded = get("ded")?;
                if ded != "-" && !matches!(ded, "1" | "2" | "3" | "4" | "5" | "6") {
                    return Err(bad("invalid record dedicated table"));
                }
                let pat = get("pat")?;
                let t = raw(get("raw")?, &[10])?;
                let parts = get("parts")?;
                let parts = if parts.is_empty() {
                    vec![]
                } else {
                    parts
                        .split(';')
                        .map(|s| raw(s, &[6, 10]))
                        .collect::<Result<Vec<_>>>()?
                };
                let mix = match kv.get("mix").copied() {
                    None | Some("-") => Value::Null,
                    Some(x) => json!(integer(x)?),
                };
                if kv
                    .keys()
                    .any(|k| !["sem", "ded", "pat", "raw", "parts", "mix"].contains(k))
                {
                    return Err(bad("unknown record field"));
                }
                insert(
                    &mut proto(&mut d, &idx)?["recs"],
                    rid,
                    json!({"sem":sem,"ded":ded,"pat":if pat=="-"{Value::Null}else{json!(hex(pat)?)},"T":t,"parts":parts,"mix":mix}),
                )?;
            }
            _ => {
                return Err(bad(format!(
                    "unknown dump line: {}",
                    line.chars().take(60).collect::<String>()
                )))
            }
        }
    }
    let protos = d["protos"].as_object().unwrap();
    if !header || protos.is_empty() {
        return Err(bad("empty dump"));
    }
    if expected != Some(protos.len()) {
        return Err(bad("prototype count mismatch"));
    }
    if !protos.contains_key(&d["main"].to_string()) {
        return Err(bad("main prototype missing"));
    }
    for (key, p) in protos {
        if !kids.contains(key) {
            return Err(bad("prototype child list missing"));
        }
        if p["nrec"].as_u64() != Some(p["recs"].as_object().unwrap().len() as u64) {
            return Err(bad("instruction count mismatch"));
        }
        for child in p["kids"].as_array().unwrap() {
            if !protos.contains_key(&child.to_string()) {
                return Err(bad("child prototype missing"));
            }
        }
    }
    Ok(d)
}

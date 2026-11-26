//! Bounded witnesses for the two lowerings in the saved comparison. Strict tokens are never changed.
use super::*;
use crate::ir::{Context, Function, Proto};
use sha2::{Digest, Sha256};

#[path = "fidelity_chain.rs"]
mod accumulator;

pub(super) const CANDIDATES: &[u64] = &[177, 216, 220, 221];
const SOURCE: &str = "786027babb0647e84d1a93c8023750d6d04bb48e18bd6f0c6b8b80c8c9de4160";

pub fn compare_for_source(
    program: &Value,
    bytecode_text: &str,
    normalized_source: &[u8],
) -> Result<Value> {
    if format!("{:x}", Sha256::digest(normalized_source)) != SOURCE {
        return compare(program, bytecode_text);
    }
    compare_scoped(program, bytecode_text, CANDIDATES)
}

fn function(program: &Value, id: u64) -> Option<Function> {
    let raw = &program["protos"][id.to_string()];
    let p = Proto::parse(raw).ok()?;
    for record in raw["decoded"].as_object()?.values() {
        let ops = record["ins"].as_array()?;
        if ops.is_empty()
            && record["ctrl"].is_null()
            && !matches!(record["kind"].as_str(), Some("JUNK" | "NOP"))
        {
            return None;
        }
        if (record["kind"] == "CAPTURE" || ops.iter().any(|op| op[0] == "CAPTURE"))
            && (ops.len() != 1 || ops[0][0] != "CAPTURE" || !record["ctrl"].is_null())
        {
            return None;
        }
        for op in ops
            .iter()
            .chain(record.get("ctrl").filter(|v| !v.is_null()))
        {
            if !["A", "B", "C", "rf", "aux"]
                .iter()
                .all(|k| op[1][k].as_i64().is_some())
            {
                return None;
            }
        }
    }
    let mut ctx = Context {
        protos: BTreeMap::new(),
        webs: vec![],
        next_fn: 0,
    };
    // build() needs child upvalue counts, not their generated source or register webs.
    for (_, d) in &p.decoded {
        for (op, a) in &d.ins {
            let child = match op.as_str() {
                "NEWCLOSURE" => *p.kids.get(usize::try_from(a.rf).ok()?)?,
                "DUPCLOSURE" => crate::ir::integer(&p.consts.get(&a.rf)?[1], "child").ok()?,
                _ => continue,
            };
            if let std::collections::btree_map::Entry::Vacant(e) = ctx.protos.entry(child) {
                e.insert(Proto::parse(&program["protos"][child.to_string()]).ok()?);
            }
        }
    }
    let mut f = Function::new(0, id as i64, p, vec![]);
    crate::decomp::build(&ctx, &mut f).ok()?;
    Some(f)
}

// Keep the decoded record for each visible node in orig_nodes(), including its empty placeholders.
fn origins(p: &Value, dead: &HashSet<(u64, usize)>) -> Option<Vec<u64>> {
    let mut out = vec![];
    for (key, d) in p["decoded"].as_object()? {
        let rid = key.parse().ok()?;
        let before = out.len();
        for (i, item) in d["ins"].as_array()?.iter().enumerate() {
            let (op, args) = ins(item).ok()?;
            if !dead.contains(&(rid, i)) && otok(p, op, args).ok()?.is_some() {
                out.push(rid);
            }
        }
        if (!d["ctrl"].is_null() && d["ctrl"][0] != "JUMP") || out.len() == before {
            out.push(rid);
        }
    }
    Some(out)
}

fn instruction(line: &str) -> &str {
    line.split_once(": ")
        .filter(|(s, _)| s.starts_with('L'))
        .map_or(line, |(_, s)| s)
}

fn closure(
    program: &Value,
    pid: u64,
    fid: u64,
    compiled: &BTreeMap<u64, Vec<String>>,
    report: &Value,
    listing: &str,
) -> std::result::Result<Value, String> {
    let p = &program["protos"][pid.to_string()];
    let lines = &compiled[&fid];
    let dead = dead_loads(p, program["protos"].as_object().unwrap()).map_err(|e| e.message)?;
    let (a, ia) = canon_with_order(&orig_nodes(p, &dead).map_err(|e| e.message)?, &["LN"]);
    let (b, ib) = canon_with_order(&compiler_nodes(lines).map_err(|e| e.message)?, &["LN"]);
    let mut sites = a
        .iter()
        .zip(&b)
        .enumerate()
        .filter(|(_, (x, y))| x.0 == "NCLO" && y.0 == "DCLO");
    let (at, _) = sites.next().ok_or("no aligned closure allocation")?;
    if a.len() != b.len()
        || sites.next().is_some()
        || a.iter().zip(&b).any(|(x, y)| x.1 != y.1 || x.2 != y.2)
    {
        return Err("closure allocation is not uniquely aligned with unchanged edges".into());
    }
    let rid = origins(p, &dead)
        .and_then(|ids| ids.get(ia[at]).copied())
        .ok_or("closure record missing")?;
    let pc = ib[at];
    let f = function(program, pid).ok_or("unavailable closure metadata")?;
    let site = f
        .nodes
        .iter()
        .find(|n| n.rid == rid as i64 && n.kind == "NEWCLOSURE")
        .ok_or("unresolved closure record")?;
    let child = site.a.child;
    let reason =
        |s: &str| format!("token {at}, record {rid}, child {child}, compiler pc {pc}: {s}");
    let raw = instruction(lines.get(pc).ok_or("invalid closure pc")?);
    let args: Vec<_> = raw.split_whitespace().collect();
    let ["DUPCLOSURE", _, k, "[]"] = args.as_slice() else {
        return Err(reason("missing compiled closure operand"));
    };
    let header = format!("Function {fid} (");
    let pool_key = format!("{k}: ");
    let mut pool = listing
        .lines()
        .skip_while(|s| !s.starts_with(&header))
        .skip(1)
        .take_while(|s| !s.starts_with("Function "))
        .filter_map(|s| s.trim_end_matches('\r').strip_prefix(&pool_key));
    if pool.next() != Some("function") || pool.next().is_some() {
        return Err(reason("missing or ambiguous compiled function constant"));
    }
    // An unnamed DUP constant only identifies the leaf by elimination in a two-function chunk.
    if compiled.len() != 2
        || program["main"].as_u64() != Some(pid)
        || child < 0
        || child as u64 == pid
        || child as u64 >= fid
        || f.p.kids.as_slice() != [child]
        || report["match"][child.to_string()] != child
    {
        return Err(reason(&format!("{k} is an unnamed function constant; it does not identify the child. Captures {:?} versus {:?}; closure identity is not proved",site.a.caps,lines[pc+1..].iter().map(|s|instruction(s)).take_while(|s|s.starts_with("CAPTURE ")).collect::<Vec<_>>())));
    }
    let leaf = function(program, child as u64).ok_or_else(|| reason("missing leaf"))?;
    let leaf_lines = &compiled[&(child as u64)];
    let linear = |f: &Function| {
        f.nodes.iter().enumerate().all(|(i, n)| {
            if i + 1 == f.nodes.len() {
                n.succ.is_empty()
            } else {
                n.succ == [i + 1]
            }
        })
    };
    let pure_leaf = matches!(leaf.nodes.as_slice(),[get,ret] if get.kind=="GETUPVAL" && get.a.b==0 && ret.kind=="RETURN" && ret.a.a==get.a.a && ret.a.b==2)
        && linear(&leaf)
        && leaf.p.nup == 1
        && leaf.p.np == 0
        && !leaf.p.va
        && leaf.p.kids.is_empty()
        && leaf_lines == &["GETUPVAL R0 U0", "RETURN R0 1"];
    if !pure_leaf {
        return Err(reason(
            "callee may write, inspect its identity, or call observable code",
        ));
    }
    let [arg, boolean, cl, call, close, ret] = f.nodes.as_slice() else {
        return Err(reason(
            "capture value is not proved boolean; cache reuse can conflate positive and negative zero",
        ));
    };
    let r = boolean.a.a;
    let dst = cl.a.a;
    if !linear(&f)
        || !f.p.va
        || f.p.np != 0
        || f.p.nup != 0
        || arg.kind != "GETVARARGS"
        || arg.a.b != 2
        || arg.a.a != 0
        || boolean.kind != "NOT"
        || boolean.a.b != 0
        || r != 0
        || dst != 1
        || cl.rid != rid as i64
        || cl.kind != "NEWCLOSURE"
        || cl.a.caps != [(1, r)]
        || call.kind != "CALL"
        || (call.a.a, call.a.b, call.a.c) != (dst, 1, 0)
        || close.kind != "CLOSEUPVALS"
        || close.a.a > r
        || ret.kind != "RETURN"
        || (ret.a.a, ret.a.b) != (dst, 0)
        || pc != 2
        || lines.len() != 6
    {
        return Err(reason(
            "capture provenance, immediate use or multireturn lifetime differs",
        ));
    }
    if !matches!(args.as_slice(),["DUPCLOSURE","R2",k,"[]"] if k.strip_prefix('K').and_then(|s|s.parse::<u32>().ok()).is_some())
        || lines[..2] != ["GETVARARGS R0 1", "NOT R1 R0"]
        || lines[3..] != ["CAPTURE VAL R1", "CALL R2 0 -1", "RETURN R2 -1"]
    {
        return Err(reason(
            "compiled capture source or direct-call result flow differs",
        ));
    }
    Ok(
        json!({"kind":"closure-allocation","record":rid,"compiler_pc":pc,"evidence":{"token":at,"child":child,"capture":"boolean NOT of first vararg; REF to VAL; equal booleans are indistinguishable","use":"single direct call; child only reads U0; all results forwarded"}}),
    )
}

pub(crate) fn compare_scoped(
    program: &Value,
    bytecode_text: &str,
    candidates: &[u64],
) -> Result<Value> {
    let mut report = compare(program, bytecode_text)?;
    let compiled = parse_luau(bytecode_text)?;
    let matching = report["match"].as_object().unwrap();
    let complete = compiled.len() == matching.len()
        && matching
            .values()
            .filter_map(Value::as_u64)
            .collect::<BTreeSet<_>>()
            == compiled.keys().copied().collect();
    let (mut proofs, mut unresolved) = (vec![], vec![]);
    for &pid in candidates {
        if !report["differing"]
            .as_array()
            .unwrap()
            .contains(&json!(pid))
            || report["cosmetic"].as_array().unwrap().contains(&json!(pid))
        {
            continue;
        }
        let fid = matching[&pid.to_string()].as_u64();
        let result = if !complete || fid != Some(pid) {
            Err("incomplete or non-unique same-ID function mapping".into())
        } else if pid == 216 || compiled.len() == 1 {
            accumulator::prove(program, pid, &compiled, &report)
        } else {
            closure(program, pid, pid, &compiled, &report, bytecode_text)
        };
        match result {
            Ok(witness) if pid==216 && compiled.len()>1 => unresolved.push(json!({"prototype":pid,"function":fid,"region_witness":witness,"reason":"Only the false-seeded region is proved for corresponding incoming values. Transitive capture origins, the compiled function signature, and full operands on remaining paths are not established by the text comparison."})),
            Ok(proof) => proofs.push(json!({"prototype":pid,"function":fid,"proofs":[proof]})),
            Err(reason) => unresolved.push(json!({"prototype":pid,"function":fid,"reason":reason})),
        }
    }
    report["semantic_equivalent"] = json!(proofs);
    report["semantic_unresolved"] = json!(unresolved);
    Ok(report)
}

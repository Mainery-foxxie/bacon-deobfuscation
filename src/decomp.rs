//! Records -> reachable nodes -> blocks -> reaching definitions -> register webs.
use crate::error::Result;
use crate::ir::*;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

pub fn is_cond(k: &str) -> bool {
    matches!(
        k,
        "JUMPIFNOT"
            | "JUMPIF"
            | "JUMPIFEQ"
            | "JUMPIFNOTEQ"
            | "JUMPIFLE"
            | "JUMPIFLT"
            | "JUMPIFNOTLT"
            | "JUMPIFNOTLE"
            | "JUMPXEQK"
            | "JUMPXEQKB"
            | "JUMPXEQKNIL"
    )
}
pub fn is_term(k: &str) -> bool {
    is_cond(k)
        || matches!(
            k,
            "RETURN" | "FORNPREP" | "FORNLOOP" | "FORGPREP" | "FORGLOOP"
        )
}
pub fn arith(k: &str) -> Option<&'static str> {
    Some(match k {
        "ADD" | "ADDK" => "+",
        "SUB" | "SUBK" | "SUBRK" => "-",
        "MUL" | "MULK" => "*",
        "DIV" | "DIVK" | "DIVRK" => "/",
        "MOD" | "MODK" => "%",
        "POW" | "POWK" => "^",
        "IDIV" | "IDIVK" => "//",
        _ => return None,
    })
}
pub fn is_rr(k: &str) -> bool {
    matches!(k, "ADD" | "SUB" | "MUL" | "DIV" | "MOD" | "POW" | "IDIV")
}
pub fn is_rk(k: &str) -> bool {
    matches!(k, "SUBRK" | "DIVRK")
}
pub fn is_k(k: &str) -> bool {
    matches!(
        k,
        "ADDK" | "SUBK" | "MULK" | "DIVK" | "MODK" | "POWK" | "IDIVK"
    )
}
pub fn result_first(k: &str) -> bool {
    is_rr(k)
        || is_k(k)
        || is_rk(k)
        || matches!(
            k,
            "GETTABLEKS"
                | "GETTABLE"
                | "GETTABLEN"
                | "LENGTH"
                | "NOT"
                | "MINUS"
                | "OR"
                | "AND"
                | "ORK"
                | "ANDK"
        )
}

pub fn prepare(ctx: &mut Context, f: &mut Function) -> Result<()> {
    build(ctx, f)?;
    annotate(f)?;
    make_blocks(f);
    webs(ctx, f)
}
fn first(
    mut rid: i64,
    records: &BTreeMap<i64, Record>,
    recnodes: &BTreeMap<i64, Vec<usize>>,
) -> Result<usize> {
    let mut seen = BTreeSet::new();
    loop {
        if !seen.insert(rid) {
            return Err(err("empty cycle"));
        }
        let d = records
            .get(&rid)
            .ok_or_else(|| err(format!("missing record {rid}")))?;
        if let Some(n) = recnodes.get(&rid).and_then(|ns| ns.first()) {
            return Ok(*n);
        }
        rid = if d.ctrl.as_ref().is_some_and(|(k, _)| k == "JUMP") {
            d.alt
        } else {
            d.nxt
        };
    }
}
pub fn build(ctx: &Context, f: &mut Function) -> Result<()> {
    let mut records: BTreeMap<_, _> = f.p.decoded.iter().cloned().collect();
    let mut recnodes = BTreeMap::<i64, Vec<usize>>::new();
    let mut nodes = vec![];
    for (rid, d) in &f.p.decoded {
        if d.kind == "CAPTURE" {
            continue;
        }
        let mut ids = vec![];
        for (k, a) in &d.ins {
            if k == "NOP" {
                continue;
            }
            if k == "LOADB" && d.ctrl.as_ref().is_some_and(|(k, _)| k == "LOADBSKIP") && a.c != 0 {
                return Err(err("LOADB with skip"));
            }
            ids.push(nodes.len());
            nodes.push(Node {
                kind: k.clone(),
                a: a.clone(),
                rid: *rid,
                succ: vec![],
                reads: vec![],
                writes: vec![],
                preds: vec![],
                blk: 0,
                topbase: None,
            });
        }
        if let Some((k, a)) = &d.ctrl {
            if k != "JUMP" && k != "LOADBSKIP" {
                ids.push(nodes.len());
                nodes.push(Node {
                    kind: k.clone(),
                    a: a.clone(),
                    rid: *rid,
                    succ: vec![],
                    reads: vec![],
                    writes: vec![],
                    preds: vec![],
                    blk: 0,
                    topbase: None,
                });
            }
        }
        recnodes.insert(*rid, ids);
    }
    for (rid, _) in &f.p.decoded {
        if let Some(ids) = recnodes.get(rid) {
            for &id in ids {
                let n = &mut nodes[id];
                if !matches!(n.kind.as_str(), "NEWCLOSURE" | "DUPCLOSURE") {
                    continue;
                }
                let child = if n.kind == "NEWCLOSURE" {
                    *f.p.kids
                        .get(n.a.rf as usize)
                        .ok_or_else(|| err("missing child prototype"))?
                } else {
                    integer(
                        &f.p.consts
                            .get(&n.a.rf)
                            .ok_or_else(|| err("missing closure constant"))?[1],
                        "closure prototype",
                    )?
                };
                let nup = ctx
                    .protos
                    .get(&child)
                    .ok_or_else(|| err(format!("missing prototype {child}")))?
                    .nup;
                let mut cur = records[rid].nxt;
                let mut caps = vec![];
                for _ in 0..nup {
                    let d = records
                        .get(&cur)
                        .ok_or_else(|| err("missing capture record"))?;
                    if d.kind != "CAPTURE" {
                        return Err(err(format!(
                            "expected CAPTURE in proto {} rec {rid}",
                            f.pid
                        )));
                    }
                    let a = &d.ins.first().ok_or_else(|| err("empty CAPTURE"))?.1;
                    caps.push((a.a, a.b));
                    cur = d.nxt;
                }
                n.a.child = child;
                n.a.caps = caps;
                records.get_mut(rid).unwrap().nxt = cur;
            }
        }
    }
    for (rid, ids) in &recnodes {
        let d = &records[rid];
        for (i, &id) in ids.iter().enumerate() {
            let k = nodes[id].kind.as_str();
            let succ = if i + 1 < ids.len() {
                vec![ids[i + 1]]
            } else if k == "RETURN" {
                vec![]
            } else if is_cond(k) || matches!(k, "FORNPREP" | "FORNLOOP" | "FORGLOOP") {
                vec![
                    first(d.nxt, &records, &recnodes)?,
                    first(d.alt, &records, &recnodes)?,
                ]
            } else {
                vec![first(
                    if d.ctrl.as_ref().is_some_and(|(k, _)| k == "JUMP") {
                        d.alt
                    } else {
                        d.nxt
                    },
                    &records,
                    &recnodes,
                )?]
            };
            nodes[id].succ = succ;
        }
    }
    let entry = first(f.p.entry, &records, &recnodes)?;
    let mut order = vec![];
    let mut seen = BTreeSet::new();
    let mut st = vec![entry];
    while let Some(n) = st.pop() {
        if !seen.insert(n) {
            continue;
        }
        order.push(n);
        st.extend(nodes[n].succ.iter().rev().copied());
    }
    let remap: BTreeMap<_, _> = order.iter().enumerate().map(|(i, &n)| (n, i)).collect();
    f.nodes = order
        .iter()
        .map(|&id| {
            let mut n = nodes[id].clone();
            n.succ = n.succ.iter().map(|s| remap[s]).collect();
            n
        })
        .collect();
    for i in 0..f.nodes.len() {
        for s in f.nodes[i].succ.clone() {
            f.nodes[s].preds.push(i);
        }
    }
    f.entry = 0;
    Ok(())
}
pub fn annotate(f: &mut Function) -> Result<()> {
    for n in &mut f.nodes {
        let a = &n.a;
        let k = n.kind.as_str();
        let (r, w) = match k {
            "LOADK" | "LOADN" | "LOADB" | "LOADNIL" | "GETIMPORT" | "GETUPVAL" | "NEWTABLE"
            | "DUPTABLE" => (vec![], vec![a.a]),
            "MOVE" | "GETTABLEKS" | "LENGTH" | "NOT" | "MINUS" | "GETTABLEN" | "ORK" | "ANDK" => {
                (vec![a.b], vec![a.a])
            }
            "SETTABLEKS" | "SETTABLEN" => (vec![a.a, a.b], vec![]),
            "SELF" => (vec![a.b], vec![a.a + 1, a.a]),
            "SETUPVAL" => (vec![a.a], vec![]),
            "OR" | "AND" | "GETTABLE" => (vec![a.b, a.c], vec![a.a]),
            "GETVARARGS" => (vec![], (0..(a.b - 1).max(0)).map(|i| a.a + i).collect()),
            "CONCAT" => ((a.b..=a.c).collect(), vec![a.a]),
            "SETTABLE" => (vec![a.a, a.b, a.c], vec![]),
            "FORGPREP" => (vec![a.a, a.a + 1, a.a + 2], vec![]),
            "FORGLOOP" => (vec![], (0..a.aux & 255).map(|i| a.a + 3 + i).collect()),
            "SETLIST" => (vec![a.a], vec![a.a]),
            "NEWCLOSURE" | "DUPCLOSURE" => (
                a.caps
                    .iter()
                    .filter(|(t, idx)| matches!(t, 0 | 1) && *idx != a.a)
                    .map(|(_, idx)| *idx)
                    .collect(),
                vec![a.a],
            ),
            "CALL" | "RETURN" | "CLOSEUPVALS" => (vec![], vec![]),
            "JUMPIFNOT" | "JUMPIF" => (vec![a.a], vec![]),
            "JUMPIFEQ" | "JUMPIFNOTEQ" | "JUMPIFLE" | "JUMPIFLT" | "JUMPIFNOTLT"
            | "JUMPIFNOTLE" => (vec![a.a, a.aux], vec![]),
            "JUMPXEQK" | "JUMPXEQKB" | "JUMPXEQKNIL" => (vec![a.a], vec![]),
            "FORNPREP" => (vec![a.a, a.a + 1, a.a + 2], vec![a.a + 2]),
            "FORNLOOP" => (vec![], vec![a.a + 2]),
            _ if is_rr(k) => (vec![a.b, a.c], vec![a.a]),
            _ if is_k(k) => (vec![a.b], vec![a.a]),
            _ if is_rk(k) => (vec![a.c], vec![a.a]),
            _ => return Err(err(format!("annotate: unknown node kind {k}"))),
        };
        n.reads = r;
        n.writes = w;
    }
    for id in 0..f.nodes.len() {
        let n = &f.nodes[id];
        if !matches!(n.kind.as_str(), "CALL" | "RETURN" | "SETLIST") {
            continue;
        }
        let a = n.a.clone();
        let k = n.kind.clone();
        let variable = if k == "SETLIST" { a.c == 0 } else { a.b == 0 };
        let mut top = None;
        if variable {
            let mut cur = id;
            let mut seen = BTreeSet::new();
            loop {
                if !seen.insert(cur) || f.nodes[cur].preds.len() != 1 {
                    return Err(err("multret consumer without single pred chain"));
                }
                cur = f.nodes[cur].preds[0];
                let p = &f.nodes[cur];
                if (p.kind == "CALL" && p.a.c == 0) || (p.kind == "GETVARARGS" && p.a.b == 0) {
                    top = Some(p.a.a);
                    break;
                }
                if is_term(&p.kind) {
                    return Err(err("multret producer not found"));
                }
            }
        }
        let n = &mut f.nodes[id];
        n.topbase = top;
        match k.as_str() {
            "CALL" => {
                let cnt = if a.b > 0 {
                    a.b - 1
                } else {
                    top.unwrap() - a.a - 1
                };
                n.reads = (0..=cnt).map(|i| a.a + i).collect();
                if a.c >= 2 {
                    n.writes = (0..a.c - 1).map(|i| a.a + i).collect();
                }
            }
            "RETURN" => {
                let cnt = if a.b > 0 { a.b - 1 } else { top.unwrap() - a.a };
                n.reads = (0..cnt).map(|i| a.a + i).collect();
            }
            _ => {
                let cnt = if a.c > 0 { a.c - 1 } else { top.unwrap() - a.b };
                n.reads = std::iter::once(a.a)
                    .chain((0..cnt).map(|i| a.b + i))
                    .collect();
                n.writes = vec![a.a];
            }
        }
    }
    Ok(())
}
pub fn make_blocks(f: &mut Function) {
    let mut leaders = BTreeSet::from([f.entry]);
    for (i, n) in f.nodes.iter().enumerate() {
        if is_term(&n.kind) {
            leaders.extend(n.succ.iter().copied());
        } else if n.succ.len() == 1 && f.nodes[n.succ[0]].preds.len() > 1 {
            leaders.insert(n.succ[0]);
        }
        if n.preds.len() > 1 {
            leaders.insert(i);
        }
    }
    let mut bs = vec![];
    for lid in &leaders {
        let mut b = Block::default();
        let mut n = *lid;
        loop {
            b.nodes.push(n);
            f.nodes[n].blk = bs.len();
            let m = &f.nodes[n];
            if is_term(&m.kind) || m.succ.len() != 1 || leaders.contains(&m.succ[0]) {
                break;
            }
            n = m.succ[0];
        }
        bs.push(b);
    }
    for b in &mut bs {
        b.succ = f.nodes[*b.nodes.last().unwrap()]
            .succ
            .iter()
            .map(|n| f.nodes[*n].blk)
            .collect();
    }
    for b in 0..bs.len() {
        for s in bs[b].succ.clone() {
            bs[s].pred.push(b);
        }
    }
    f.entry_block = f.nodes[f.entry].blk;
    f.blocks = bs;
}
fn find(uf: &mut [usize], mut x: usize) -> usize {
    while uf[x] != x {
        uf[x] = uf[uf[x]];
        x = uf[x];
    }
    x
}
fn union(uf: &mut [usize], a: usize, b: usize) {
    let a = find(uf, a);
    let b = find(uf, b);
    uf[b] = a;
}
pub fn webs(ctx: &mut Context, f: &mut Function) -> Result<()> {
    let mut defs = vec![];
    let mut nd = BTreeMap::new();
    let mut regdefs = BTreeMap::<Reg, DefSet>::new();
    for r in 0..f.p.np {
        defs.push(Def {
            id: defs.len(),
            node: None,
            reg: r as Reg,
        });
    }
    for (i, n) in f.nodes.iter().enumerate() {
        for &r in &n.writes {
            nd.insert((i, r), defs.len());
            defs.push(Def {
                id: defs.len(),
                node: Some(i),
                reg: r,
            });
        }
    }
    for d in &defs {
        regdefs.entry(d.reg).or_default().insert(d.id);
    }
    let mut gens = vec![];
    for b in &f.blocks {
        let mut g = BTreeMap::new();
        for &id in &b.nodes {
            for &r in &f.nodes[id].writes {
                g.insert(r, nd[&(id, r)]);
            }
        }
        gens.push(g);
    }
    let mut ins = vec![BTreeMap::<Reg, DefSet>::new(); f.blocks.len()];
    let mut outs = ins.clone();
    let init: BTreeMap<_, _> = defs
        .iter()
        .filter(|d| d.node.is_none())
        .map(|d| (d.reg, BTreeSet::from([d.id])))
        .collect();
    let mut work: VecDeque<_> = (0..f.blocks.len()).collect();
    let mut inwork: BTreeSet<_> = (0..f.blocks.len()).collect();
    while let Some(b) = work.pop_front() {
        inwork.remove(&b);
        let mut cur = BTreeMap::<Reg, DefSet>::new();
        for p in &f.blocks[b].pred {
            for (r, ds) in &outs[*p] {
                cur.entry(*r).or_default().extend(ds);
            }
        }
        if b == f.entry_block {
            for (r, ds) in &init {
                cur.entry(*r).or_default().extend(ds);
            }
        }
        ins[b] = cur.clone();
        for (r, d) in &gens[b] {
            cur.insert(*r, BTreeSet::from([*d]));
        }
        if cur != outs[b] {
            outs[b] = cur;
            for s in &f.blocks[b].succ {
                if inwork.insert(*s) {
                    work.push_back(*s);
                }
            }
        }
    }
    let mut uf: Vec<_> = (0..defs.len()).collect();
    let mut loop_base = BTreeMap::<usize, Reg>::new();
    for n in &f.nodes {
        if !matches!(n.kind.as_str(), "FORNPREP" | "FORGPREP") || n.succ.is_empty() {
            continue;
        }
        let (body, leave) = if n.kind == "FORNPREP" {
            (n.succ[0], n.succ[1])
        } else {
            let lp = &f.nodes[n.succ[0]];
            if lp.kind != "FORGLOOP" || lp.succ.len() != 2 {
                continue;
            }
            (lp.succ[1], lp.succ[0])
        };
        let mut seen = BTreeSet::new();
        let mut todo = vec![body];
        while let Some(i) = todo.pop() {
            if i == leave || !seen.insert(i) {
                continue;
            }
            todo.extend(&f.nodes[i].succ);
        }
        for i in seen {
            loop_base
                .entry(i)
                .and_modify(|a| *a = (*a).max(n.a.a))
                .or_insert(n.a.a);
        }
    }
    let mut usedefs = BTreeMap::<(usize, Reg), DefSet>::new();
    let mut movers = vec![];
    let mut assigns = vec![];
    for (bi, b) in f.blocks.iter().enumerate() {
        let mut cur = ins[bi].clone();
        for &id in &b.nodes {
            let n = &f.nodes[id];
            for &r in &n.reads {
                let ds=cur.get(&r).filter(|s|!s.is_empty()).ok_or_else(||err(format!("read of undefined register R{r} at node {id} (rec {}, kind {}) in proto {}",n.rid,n.kind,f.pid)))?;
                usedefs.insert((id, r), ds.clone());
                let first = *ds.first().unwrap();
                for &x in ds.iter().skip(1) {
                    union(&mut uf, first, x);
                }
            }
            if n.kind == "MOVE" && n.a.a < n.a.b {
                if let Some(prev) = cur.get(&n.a.a) {
                    movers.push((id, prev.clone()));
                }
            }
            if let Some(base) = loop_base.get(&id) {
                for &r in &n.writes {
                    if r < *base {
                        if let Some(prev) = cur.get(&r) {
                            assigns.push((id, r, prev.clone()));
                        }
                    }
                }
            }
            for &r in &n.writes {
                cur.insert(r, BTreeSet::from([nd[&(id, r)]]));
            }
            if matches!(n.kind.as_str(), "NEWCLOSURE" | "DUPCLOSURE") {
                for (t, idx) in &n.a.caps {
                    if matches!(t, 0 | 1) && *idx == n.a.a {
                        usedefs.insert((id, *idx), BTreeSet::from([nd[&(id, *idx)]]));
                    }
                }
            }
        }
    }
    let mut readers = BTreeMap::<usize, Vec<usize>>::new();
    for ((id, _), ds) in &usedefs {
        for d in ds {
            readers.entry(*d).or_default().push(*id);
        }
    }
    for (id, prev) in movers {
        let n = &f.nodes[id];
        let d = nd[&(id, n.a.a)];
        if let Some(rd) = readers.get(&d) {
            if !rd.is_empty()
                && (rd.iter().all(|i| {
                    let k = &f.nodes[*i];
                    k.kind == "CONCAT" && k.a.b <= n.a.a && n.a.a <= k.a.c
                }) || rd.iter().all(|i| {
                    let k = &f.nodes[*i];
                    k.kind == "FORNPREP" && k.a.a <= n.a.a && n.a.a <= k.a.a + 2
                }))
            {
                continue;
            }
        }
        for x in prev {
            union(&mut uf, d, x);
        }
    }
    for (id, r, prev) in assigns {
        for x in prev {
            union(&mut uf, nd[&(id, r)], x);
        }
    }
    for (id, n) in f.nodes.iter().enumerate() {
        if matches!(n.kind.as_str(), "NEWCLOSURE" | "DUPCLOSURE") {
            for (t, idx) in &n.a.caps {
                if *t != 1 {
                    continue;
                }
                let d0 = *usedefs[&(id, *idx)].first().unwrap();
                let mut seen = BTreeSet::new();
                let mut st = n.succ.clone();
                while let Some(m) = st.pop() {
                    if !seen.insert(m) {
                        continue;
                    }
                    let k = &f.nodes[m];
                    if k.kind == "CLOSEUPVALS" && k.a.a <= *idx {
                        continue;
                    }
                    if k.writes.contains(idx) {
                        union(&mut uf, d0, nd[&(m, *idx)]);
                    }
                    st.extend(&k.succ);
                }
            }
        }
    }
    for r in 0..f.p.np {
        if let Some(ds) = regdefs.get(&(r as Reg)) {
            for &d in ds {
                union(&mut uf, r, d);
            }
        }
    }
    let mut ws = BTreeMap::new();
    let mut defweb = vec![];
    for d in &defs {
        let root = find(&mut uf, d.id);
        let w = *ws.entry(root).or_insert_with(|| {
            let w = ctx.web("var", Some(f.id), f.pid);
            f.webs.push(w);
            w
        });
        ctx.webs[w].defs.push(d.clone());
        defweb.push(w);
    }
    for (key, ds) in &usedefs {
        let w = defweb[*ds.first().unwrap()];
        ctx.webs[w].uses.push(*key);
        f.use_web.insert(*key, w);
    }
    for (key, d) in &nd {
        f.def_web.insert(*key, defweb[*d]);
    }
    for (i, &w) in defweb.iter().take(f.p.np).enumerate() {
        ctx.webs[w].kind = "param";
        ctx.webs[w].param_index = i + 1;
        f.params.push(w);
    }
    for (id, n) in f.nodes.iter().enumerate() {
        if matches!(n.kind.as_str(), "NEWCLOSURE" | "DUPCLOSURE") {
            for (t, idx) in &n.a.caps {
                if matches!(t, 0 | 1) {
                    ctx.webs[f.use_web[&(id, *idx)]].captured = true;
                }
            }
        }
        if matches!(n.kind.as_str(), "FORNPREP" | "FORNLOOP") {
            ctx.webs[f.def_web[&(id, n.a.a + 2)]].kind = "loop";
        }
        if n.kind == "FORGLOOP" {
            for i in 0..(n.a.aux & 255) {
                ctx.webs[f.def_web[&(id, n.a.a + 3 + i)]].kind = "loop";
            }
        }
    }
    f.defs = defs;
    f.node_defs = nd;
    f.read_defs = usedefs;
    Ok(())
}
pub fn fmt_num(v: f64) -> String {
    if v.is_nan() {
        return "(0/0)".into();
    }
    if v.is_infinite() {
        return if v > 0.0 { "math.huge" } else { "-math.huge" }.into();
    }
    if v == 0.0 {
        return "0".into();
    }
    if v.fract() == 0.0 && v.abs() < 9007199254740992.0 {
        return format!("{v:.0}");
    }
    let s = format!("{v:?}");
    if let Some((mantissa, exponent)) = s.split_once('e') {
        if let Ok(exp) = exponent.parse::<i32>() {
            return format!("{mantissa}e{exp:+03}");
        }
    }
    s
}
pub fn lua_str(b: &[u8]) -> String {
    let mut out = String::from("\"");
    let mut push = |c: char| match c {
        '"' => out.push_str("\\\""),
        '\\' => out.push_str("\\\\"),
        '\n' => out.push_str("\\n"),
        '\r' => out.push_str("\\r"),
        '\t' => out.push_str("\\t"),
        c if (c as u32) < 32 || c == '\u{7f}' => out.push_str(&format!("\\{:03}", c as u32)),
        c => out.push(c),
    };
    if let Ok(s) = std::str::from_utf8(b) {
        for c in s.chars() {
            push(c);
        }
    } else {
        for &c in b {
            if c >= 128 {
                out.push_str(&format!("\\{c:03}"));
            } else {
                match c {
                    34 => out.push_str("\\\""),
                    92 => out.push_str("\\\\"),
                    10 => out.push_str("\\n"),
                    13 => out.push_str("\\r"),
                    9 => out.push_str("\\t"),
                    32..=126 => out.push(char::from(c)),
                    _ => out.push_str(&format!("\\{c:03}")),
                }
            }
        }
    }
    out.push('"');
    out
}

//! Recognize a single false-seeded accumulator, following actual successors and register values.
use super::*;

#[derive(Debug, PartialEq, Eq)]
struct Chain {
    effects: Vec<String>,
    results: Vec<String>,
}

// Each call result is an arbitrary Luau value. A trace prefix also fixes the first error,
// since both sides read the same cells and call the same values with the same arguments.
fn chain(
    f: &Function,
    start: usize,
    exit: usize,
    acc: i64,
    original: bool,
) -> Option<(Chain, BTreeSet<usize>)> {
    let mut trace = Chain {
        effects: vec![],
        results: vec![],
    };
    let mut seen = BTreeSet::new();
    let mut regs = BTreeMap::<i64, String>::new();
    let mut pc = start;
    let mut joins = vec![];
    let mut last = String::new();
    let read = |regs: &BTreeMap<i64, String>, r: i64| {
        regs.get(&r)
            .cloned()
            .or_else(|| (0..acc).contains(&r).then(|| format!("R{r}")))
    };
    if original {
        let seed = f.nodes.get(pc)?;
        if seed.kind != "LOADB"
            || (seed.a.a, seed.a.b, seed.a.c) != (acc, 0, 0)
            || seed.succ.len() != 1
        {
            return None;
        }
        seen.insert(pc);
        pc = seed.succ[0];
        let guard = f.nodes.get(pc)?;
        if guard.kind != "JUMPIF" || guard.a.a != acc || guard.succ.len() != 2 {
            return None;
        }
        seen.insert(pc);
        joins.push(guard.succ[1]);
        pc = guard.succ[0];
    }
    let mut body_effects = trace.effects.len();
    while pc != exit {
        if !seen.insert(pc) || seen.len() > 96 {
            return None;
        }
        let n = f.nodes.get(pc)?;
        let a = &n.a;
        if n.kind == "JUMPIF" {
            if n.succ.len() != 2
                || a.a != acc
                || regs.get(&acc) != Some(&last)
                || trace.effects.len() == body_effects
                || last.is_empty()
            {
                return None;
            }
            if original {
                if joins.last() != Some(&pc) {
                    return None;
                }
                joins.push(n.succ[1]);
            } else if n.succ[1] != exit {
                return None;
            }
            trace.results.push(std::mem::take(&mut last));
            regs.clear();
            body_effects = trace.effects.len();
            pc = n.succ[0];
            continue;
        }
        if n.succ.len() != 1 || !(acc..=31).contains(&a.a) {
            return None;
        }
        let value = match n.kind.as_str() {
            "GETUPVAL" if (0..f.p.nup as i64).contains(&a.b) => {
                let id = format!("u{}@{}", a.b, trace.effects.len());
                trace.effects.push(id.clone());
                id
            }
            "MOVE" => read(&regs, a.b)?,
            "OR" => format!("or({},{})", read(&regs, a.b)?, read(&regs, a.c)?),
            "CALL" if (1..=4).contains(&a.b) && a.c == 2 => {
                let inputs = (a.a..a.a + a.b)
                    .map(|r| read(&regs, r))
                    .collect::<Option<Vec<_>>>()?;
                last = format!("result@{}", trace.effects.len());
                trace.effects.push(format!("call({})", inputs.join(",")));
                last.clone()
            }
            _ => return None,
        };
        regs.insert(a.a, value);
        pc = n.succ[0];
    }
    if original && joins.last() != Some(&exit)
        || regs.get(&acc) != Some(&last)
        || last.is_empty()
        || trace.results.is_empty()
    {
        return None;
    }
    trace.results.push(last);
    if seen
        .iter()
        .any(|&i| i != start && f.nodes[i].preds.iter().any(|p| !seen.contains(p)))
    {
        return None;
    }
    Some((trace, seen))
}

fn operand(s: &str, prefix: char) -> Option<i64> {
    let n = s.strip_prefix(prefix)?.parse().ok()?;
    (0..=255).contains(&n).then_some(n)
}

// Only lower the instructions used inside this proof. Other instructions stay opaque.
fn compiled_flow(lines: &[String], original: &Function) -> Option<Function> {
    let g = compiler_nodes(lines).ok()?;
    let mut labels = HashSet::new();
    for l in lines {
        if let Some((label, _)) = l
            .split_once(": ")
            .filter(|(k, _)| k.starts_with('L') && k[1..].bytes().all(|b| b.is_ascii_digit()))
        {
            if !labels.insert(label) {
                return None;
            }
        }
    }
    let mut f = Function::new(0, 0, original.p.clone(), vec![]);
    for (i, line) in lines.iter().enumerate() {
        let words: Vec<_> = instruction(line).split_whitespace().collect();
        let mut a = crate::ir::Args::default();
        let kind = match words.as_slice() {
            ["GETUPVAL", r, u] => {
                a.a = operand(r, 'R')?;
                a.b = operand(u, 'U')?;
                "GETUPVAL"
            }
            ["GETTABLEKS", d, r, k, ..] => {
                a.a = operand(d, 'R')?;
                a.b = operand(r, 'R')?;
                a.aux = operand(k, 'K')?;
                "GETTABLEKS"
            }
            ["MOVE", r, s] => {
                a.a = operand(r, 'R')?;
                a.b = operand(s, 'R')?;
                "MOVE"
            }
            ["OR", d, r, s] => {
                a.a = operand(d, 'R')?;
                a.b = operand(r, 'R')?;
                a.c = operand(s, 'R')?;
                "OR"
            }
            [op @ ("CALL" | "CALLFB"), r, b, c, rest @ ..]
                if (*op == "CALL" && rest.is_empty())
                    || (*op == "CALLFB"
                        && rest.len() == 1
                        && rest[0]
                            .strip_prefix('[')?
                            .strip_suffix(']')?
                            .parse::<u32>()
                            .is_ok()) =>
            {
                a.a = operand(r, 'R')?;
                a.b = b.parse::<i16>().ok()? as i64 + 1;
                a.c = c.parse::<i16>().ok()? as i64 + 1;
                "CALL"
            }
            [op @ ("JUMPIF" | "JUMPIFNOT"), r, _] => {
                a.a = operand(r, 'R')?;
                *op
            }
            [op @ ("GETVARARGS" | "RETURN"), r, b] => {
                a.a = operand(r, 'R')?;
                a.b = b.parse::<i16>().ok()? as i64 + 1;
                *op
            }
            _ => "opaque",
        };
        let n = &g.nodes[i];
        f.nodes.push(crate::ir::Node {
            kind: kind.into(),
            a,
            rid: i as i64,
            succ: n.next.into_iter().chain(n.alt).collect(),
            reads: vec![],
            writes: vec![],
            preds: vec![],
            blk: 0,
            topbase: None,
        });
    }
    for i in 0..f.nodes.len() {
        for dest in f.nodes[i].succ.clone() {
            f.nodes[dest].preds.push(i);
        }
    }
    Some(f)
}

// Compare every edge and operand that can feed the entry, including target and argument
// registers. Paths ending before the chain are still compared by the strict outer graph.
fn same_entry(f: &Function, cf: &Function, lines: &[String], seed: usize, start: usize) -> bool {
    fn ancestors(f: &Function, id: usize) -> BTreeSet<usize> {
        let mut todo = vec![id];
        let mut seen = BTreeSet::new();
        while let Some(i) = todo.pop() {
            if seen.insert(i) {
                todo.extend(&f.nodes[i].preds);
            }
        }
        seen
    }
    let (a, b) = (ancestors(f, seed), ancestors(cf, start));
    if a.len() != b.len()
        || a.len() > 48
        || f.nodes
            .iter()
            .any(|n| n.a.caps.iter().any(|&(mode, _)| mode == 1))
        || lines
            .iter()
            .any(|l| instruction(l).starts_with("CAPTURE REF "))
    {
        return false;
    }
    let mut pairs = BTreeMap::new();
    let mut todo = vec![(f.entry, cf.entry)];
    while let Some((i, j)) = todo.pop() {
        if let Some(previous) = pairs.insert(i, j) {
            if previous != j {
                return false;
            }
            continue;
        }
        if i == seed || j == start {
            if (i, j) != (seed, start) {
                return false;
            }
            continue;
        }
        let (n, m) = (&f.nodes[i], &cf.nodes[j]);
        let (x, y) = (&n.a, &m.a);
        if n.kind != m.kind || x.a != y.a || n.succ.len() != m.succ.len() {
            return false;
        }
        let same = match n.kind.as_str() {
            "GETUPVAL" | "MOVE" => x.b == y.b,
            "GETTABLEKS" => {
                x.b == y.b
                    && x.aux == y.aux
                    && f.p
                        .kbytes(x.aux)
                        .ok()
                        .is_some_and(|k| ann(&lines[j]) == Some(format!("'{k}'")))
            }
            "CALL" => x.b == y.b && x.c == y.c,
            "JUMPIF" | "JUMPIFNOT" => true,
            _ => false,
        };
        if !same {
            return false;
        }
        for (&s, &t) in n.succ.iter().zip(&m.succ) {
            if a.contains(&s) != b.contains(&t) {
                return false;
            }
            if a.contains(&s) {
                todo.push((s, t));
            }
        }
    }
    pairs.len() == a.len() && pairs.values().copied().collect::<BTreeSet<_>>() == b
}

fn one<I: Iterator>(mut iter: I) -> Option<I::Item> {
    let first = iter.next()?;
    iter.next().is_none().then_some(first)
}

fn region_graph(mut g: Graph, start: usize, exit: usize) -> Option<Canon> {
    *g.nodes.get_mut(start)? = Node {
        tok: Some("PROVED-REGION".into()),
        next: Some(exit),
        alt: None,
        cond: false,
    };
    Some(canon(&g, &["LN"]))
}

// The listing has one closure site per non-root function. At this arity and above, each
// referenced upvalue bound consumes a distinct site of that exact size (largest first).
fn capture_origin(
    program: &Value,
    pid: u64,
    compiled: &BTreeMap<u64, Vec<String>>,
    report: &Value,
    nup: usize,
) -> Option<Value> {
    let mut bounds = vec![];
    let mut sites = vec![];
    let mut all_sites = 0;
    for (&fid, lines) in compiled {
        let mut bound = 0;
        for (pc, l) in lines.iter().enumerate() {
            let w: Vec<_> = instruction(l).split_whitespace().collect();
            if let ["GETUPVAL" | "SETUPVAL", _, u] | ["CAPTURE", "UPVAL", u] = w.as_slice() {
                bound = bound.max(operand(u, 'U')? as usize + 1);
            }
            if matches!(w.first(), Some(&"NEWCLOSURE" | &"DUPCLOSURE")) {
                all_sites += 1;
                let caps: Vec<_> = lines[pc + 1..]
                    .iter()
                    .map(|s| instruction(s))
                    .take_while(|s| s.starts_with("CAPTURE "))
                    .collect();
                if caps.len() >= nup {
                    sites.push((caps.len(), fid, pc, caps));
                }
            }
        }
        if bound >= nup {
            bounds.push((bound, fid));
        }
    }
    bounds.sort_unstable();
    sites.sort_unstable();
    if all_sites + 1 != compiled.len()
        || bounds.iter().map(|v| v.0).ne(sites.iter().map(|v| v.0))
        || one(bounds.iter().filter(|v| v.0 == nup))?.1 != pid
    {
        return None;
    }
    let (_, parent, pc, caps) = one(sites.iter().filter(|s| s.0 == nup))?;
    if report["match"][parent.to_string()] != *parent {
        return None;
    }
    let pf = function(program, *parent)?;
    let site = one(pf
        .nodes
        .iter()
        .filter(|n| n.kind == "NEWCLOSURE" && n.a.child == pid as i64))?;
    let slot = format!("P{}", site.a.rf);
    if instruction(&compiled[parent][*pc])
        .split_whitespace()
        .last()
        != Some(&slot)
        || site.a.caps.len() != caps.len()
        || site
            .a
            .caps
            .iter()
            .zip(caps)
            .any(|(&(mode, i), s)| mode != 2 || *s != format!("CAPTURE UPVAL U{i}"))
    {
        return None;
    }
    Some(
        json!({"parent":parent,"record":site.rid,"compiler_pc":pc,"captures":caps,"arity_bounds":bounds}),
    )
}

pub(super) fn prove(
    program: &Value,
    pid: u64,
    compiled: &BTreeMap<u64, Vec<String>>,
    report: &Value,
) -> std::result::Result<Value, String> {
    let p = &program["protos"][pid.to_string()];
    let f = function(program, pid).ok_or("incomplete decoded operand or capture data")?;
    let seed = one(f
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| n.kind == "LOADB" && n.a.b == 0 && n.a.c == 0))
    .ok_or("no unique literal-false seed")?
    .0;
    let acc = f.nodes[seed].a.a;
    let lines = &compiled[&pid];
    let cf = compiled_flow(lines, &f).ok_or("unavailable compiler CFG")?;
    let (start, exit, c_exit, binding) = if program["main"] == pid && compiled.len() == 1 {
        let arg = &f.nodes[f.entry];
        let carg = cf.nodes.first().ok_or("empty compiled function")?;
        let exit = one(f
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| n.kind == "RETURN"))
        .ok_or("no unique original return")?
        .0;
        let end = one(cf
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| n.kind == "RETURN"))
        .ok_or("no unique compiled return")?
        .0;
        if f.p.np != 0
            || !f.p.va
            || f.p.nup != 0
            || !f.p.kids.is_empty()
            || arg.kind != "GETVARARGS"
            || carg.kind != arg.kind
            || (arg.a.a, arg.a.b) != (0, acc + 1)
            || (carg.a.a, carg.a.b) != (0, acc + 1)
            || arg.succ != [seed]
            || carg.succ.len() != 1
            || f.nodes[exit].a.a != acc
            || cf.nodes[end].a.a != acc
            || f.nodes[exit].a.b != 2
            || cf.nodes[end].a.b != 2
        {
            return Err("entry bindings, immediate seed or single returned value differ".into());
        }
        (
            carg.succ[0],
            exit,
            end,
            json!({"source":"fixed first varargs"}),
        )
    } else {
        // These anchors are the observed region, not evidence of equivalence. Inspect both
        // edges at entry and exit; all other paths must still pass the ordinary strict CFG check.
        let exit = one(f
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| n.rid == 304099927))
        .ok_or("original region exit not found")?
        .0;
        let (start, end) = (24, 57);
        let guard = &f.nodes[*one(f.nodes[seed].preds.iter()).ok_or("non-unique seed entry")?];
        if f.nodes[seed].rid != 304099439
            || acc != 3
            || guard.kind != "JUMPIFNOT"
            || guard.a.a != acc
            || guard.succ.first() != Some(&seed)
            || cf.nodes.get(start - 1).is_none_or(|n| {
                n.kind != "JUMPIFNOT" || n.a.a != acc || n.succ.first() != Some(&start)
            })
            || cf
                .nodes
                .get(end)
                .is_none_or(|n| n.kind != "JUMPIF" || n.a.a != acc)
            || f.nodes[exit].kind != "JUMPIF"
            || f.nodes[exit].a.a != acc
        {
            return Err("entry guard or accumulator exit differs".into());
        }
        if !same_entry(&f, &cf, lines, seed, start) {
            return Err(
                "incoming bindings, control flow or observable register cells differ".into(),
            );
        }
        let origin = capture_origin(program, pid, compiled, report, f.p.nup)
            .ok_or("unique compiler closure or parent upvalue bindings not established")?;
        (start, exit, end, origin)
    };
    let (a, sa) = chain(&f, seed, exit, acc, true)
        .ok_or("original accumulator has an unaccounted operand, effect or edge")?;
    let (b, sb) = chain(&cf, start, c_exit, acc, false)
        .ok_or("compiled accumulator has an unaccounted operand, effect or edge")?;
    if a != b {
        return Err("ordered reads, call targets, arguments, false/nil result or conditional evaluation differ".into());
    }
    if compiled.len() > 1 {
        // Only R0..R3 are live at this exit. Both lowered suffixes overwrite each scratch slot
        // before any path can read it. Multireturn and unknown instructions are rejected.
        let upper = sa
            .iter()
            .map(|&i| f.nodes[i].a.a)
            .chain(sb.iter().map(|&i| cf.nodes[i].a.a))
            .max()
            .unwrap_or(acc);
        scratch_dead(&f, &cf, lines, (exit, c_exit), acc, upper)?;
    }
    let dead = dead_loads(p, program["protos"].as_object().unwrap()).map_err(|e| e.message)?;
    let ids = origins(p, &dead).ok_or("original node origins missing")?;
    let index = |rid| {
        one(ids
            .iter()
            .enumerate()
            .filter(|(_, id)| **id == rid)
            .map(|(i, _)| i))
    };
    let ga = region_graph(
        orig_nodes(p, &dead).map_err(|e| e.message)?,
        index(f.nodes[seed].rid as u64).ok_or("ambiguous seed record")?,
        index(f.nodes[exit].rid as u64).ok_or("ambiguous exit record")?,
    );
    let gb = region_graph(compiler_nodes(lines).map_err(|e| e.message)?, start, c_exit);
    if ga.is_none() || ga != gb {
        return Err("strict CFG still differs outside the proved region".into());
    }
    Ok(
        json!({"kind":"false-seed-short-circuit","record":f.nodes[seed].rid,"compiler_pc":start,"evidence":{"exit_record":f.nodes[exit].rid,"exit_pc":c_exit,"accumulator":acc,"ordered_effects":a.effects,"first_truthy_or_last":a.results,"original_nodes":sa.len(),"compiler_nodes":sb.len(),"bindings":binding}}),
    )
}

fn scratch_dead(
    f: &Function,
    cf: &Function,
    lines: &[String],
    (exit, c_exit): (usize, usize),
    acc: i64,
    upper: i64,
) -> std::result::Result<(), String> {
    let mut annotated = f.clone();
    crate::decomp::annotate(&mut annotated).map_err(|e| e.message)?;
    if upper > 31 || acc < 0 {
        return Err("scratch bound exceeded".into());
    }
    for (fun, at, compiled) in [(&annotated, exit, false), (cf, c_exit, true)] {
        let mask = (acc + 1..=upper).fold(0u32, |m, r| m | (1 << r));
        let mut todo = vec![(at, mask)];
        let mut visited = BTreeSet::new();
        while let Some((i, mut live)) = todo.pop() {
            if live == 0 || !visited.insert((i, live)) {
                continue;
            }
            let n = &fun.nodes[i];
            let (reads, writes) = if compiled {
                let words: Vec<_> = instruction(&lines[i]).split_whitespace().collect();
                match words.as_slice() {
                    ["RETURN", _, "0"] => continue,
                    ["JUMPIF" | "JUMPIFNOT", r, _] => (vec![operand(r, 'R')], vec![]),
                    ["GETUPVAL" | "LOADK", r, ..] => (vec![], vec![operand(r, 'R')]),
                    ["GETTABLEKS", d, r, ..] => (vec![operand(r, 'R')], vec![operand(d, 'R')]),
                    ["SETTABLEKS", d, r, ..] => (vec![operand(d, 'R'), operand(r, 'R')], vec![]),
                    ["OR", d, l, r] => (
                        vec![operand(l, 'R'), operand(r, 'R')],
                        vec![operand(d, 'R')],
                    ),
                    ["CALLFB", r, "0", "1", _] => (vec![operand(r, 'R')], vec![operand(r, 'R')]),
                    _ => return Err(format!("scratch liveness unknown at compiler pc {i}")),
                }
            } else {
                (
                    n.reads.iter().copied().map(Some).collect(),
                    n.writes.iter().copied().map(Some).collect(),
                )
            };
            for r in reads {
                let r = r.ok_or("invalid read register")?;
                if (0..32).contains(&r) && live & (1 << r) != 0 {
                    return Err(format!("scratch R{r} live at exit successor {}", n.rid));
                }
            }
            for r in writes {
                let r = r.ok_or("invalid written register")?;
                if (0..32).contains(&r) {
                    live &= !(1 << r);
                }
            }
            todo.extend(n.succ.iter().map(|&s| (s, live)));
        }
    }
    Ok(())
}

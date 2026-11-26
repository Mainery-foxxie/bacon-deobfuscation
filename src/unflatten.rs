//! Follow private constant dispatchers only after proving every transition and selector use.
use crate::error::Result;
use crate::source_cleanup::{
    apply, arr, id, kind, lkey, local_key, state_integer, text, unparen, walk, Bindings, Edit, Src,
};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};

fn assigns(n: &Value, selector: &str) -> bool {
    kind(n) == "AstStatAssign"
        && arr(n, "vars")
            .iter()
            .any(|v| local_key(v).as_deref() == Some(selector))
}
fn writes(n: &Value, selector: &str) -> bool {
    assigns(n, selector)
        || kind(n) == "AstStatCompoundAssign" && local_key(&n["var"]).as_deref() == Some(selector)
}
fn selector_xor_mask(n: &Value, selector: &str, b: &Bindings<'_>) -> Option<u32> {
    let n = unparen(n);
    if local_key(n).as_deref() == Some(selector) {
        return Some(0);
    }
    if kind(n) != "AstExprCall" || n["self"] == true || !b.is_path(&n["func"], &["bit32", "bxor"]) {
        return None;
    }
    let args = arr(n, "args");
    if args.len() < 2 {
        return None;
    }
    let mut found = false;
    let mut mask = 0u32;
    for arg in args {
        if local_key(arg).as_deref() == Some(selector) {
            if found {
                return None;
            }
            found = true;
        } else {
            mask ^= u32::try_from(state_integer(arg, b, 0)?).ok()?;
        }
    }
    found.then_some(mask)
}

fn logical_state(encoded: i64, mask: u32) -> Option<i64> {
    if mask == 0 {
        Some(encoded)
    } else {
        Some(i64::from(u32::try_from(encoded).ok()? ^ mask))
    }
}

fn state_case(n: &Value, selector: &str, b: &Bindings<'_>) -> Option<(u32, i64)> {
    let n = unparen(n);
    if kind(n) != "AstExprBinary" || text(n, "op") != "CompareEq" {
        return None;
    }
    let (state, value) = if selector_xor_mask(&n["left"], selector, b).is_some() {
        (&n["left"], &n["right"])
    } else if selector_xor_mask(&n["right"], selector, b).is_some() {
        (&n["right"], &n["left"])
    } else {
        return None;
    };
    let mask = selector_xor_mask(state, selector, b)?;
    let expected = state_integer(value, b, 0)?;
    if kind(unparen(state)) == "AstExprCall" {
        u32::try_from(expected).ok()?;
    }
    Some((mask, expected))
}

fn state_transition(
    value: &Value,
    selector: &str,
    current: i64,
    mask: u32,
    b: &Bindings<'_>,
) -> Option<Option<i64>> {
    let value = unparen(value);
    if kind(value) == "AstExprConstantNil" {
        return Some(None);
    }
    if local_key(value).as_deref() == Some(selector) {
        return Some(Some(current));
    }
    if let Some(delta) = selector_xor_mask(value, selector, b) {
        return Some(Some(i64::from(u32::try_from(current).ok()? ^ delta)));
    }
    Some(Some(logical_state(state_integer(value, b, 0)?, mask)?))
}

fn arms<'a>(
    block: &'a Value,
    selector: &str,
    b: &Bindings<'_>,
) -> Option<(BTreeMap<i64, &'a Value>, u32)> {
    fn scan<'a>(
        st: &'a Value,
        selector: &str,
        b: &Bindings<'_>,
        following: bool,
        mask: &mut Option<(u32, bool)>,
        out: &mut BTreeMap<i64, &'a Value>,
    ) -> Option<()> {
        if kind(st) != "AstStatIf" {
            return None;
        }
        let (current_mask, state) = state_case(&st["condition"], selector, b)?;
        let condition = unparen(&st["condition"]);
        let xor =
            local_key(&condition["left"]).is_none() && local_key(&condition["right"]).is_none();
        // Only a terminal return prevents evaluation of a later sibling XOR guard.
        if following
            && xor
            && arr(&st["thenbody"], "body")
                .last()
                .is_none_or(|n| kind(n) != "AstStatReturn")
        {
            return None;
        }
        // Direct equality and XOR by zero have different signed-number domains.
        let current_mask = (current_mask, xor);
        if mask.is_some_and(|mask| mask != current_mask) {
            return None;
        }
        *mask = Some(current_mask);
        if out.insert(state, &st["thenbody"]).is_some() {
            return None;
        }
        let other = &st["elsebody"];
        if other.is_null() {
            return Some(());
        }
        match kind(other) {
            "AstStatIf" => scan(other, selector, b, following, mask, out),
            "AstStatBlock" => {
                let body = arr(other, "body");
                for (i, st) in body.iter().enumerate() {
                    scan(st, selector, b, following || i + 1 < body.len(), mask, out)?;
                }
                Some(())
            }
            _ => None,
        }
    }
    let body = arr(block, "body");
    if body.is_empty() {
        return None;
    }
    let mut out = BTreeMap::new();
    let mut mask = None;
    for (i, st) in body.iter().enumerate() {
        scan(st, selector, b, i + 1 < body.len(), &mut mask, &mut out)?;
    }
    Some((out, mask?.0))
}
fn targets_loop(n: &Value, loop_: &Value, parents: &HashMap<usize, Option<&Value>>) -> bool {
    let mut parent = parents.get(&id(n)).copied().flatten();
    while let Some(p) = parent {
        if id(p) == id(loop_) {
            return true;
        }
        if matches!(
            kind(p),
            "AstStatFor" | "AstStatForIn" | "AstStatWhile" | "AstStatRepeat" | "AstExprFunction"
        ) {
            return false;
        }
        parent = parents.get(&id(p)).copied().flatten();
    }
    false
}
pub(crate) fn dedent(snippet: &str) -> Vec<String> {
    let lines: Vec<_> = snippet.split('\n').collect();
    let prefixes: Vec<_> = lines
        .iter()
        .skip(1)
        .filter(|s| !s.trim().is_empty())
        .map(|s| &s[..s.len() - s.trim_start_matches([' ', '\t']).len()])
        .collect();
    let Some(mut common) = prefixes.first().copied() else {
        return lines.iter().map(|s| s.to_string()).collect();
    };
    for p in &prefixes[1..] {
        while !p.starts_with(common) {
            common = &common[..common.len() - 1];
        }
    }
    lines
        .iter()
        .enumerate()
        .map(|(i, l)| {
            if i == 0 {
                l.to_string()
            } else if l.trim().is_empty() {
                String::new()
            } else {
                l[common.len()..].to_string()
            }
        })
        .collect()
}
pub fn flatten_dispatchers(source: &str, root: &Value) -> Result<(String, usize)> {
    let src = Src::new(source);
    let nodes = walk(root);
    let parents: HashMap<_, _> = nodes.iter().map(|(n, p)| (id(n), *p)).collect();
    let bindings = Bindings::new(root);
    let mut edits: Vec<Edit> = vec![];
    let mut flattened = 0;
    for (loop_, parent) in &nodes {
        if kind(loop_) != "AstStatWhile" {
            continue;
        }
        if walk(loop_).iter().any(|(n, _)| {
            kind(n) == "AstExprConstantString" && src.slice(n).is_ok_and(|s| s.contains('\n'))
        }) {
            continue;
        }
        let Some(selector) = local_key(&loop_["condition"]) else {
            continue;
        };
        let Some(block) = parent.filter(|p| kind(p) == "AstStatBlock") else {
            continue;
        };
        let Some(index) = arr(block, "body").iter().position(|p| id(p) == id(loop_)) else {
            continue;
        };
        let (mut declaration, mut start) = (None, None);
        for previous in arr(block, "body")[..index].iter().rev() {
            if writes(previous, &selector) {
                break;
            }
            if kind(previous) == "AstStatLocal"
                && arr(previous, "vars").iter().any(|v| lkey(v) == selector)
            {
                if arr(previous, "vars").len() == 1 && arr(previous, "values").len() == 1 {
                    declaration = Some(previous);
                    start = state_integer(&previous["values"][0], &bindings, 0);
                }
                break;
            }
        }
        let (Some(declaration), Some(encoded_start)) = (declaration, start) else {
            continue;
        };
        let inside: HashSet<_> = walk(loop_).iter().map(|(n, _)| id(n)).collect();
        let refs: Vec<_> = nodes
            .iter()
            .filter(|(n, _)| kind(n) == "AstExprLocal" && lkey(&n["local"]) == selector)
            .map(|(n, _)| *n)
            .collect();
        if refs.iter().any(|n| !inside.contains(&id(n))) {
            continue;
        }
        let Some((arms, mask)) = arms(&loop_["body"], &selector, &bindings) else {
            continue;
        };
        let Some(start) = logical_state(encoded_start, mask) else {
            continue;
        };
        let mut exits = vec![];
        let mut resets = HashSet::new();
        for arm in arms.values() {
            let Some(last) = arr(arm, "body").last() else {
                continue;
            };
            if !assigns(last, &selector)
                || arr(last, "vars").len() != 1
                || arr(last, "values").len() != 1
                || kind(unparen(&last["values"][0])) != "AstExprConstantNil"
            {
                continue;
            }
            for (n, _) in walk(arm) {
                if kind(n) != "AstStatContinue" || !targets_loop(n, loop_, &parents) {
                    continue;
                }
                let Some(p) = parents.get(&id(n)).copied().flatten() else {
                    continue;
                };
                let body = arr(p, "body");
                if kind(p) != "AstStatBlock"
                    || body.len() < 2
                    || body.last().is_none_or(|last| id(last) != id(n))
                {
                    continue;
                }
                let reset = &body[body.len() - 2];
                if assigns(reset, &selector)
                    && arr(reset, "vars").len() == 1
                    && arr(reset, "values").len() == 1
                    && kind(unparen(&reset["values"][0])) == "AstExprConstantNil"
                {
                    resets.insert(id(reset));
                    exits.push((*arm, reset, n));
                }
            }
        }
        let mut path = vec![];
        let mut seen = HashSet::new();
        let mut current = start;
        let mut assignments = 0;
        let mut state_reads = 0;
        let mut valid = true;
        loop {
            if !seen.insert(current) {
                valid = false;
                break;
            }
            let Some(arm) = arms.get(&current) else {
                valid = false;
                break;
            };
            path.push(*arm);
            let body = arr(arm, "body");
            let Some(last) = body.last() else {
                valid = false;
                break;
            };
            let next = if kind(last) == "AstStatReturn" {
                None
            } else if assigns(last, &selector) {
                assignments += 1;
                if arr(last, "vars").len() != 1 || arr(last, "values").len() != 1 {
                    valid = false;
                    break;
                }
                state_reads += walk(&last["values"][0])
                    .iter()
                    .filter(|(n, _)| kind(n) == "AstExprLocal" && lkey(&n["local"]) == selector)
                    .count();
                let Some(next) =
                    state_transition(&last["values"][0], &selector, current, mask, &bindings)
                else {
                    valid = false;
                    break;
                };
                next
            } else {
                valid = false;
                break;
            };
            if body[..body.len() - 1]
                .iter()
                .flat_map(walk)
                .any(|(n, _)| assigns(n, &selector) && !resets.contains(&id(n)))
            {
                valid = false;
                break;
            }
            match next {
                Some(next) => current = next,
                None => break,
            }
        }
        // Only bodies on the proven path can observe or mutate the selector.
        let active_nodes: Vec<_> = path.iter().flat_map(|arm| walk(arm)).collect();
        assignments += active_nodes
            .iter()
            .filter(|(n, _)| resets.contains(&id(n)))
            .count();
        let active_refs = active_nodes
            .iter()
            .filter(|(n, _)| kind(n) == "AstExprLocal" && lkey(&n["local"]) == selector)
            .count();
        let allowed: HashSet<_> = exits.iter().map(|(_, _, n)| id(n)).collect();
        if !valid
            || active_nodes.iter().any(|(n, _)| {
                matches!(kind(n), "AstStatBreak" | "AstStatContinue")
                    && !allowed.contains(&id(n))
                    && targets_loop(n, loop_, &parents)
            })
            || active_refs != assignments + state_reads
        {
            continue;
        }
        let (a, z) = src.span(loop_)?;
        let line = source[..a].rfind('\n').map_or(0, |x| x + 1);
        let indent = &source[line..a];
        if !indent.trim().is_empty() {
            continue;
        }
        let mut lines = vec![];
        for arm in path {
            let mut statements = arr(arm, "body");
            if statements.last().is_some_and(|n| assigns(n, &selector)) {
                statements = &statements[..statements.len() - 1];
            }
            if statements.is_empty() {
                continue;
            }
            let first = src.span(&statements[0])?.0;
            let last = src.span(statements.last().unwrap())?.1;
            let mut replacements = vec![];
            let arm_exits: Vec<_> = exits
                .iter()
                .filter(|(owner, _, _)| id(owner) == id(arm))
                .collect();
            for (_, reset, n) in &arm_exits {
                let (r, s) = src.span(reset)?;
                let (c, d) = src.span(n)?;
                replacements.push((r - first, s - first, String::new()));
                replacements.push((c - first, d - first, "break".to_string()));
            }
            let content = apply(&source[first..last], replacements)?;
            let content = dedent(&content);
            let declares = statements
                .iter()
                .any(|s| matches!(kind(s), "AstStatLocal" | "AstStatLocalFunction"));
            let scoped = declares
                || !arm_exits.is_empty()
                || statements
                    .last()
                    .is_some_and(|s| kind(s) == "AstStatReturn");
            let base = if scoped {
                format!("{indent}\t")
            } else {
                indent.to_owned()
            };
            if !arm_exits.is_empty() {
                lines.push(format!("{indent}repeat"));
            } else if scoped {
                lines.push(format!("{indent}do"));
            }
            for (i, line) in content.iter().enumerate() {
                lines.push(if i == 0 || !line.trim().is_empty() {
                    format!("{base}{line}")
                } else {
                    String::new()
                });
            }
            if !arm_exits.is_empty() {
                lines.push(format!("{indent}until true"));
            } else if scoped {
                lines.push(format!("{indent}end"));
            }
        }
        if lines.is_empty() {
            continue;
        }
        if lines[0].starts_with(indent) {
            lines[0] = lines[0][indent.len()..].to_owned();
        }
        edits.push((a, z, lines.join("\n")));
        let (a, z) = src.span(declaration)?;
        edits.push((a, z, String::new()));
        flattened += 1;
    }
    // Reference selection is by ascending span, keeping the enclosing dispatcher.
    edits.sort();
    edits.dedup();
    let mut kept: Vec<Edit> = vec![];
    for edit in edits {
        if kept.last().is_some_and(|p| edit.0 < p.1) {
            continue;
        }
        kept.push(edit);
    }
    Ok((apply(source, kept)?, flattened))
}

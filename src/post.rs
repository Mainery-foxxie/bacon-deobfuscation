//! Value/boolean folding, safe inlining, constructors and lexical declaration repair.
use crate::error::Result;
use crate::gen::{has_call, negate};
use crate::ir::*;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

pub fn refs(x: &E, w: WebId) -> bool {
    match x.as_ref() {
        Expr::Var(v) => *v == w,
        Expr::Func { caps, .. } => caps.contains(&w),
        _ => children(x).iter().any(|x| refs(x, w)),
    }
}
pub fn count_var(x: &E, w: WebId) -> usize {
    match x.as_ref() {
        Expr::Var(v) => usize::from(*v == w),
        Expr::Func { .. } => 0,
        _ => children(x).iter().map(|x| count_var(x, w)).sum(),
    }
}
pub fn map_expr(x: &E, f: &mut impl FnMut(&E) -> Option<E>) -> E {
    if let Some(r) = f(x) {
        return r;
    }
    match x.as_ref() {
        Expr::Idx(o, k) => idx(
            map_expr(o, f),
            match k {
                Key::Str(s) => Key::Str(s.clone()),
                Key::Expr(k) => Key::Expr(map_expr(k, f)),
            },
        ),
        Expr::Call { func, args, method } => e(Expr::Call {
            func: map_expr(func, f),
            args: args.iter().map(|x| map_expr(x, f)).collect(),
            method: method.clone(),
        }),
        Expr::Bin(op, l, r) => bin(op, map_expr(l, f), map_expr(r, f)),
        Expr::Not(x) => not(map_expr(x, f)),
        Expr::Len(x) => e(Expr::Len(map_expr(x, f))),
        Expr::Neg(x) => e(Expr::Neg(map_expr(x, f))),
        Expr::Multi(x) => multi(map_expr(x, f)),
        Expr::Paren(x) => paren(map_expr(x, f)),
        Expr::Conditional(c, y, n) => e(Expr::Conditional(
            map_expr(c, f),
            map_expr(y, f),
            map_expr(n, f),
        )),
        Expr::Table(t) => e(Expr::Table(Table {
            items: t.items.iter().map(|x| map_expr(x, f)).collect(),
            fields: t
                .fields
                .iter()
                .map(|(k, v)| (k.clone(), v.as_ref().map(|x| map_expr(x, f))))
                .collect(),
            pending: t.pending.clone(),
            fromnew: t.fromnew,
        })),
        _ => x.clone(),
    }
}
pub fn is_truthy_const(x: &E) -> bool {
    matches!(x.as_ref(), Expr::Str(_) | Expr::Num(..) | Expr::Bool(true))
}
pub fn is_boolish(x: &E) -> bool {
    match x.as_ref() {
        Expr::Not(_) | Expr::Bool(_) => true,
        Expr::Bin(op, l, r) => {
            matches!(op.as_str(), "==" | "~=" | "<" | "<=" | ">" | ">=")
                || matches!(op.as_str(), "and" | "or") && is_boolish(l) && is_boolish(r)
        }
        _ => false,
    }
}
pub fn target(s: &Stmt) -> Option<WebId> {
    match s {
        Stmt::Local(w, Some(_)) => Some(*w),
        Stmt::Assign(t, _) => {
            if let Expr::Var(w) = t.as_ref() {
                Some(*w)
            } else {
                None
            }
        }
        _ => None,
    }
}
fn value(s: &Stmt) -> Option<E> {
    match s {
        Stmt::Local(_, x) => x.clone(),
        Stmt::Assign(_, x) => Some(x.clone()),
        _ => None,
    }
}
fn mk_set(w: WebId, x: E, declare: bool) -> Stmt {
    if declare {
        Stmt::Local(w, Some(x))
    } else {
        Stmt::Assign(var(w), x)
    }
}
fn stable_simple(x: &E) -> bool {
    match x.as_ref() {
        Expr::Str(_) | Expr::Num(..) | Expr::Bool(_) | Expr::Nil | Expr::Imp(_) | Expr::Var(_) => {
            true
        }
        Expr::Idx(o, k) => {
            (matches!(k, Key::Str(_)) || matches!(k,Key::Expr(k)if k.is_const()))
                && stable_simple(o)
        }
        _ => false,
    }
}
fn single_set(body: &[Stmt], w: Option<WebId>) -> Option<(WebId, E, bool)> {
    if body.len() != 1 {
        return None;
    }
    let x = target(&body[0])?;
    if w.is_some_and(|w| w != x) {
        return None;
    }
    Some((x, value(&body[0])?, matches!(body[0], Stmt::Local(..))))
}
pub fn flat(x: &E, op: &str) -> Vec<E> {
    if let Expr::Bin(k, l, r) = x.as_ref() {
        if k == op {
            return [flat(l, op), flat(r, op)].concat();
        }
    }
    vec![x.clone()]
}
fn chain(xs: &[E], op: &str) -> E {
    let mut x = xs[0].clone();
    for y in &xs[1..] {
        x = bin(op, x, y.clone());
    }
    x
}
fn try_and_or(s1: &Stmt, s2: &Stmt) -> Option<Stmt> {
    let w = target(s1)?;
    let (c, t, el) = if let Stmt::If(c, t, el) = s2 {
        (c, t, el)
    } else {
        return None;
    };
    if !el.is_empty() || t.len() != 1 {
        return None;
    }
    let b = if let Stmt::Assign(v, x) = &t[0] {
        if matches!(v.as_ref(),Expr::Var(v)if *v==w) {
            x.clone()
        } else {
            return None;
        }
    } else {
        return None;
    };
    let op = match c.as_ref() {
        Expr::Var(v) if *v == w => "and",
        Expr::Not(x) if matches!(x.as_ref(),Expr::Var(v)if *v==w) => "or",
        _ => return None,
    };
    let a = value(s1)?;
    let b = if refs(&b, w) {
        if !stable_simple(&a) || refs(&a, w) {
            return None;
        }
        map_expr(&b, &mut |x| {
            if matches!(x.as_ref(),Expr::Var(v)if *v==w) {
                Some(a.clone())
            } else {
                None
            }
        })
    } else {
        b
    };
    Some(mk_set(w, bin(op, a, b), matches!(s1, Stmt::Local(..))))
}
fn try_bool_acc(s1: &Stmt, s2: &Stmt) -> Option<Stmt> {
    let w = target(s1)?;
    let a = value(s1)?;
    let av = if let Expr::Bool(v) = a.as_ref() {
        *v
    } else {
        return None;
    };
    let (c, t, el) = if let Stmt::If(c, t, el) = s2 {
        (c, t, el)
    } else {
        return None;
    };
    if !el.is_empty() {
        return None;
    }
    let (_, d, _) = single_set(t, Some(w))?;
    if refs(&d, w) || refs(c, w) {
        return None;
    }
    let c = if av { negate(c.clone()) } else { c.clone() };
    if !is_boolish(&c) {
        return None;
    }
    Some(mk_set(
        w,
        bin(if av { "or" } else { "and" }, c, d),
        matches!(s1, Stmt::Local(..)),
    ))
}
fn try_ternary(s: &Stmt) -> Option<Stmt> {
    let (c, th, el) = if let Stmt::If(c, t, e) = s {
        (c, t, e)
    } else {
        return None;
    };
    for flip in [false, true] {
        let (a, b, cond) = if flip {
            (el, th, negate(c.clone()))
        } else {
            (th, el, c.clone())
        };
        if a.len() == 2 {
            if let Stmt::If(ic, it, ie) = &a[1] {
                if let Some(w) = target(&a[0]) {
                    if matches!(ic.as_ref(),Expr::Not(v)if matches!(v.as_ref(),Expr::Var(v)if *v==w))
                        && ie.is_empty()
                    {
                        if let (Some((_, v1, _)), Some((_, v2, d2))) =
                            (single_set(it, Some(w)), single_set(b, Some(w)))
                        {
                            if v1 == v2 && !refs(&v1, w) {
                                return Some(mk_set(
                                    w,
                                    bin("or", bin("and", cond.clone(), value(&a[0])?), v1),
                                    matches!(a[0], Stmt::Local(..)) || d2,
                                ));
                            }
                        }
                    }
                }
            }
        }
        let Some((w, av, da)) = single_set(a, None) else {
            continue;
        };
        let Some((_, bv, db)) = single_set(b, Some(w)) else {
            continue;
        };
        let declare = da || db;
        if let Expr::Bin(op, l, r) = av.as_ref() {
            if op == "or" && r == &bv && !refs(&bv, w) && !refs(&cond, w) && !refs(l, w) {
                let a = if bv.is_const() && !Rc::ptr_eq(r, &bv) {
                    av.clone()
                } else {
                    l.clone()
                };
                return Some(mk_set(
                    w,
                    bin("or", bin("and", cond.clone(), a), bv.clone()),
                    declare,
                ));
            }
        }
        let fa = flat(&av, "or");
        let fb = flat(&bv, "or");
        let k = fb.len();
        if fa.len() > k
            && fa[fa.len() - k..] == fb
            && !fa.iter().chain(fb.iter()).any(|x| refs(x, w))
            && !refs(&cond, w)
        {
            let mut xs = vec![bin("and", cond.clone(), chain(&fa[..fa.len() - k], "or"))];
            xs.extend(fb);
            return Some(mk_set(w, chain(&xs, "or"), declare));
        }
        let fb = flat(&bv, "and");
        if fb.len() > 1
            && fb.last() == Some(&av)
            && !fb.iter().any(|x| refs(x, w))
            && !refs(&av, w)
            && !refs(&cond, w)
        {
            return Some(mk_set(
                w,
                bin(
                    "and",
                    bin("or", cond.clone(), chain(&fb[..fb.len() - 1], "and")),
                    av.clone(),
                ),
                declare,
            ));
        }
        if is_truthy_const(&av) && !refs(&bv, w) && !refs(&cond, w) {
            return Some(mk_set(
                w,
                bin("or", bin("and", cond.clone(), av.clone()), bv.clone()),
                declare,
            ));
        }
        if let (Expr::Bool(a), Expr::Bool(b)) = (av.as_ref(), bv.as_ref()) {
            if a != b && is_boolish(&cond) && !refs(&cond, w) {
                return Some(mk_set(
                    w,
                    if *a {
                        cond.clone()
                    } else {
                        negate(cond.clone())
                    },
                    declare,
                ));
            }
        }
    }
    None
}
fn recurse(body: Body, mut f: impl FnMut(Body) -> Body) -> Body {
    body.into_iter()
        .map(|s| match s {
            Stmt::If(c, t, e) => Stmt::If(c, f(t), f(e)),
            Stmt::While(c, b) => Stmt::While(c, f(b)),
            Stmt::For(w, i, l, s, b) => Stmt::For(w, i, l, s, f(b)),
            Stmt::ForIn(w, x, b) => Stmt::ForIn(w, x, f(b)),
            s => s,
        })
        .collect()
}
pub fn fold_values(body: Body) -> Body {
    let mut body = recurse(body, fold_values);
    loop {
        let mut out = vec![];
        let mut i = 0;
        let mut changed = false;
        while i < body.len() {
            if i + 1 < body.len() {
                if let Some(r) = try_and_or(&body[i], &body[i + 1])
                    .or_else(|| try_bool_acc(&body[i], &body[i + 1]))
                {
                    out.push(r);
                    i += 2;
                    changed = true;
                    continue;
                }
            }
            if let Some(r) = try_ternary(&body[i]) {
                out.push(r);
                changed = true;
            } else {
                out.push(body[i].clone());
            }
            i += 1;
        }
        body = out;
        if !changed {
            return body;
        }
    }
}
pub fn count_uses(body: &[Stmt]) -> BTreeMap<WebId, usize> {
    fn ex(x: &E, cnt: &mut BTreeMap<WebId, usize>) {
        match x.as_ref() {
            Expr::Var(w) => *cnt.entry(*w).or_default() += 1,
            Expr::Func { caps, .. } => {
                for w in caps {
                    *cnt.entry(*w).or_default() += 2;
                }
            }
            _ => {
                for c in children(x) {
                    ex(&c, cnt)
                }
            }
        }
    }
    let mut cnt = BTreeMap::new();
    walk_stmts(body, &mut |s| {
        for x in all_heads(s) {
            ex(&x, &mut cnt);
        }
    });
    cnt
}
pub fn walk_stmts(body: &[Stmt], f: &mut impl FnMut(&Stmt)) {
    for s in body {
        f(s);
        match s {
            Stmt::If(_, t, e) => {
                walk_stmts(t, f);
                walk_stmts(e, f);
            }
            Stmt::While(_, b) | Stmt::For(_, _, _, _, b) | Stmt::ForIn(_, _, b) => walk_stmts(b, f),
            _ => {}
        }
    }
}
fn all_heads(s: &Stmt) -> Vec<E> {
    if let Stmt::Assign(t, x) = s {
        vec![t.clone(), x.clone()]
    } else if let Stmt::While(c, _) = s {
        vec![c.clone()]
    } else {
        head_exprs(s).unwrap_or_default()
    }
}
pub fn head_exprs(s: &Stmt) -> Option<Vec<E>> {
    Some(match s {
        Stmt::Assign(t, x) => {
            if let Expr::Idx(o, k) = t.as_ref() {
                let mut h = vec![o.clone()];
                if let Key::Expr(k) = k {
                    h.push(k.clone());
                }
                h.push(x.clone());
                h
            } else {
                vec![x.clone()]
            }
        }
        Stmt::Call(x) | Stmt::CallCond(x) | Stmt::LocalN(_, x) => vec![x.clone()],
        Stmt::Return(xs) | Stmt::ForIn(_, xs, _) => xs.clone(),
        Stmt::If(c, _, _) => vec![c.clone()],
        Stmt::Local(_, x) => x.iter().cloned().collect(),
        Stmt::For(_, i, l, s, _) => vec![i.clone(), l.clone(), s.clone()],
        _ => return None,
    })
}
fn rebuild_head(s: &Stmt, h: Vec<E>) -> Stmt {
    match s {
        Stmt::Assign(t, _) => {
            if let Expr::Idx(_, k) = t.as_ref() {
                let (key, n) = match k {
                    Key::Str(s) => (Key::Str(s.clone()), 1),
                    Key::Expr(_) => (Key::Expr(h[1].clone()), 2),
                };
                Stmt::Assign(idx(h[0].clone(), key), h[n].clone())
            } else {
                Stmt::Assign(t.clone(), h[0].clone())
            }
        }
        Stmt::Call(_) => Stmt::Call(h[0].clone()),
        Stmt::CallCond(_) => Stmt::CallCond(h[0].clone()),
        Stmt::Return(_) => Stmt::Return(h),
        Stmt::If(_, t, e) => Stmt::If(h[0].clone(), t.clone(), e.clone()),
        Stmt::Local(w, _) => Stmt::Local(*w, h.first().cloned()),
        Stmt::LocalN(w, _) => Stmt::LocalN(w.clone(), h[0].clone()),
        Stmt::ForIn(w, _, b) => Stmt::ForIn(w.clone(), h, b.clone()),
        Stmt::For(w, _, _, _, b) => {
            Stmt::For(*w, h[0].clone(), h[1].clone(), h[2].clone(), b.clone())
        }
        _ => s.clone(),
    }
}
fn condlike(x: &E) -> bool {
    matches!(x.as_ref(), Expr::Not(_) | Expr::Conditional(..))
        || matches!(x.as_ref(),Expr::Bin(op,_,_)if matches!(op.as_str(),"and"|"or"|"=="|"~="|"<"|"<="|">"|">="))
}
struct Inline<'a> {
    web: WebId,
    repl: &'a E,
    ctx: &'a Context,
    owner: Option<usize>,
    eff: bool,
    reads: bool,
    done: bool,
    bad: bool,
    rc: bool,
    simple: bool,
    rcond: bool,
}
impl Inline<'_> {
    fn visit(&mut self, x: &E, many: bool, cond: bool, tested: bool) -> E {
        if matches!(x.as_ref(),Expr::Var(w)if *w==self.web) {
            if (self.eff && !self.simple)
                || (self.rc && self.reads)
                || (cond && !self.simple)
                || (tested && self.rcond)
            {
                self.bad = true;
            }
            self.done = true;
            return if many && matches!(self.repl.as_ref(), Expr::Call { .. } | Expr::Multi(_)) {
                paren(self.repl.clone())
            } else {
                self.repl.clone()
            };
        }
        if matches!(self.repl.as_ref(), Expr::Conditional(..))
            && (matches!(x.as_ref(), Expr::Upv(_) | Expr::Imp(_))
                || matches!(x.as_ref(),Expr::Var(w)if self.ctx.webs[*w].captured||self.owner.is_some()&&self.ctx.webs[*w].owner!=self.owner))
        {
            self.eff = true;
        }
        match x.as_ref() {
            Expr::Str(_)
            | Expr::Num(..)
            | Expr::Bool(_)
            | Expr::Nil
            | Expr::Imp(_)
            | Expr::Var(_)
            | Expr::Upv(_)
            | Expr::Func { .. } => x.clone(),
            Expr::Call { func, args, method } => {
                let func = self.visit(func, false, cond, false);
                let args = args
                    .iter()
                    .enumerate()
                    .map(|(i, a)| self.visit(a, i + 1 == args.len(), cond, false))
                    .collect();
                self.eff = true;
                e(Expr::Call {
                    func,
                    args,
                    method: method.clone(),
                })
            }
            Expr::Idx(o, k) => {
                let o = self.visit(o, false, cond, false);
                let k = match k {
                    Key::Str(s) => Key::Str(s.clone()),
                    Key::Expr(k) => Key::Expr(self.visit(k, false, cond, false)),
                };
                self.reads = true;
                self.eff = true;
                idx(o, k)
            }
            Expr::Conditional(c, y, n) => {
                let c = self.visit(c, false, cond, true);
                let y = self.visit(y, false, true, false);
                let n = self.visit(n, false, true, false);
                e(Expr::Conditional(c, y, n))
            }
            Expr::Bin(op, l, r) if matches!(op.as_str(), "and" | "or") => {
                let l = self.visit(l, false, cond, tested);
                let r = self.visit(r, false, true, tested);
                bin(op, l, r)
            }
            Expr::Not(x) => not(self.visit(x, false, cond, tested)),
            Expr::Table(t) => {
                let fields = t
                    .fields
                    .iter()
                    .map(|(k, v)| {
                        (
                            k.clone(),
                            v.as_ref().map(|v| self.visit(v, false, cond, false)),
                        )
                    })
                    .collect();
                let items = t
                    .items
                    .iter()
                    .enumerate()
                    .map(|(i, x)| self.visit(x, i + 1 == t.items.len(), cond, false))
                    .collect();
                e(Expr::Table(Table {
                    fields,
                    items,
                    pending: t.pending.clone(),
                    fromnew: t.fromnew,
                }))
            }
            Expr::Bin(op, l, r) => {
                let l = self.visit(l, false, cond, false);
                let r = self.visit(r, false, cond, false);
                self.eff = true;
                bin(op, l, r)
            }
            Expr::Len(x) => {
                let x = self.visit(x, false, cond, false);
                self.eff = true;
                e(Expr::Len(x))
            }
            Expr::Neg(x) => {
                let x = self.visit(x, false, cond, false);
                self.eff = true;
                e(Expr::Neg(x))
            }
            Expr::Multi(x) => multi(self.visit(x, false, cond, false)),
            Expr::Paren(x) => paren(self.visit(x, false, cond, false)),
            Expr::Vararg(_) => x.clone(),
        }
    }
}
pub fn inline_use(
    heads: &[E],
    w: WebId,
    repl: &E,
    multi_last: bool,
    test: bool,
    ctx: &Context,
    owner: Option<usize>,
) -> Option<Vec<E>> {
    let mut st = Inline {
        web: w,
        repl,
        ctx,
        owner,
        eff: false,
        reads: false,
        done: false,
        bad: false,
        rc: has_call(repl),
        simple: repl.is_const() || matches!(repl.as_ref(), Expr::Imp(_) | Expr::Var(_)),
        rcond: condlike(repl),
    };
    let out = heads
        .iter()
        .enumerate()
        .map(|(i, h)| st.visit(h, multi_last && i + 1 == heads.len(), false, test))
        .collect();
    if st.bad || !st.done {
        None
    } else {
        Some(out)
    }
}
fn inline_single_use(
    body: Body,
    cnt: &mut BTreeMap<WebId, usize>,
    ctx: &Context,
    owner: Option<usize>,
) -> Body {
    let mut body = recurse(body, |b| inline_single_use(b, cnt, ctx, owner));
    let mut i = 0;
    while i + 1 < body.len() {
        let s = &body[i];
        if let Stmt::If(c, t, e) = s {
            if let Some((w, y, _)) = single_set(t, None) {
                if let Some((_, n, _)) = single_set(e, Some(w)) {
                    let web = &ctx.webs[w];
                    let writes = usize::from(matches!(t[0], Stmt::Assign(..)))
                        + usize::from(matches!(e[0], Stmt::Assign(..)));
                    if web.kind == "var"
                        && !web.captured
                        && cnt.get(&w).copied().unwrap_or(0) == writes + 1
                        && ![c, &y, &n].iter().any(|x| refs(x, w))
                    {
                        if let Some(heads) = head_exprs(&body[i + 1]) {
                            if heads.iter().any(|x|matches!(x.as_ref(),Expr::Call{args,..}if args.iter().any(|a|refs(a,w)))){let replacement=e_cond(c.clone(),y,n);if let Some(changed)=inline_use(&heads,w,&replacement,matches!(body[i+1],Stmt::Return(_)|Stmt::ForIn(..)),false,ctx,owner){body[i+1]=rebuild_head(&body[i+1],changed);body.remove(i);cnt.insert(w,0);i=i.saturating_sub(1);continue;}}
                        }
                    }
                }
            }
        }
        if let Stmt::Local(w, Some(x)) = s {
            let web = &ctx.webs[*w];
            let ancestor =
                matches!(x.as_ref(),Expr::Var(b)if owner.is_some()&&ctx.webs[*b].owner!=owner);
            if web.kind == "var"
                && cnt.get(w) == Some(&1)
                && (!web.keep || ancestor)
                && !matches!(x.as_ref(), Expr::Func { .. } | Expr::Nil)
            {
                let next = &body[i + 1];
                if let Some(heads) = head_exprs(next) {
                    if heads.iter().any(|h| refs(h, *w)) {
                        if let Some(h) = inline_use(
                            &heads,
                            *w,
                            x,
                            matches!(next, Stmt::Return(_) | Stmt::ForIn(..)),
                            matches!(next, Stmt::If(..)),
                            ctx,
                            owner,
                        ) {
                            let w = *w;
                            body[i + 1] = rebuild_head(next, h);
                            body.remove(i);
                            cnt.insert(w, 0);
                            i = i.saturating_sub(1);
                            continue;
                        }
                    }
                }
            }
        }
        i += 1;
    }
    body
}
fn e_cond(c: E, y: E, n: E) -> E {
    e(Expr::Conditional(c, y, n))
}

pub fn fold_tables(body: Body, ctx: &Context) -> Body {
    let body = recurse(body, |b| fold_tables(b, ctx));
    let mut out = vec![];
    let mut i = 0;
    while i < body.len() {
        if let Stmt::Local(w, Some(x)) = &body[i] {
            if let Expr::Table(t) = x.as_ref() {
                if t.items.is_empty() && ctx.webs[*w].kind == "var" {
                    let mut added: Vec<(String, E)> = vec![];
                    let mut j = i + 1;
                    while j < body.len() {
                        let (k, v) = if let Stmt::Assign(t, v) = &body[j] {
                            if let Expr::Idx(o, k) = t.as_ref() {
                                if !matches!(o.as_ref(),Expr::Var(v)if v==w) {
                                    break;
                                }
                                (k, v)
                            } else {
                                break;
                            }
                        } else {
                            break;
                        };
                        let key = match k {
                            Key::Str(s) => s.clone(),
                            Key::Expr(k) => {
                                if let Expr::Str(b) = k.as_ref() {
                                    b.iter().copied().map(char::from).collect()
                                } else {
                                    break;
                                }
                            }
                        };
                        if refs(v, *w)
                            || t.fields.iter().any(|(k, v)| *k == key && v.is_some())
                            || added.iter().any(|(k, _)| *k == key)
                        {
                            break;
                        }
                        added.push((key, v.clone()));
                        j += 1;
                    }
                    if !added.is_empty() && (!t.fromnew || added.len() > 32) {
                        let mut fields: Vec<_> = t
                            .fields
                            .iter()
                            .filter(|(_, v)| v.is_some())
                            .cloned()
                            .collect();
                        fields.extend(added.into_iter().map(|(k, v)| (k, Some(v))));
                        out.push(Stmt::Local(
                            *w,
                            Some(e(Expr::Table(Table {
                                items: vec![],
                                fields,
                                pending: vec![],
                                fromnew: false,
                            }))),
                        ));
                        i = j;
                        continue;
                    }
                }
            }
        }
        out.push(body[i].clone());
        i += 1;
    }
    out
}
fn slot_store(s: &Stmt, w: WebId) -> Option<(f64, E)> {
    if let Stmt::Assign(t, v) = s {
        if let Expr::Idx(o, Key::Expr(k)) = t.as_ref() {
            if matches!(o.as_ref(),Expr::Var(v)if *v==w) {
                if let Expr::Num(n, true) = k.as_ref() {
                    return Some((*n, v.clone()));
                }
            }
        }
    }
    None
}
pub fn fold_lists(body: Body, ctx: &Context) -> Body {
    let mut body = recurse(body, |b| fold_lists(b, ctx));
    let mut i = 0;
    while i < body.len() {
        if let Stmt::Local(w, Some(x)) = &body[i] {
            let w = *w;
            if let Expr::Table(t) = x.as_ref() {
                if t.items.is_empty()
                    && t.fields.is_empty()
                    && t.pending.is_empty()
                    && ctx.webs[w].kind == "var"
                    && ctx.webs[w].defs.len() <= 1
                {
                    let mut j = i + 1;
                    while j < body.len() && slot_store(&body[j], w).is_none() {
                        if count_uses(&body[j..j + 1]).get(&w).is_some_and(|n| *n > 0) {
                            break;
                        }
                        j += 1;
                    }
                    let mut vals = vec![];
                    while j + vals.len() < body.len() {
                        let Some((n, v)) = slot_store(&body[j + vals.len()], w) else {
                            break;
                        };
                        if n != (vals.len() + 1) as f64 || refs(&v, w) {
                            break;
                        }
                        let is_multi = matches!(v.as_ref(), Expr::Multi(_));
                        vals.push(v);
                        if is_multi {
                            break;
                        }
                    }
                    if !vals.is_empty() {
                        let count = vals.len();
                        let mut replacement = body[i + 1..j].to_vec();
                        replacement.push(Stmt::Local(w, Some(e(Expr::Table(Table::new(vals))))));
                        body.splice(i..j + count, replacement);
                        continue;
                    }
                }
            }
        }
        i += 1;
    }
    body
}
pub fn check_open_lists(body: &[Stmt]) -> Result<()> {
    let mut bad = false;
    walk_stmts(body, &mut |s| {
        if matches!(s,Stmt::Assign(t,v)if matches!(t.as_ref(),Expr::Idx(_,Key::Expr(k))if matches!(k.as_ref(),Expr::Num(_,true)))&&matches!(v.as_ref(),Expr::Multi(_)))
        {
            bad = true;
        }
    });
    if bad {
        Err(err(
            "SETLIST of an open result list on a table that is not a constructor in progress",
        ))
    } else {
        Ok(())
    }
}
fn inline_aliases(body: &[Stmt], ctx: &Context, owner: Option<usize>) -> BTreeMap<WebId, WebId> {
    let mut assigned = BTreeSet::new();
    let mut capset = BTreeSet::new();
    fn caps(x: &E, c: &mut BTreeSet<WebId>) {
        if let Expr::Func { caps, .. } = x.as_ref() {
            c.extend(caps);
        } else {
            for x in children(x) {
                caps_rec(&x, c);
            }
        }
    }
    fn caps_rec(x: &E, c: &mut BTreeSet<WebId>) {
        caps(x, c)
    }
    walk_stmts(body, &mut |s| {
        if let Stmt::Assign(t, _) = s {
            if let Expr::Var(w) = t.as_ref() {
                assigned.insert(*w);
            }
        }
        for x in all_heads(s) {
            caps(&x, &mut capset);
        }
    });
    let mut subst = BTreeMap::new();
    walk_stmts(body, &mut |s| {
        if let Stmt::Local(a, Some(x)) = s {
            if let Expr::Var(b) = x.as_ref() {
                let av = &ctx.webs[*a];
                let bv = &ctx.webs[*b];
                if av.kind == "var"
                    && !assigned.contains(a)
                    && !assigned.contains(b)
                    && bv.defs.len() <= 1
                    && matches!(bv.kind, "var" | "param")
                    && !capset.contains(a)
                    && (owner.is_none() || bv.owner == owner)
                {
                    subst.insert(*a, *b);
                }
            }
        }
    });
    subst
}
fn map_statement(s: Stmt, f: &mut impl FnMut(&E) -> Option<E>) -> Stmt {
    match s {
        Stmt::Local(w, x) => Stmt::Local(w, x.map(|x| map_expr(&x, f))),
        Stmt::LocalN(w, x) => Stmt::LocalN(w, map_expr(&x, f)),
        Stmt::Assign(t, x) => Stmt::Assign(map_expr(&t, f), map_expr(&x, f)),
        Stmt::Call(x) => Stmt::Call(map_expr(&x, f)),
        Stmt::CallCond(x) => Stmt::CallCond(map_expr(&x, f)),
        Stmt::Return(xs) => Stmt::Return(xs.iter().map(|x| map_expr(x, f)).collect()),
        Stmt::If(c, t, e) => Stmt::If(
            map_expr(&c, f),
            t.into_iter().map(|s| map_statement(s, f)).collect(),
            e.into_iter().map(|s| map_statement(s, f)).collect(),
        ),
        Stmt::While(c, b) => Stmt::While(
            map_expr(&c, f),
            b.into_iter().map(|s| map_statement(s, f)).collect(),
        ),
        Stmt::For(w, i, l, s, b) => Stmt::For(
            w,
            map_expr(&i, f),
            map_expr(&l, f),
            map_expr(&s, f),
            b.into_iter().map(|s| map_statement(s, f)).collect(),
        ),
        Stmt::ForIn(w, x, b) => Stmt::ForIn(
            w,
            x.iter().map(|x| map_expr(x, f)).collect(),
            b.into_iter().map(|s| map_statement(s, f)).collect(),
        ),
        s => s,
    }
}
fn apply_subst(body: Body, sub: &BTreeMap<WebId, WebId>) -> Body {
    let body = recurse(body, |b| apply_subst(b, sub));
    body.into_iter()
        .filter_map(|s| {
            if matches!(&s, Stmt::Local(w, Some(x)) if sub.contains_key(w) && matches!(x.as_ref(), Expr::Var(_))) {
                return None;
            }
            Some(map_statement(s, &mut |x| {
                let Expr::Var(initial) = x.as_ref() else { return None; };
                let mut w = *initial;
                let mut seen = BTreeSet::new();
                while let Some(b) = sub.get(&w) {
                    if !seen.insert(w) { break; }
                    w = *b;
                }
                (w != *initial).then(|| var(w))
            }))
        })
        .collect()
}

type Path = Vec<(usize, u8)>;
#[derive(Clone)]
struct Occ {
    path: Path,
    index: usize,
    chain: Vec<(Path, usize)>,
}
#[derive(Default)]
struct Occs {
    decls: Vec<Occ>,
    uses: Vec<Occ>,
}
fn collect_occ(
    body: &[Stmt],
    path: &Path,
    chain: &[(Path, usize)],
    occ: &mut BTreeMap<WebId, Occs>,
) {
    for (i, s) in body.iter().enumerate() {
        let mut here = chain.to_vec();
        here.push((path.clone(), i));
        let loc = Occ {
            path: path.clone(),
            index: i,
            chain: here.clone(),
        };
        let selfw = if let Stmt::Local(w, Some(x)) = s {
            if matches!(x.as_ref(), Expr::Func { .. }) {
                Some(*w)
            } else {
                None
            }
        } else {
            None
        };
        fn expr(x: &E, ws: &mut Vec<WebId>, selfw: Option<WebId>) {
            match x.as_ref() {
                Expr::Var(w) => ws.push(*w),
                Expr::Func { caps, .. } => {
                    ws.extend(caps.iter().copied().filter(|w| Some(*w) != selfw))
                }
                _ => {
                    for x in children(x) {
                        expr(&x, ws, selfw)
                    }
                }
            }
        }
        let mut ws = vec![];
        for x in all_heads(s) {
            expr(&x, &mut ws, selfw);
        }
        let decls = match s {
            Stmt::Local(w, _) => vec![*w],
            Stmt::LocalN(ws, _) => ws.iter().flatten().copied().collect(),
            _ => vec![],
        };
        for w in decls {
            occ.entry(w).or_default().decls.push(loc.clone());
        }
        for w in ws {
            occ.entry(w).or_default().uses.push(loc.clone());
        }
        let subs: Vec<(u8, &Body)> = match s {
            Stmt::If(_, t, e) => vec![(0, t), (1, e)],
            Stmt::While(_, b) | Stmt::For(_, _, _, _, b) | Stmt::ForIn(_, _, b) => vec![(0, b)],
            _ => vec![],
        };
        for (arm, b) in subs {
            let mut subpath = path.clone();
            subpath.push((i, arm));
            collect_occ(b, &subpath, &here, occ);
        }
    }
}
fn body_mut<'a>(body: &'a mut Body, path: &[(usize, u8)]) -> Result<&'a mut Body> {
    if path.is_empty() {
        return Ok(body);
    }
    let (i, arm) = path[0];
    let s = body
        .get_mut(i)
        .ok_or_else(|| err("hoist: stale statement path"))?;
    let b = match s {
        Stmt::If(_, t, e) => {
            if arm == 0 {
                t
            } else {
                e
            }
        }
        Stmt::While(_, b) | Stmt::For(_, _, _, _, b) | Stmt::ForIn(_, _, b) => b,
        _ => return Err(err("hoist: invalid block path")),
    };
    body_mut(b, &path[1..])
}
pub fn hoist_decls(mut body: Body) -> Result<Body> {
    for _ in 0..500 {
        let mut occ = BTreeMap::new();
        collect_occ(&body, &vec![], &[], &mut occ);
        let mut fixed = false;
        for (w, o) in occ {
            if o.decls.is_empty() {
                continue;
            }
            let covers =
                |d: &Occ, u: &Occ| u.chain.iter().any(|(p, i)| *p == d.path && *i > d.index);
            let bad = o.decls.iter().any(|d| o.decls.iter().any(|u| covers(d, u)))
                || o.uses.iter().any(|u| !o.decls.iter().any(|d| covers(d, u)));
            if !bad {
                continue;
            }
            let chains: Vec<_> = o
                .decls
                .iter()
                .chain(o.uses.iter())
                .map(|o| &o.chain)
                .collect();
            let mut depth = 0;
            while chains.iter().all(|c| c.len() > depth)
                && chains.iter().all(|c| c[depth].0 == chains[0][depth].0)
            {
                depth += 1;
            }
            if depth == 0 {
                return Err(err("hoist: no common block"));
            }
            let path = chains[0][depth - 1].0.clone();
            let mut pos = chains.iter().map(|c| c[depth - 1].1).min().unwrap();
            let mut decls = o.decls.clone();
            decls.sort_by(|a, b| b.path.cmp(&a.path).then(b.index.cmp(&a.index)));
            for d in decls {
                let b = body_mut(&mut body, &d.path)?;
                match b[d.index].clone() {
                    Stmt::Local(w, Some(x)) => b[d.index] = Stmt::Assign(var(w), x),
                    Stmt::Local(_, None) => {
                        b.remove(d.index);
                        if d.path == path && d.index < pos {
                            pos -= 1;
                        }
                    }
                    _ => return Err(err("hoist of multi-assignment not supported")),
                }
            }
            body_mut(&mut body, &path)?.insert(pos, Stmt::Local(w, None));
            fixed = true;
            break;
        }
        if !fixed {
            return Ok(body);
        }
    }
    Err(err("hoist: declaration repair limit exceeded"))
}
pub fn post_passes(mut body: Body, ctx: &mut Context, owner: Option<usize>) -> Result<Body> {
    for _ in 0..6 {
        let before = body.clone();
        body = fold_values(body);
        body = fold_lists(body, ctx);
        body = fold_tables(body, ctx);
        let mut cnt = count_uses(&body);
        let sub = inline_aliases(&body, ctx, owner);
        if !sub.is_empty() {
            body = apply_subst(body, &sub);
            cnt = count_uses(&body);
        }
        body = inline_single_use(body, &mut cnt, ctx, owner);
        body = fold_tables(body, ctx);
        if body == before {
            break;
        }
    }
    body = fold_tables(body, ctx);
    body = fold_lists(body, ctx);
    check_open_lists(&body)?;
    hoist_decls(body)
}

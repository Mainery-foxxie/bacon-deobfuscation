//! Symbolic block evaluation, order-preserving temporary inlining and CFG structuring.
use crate::decomp::*;
use crate::error::Result;
use crate::ir::*;
use std::collections::{BTreeMap, BTreeSet};

pub fn negate(x: E) -> E {
    match x.as_ref() {
        Expr::Not(v) => v.clone(),
        Expr::Bin(op, l, r) if op == "==" => bin("~=", l.clone(), r.clone()),
        Expr::Bin(op, l, r) if op == "~=" => bin("==", l.clone(), r.clone()),
        _ => not(x),
    }
}
pub fn has_call(x: &E) -> bool {
    matches!(x.as_ref(), Expr::Call { .. }) || children(x).iter().any(has_call)
}
pub fn camel(s: &str) -> Option<String> {
    let mut s: String = s
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    if s.is_empty() {
        return None;
    }
    if s.as_bytes()[0].is_ascii_digit() {
        s.insert(0, '_');
    }
    let caps = s.bytes().take_while(u8::is_ascii_uppercase).count();
    if caps > 0 {
        let n = if caps == s.len() || caps == 1 {
            caps
        } else {
            caps - 1
        };
        s.replace_range(..n, &s[..n].to_ascii_lowercase());
    }
    Some(s)
}
fn hint(s: impl Into<String>) -> Option<Hint> {
    Some(Hint::Name(s.into()))
}
pub fn suggest(x: &E, ctx: &Context) -> Option<Hint> {
    match x.as_ref() {
        Expr::Call { func, args, method } => {
            let first = args.first().and_then(|x| {
                if let Expr::Str(b) = x.as_ref() {
                    Some(b.iter().copied().map(char::from).collect::<String>())
                } else {
                    None
                }
            });
            if let Some(m) = method {
                return match m.as_str() {
                    "GetService" => first.map(|s| {
                        Hint::Name(
                            s.chars()
                                .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
                                .collect(),
                        )
                    }),
                    "WaitForChild"
                    | "FindFirstChild"
                    | "FindFirstChildOfClass"
                    | "FindFirstChildWhichIsA"
                    | "FindFirstAncestor" => first.and_then(|s| camel(&s)).map(Hint::Name),
                    "Clone" => {
                        if let Expr::Var(w) = func.as_ref() {
                            Some(Hint::Clone(*w))
                        } else {
                            hint("clone")
                        }
                    }
                    "GetChildren" | "GetDescendants" => hint("children"),
                    "Connect" => hint("connection"),
                    _ => camel(m).map(Hint::Name),
                };
            }
            if args.len() == 1 && !matches!(func.as_ref(), Expr::Imp(_)) {
                if let Some(s) = first.as_ref() {
                    if service_names().contains(&s.as_str()) {
                        return hint(s.clone());
                    }
                }
            }
            if let Expr::Imp(p) = func.as_ref() {
                if p.as_slice() == ["Instance", "new"] {
                    if let Some(s) = first {
                        return camel(&s).map(Hint::Name);
                    }
                }
                if p.len() >= 2 {
                    let last = p.last()?;
                    let name = if matches!(
                        last.as_str(),
                        "new" | "fromRGB" | "fromScale" | "fromOffset" | "fromHSV"
                    ) && p[0] != "math"
                    {
                        &p[0]
                    } else {
                        last
                    };
                    return camel(name).map(Hint::Name);
                }
                return p.first().and_then(|s| camel(s)).map(Hint::Name);
            }
            None
        }
        Expr::Idx(_, Key::Str(k)) => camel(k).map(Hint::Name),
        Expr::Var(w) => ctx.webs[*w].hint.clone(),
        Expr::Bin(op, _, r) if op == "and" => suggest(r, ctx),
        Expr::Imp(p) => p.last().and_then(|s| camel(s)).map(Hint::Name),
        Expr::Func { .. } => hint("functionValue"),
        Expr::Table(_) => hint("items"),
        Expr::Bool(_) => hint("flag"),
        Expr::Num(..) => hint("number"),
        Expr::Str(_) => hint("text"),
        _ => None,
    }
}
fn service_names() -> Vec<&'static str> {
    "Players ReplicatedStorage ReplicatedFirst Workspace RunService VirtualUser VirtualInputManager TweenService UserInputService CollectionService BadgeService CoreGui HttpService Lighting StarterGui StarterPack StarterPlayer TeleportService MarketplaceService SoundService ContextActionService GuiService Debris ServerStorage ServerScriptService TextService TextChatService PathfindingService ProximityPromptService Stats GroupService LocalizationService PhysicsService NetworkClient ScriptContext SocialService UserService VoiceChatService AssetService ContentProvider InsertService KeyframeSequenceProvider LogService MaterialService Teams TestService".split_whitespace().collect()
}

pub struct Generator {
    pub ctx: Context,
    pub seq: usize,
    pub log: Vec<String>,
}
impl Generator {
    pub fn new(protos: BTreeMap<i64, Proto>) -> Self {
        Self {
            ctx: Context {
                protos,
                webs: vec![],
                next_fn: 0,
            },
            seq: 0,
            log: vec![],
        }
    }
    pub fn nseq(&mut self) -> usize {
        self.seq += 1;
        self.seq
    }
    pub fn run(&mut self, pid: i64, upvals: Vec<WebId>) -> Result<Function> {
        let p = self
            .ctx
            .protos
            .get(&pid)
            .ok_or_else(|| err(format!("missing prototype {pid}")))?
            .clone();
        if p.nup != upvals.len() {
            return Err(err("upvalue count mismatch"));
        }
        let id = self.ctx.next_fn;
        self.ctx.next_fn += 1;
        let mut f = Function::new(id, pid, p, upvals);
        prepare(&mut self.ctx, &mut f)?;
        for bi in 0..f.blocks.len() {
            let (mut eval, term) = Evaluator::new(self, &f, bi).run()?;
            f.blocks[bi].stmts.append(&mut eval);
            f.blocks[bi].term = term;
        }
        self.merge_conds(&mut f);
        let body = self.structure(&f)?;
        let body = simplify(body);
        let body = strip_tail_return(body);
        f.body = crate::post::post_passes(body, &mut self.ctx, Some(f.id))?;
        Ok(f)
    }
    pub fn merge_conds(&mut self, f: &mut Function) {
        loop {
            let mut changed = false;
            for bi in 0..f.blocks.len() {
                if f.blocks[bi].dead {
                    continue;
                }
                let (c1, t1, e1) = match f.blocks[bi].term.clone() {
                    Term::Cond(c, t, e) => (c, t, e),
                    _ => continue,
                };
                let p1 = negate(c1);
                for side in [true, false] {
                    let si = if side { t1 } else { e1 };
                    if si == bi || f.blocks[si].dead || f.blocks[si].pred != [bi] {
                        continue;
                    }
                    let (mut c2, t2, e2) = match f.blocks[si].term.clone() {
                        Term::Cond(c, t, e) => (c, t, e),
                        _ => continue,
                    };
                    if !f.blocks[si].stmts.is_empty() {
                        let st = &f.blocks[si].stmts;
                        if st.len() != 1 {
                            continue;
                        }
                        let (w, ex) = match &st[0] {
                            Stmt::Local(w, Some(ex)) => (*w, ex.clone()),
                            _ => continue,
                        };
                        let web = &self.ctx.webs[w];
                        if web.captured
                            || web.defs.len() != 1
                            || web.uses.len() != 1
                            || crate::post::count_var(&c2, w) != 1
                            || !pure_cond(&ex)
                        {
                            continue;
                        }
                        if let Some(h) = crate::post::inline_use(
                            &[c2],
                            w,
                            &ex,
                            false,
                            true,
                            &self.ctx,
                            Some(f.id),
                        ) {
                            c2 = h[0].clone();
                        } else {
                            continue;
                        }
                    }
                    let p2 = negate(c2);
                    let new = if side {
                        if t2 == e1 {
                            Some((bin("or", negate(p1.clone()), p2), e1, e2))
                        } else if e2 == e1 {
                            Some((bin("and", p1.clone(), p2), t2, e1))
                        } else {
                            None
                        }
                    } else if t2 == t1 {
                        Some((bin("or", p1.clone(), p2), t1, e2))
                    } else if e2 == t1 {
                        Some((bin("or", p1.clone(), negate(p2)), t1, t2))
                    } else {
                        None
                    };
                    if let Some((p, t, e)) = new {
                        f.blocks[bi].term = Term::Cond(negate(p), t, e);
                        f.blocks[si].dead = true;
                        for x in [t2, e2] {
                            let pred = &mut f.blocks[x].pred;
                            if let Some(pos) = pred.iter().position(|p| *p == si) {
                                if pred.contains(&bi) {
                                    pred.remove(pos);
                                } else {
                                    pred[pos] = bi;
                                }
                            }
                        }
                        changed = true;
                        break;
                    }
                }
            }
            if !changed {
                break;
            }
        }
    }
    pub fn structure(&mut self, f: &Function) -> Result<Body> {
        Structurer::new(f, &mut self.log)?.run()
    }
}
fn pure_cond(x: &E) -> bool {
    match x.as_ref() {
        Expr::Str(_)
        | Expr::Num(..)
        | Expr::Bool(_)
        | Expr::Nil
        | Expr::Imp(_)
        | Expr::Var(_)
        | Expr::Upv(_) => true,
        Expr::Idx(o, k) => {
            (matches!(k, Key::Str(_)) || matches!(k,Key::Expr(k) if k.is_const())) && pure_cond(o)
        }
        Expr::Bin(_, l, r) => pure_cond(l) && pure_cond(r),
        Expr::Not(x) => pure_cond(x),
        _ => false,
    }
}
#[derive(Clone)]
struct Pending {
    expr: E,
    web: Option<WebId>,
    leaves: Vec<usize>,
    kind: &'static str,
    link: Option<usize>,
    method: Option<String>,
}
struct Link {
    obj: E,
    var: Option<E>,
    m: usize,
    o: usize,
}
struct Evaluator<'a> {
    g: &'a mut Generator,
    f: &'a Function,
    bi: usize,
    regs: BTreeMap<Reg, usize>,
    ps: Vec<Pending>,
    links: Vec<Link>,
    stmts: Body,
    multi: Option<usize>,
    multi3: Option<usize>,
}
impl<'a> Evaluator<'a> {
    fn new(g: &'a mut Generator, f: &'a Function, bi: usize) -> Self {
        Self {
            g,
            f,
            bi,
            regs: BTreeMap::new(),
            ps: vec![],
            links: vec![],
            stmts: vec![],
            multi: None,
            multi3: None,
        }
    }
    fn p(&mut self, expr: E, web: Option<WebId>, leaves: Vec<usize>) -> usize {
        let id = self.ps.len();
        self.ps.push(Pending {
            expr,
            web,
            leaves,
            kind: "val",
            link: None,
            method: None,
        });
        id
    }
    fn ex(&self, p: usize) -> E {
        self.ps[p].expr.clone()
    }
    fn leaves(&self, ps: &[usize]) -> Vec<usize> {
        ps.iter()
            .flat_map(|p| self.ps[*p].leaves.iter().copied())
            .collect()
    }
    fn below_result(&mut self, w: WebId) -> bool {
        let uses: Vec<_> = self.g.ctx.webs[w]
            .uses
            .iter()
            .copied()
            .filter(|(id, _)| !matches!(self.f.nodes[*id].kind.as_str(), "JUMPIF" | "JUMPIFNOT"))
            .collect();
        if uses.len() == 1 {
            let (id, r) = uses[0];
            let u = &self.f.nodes[id];
            if result_first(&u.kind) && r < u.a.a {
                self.g.ctx.webs[w].keep = true;
                return true;
            }
        }
        false
    }
    fn inline_ok(&mut self, w: WebId) -> bool {
        let v = &self.g.ctx.webs[w];
        let ok = v.kind == "var"
            && !v.captured
            && v.defs.len() == 1
            && v.uses.len() == 1
            && !v.noinline
            && v.defs[0]
                .node
                .is_some_and(|id| self.f.nodes[id].blk == self.bi)
            && self.f.nodes[v.uses[0].0].blk == self.bi
            && {
                let u = &self.f.nodes[v.uses[0].0];
                !(u.kind == "MOVE" && u.a.a > u.a.b)
            };
        if ok {
            let (id, r) = v.uses[0];
            let u = &self.f.nodes[id];
            if (matches!(u.kind.as_str(), "SETTABLEKS" | "SETTABLE" | "SETTABLEN")
                && r == u.a.a
                && u.a.a < u.a.b)
                || (u.kind == "SELF" && r == u.a.b && u.a.b != u.a.a)
            {
                self.g.ctx.webs[w].keep = true;
                return false;
            }
            if self.below_result(w) {
                return false;
            }
        } else if v.kind == "var" && !v.captured && v.defs.len() > 1 {
            let real: Vec<_> = v
                .uses
                .iter()
                .copied()
                .filter(|(id, _)| {
                    !matches!(self.f.nodes[*id].kind.as_str(), "JUMPIF" | "JUMPIFNOT")
                })
                .collect();
            if real.len() == 1 {
                let (id, r) = real[0];
                let u = &self.f.nodes[id];
                if matches!(u.kind.as_str(), "SETTABLEKS" | "SETTABLE")
                    && r == u.a.a
                    && u.a.a < u.a.b
                {
                    self.g.ctx.webs[w].keep = true;
                }
            }
        }
        ok
    }
    fn mark_materialized(&mut self, w: WebId) {
        let v = &self.g.ctx.webs[w];
        if v.kind == "var"
            && v.defs.len() >= 2
            && !v.uses.is_empty()
            && v.defs.iter().any(|d| {
                d.node
                    .is_some_and(|id| matches!(self.f.nodes[id].kind.as_str(), "LOADB" | "MOVE"))
            })
            && v.uses.iter().all(|(id, _)| {
                let n = &self.f.nodes[*id];
                matches!(n.kind.as_str(), "JUMPIF" | "JUMPIFNOT")
                    || n.kind == "MOVE" && n.a.a > n.a.b
            })
        {
            self.g.ctx.webs[w].keep = true;
        }
    }
    fn rd(&mut self, n: usize, r: Reg) -> Result<usize> {
        let w = *self
            .f
            .use_web
            .get(&(n, r))
            .ok_or_else(|| err("missing register use web"))?;
        if let Some(&p) = self.regs.get(&r) {
            if self.ps[p].web == Some(w) {
                self.regs.remove(&r);
                return Ok(p);
            }
        }
        if self.inline_ok(w) {
            return Err(err(format!(
                "inline candidate web not pending: proto {} rec {} node {} R{r}",
                self.f.pid, self.f.nodes[n].rid, self.f.nodes[n].kind
            )));
        }
        Ok(self.p(var(w), None, vec![]))
    }
    fn tmp(&mut self, h: Option<Hint>) -> WebId {
        let w = self.g.ctx.web("tmp", Some(self.f.id), self.f.pid);
        self.g.ctx.webs[w].hint = h;
        w
    }
    fn materialize(&mut self, p: usize) -> Result<()> {
        if matches!(self.ps[p].kind, "selfm" | "selfo") {
            let lk = self.ps[p].link.unwrap();
            if self.links[lk].var.is_none() {
                let obj = self.links[lk].obj.clone();
                let w = self.tmp(suggest(&obj, &self.g.ctx));
                self.g.ctx.webs[w].declared = true;
                self.stmts.push(Stmt::Local(w, Some(obj)));
                self.links[lk].var = Some(var(w));
            }
            let v = self.links[lk].var.clone().unwrap();
            let o = self.links[lk].o;
            let m = self.links[lk].m;
            self.ps[o].expr = v.clone();
            self.ps[m].expr = idx(v, Key::Str(self.ps[m].method.clone().unwrap()));
            self.ps[o].leaves.clear();
            self.ps[m].leaves.clear();
            return Ok(());
        }
        if self.ps[p].kind == "multi" {
            return Err(err(format!("materialize multi in proto {}", self.f.pid)));
        }
        let w = if let Some(w) = self.ps[p].web {
            w
        } else {
            self.tmp(None)
        };
        if self.g.ctx.webs[w].hint.is_none() {
            self.g.ctx.webs[w].hint = suggest(&self.ex(p), &self.g.ctx);
        }
        self.g.ctx.webs[w].declared = true;
        self.g.ctx.webs[w].keep = true;
        self.stmts.push(Stmt::Local(w, Some(self.ex(p))));
        self.ps[p].expr = var(w);
        self.ps[p].leaves.clear();
        Ok(())
    }
    fn settle(&mut self, ops: &[usize], is_stmt: bool, ctor: bool) -> Result<()> {
        let mut flush = BTreeSet::new();
        if is_stmt {
            let mut others: Vec<_> = self
                .regs
                .values()
                .copied()
                .filter(|p| !self.ps[*p].leaves.is_empty())
                .collect();
            if self.multi.is_some_and(|p| !ops.contains(&p)) {
                return Err(err(format!(
                    "pending multi value interrupted by statement in proto {}",
                    self.f.pid
                )));
            }
            if ctor {
                let rest: Vec<_> = others
                    .iter()
                    .copied()
                    .filter(|p| !matches!(self.ex(*p).as_ref(),Expr::Table(t)if t.items.is_empty()))
                    .collect();
                let lim = rest
                    .iter()
                    .flat_map(|p| self.ps[*p].leaves.iter())
                    .max()
                    .copied()
                    .unwrap_or(0);
                others.retain(|p| {
                    rest.contains(p) || self.ps[*p].leaves.iter().min().is_some_and(|v| *v < lim)
                });
            }
            if !others.is_empty() {
                let mx = others
                    .iter()
                    .flat_map(|p| self.ps[*p].leaves.iter())
                    .max()
                    .copied()
                    .unwrap();
                flush.extend(others);
                for p in ops {
                    if self.ps[*p].leaves.iter().min().is_some_and(|v| *v < mx) {
                        flush.insert(*p);
                    }
                }
            }
        }
        let mut flush: Vec<_> = flush.into_iter().collect();
        flush.sort_by_key(|p| self.ps[*p].leaves.iter().min().copied().unwrap_or(0));
        for p in flush {
            self.materialize(p)?;
        }
        let ordered = |ps: &Vec<Pending>, xs: &[usize]| {
            let mut cur = 0;
            for &p in xs {
                if let Some(lo) = ps[p].leaves.iter().min() {
                    if *lo < cur {
                        return false;
                    }
                    cur = *ps[p].leaves.iter().max().unwrap();
                }
            }
            true
        };
        if !ordered(&self.ps, ops) {
            let mut cand: Vec<_> = ops
                .iter()
                .copied()
                .filter(|p| !self.ps[*p].leaves.is_empty())
                .collect();
            cand.sort_by_key(|p| *self.ps[*p].leaves.iter().min().unwrap());
            let mut k = 0;
            while k < cand.len() {
                let rest: Vec<_> = ops
                    .iter()
                    .copied()
                    .filter(|p| cand[k..].contains(p))
                    .collect();
                if ordered(&self.ps, &rest) {
                    break;
                }
                k += 1;
            }
            for &p in &cand[..k] {
                self.materialize(p)?;
            }
        }
        Ok(())
    }
    fn declare(&mut self, w: WebId, x: E) {
        if self.g.ctx.webs[w].hint.is_none() {
            self.g.ctx.webs[w].hint = suggest(&x, &self.g.ctx);
        }
        let web = &mut self.g.ctx.webs[w];
        if matches!(web.kind, "param" | "loop") || web.declared {
            self.stmts.push(Stmt::Assign(var(w), x));
        } else {
            web.declared = true;
            self.stmts.push(Stmt::Local(w, Some(x)));
        }
    }
    fn set(&mut self, id: usize, r: Reg, x: E, lv: Vec<usize>, ctor: bool) -> Result<()> {
        let w = self.f.def_web[&(id, r)];
        if x.is_const() && !self.g.ctx.webs[w].captured {
            let d = self.f.node_defs[&(id, r)];
            if !self.g.ctx.webs[w]
                .uses
                .iter()
                .any(|u| self.f.read_defs[u].contains(&d))
            {
                return Ok(());
            }
        }
        if matches!(x.as_ref(), Expr::Func { .. }) {
            let first = self.g.ctx.webs[w]
                .uses
                .iter()
                .map(|(id, _)| *id)
                .filter(|uid| *uid > id)
                .min();
            if first.is_some_and(|u| u > id + 1) {
                self.g.ctx.webs[w].noinline = true;
            }
        }
        if self.inline_ok(w) {
            let p = self.p(x, Some(w), lv);
            self.regs.insert(r, p);
            return Ok(());
        }
        self.below_result(w);
        let p = self.p(x, None, lv);
        if self.g.ctx.webs[w].uses.is_empty() {
            match self.ex(p).as_ref() {
                Expr::Call { .. } => {
                    self.settle(&[p], true, false)?;
                    self.stmts.push(Stmt::Call(self.ex(p)));
                }
                Expr::Table(_) | Expr::Func { .. } => {
                    self.settle(&[p], true, false)?;
                    self.declare(w, self.ex(p));
                }
                Expr::Str(_) | Expr::Num(..) | Expr::Bool(_) | Expr::Nil | Expr::Var(_) => {
                    self.g.log.push(format!(
                        "dropped dead value in proto {} rec {} ({})",
                        self.f.pid, self.f.nodes[id].rid, self.f.nodes[id].kind
                    ))
                }
                _ => {
                    self.settle(&[p], true, false)?;
                    self.stmts.push(Stmt::CallCond(self.ex(p)));
                }
            }
            return Ok(());
        }
        self.settle(&[p], true, ctor)?;
        self.declare(w, self.ex(p));
        Ok(())
    }
    fn read_many(&mut self, id: usize, rs: &[Reg]) -> Result<Vec<usize>> {
        rs.iter().map(|r| self.rd(id, *r)).collect()
    }
    fn import_path(&self, aux: i64) -> Result<Vec<String>> {
        if !(1..=3).contains(&(aux >> 30)) {
            return Err(err("invalid import path length"));
        }
        let ks = [(aux >> 20) & 1023, (aux >> 10) & 1023, aux & 1023];
        ks[..(aux >> 30) as usize]
            .iter()
            .map(|k| self.f.p.kbytes(*k))
            .collect()
    }
    fn run(mut self) -> Result<(Body, Term)> {
        let mut term = None;
        for id in self.f.blocks[self.bi].nodes.clone() {
            let n = self.f.nodes[id].clone();
            let a = n.a.clone();
            let k = n.kind.as_str();
            match k {
                "LOADK" => {
                    let x = self.f.p.constant(a.rf)?;
                    self.set(id, a.a, x, vec![], false)?;
                }
                "LOADN" => self.set(id, a.a, e(Expr::Num(a.rf as f64, false)), vec![], false)?,
                "LOADB" => {
                    let w = self.f.def_web[&(id, a.a)];
                    self.mark_materialized(w);
                    self.set(id, a.a, e(Expr::Bool(a.b == 1)), vec![], false)?;
                }
                "LOADNIL" => self.set(id, a.a, nil(), vec![], false)?,
                "GETIMPORT" => self.set(
                    id,
                    a.a,
                    e(Expr::Imp(self.import_path(a.aux)?)),
                    vec![],
                    false,
                )?,
                "MOVE" => {
                    let p = self.rd(id, a.b)?;
                    self.mark_materialized(self.f.def_web[&(id, a.a)]);
                    if self.ps[p].web.is_none() && a.a > a.b {
                        self.g.ctx.webs[self.f.use_web[&(id, a.b)]].keep = true;
                    }
                    let mut lv = self.ps[p].leaves.clone();
                    let w = self.f.use_web[&(id, a.b)];
                    if self.ps[p].web.is_none()
                        && lv.is_empty()
                        && (self.g.ctx.webs[w].captured || self.g.ctx.webs[w].defs.len() > 1)
                    {
                        lv.push(self.g.nseq());
                    }
                    self.set(id, a.a, self.ex(p), lv, false)?;
                }
                "GETTABLEKS" => {
                    let p = self.rd(id, a.b)?;
                    let x = idx(self.ex(p), Key::Str(self.f.p.kbytes(a.aux)?));
                    let mut lv = self.leaves(&[p]);
                    lv.push(self.g.nseq());
                    self.set(id, a.a, x, lv, false)?;
                }
                "OR" | "AND" => {
                    let x = self.rd(id, a.b)?;
                    let y = self.rd(id, a.c)?;
                    self.settle(&[x, y], false, false)?;
                    let ex = bin(&k.to_ascii_lowercase(), self.ex(x), self.ex(y));
                    let mut lv = self.leaves(&[x, y]);
                    lv.push(self.g.nseq());
                    self.set(id, a.a, ex, lv, false)?;
                }
                "ORK" | "ANDK" => {
                    let x = self.rd(id, a.b)?;
                    self.settle(&[x], false, false)?;
                    let ex = bin(
                        &k[..k.len() - 1].to_ascii_lowercase(),
                        self.ex(x),
                        self.f.p.constant(a.c)?,
                    );
                    let mut lv = self.leaves(&[x]);
                    lv.push(self.g.nseq());
                    self.set(id, a.a, ex, lv, false)?;
                }
                "GETTABLE" => {
                    let o = self.rd(id, a.b)?;
                    let key = self.rd(id, a.c)?;
                    self.settle(&[o, key], false, false)?;
                    let x = idx(self.ex(o), Key::Expr(self.ex(key)));
                    let mut lv = self.leaves(&[o, key]);
                    lv.push(self.g.nseq());
                    self.set(id, a.a, x, lv, false)?;
                }
                "GETTABLEN" | "NOT" | "MINUS" | "LENGTH" => {
                    let p = self.rd(id, a.b)?;
                    self.settle(&[p], false, false)?;
                    let x = match k {
                        "GETTABLEN" => {
                            idx(self.ex(p), Key::Expr(e(Expr::Num((a.c + 1) as f64, false))))
                        }
                        "NOT" => not(self.ex(p)),
                        "MINUS" => e(Expr::Neg(self.ex(p))),
                        _ => e(Expr::Len(self.ex(p))),
                    };
                    let mut lv = self.leaves(&[p]);
                    lv.push(self.g.nseq());
                    self.set(id, a.a, x, lv, false)?;
                }
                "CONCAT" => {
                    let ops = self.read_many(id, &n.reads)?;
                    self.settle(&ops, false, false)?;
                    let mut x = self.ex(*ops.last().ok_or_else(|| err("empty CONCAT"))?);
                    for &p in ops[..ops.len() - 1].iter().rev() {
                        x = bin("..", self.ex(p), x);
                    }
                    let mut lv = self.leaves(&ops);
                    lv.push(self.g.nseq());
                    self.set(id, a.a, x, lv, false)?;
                }
                "DUPTABLE" => {
                    let tc = self
                        .f
                        .p
                        .consts
                        .get(&a.rf)
                        .ok_or_else(|| err("missing table template"))?;
                    let mut fields = vec![];
                    let mut pending = vec![];
                    let shape = tc[1]
                        .as_array()
                        .ok_or_else(|| err("invalid table template"))?;
                    for item in shape {
                        let (kid, vid) = if tc[0] == "i" {
                            (integer(item, "table key")?, -1)
                        } else if tc[0] == "t" {
                            (
                                integer(&item[0], "table key")?,
                                integer(&item[1], "table value")?,
                            )
                        } else {
                            return Err(err("expected table template"));
                        };
                        let key = self.f.p.kbytes(kid)?;
                        let value = if vid >= 0 {
                            Some(self.f.p.constant(vid)?)
                        } else {
                            pending.push(key.clone());
                            None
                        };
                        fields.push((key, value));
                    }
                    let w = self.f.def_web[&(id, a.a)];
                    let nonfill = self.g.ctx.webs[w]
                        .uses
                        .iter()
                        .filter(|(i, r)| {
                            let n = &self.f.nodes[*i];
                            !(n.kind == "SETTABLEKS" && *r == n.a.b && n.a.a != n.a.b)
                        })
                        .count();
                    let ctor = !pending.is_empty() && nonfill == 1;
                    self.g.ctx.webs[w].ctor = ctor;
                    let seq = self.g.nseq();
                    self.set(
                        id,
                        a.a,
                        e(Expr::Table(Table {
                            items: vec![],
                            fields,
                            pending,
                            fromnew: false,
                        })),
                        vec![seq],
                        ctor,
                    )?;
                }
                "SETTABLE" => {
                    let o = self.rd(id, a.b)?;
                    let key = self.rd(id, a.c)?;
                    let v = self.rd(id, a.a)?;
                    self.settle(&[o, key, v], true, false)?;
                    self.stmts.push(Stmt::Assign(
                        idx(self.ex(o), Key::Expr(self.ex(key))),
                        self.ex(v),
                    ));
                }
                "NEWTABLE" => {
                    let w = self.f.def_web[&(id, a.a)];
                    let ctor = self.g.ctx.webs[w].uses.iter().any(|(i, r)| {
                        self.f.nodes[*i].kind == "SETLIST" && *r == self.f.nodes[*i].a.a
                    });
                    let lv = if ctor { vec![] } else { vec![self.g.nseq()] };
                    let mut t = Table::new(vec![]);
                    t.fromnew = true;
                    self.set(id, a.a, e(Expr::Table(t)), lv, false)?;
                }
                "GETUPVAL" => {
                    let w = *self
                        .f
                        .upvals
                        .get(a.b as usize)
                        .ok_or_else(|| err("invalid upvalue read"))?;
                    let seq = self.g.nseq();
                    self.set(id, a.a, var(w), vec![seq], false)?;
                }
                "SETUPVAL" => {
                    let p = self.rd(id, a.a)?;
                    self.settle(&[p], true, false)?;
                    let w = *self
                        .f
                        .upvals
                        .get(a.b as usize)
                        .ok_or_else(|| err("invalid upvalue write"))?;
                    self.stmts.push(Stmt::Assign(var(w), self.ex(p)));
                }
                "SETTABLEKS" | "SETTABLEN" => {
                    let o = self.rd(id, a.b)?;
                    let v = self.rd(id, a.a)?;
                    let ctor =
                        matches!(self.ex(o).as_ref(),Expr::Var(w)if self.g.ctx.webs[*w].ctor);
                    self.settle(&[o, v], true, ctor)?;
                    let key = if k == "SETTABLEKS" {
                        Key::Str(self.f.p.kbytes(a.aux)?)
                    } else {
                        Key::Expr(e(Expr::Num((a.c + 1) as f64, false)))
                    };
                    self.stmts
                        .push(Stmt::Assign(idx(self.ex(o), key), self.ex(v)));
                }
                "SELF" => {
                    let o = self.rd(id, a.b)?;
                    let key = self.f.p.kbytes(a.aux)?;
                    let wa = self.f.def_web[&(id, a.a)];
                    let wo = self.f.def_web[&(id, a.a + 1)];
                    if !(self.inline_ok(wa) && self.inline_ok(wo)) {
                        return Err(err(format!(
                            "SELF with non-inline webs in proto {}",
                            self.f.pid
                        )));
                    }
                    let li = self.links.len();
                    let po = self.p(self.ex(o), Some(wo), self.leaves(&[o]));
                    let pm = self.p(
                        idx(self.ex(o), Key::Str(key.clone())),
                        Some(wa),
                        self.leaves(&[o]),
                    );
                    self.ps[po].kind = "selfo";
                    self.ps[pm].kind = "selfm";
                    self.ps[po].link = Some(li);
                    self.ps[pm].link = Some(li);
                    self.ps[pm].method = Some(key);
                    self.links.push(Link {
                        obj: self.ex(o),
                        var: None,
                        m: pm,
                        o: po,
                    });
                    self.regs.insert(a.a + 1, po);
                    self.regs.insert(a.a, pm);
                }
                "CALL" => {
                    let fp = self.rd(id, a.a)?;
                    let args = self.read_many(id, &n.reads[1..])?;
                    let mp = if a.b == 0 {
                        Some(
                            self.multi
                                .take()
                                .ok_or_else(|| err("multret consumer without producer"))?,
                        )
                    } else {
                        None
                    };
                    let method = self.ps[fp].kind == "selfm"
                        && args.first().is_some_and(|p| {
                            self.ps[*p].kind == "selfo" && self.ps[*p].link == self.ps[fp].link
                        });
                    if self.ps[fp].kind == "selfm" && !method {
                        return Err(err("split method call"));
                    }
                    let rest = if method { &args[1..] } else { &args[..] };
                    let ops: Vec<_> = std::iter::once(fp)
                        .chain(rest.iter().copied())
                        .chain(mp)
                        .collect();
                    if a.c == 1 {
                        self.settle(&ops, true, false)?;
                        let call = self.call(fp, rest, mp, method)?;
                        self.stmts.push(Stmt::Call(call));
                    } else if a.c == 2 {
                        let w = self.f.def_web[&(id, a.a)];
                        if self.inline_ok(w) {
                            self.settle(&ops, false, false)?;
                            let mut lv = self.leaves(&ops);
                            lv.push(self.g.nseq());
                            let call = self.call(fp, rest, mp, method)?;
                            let p = self.p(call, Some(w), lv);
                            self.regs.insert(a.a, p);
                        } else {
                            self.settle(&ops, true, false)?;
                            let call = self.call(fp, rest, mp, method)?;
                            self.declare(w, call);
                        }
                    } else if a.c == 0 {
                        self.settle(&ops, false, false)?;
                        let mut lv = self.leaves(&ops);
                        lv.push(self.g.nseq());
                        let call = self.call(fp, rest, mp, method)?;
                        let p = self.p(call, None, lv);
                        self.ps[p].kind = "multi";
                        self.multi = Some(p);
                    } else {
                        let ws: Vec<_> = (0..a.c - 1)
                            .map(|i| self.f.def_web[&(id, a.a + i)])
                            .collect();
                        let used: Vec<_> = ws
                            .iter()
                            .enumerate()
                            .filter(|(_, w)| !self.g.ctx.webs[**w].uses.is_empty())
                            .map(|(i, _)| i)
                            .collect();
                        let nx = if n.succ.len() == 1 {
                            Some(n.succ[0])
                        } else {
                            None
                        };
                        let for3 = a.c == 4
                            && nx.is_some_and(|nx| {
                                let n = &self.f.nodes[nx];
                                n.kind == "FORGPREP"
                                    && n.a.a == a.a
                                    && n.blk == self.bi
                                    && ws.iter().all(|w| {
                                        self.g.ctx.webs[*w].uses.len() == 1
                                            && self.g.ctx.webs[*w].uses[0].0 == nx
                                    })
                            });
                        if for3 {
                            self.settle(&ops, false, false)?;
                            let mut lv = self.leaves(&ops);
                            lv.push(self.g.nseq());
                            let call = self.call(fp, rest, mp, method)?;
                            let p = self.p(call, None, lv);
                            self.ps[p].kind = "multi3";
                            self.multi3 = Some(p);
                        } else if used == [0]
                            && (self.g.ctx.webs[ws[0]].declared
                                || matches!(self.g.ctx.webs[ws[0]].kind, "param" | "loop"))
                        {
                            self.settle(&ops, true, false)?;
                            let call = self.call(fp, rest, mp, method)?;
                            self.declare(ws[0], call);
                        } else {
                            self.settle(&ops, true, false)?;
                            for &w in &ws {
                                let web = &mut self.g.ctx.webs[w];
                                web.noinline = true;
                                if web.declared || matches!(web.kind, "param" | "loop") {
                                    return Err(err("multi-assign into declared variable"));
                                }
                            }
                            for &i in &used {
                                self.g.ctx.webs[ws[i]].declared = true;
                            }
                            let targets = ws
                                .iter()
                                .map(|w| {
                                    if self.g.ctx.webs[*w].uses.is_empty() {
                                        None
                                    } else {
                                        Some(*w)
                                    }
                                })
                                .collect();
                            let call = self.call(fp, rest, mp, method)?;
                            self.stmts.push(Stmt::LocalN(targets, call));
                        }
                    }
                }
                "GETVARARGS" => {
                    if a.b == 0 {
                        let p = self.p(e(Expr::Vararg(false)), None, vec![]);
                        self.ps[p].kind = "multi";
                        self.multi = Some(p);
                    } else if a.b == 2 {
                        self.set(id, a.a, e(Expr::Vararg(true)), vec![], false)?;
                    } else {
                        let mut ws = vec![];
                        for i in 0..a.b - 1 {
                            let w = self.f.def_web[&(id, a.a + i)];
                            let web = &mut self.g.ctx.webs[w];
                            web.noinline = true;
                            if web.declared || matches!(web.kind, "param" | "loop") {
                                return Err(err("vararg read into a declared variable"));
                            }
                            if web.uses.is_empty() {
                                ws.push(None);
                            } else {
                                web.declared = true;
                                ws.push(Some(w));
                            }
                        }
                        self.stmts.push(Stmt::LocalN(ws, e(Expr::Vararg(false))));
                    }
                }
                "NEWCLOSURE" | "DUPCLOSURE" => {
                    let mut caps = vec![];
                    for (t, r) in a.caps {
                        if matches!(t, 0 | 1) {
                            let mut w = self.f.use_web[&(id, r)];
                            if t == 0
                                && self.g.ctx.webs[w]
                                    .defs
                                    .iter()
                                    .any(|d| d.node.is_some_and(|n| n > id))
                            {
                                let p = self.rd(id, r)?;
                                self.settle(&[p], true, false)?;
                                let snap = self.g.ctx.web("var", Some(self.f.id), self.f.pid);
                                let seq = self.g.nseq();
                                self.g.ctx.webs[snap].defs.push(Def {
                                    id: self.f.nodes.len() + seq,
                                    node: None,
                                    reg: r,
                                });
                                self.g.ctx.webs[snap].captured = true;
                                self.declare(snap, self.ex(p));
                                w = snap;
                            }
                            caps.push(w);
                        } else {
                            caps.push(
                                *self
                                    .f
                                    .upvals
                                    .get(r as usize)
                                    .ok_or_else(|| err("invalid outer capture"))?,
                            );
                        }
                    }
                    self.set(id, a.a, e(Expr::Func { pid: a.child, caps }), vec![], false)?;
                }
                "CLOSEUPVALS" => {}
                "SETLIST" => {
                    let tbl = self.rd(id, a.a)?;
                    let items = self.read_many(id, &n.reads[1..])?;
                    let mp = if a.c == 0 {
                        Some(
                            self.multi
                                .take()
                                .ok_or_else(|| err("multret consumer without producer"))?,
                        )
                    } else {
                        None
                    };
                    let all: Vec<_> = std::iter::once(tbl)
                        .chain(items.iter().copied())
                        .chain(mp)
                        .collect();
                    let t = self.ex(tbl);
                    match t.as_ref() {
                        Expr::Table(t)
                            if self.ps[tbl].web.is_some() && a.aux == 1 + t.items.len() as i64 =>
                        {
                            let mut its = t.items.clone();
                            self.settle(&all, false, false)?;
                            its.extend(items.iter().map(|p| self.ex(*p)));
                            if let Some(mp) = mp {
                                its.push(multi(self.ex(mp)));
                            }
                            self.set(
                                id,
                                a.a,
                                e(Expr::Table(Table::new(its))),
                                self.leaves(&all),
                                false,
                            )?;
                        }
                        Expr::Var(_) => {
                            self.settle(&all, true, false)?;
                            for (i, &p) in items.iter().enumerate() {
                                self.stmts.push(Stmt::Assign(
                                    idx(
                                        self.ex(tbl),
                                        Key::Expr(e(Expr::Num((a.aux + i as i64) as f64, true))),
                                    ),
                                    self.ex(p),
                                ));
                            }
                            if let Some(mp) = mp {
                                self.stmts.push(Stmt::Assign(
                                    idx(
                                        self.ex(tbl),
                                        Key::Expr(e(Expr::Num(
                                            (a.aux + items.len() as i64) as f64,
                                            true,
                                        ))),
                                    ),
                                    multi(self.ex(mp)),
                                ));
                            }
                            self.set(id, a.a, self.ex(tbl), vec![], false)?;
                        }
                        _ => return Err(err("SETLIST on a table that is not a variable")),
                    }
                }
                "RETURN" => {
                    let mut ops = self.read_many(id, &n.reads)?;
                    if a.b == 0 {
                        ops.push(
                            self.multi
                                .take()
                                .ok_or_else(|| err("multret return without producer"))?,
                        );
                    }
                    self.settle(&ops, true, false)?;
                    let mut xs: Vec<_> = ops.iter().map(|p| self.ex(*p)).collect();
                    if a.b == 0 {
                        if let Some(last) = xs.last_mut() {
                            *last = multi(last.clone());
                        }
                    }
                    term = Some(Term::Ret(xs));
                }
                "FORNPREP" => {
                    let ini = self.rd(id, a.a + 2)?;
                    let lim = self.rd(id, a.a)?;
                    let step = self.rd(id, a.a + 1)?;
                    self.settle(&[ini, lim, step], true, false)?;
                    let w = self.f.def_web[&(id, a.a + 2)];
                    self.g.ctx.webs[w].kind = "loop";
                    self.g.ctx.webs[w].declared = true;
                    term = Some(Term::For(
                        w,
                        self.ex(ini),
                        self.ex(lim),
                        self.ex(step),
                        self.f.nodes[n.succ[0]].blk,
                        self.f.nodes[n.succ[1]].blk,
                    ));
                }
                "FORNLOOP" => {
                    term = Some(Term::ForNLoop(
                        self.f.nodes[n.succ[1]].blk,
                        self.f.nodes[n.succ[0]].blk,
                    ));
                }
                "FORGPREP" => {
                    let exprs = if let Some(mp) = self.multi3.take() {
                        self.settle(&[mp], true, false)?;
                        vec![multi(self.ex(mp))]
                    } else {
                        let ops = self.read_many(id, &[a.a, a.a + 1, a.a + 2])?;
                        self.settle(&ops, true, false)?;
                        ops.iter().map(|p| self.ex(*p)).collect()
                    };
                    term = Some(Term::ForGPrep(exprs, self.f.nodes[n.succ[0]].blk));
                }
                "FORGLOOP" => {
                    let ws: Vec<_> = (0..(a.aux & 255))
                        .map(|i| self.f.def_web[&(id, a.a + 3 + i)])
                        .collect();
                    for w in &ws {
                        self.g.ctx.webs[*w].kind = "loop";
                        self.g.ctx.webs[*w].declared = true;
                    }
                    term = Some(Term::ForGLoop(
                        ws,
                        self.f.nodes[n.succ[1]].blk,
                        self.f.nodes[n.succ[0]].blk,
                    ));
                }
                _ if is_rr(k) => {
                    let x = self.rd(id, a.b)?;
                    let y = self.rd(id, a.c)?;
                    self.settle(&[x, y], false, false)?;
                    let ex = bin(arith(k).unwrap(), self.ex(x), self.ex(y));
                    let mut lv = self.leaves(&[x, y]);
                    lv.push(self.g.nseq());
                    self.set(id, a.a, ex, lv, false)?;
                }
                _ if is_k(k) || is_rk(k) => {
                    let r = if is_k(k) { a.b } else { a.c };
                    let p = self.rd(id, r)?;
                    self.settle(&[p], false, false)?;
                    let kc = self.f.p.constant(if is_k(k) { a.c } else { a.b })?;
                    let (l, r) = if is_k(k) {
                        (self.ex(p), kc)
                    } else {
                        (kc, self.ex(p))
                    };
                    let ex = bin(arith(k).unwrap(), l, r);
                    let mut lv = self.leaves(&[p]);
                    lv.push(self.g.nseq());
                    self.set(id, a.a, ex, lv, false)?;
                }
                _ if is_cond(k) => {
                    let x = self.rd(id, a.a)?;
                    let cond = if matches!(k, "JUMPIF" | "JUMPIFNOT") {
                        self.settle(&[x], true, false)?;
                        if k == "JUMPIF" {
                            self.ex(x)
                        } else {
                            not(self.ex(x))
                        }
                    } else if matches!(
                        k,
                        "JUMPIFEQ"
                            | "JUMPIFNOTEQ"
                            | "JUMPIFLE"
                            | "JUMPIFLT"
                            | "JUMPIFNOTLT"
                            | "JUMPIFNOTLE"
                    ) {
                        let y = self.rd(id, a.aux)?;
                        self.settle(&[x, y], true, false)?;
                        let op = match k {
                            "JUMPIFEQ" => "==",
                            "JUMPIFNOTEQ" => "~=",
                            "JUMPIFLE" | "JUMPIFNOTLE" => "<=",
                            _ => "<",
                        };
                        let (l, r, op) = if matches!(op, "<" | "<=") && a.aux < a.a {
                            (self.ex(y), self.ex(x), if op == "<" { ">" } else { ">=" })
                        } else {
                            (self.ex(x), self.ex(y), op)
                        };
                        let c = bin(op, l, r);
                        if matches!(k, "JUMPIFNOTLT" | "JUMPIFNOTLE") {
                            not(c)
                        } else {
                            c
                        }
                    } else {
                        self.settle(&[x], true, false)?;
                        let flag = (a.aux >> 31) & 1;
                        let kc = match k {
                            "JUMPXEQKNIL" => nil(),
                            "JUMPXEQK" => self.f.p.constant(a.aux & 0xffffff)?,
                            _ => e(Expr::Bool(a.aux & 1 != 0)),
                        };
                        bin(if flag == 0 { "==" } else { "~=" }, self.ex(x), kc)
                    };
                    term = Some(Term::Cond(
                        cond,
                        self.f.nodes[n.succ[0]].blk,
                        self.f.nodes[n.succ[1]].blk,
                    ));
                }
                _ => return Err(err(format!("eval: unhandled node kind {k}"))),
            }
        }
        let mut left: Vec<_> = self.regs.iter().map(|(r, p)| (*r, *p)).collect();
        left.sort_by_key(|(_, p)| self.ps[*p].leaves.iter().min().copied().unwrap_or(0));
        for (r, p) in left {
            self.g.log.push(format!(
                "leftover pending value in proto {} block {} R{r}",
                self.f.pid, self.bi
            ));
            self.materialize(p)?;
        }
        if self.multi.is_some() || self.multi3.is_some() {
            return Err(err("unconsumed multret value"));
        }
        let term = if let Some(t) = term {
            t
        } else {
            let succ = &self.f.blocks[self.bi].succ;
            if succ.len() != 1 {
                return Err(err("unterminated basic block"));
            }
            Term::Goto(succ[0])
        };
        Ok((self.stmts, term))
    }
    fn call(&self, fp: usize, args: &[usize], mp: Option<usize>, method: bool) -> Result<E> {
        let mut xs: Vec<_> = args.iter().map(|p| self.ex(*p)).collect();
        if let Some(mp) = mp {
            xs.push(multi(self.ex(mp)));
        }
        let (func, method) = if method {
            if let Expr::Idx(o, _) = self.ex(fp).as_ref() {
                (o.clone(), self.ps[fp].method.clone())
            } else {
                return Err(err("invalid method pending value"));
            }
        } else {
            (self.ex(fp), None)
        };
        Ok(e(Expr::Call {
            func,
            args: xs,
            method,
        }))
    }
}

const EXIT: usize = usize::MAX;
#[derive(Clone)]
struct LoopCtx {
    exit: Option<usize>,
    body: BTreeSet<usize>,
    header: Option<usize>,
}
struct Structurer<'a> {
    f: &'a Function,
    log: &'a mut Vec<String>,
    succ: Vec<Vec<usize>>,
    loops: BTreeMap<usize, BTreeSet<usize>>,
    lexit: BTreeMap<usize, Option<usize>>,
    ip: Vec<Option<usize>>,
    dag: Vec<Vec<usize>>,
    rpo: BTreeMap<usize, usize>,
    rset: Vec<BTreeSet<usize>>,
    lregion: BTreeMap<usize, BTreeSet<usize>>,
    emitted: Vec<usize>,
}
fn reachable(graph: &[Vec<usize>], start: usize, avoid: Option<usize>) -> BTreeSet<usize> {
    let mut seen = BTreeSet::new();
    let mut st = vec![start];
    while let Some(i) = st.pop() {
        if i == EXIT || Some(i) == avoid || !seen.insert(i) {
            continue;
        }
        st.extend(&graph[i]);
    }
    seen
}
type DfsResult = (Vec<Vec<usize>>, Vec<usize>, Vec<(usize, usize)>);
fn dfs_graph(graph: &[Vec<usize>], entry: usize) -> DfsResult {
    let mut state = vec![0; graph.len()];
    let mut dag = vec![vec![]; graph.len()];
    let mut post = vec![];
    let mut back = vec![];
    state[entry] = 1;
    let mut stack = vec![(entry, 0)];
    while let Some((b, i)) = stack.last_mut() {
        let bid = *b;
        if *i < graph[bid].len() {
            let x = graph[bid][*i];
            *i += 1;
            if x == EXIT {
                continue;
            }
            if state[x] == 0 {
                state[x] = 1;
                dag[bid].push(x);
                stack.push((x, 0));
            } else if state[x] == 2 {
                dag[bid].push(x);
            } else {
                back.push((bid, x));
            }
        } else {
            state[bid] = 2;
            post.push(bid);
            stack.pop();
        }
    }
    (dag, post, back)
}
impl<'a> Structurer<'a> {
    fn new(f: &'a Function, log: &'a mut Vec<String>) -> Result<Self> {
        let count = f.blocks.len();
        if count == 0 {
            return Err(err("empty function graph"));
        }
        let succ: Vec<_> = f
            .blocks
            .iter()
            .map(|b| {
                let s = b.term.succ();
                if s.is_empty() {
                    vec![EXIT]
                } else {
                    s
                }
            })
            .collect();
        let (_, _, back) = dfs_graph(&succ, f.entry_block);
        let mut loops = BTreeMap::<usize, BTreeSet<usize>>::new();
        for (b, h) in back {
            if matches!(&f.blocks[b].term,Term::ForNLoop(body,_)if h==*body)
                || matches!(&f.blocks[b].term,Term::ForGLoop(_,body,_)if h==*body)
                || matches!(&f.blocks[h].term, Term::ForNLoop(..) | Term::ForGLoop(..))
            {
                continue;
            }
            let mut body = BTreeSet::from([h]);
            let mut work = vec![b];
            while let Some(x) = work.pop() {
                if !body.insert(x) {
                    continue;
                }
                work.extend(&f.blocks[x].pred);
            }
            loops.entry(h).or_default().extend(body);
        }
        let mut lexit = BTreeMap::new();
        for (&h, body) in &loops {
            let tg: BTreeSet<_> = body
                .iter()
                .flat_map(|x| succ[*x].iter().copied())
                .filter(|y| *y != EXIT && !body.contains(y))
                .collect();
            let exit = if tg.len() > 1 {
                let mut x = None;
                if f.blocks[h].stmts.is_empty() {
                    if let Term::Cond(_, a, b) = &f.blocks[h].term {
                        let outs: Vec<_> =
                            [*a, *b].into_iter().filter(|y| !body.contains(y)).collect();
                        if outs.len() == 1 && tg.contains(&outs[0]) {
                            x = Some(outs[0]);
                        }
                    }
                }
                if x.is_none() {
                    let nt: Vec<_> = tg
                        .iter()
                        .copied()
                        .filter(|t| {
                            let r = reachable(&succ, *t, None);
                            !r.is_disjoint(body) || tg.iter().any(|u| u != t && r.contains(u))
                        })
                        .collect();
                    if nt.len() > 1 {
                        return Err(err(format!("loop with several exits in proto {}", f.pid)));
                    }
                    x = nt.first().copied();
                }
                x
            } else {
                tg.first().copied()
            };
            lexit.insert(h, exit);
        }
        let mut pd_succ = succ.clone();
        for (&h, body) in &loops {
            if lexit[&h].is_none() {
                for &x in body {
                    if pd_succ[x].contains(&h) && !pd_succ[x].contains(&EXIT) {
                        pd_succ[x].push(EXIT);
                    }
                }
            }
        }
        let all: BTreeSet<_> = (0..count).chain(std::iter::once(EXIT)).collect();
        let mut pd = vec![all; count];
        loop {
            let mut changed = false;
            for b in 0..count {
                let mut new = None::<BTreeSet<usize>>;
                for &x in &pd_succ[b] {
                    let set = if x == EXIT {
                        BTreeSet::from([EXIT])
                    } else {
                        pd[x].clone()
                    };
                    new = Some(if let Some(old) = new {
                        old.intersection(&set).copied().collect()
                    } else {
                        set
                    });
                }
                let mut new = new.unwrap_or_default();
                new.insert(b);
                if new != pd[b] {
                    pd[b] = new;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        let mut ip = vec![None; count];
        for b in 0..count {
            let cand: BTreeSet<_> = pd[b].iter().copied().filter(|x| *x != b).collect();
            for &c in &cand {
                let cp = if c == EXIT {
                    BTreeSet::from([EXIT])
                } else {
                    pd[c].clone()
                };
                if cp == cand {
                    ip[b] = Some(c);
                }
            }
        }
        let ds: Vec<_> = f
            .blocks
            .iter()
            .enumerate()
            .map(|(b, bl)| match &bl.term {
                Term::ForNLoop(_, x) | Term::ForGLoop(_, _, x) => vec![*x],
                Term::ForGPrep(_, lp) => {
                    let mut s = vec![*lp];
                    if let Term::ForGLoop(_, body, _) = &f.blocks[*lp].term {
                        s.push(*body);
                    }
                    s
                }
                _ => succ[b].iter().copied().filter(|x| *x != EXIT).collect(),
            })
            .collect();
        let (dag, post, _) = dfs_graph(&ds, f.entry_block);
        let rpo = post
            .iter()
            .rev()
            .enumerate()
            .map(|(i, &b)| (b, i))
            .collect();
        let mut rset = vec![BTreeSet::new(); count];
        for b in post {
            let mut rs = BTreeSet::from([b]);
            for &x in &dag[b] {
                rs.extend(&rset[x]);
            }
            rset[b] = rs;
        }
        let lregion = loops
            .keys()
            .map(|h| (*h, reachable(&dag, *h, lexit[h])))
            .collect();
        Ok(Self {
            f,
            log,
            succ,
            loops,
            lexit,
            ip,
            dag,
            rpo,
            rset,
            lregion,
            emitted: vec![0; count],
        })
    }
    fn continue_chain(&self, mut b: usize, lp: Option<&LoopCtx>) -> Option<Vec<usize>> {
        let header = lp?.header?;
        let mut chain = vec![];
        let mut seen = BTreeSet::new();
        while b != EXIT && seen.insert(b) {
            if b == header {
                return Some(chain);
            }
            let block = &self.f.blocks[b];
            if !block.stmts.iter().all(
                |s| matches!(s,Stmt::Assign(v,c)if matches!(v.as_ref(),Expr::Var(_))&&c.is_const()),
            ) {
                return None;
            }
            if let Term::Goto(x) = block.term {
                chain.push(b);
                b = x;
            } else {
                return None;
            }
        }
        None
    }
    fn join(&self, t: usize, e: usize, lp: Option<&LoopCtx>) -> Option<usize> {
        let header = lp.and_then(|l| l.header);
        let rt = if Some(t) == header {
            BTreeSet::new()
        } else {
            self.rset[t].clone()
        };
        let re = if Some(e) == header {
            BTreeSet::new()
        } else {
            self.rset[e].clone()
        };
        rt.intersection(&re)
            .copied()
            .min_by_key(|b| self.rpo.get(b).copied().unwrap_or(usize::MAX))
    }
    fn emit_loop(&mut self, h: usize, stop: &BTreeSet<usize>) -> Result<Stmt> {
        let body = self.loops[&h].clone();
        let lp = LoopCtx {
            exit: self.lexit[&h],
            body: body.clone(),
            header: Some(h),
        };
        let hb = &self.f.blocks[h];
        let mut stop = stop.clone();
        stop.insert(h);
        if hb.stmts.is_empty() {
            if let Term::Cond(c, n, a) = hb.term.clone() {
                let stay = if body.contains(&n) && !body.contains(&a) {
                    Some((n, negate(c)))
                } else if body.contains(&a) && !body.contains(&n) {
                    Some((a, c))
                } else {
                    None
                };
                if let Some((stay, c)) = stay {
                    self.emitted[h] += 1;
                    let body = self.region(Some(stay), &stop, Some(&lp), false)?;
                    return Ok(Stmt::While(c, strip_cont(body)));
                }
            }
        }
        let body = self.region(Some(h), &stop, Some(&lp), true)?;
        Ok(Stmt::While(e(Expr::Bool(true)), strip_cont(body)))
    }
    fn region(
        &mut self,
        mut bid: Option<usize>,
        stop: &BTreeSet<usize>,
        lp: Option<&LoopCtx>,
        mut first: bool,
    ) -> Result<Body> {
        let mut out = vec![];
        let mut guard = 0;
        while let Some(bi) = bid {
            guard += 1;
            if guard > 100000 {
                return Err(err("structuring loop"));
            }
            let skip = first;
            first = false;
            if !skip {
                if lp.is_some_and(|l| Some(bi) == l.exit) {
                    out.push(Stmt::Break);
                    return Ok(out);
                }
                if lp.is_some_and(|l| Some(bi) == l.header) {
                    out.push(Stmt::Continue);
                    return Ok(out);
                }
                if stop.contains(&bi) {
                    return Ok(out);
                }
                if self.loops.contains_key(&bi) {
                    out.push(self.emit_loop(bi, stop)?);
                    bid = self.lexit[&bi];
                    continue;
                }
            }
            self.emitted[bi] += 1;
            out.extend(self.f.blocks[bi].stmts.clone());
            match self.f.blocks[bi].term.clone() {
                Term::Ret(xs) => {
                    out.push(Stmt::Return(xs));
                    return Ok(out);
                }
                Term::Goto(x) => bid = Some(x),
                Term::ForNLoop(..) | Term::ForGLoop(..) => return Ok(out),
                Term::ForGPrep(exprs, fl) => {
                    let (ws, b, x) =
                        if let Term::ForGLoop(ws, b, x) = self.f.blocks[fl].term.clone() {
                            (ws, b, x)
                        } else {
                            return Err(err("FORGPREP without FORGLOOP"));
                        };
                    let inner = LoopCtx {
                        exit: Some(x),
                        body: reachable(&self.succ, b, Some(x)),
                        header: None,
                    };
                    let body = self.region(Some(b), stop, Some(&inner), false)?;
                    out.push(Stmt::ForIn(ws, exprs, body));
                    bid = Some(x);
                }
                Term::For(w, ini, lim, step, b, x) => {
                    let inner = LoopCtx {
                        exit: Some(x),
                        body: reachable(&self.succ, b, Some(x)),
                        header: None,
                    };
                    let body = self.region(Some(b), stop, Some(&inner), false)?;
                    out.push(Stmt::For(w, ini, lim, step, body));
                    bid = Some(x);
                }
                Term::Cond(c, n, a) => {
                    let mut early_done = false;
                    for (early, normal, condition) in [(n, a, negate(c.clone())), (a, n, c.clone())]
                    {
                        if stop.contains(&normal) {
                            if let Some(chain) = self.continue_chain(early, lp) {
                                if chain.iter().all(|x| !stop.contains(x)) {
                                    let body = self.region(Some(early), stop, lp, false)?;
                                    out.push(Stmt::If(condition, body, vec![]));
                                    bid = Some(normal);
                                    early_done = true;
                                    break;
                                }
                            }
                        }
                    }
                    if early_done {
                        continue;
                    }
                    let mut j = self.join(n, a, lp);
                    if let Some(join) = j {
                        let later: BTreeSet<_> = self.rset[join]
                            .iter()
                            .copied()
                            .filter(|x| *x != join)
                            .collect();
                        let hdr = lp.and_then(|l| l.header);
                        let bad = [n, a].iter().any(|s| {
                            *s != join
                                && Some(*s) != hdr
                                && reachable(&self.dag, *s, Some(join))
                                    .intersection(&later)
                                    .any(|q| self.continue_chain(*q, lp).is_none())
                        });
                        if bad {
                            j = self.ip[bi].filter(|j| *j != EXIT);
                        }
                    }
                    let outside = if let (Some(l), Some(j)) = (lp, j) {
                        if let Some(h) = l.header {
                            !self.lregion.get(&h).unwrap_or(&l.body).contains(&j)
                        } else {
                            !l.body.contains(&j)
                        }
                    } else {
                        false
                    };
                    if j.is_none() || outside {
                        let th = self.region(Some(n), stop, lp, false)?;
                        let el = self.region(Some(a), stop, lp, false)?;
                        out.push(Stmt::If(negate(c), th, el));
                        return Ok(out);
                    }
                    let j = j.unwrap();
                    let mut st2 = stop.clone();
                    st2.insert(j);
                    let th = if n == j {
                        vec![]
                    } else {
                        self.region(Some(n), &st2, lp, false)?
                    };
                    let el = if a == j {
                        vec![]
                    } else {
                        self.region(Some(a), &st2, lp, false)?
                    };
                    out.push(Stmt::If(negate(c), th, el));
                    bid = Some(j);
                }
            }
        }
        Ok(out)
    }
    fn run(mut self) -> Result<Body> {
        let body = self.region(Some(self.f.entry_block), &BTreeSet::new(), None, false)?;
        let dup: Vec<_> = self
            .emitted
            .iter()
            .enumerate()
            .filter(|(_, n)| **n > 1)
            .map(|(b, n)| (b, *n))
            .collect();
        if !dup.is_empty() {
            self.log.push(format!(
                "proto {}: blocks emitted more than once: {dup:?}",
                self.f.pid
            ));
        }
        Ok(body)
    }
}
pub fn mk_if(c: E, th: Body, el: Body) -> Stmt {
    if el.is_empty() && th.len() == 1 {
        if let Stmt::If(c2, t2, e2) = &th[0] {
            if e2.is_empty() {
                return mk_if(and_join(c, c2.clone()), t2.clone(), vec![]);
            }
        }
    }
    Stmt::If(c, th, el)
}
fn and_join(a: E, b: E) -> E {
    if let Expr::Bin(op, l, r) = b.as_ref() {
        if op == "and" {
            return and_join(and_join(a, l.clone()), r.clone());
        }
    }
    bin("and", a, b)
}
fn ends(body: &Body) -> bool {
    matches!(
        body.last(),
        Some(Stmt::Return(_) | Stmt::Break | Stmt::Continue)
    )
}
pub fn simplify(body: Body) -> Body {
    let mut out = vec![];
    for s in body {
        match s {
            Stmt::If(mut c, t, e) => {
                let (mut t, mut el) = (simplify(t), simplify(e));
                if t.is_empty() && !el.is_empty() {
                    c = negate(c);
                    std::mem::swap(&mut t, &mut el);
                }
                if t.is_empty() && el.is_empty() {
                    if has_call(&c) {
                        out.push(Stmt::CallCond(c));
                    }
                    continue;
                }
                if ends(&t) && !el.is_empty() {
                    out.push(mk_if(c, t, vec![]));
                    out.extend(el);
                } else if ends(&el) && !ends(&t) && (el.len() <= 3 || el.len() <= t.len()) {
                    out.push(mk_if(negate(c), el, vec![]));
                    out.extend(t);
                } else {
                    out.push(mk_if(c, t, el));
                }
            }
            Stmt::For(w, i, l, s, b) => out.push(Stmt::For(w, i, l, s, simplify(b))),
            Stmt::ForIn(w, x, b) => out.push(Stmt::ForIn(w, x, simplify(b))),
            Stmt::While(c, b) => out.push(Stmt::While(c, simplify(b))),
            s => out.push(s),
        }
    }
    out
}
pub fn strip_cont(mut body: Body) -> Body {
    loop {
        match body.last().cloned() {
            Some(Stmt::Continue) => {
                body.pop();
            }
            Some(Stmt::If(mut c, t, e)) => {
                body.pop();
                let (mut t, mut el) = (strip_cont(t), strip_cont(e));
                if !t.is_empty() || !el.is_empty() {
                    if t.is_empty() {
                        c = negate(c);
                        std::mem::swap(&mut t, &mut el);
                    }
                    body.push(Stmt::If(c, t, el));
                    return body;
                }
                if has_call(&c) {
                    body.push(Stmt::CallCond(c));
                    return body;
                }
            }
            _ => return body,
        }
    }
}
pub fn strip_tail_return(mut body: Body) -> Body {
    loop {
        match body.last().cloned() {
            Some(Stmt::Return(xs)) if xs.is_empty() => {
                body.pop();
            }
            Some(Stmt::If(mut c, t, e)) => {
                body.pop();
                let (mut t, mut el) = (
                    if e.is_empty() {
                        t
                    } else {
                        strip_tail_return(t)
                    },
                    strip_tail_return(e),
                );
                if !t.is_empty() || !el.is_empty() {
                    if t.is_empty() {
                        c = negate(c);
                        std::mem::swap(&mut t, &mut el);
                    }
                    body.push(Stmt::If(c, t, el));
                    return body;
                }
                if has_call(&c) {
                    body.push(Stmt::CallCond(c));
                    return body;
                }
            }
            _ => return body,
        }
    }
}

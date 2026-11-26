//! Lexical naming and precedence-aware Luau source printing.
use crate::decomp::{fmt_num, lua_str};
use crate::error::Result;
use crate::gen::{suggest, Generator};
use crate::ir::*;
use crate::post::{flat, is_boolish};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

const KEYWORDS:&str="and break do else elseif end false for function if in local nil not or repeat return then true until while continue";
const GLOBALS:&str="game workspace script Instance Enum task Vector2 Vector3 Vector2int16 Vector3int16 CFrame Color3 BrickColor UDim UDim2 TweenInfo Ray Rect Region3 Region3int16 NumberRange NumberSequence NumberSequenceKeypoint ColorSequence ColorSequenceKeypoint PhysicalProperties Random DateTime Font OverlapParams RaycastParams PathWaypoint Content CatalogSearchParams DockWidgetPluginGuiInfo FloatCurveKey RotationCurveKey math string table os utf8 bit32 coroutine debug buffer io print warn error pcall xpcall tostring tonumber type typeof pairs ipairs next select unpack rawget rawset rawequal setmetatable getmetatable assert require wait spawn delay yield tick time elapsedTime getfenv setfenv loadstring newproxy gcinfo collectgarbage version shared _G _VERSION PluginManager settings Debug Drawing syn request http_request isfile readfile writefile identifyexecutor gethui hookfunction newcclosure clonefunction getrawmetatable setreadonly islclosure checkcaller fireclickdetector firetouchinterest getconnections setclipboard queue_on_teleport
cache cloneref compareinstances crypt base64 base64_decode base64_encode base64decode base64encode lz4compress lz4decompress zstdcompress zstddecompress json appendfile delfile delfolder deletefile deletefolder dofile getcustomasset getsynasset writecustomasset isfolder listfiles loadfile makefolder openfiledialog openfilesdialog openfolderdialog savefiledialog rconsoleclear rconsolecreate rconsoledestroy rconsoleerr rconsolehide rconsoleinfo rconsoleinput rconsolename rconsoleprint rconsoleshow rconsolesettitle rconsolewarn rconsoledebug consoleclear consolecreate consoledestroy consoleerr consolehide consoleinfo consoleinput consolename consoleprint consoleshow consolesettitle consolewarn consoledebug consoleerror cleardrawcache getrenderproperty setrenderproperty getrenderobj setrenderobj isrenderobj get_render_property set_render_property is_render_property isrenderproperty imgui isrbxactive isgameactive iswindowactive keyclick keypress keyrelease keytap mouse1click mouse1press mouse1release mouse2click mouse2press mouse2release mousemoveabs mousemoverel mousescroll fireproximityprompt getcallbackvalue getinstances getnilinstances get_nil_instances get_instances getobjects isnetworkowner testisnetworkowner setsimulationradius gethiddenproperty isscriptable sethiddenproperty setscriptable getscriptable protectinstance set_secure_instance setsecureinstance set_normal_instance setnormalinstance is_secured_instance issecuredinstance createsecurefolder create_secure_folder decompile dumpstring getcallingscript get_calling_script getscriptcaller getcaller getinstancesbyclass getinstancesofclass getloadedmodules get_loaded_modules getmodules get_modules getmenv getreferencedby getrunningscripts getscriptbytecode getscriptclosure getscripthash getscripts get_scripts getscriptthread getscriptfunction get_script_function getsenv islocalsourcecontainer cansignalreplicate firesignal getsignalarguments getsignalargumentsinfo getsignalwhitelist getallreplicatesignals replicatesignal restoreclosure restoresignal hooksignal lockassetsignal issignalhooked isconnectionenabled enableconnection disableconnection WebSocket create_comm_channel get_comm_channel getactorfromthread getactors getactorthreads getcurrentactor getdeletedactors get_deleted_actors get_actors get_actor_from_thread get_thread_actor getthreadactor getthreadscript isparallel is_parallel checkparallel run_on_actor runonactor run_on_thread runonthread getthreadidentity get_thread_identity setthreadidentity set_thread_identity getthreadcontext get_thread_context setthreadcontext set_thread_context getidentity setidentity getcontext setcontext checkcaller clonefunc comparefunctions comparefunction getfunctionhash hookfunc hookmetamethod iscclosure is_c_closure isexecutorclosure is_executor_closure isfunctionhooked ishiddenstack isstackhidden is_l_closure isluaclosure isnewcclosure newlclosure restorefunction restorefunc sethiddenstack setstackhidden checkclosure isourclosure is_our_closure isourcallstack is_synapse_function issynapsefunction is_fluxus_function isfluxusfunction is_fluxus_closure isfluxusclosure is_krnl_closure is_protosmasher_closure protectfunction protectclosure detourfunction detour_function replacefunction replacefunc replaceclosure getconstant getconstants getinfo getproto getprotos getreg getregistry getstack getupvalue getupvalues isvalidlevel validlevel setconstant setstack setupvalue get_gc_objects filtergc filter_gc getgc debuginstance disassemble islineinfo getgenv getrenv getrenvshared getrenvglobal getscriptglobals gettenv getstateenv toclipboard getclipboard fromclipboard setrbxclipboard getmetafield getnamecallmethod get_namecall_method hasmetafield hasmetatable isreadonly makereadonly make_readonly iswritable iswriteable makewritable make_writeable makewriteable setnamecallmethod set_namecall_method setrawmetatable make_writable getscriptfromthread isourthread is_our_thread setclosurecaps setclosureidentity clear_teleport_queue clearteleportqueue queueonteleport getexecutorname getexecutorversion getfflag getfflagtype getfpscap gethwid get_hwid info messagebox messageboxasync httpget setfflag setfpscap RakNet Signal signal actor bit http SCRIPTABLE_TABLE saveinstance updateuiprocinfo printuiwarn printuierror printuiconsole erroruiconsole infouiconsole warnuiconsole debuguiconsole printdebug debugprint loginfo logdebug logwarn logerror logprint";
pub fn identifier(s: &str) -> bool {
    let mut bs = s.bytes();
    bs.next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == b'_')
        && bs.all(|c| c.is_ascii_alphanumeric() || c == b'_')
        && !KEYWORDS.split_whitespace().any(|k| k == s)
}
fn latin_bytes(s: &str) -> Vec<u8> {
    s.chars().map(|c| c as u8).collect()
}
pub fn wkey(w: &Web) -> String {
    if w.kind == "param" {
        format!("{}:p{}", w.pid, w.param_index)
    } else {
        format!("{}:{}", w.pid, w.defs.first().map(|d| d.id).unwrap_or(0))
    }
}
pub struct Scope {
    parent: Option<Rc<Scope>>,
    used: Rc<RefCell<BTreeSet<String>>>,
}
impl Scope {
    pub fn new(parent: Option<Rc<Scope>>, function: bool) -> Rc<Self> {
        let used = match (&parent, function) {
            (Some(parent), false) => parent.used.clone(),
            _ => Rc::new(RefCell::new(BTreeSet::new())),
        };
        Rc::new(Self { parent, used })
    }
    fn visible(&self, name: &str) -> bool {
        self.used.borrow().contains(name) || self.parent.as_ref().is_some_and(|p| p.visible(name))
    }
}
fn prec(op: &str) -> i32 {
    match op {
        "or" => 1,
        "and" => 2,
        "==" | "~=" | "<" | ">" | "<=" | ">=" => 3,
        ".." => 4,
        "+" | "-" => 5,
        "*" | "/" | "%" | "//" => 6,
        "^" => 8,
        _ => 0,
    }
}
fn chain(op: &str, xs: &[E]) -> E {
    let mut x = xs.last().unwrap().clone();
    for a in xs[..xs.len() - 1].iter().rev() {
        x = bin(op, a.clone(), x);
    }
    x
}
fn group_or(xs: Vec<E>) -> Vec<E> {
    let unnot = |x: &E| {
        if let Expr::Not(v) = x.as_ref() {
            v.clone()
        } else {
            x.clone()
        }
    };
    if xs.len() >= 2 && xs.iter().all(|x| matches!(x.as_ref(), Expr::Not(_))) {
        return vec![not(chain("and", &xs.iter().map(unnot).collect::<Vec<_>>()))];
    }
    let mut out = vec![];
    let mut i = 0;
    while i < xs.len() {
        if matches!(xs[i].as_ref(), Expr::Not(_)) {
            let mut j = i;
            while j < xs.len() && matches!(xs[j].as_ref(), Expr::Not(_)) {
                j += 1;
            }
            if j - i >= 3 {
                out.push(not(chain(
                    "and",
                    &xs[i..j].iter().map(unnot).collect::<Vec<_>>(),
                )));
                i = j;
                continue;
            }
        }
        out.push(xs[i].clone());
        i += 1;
    }
    out
}
fn nots(x: &E) -> usize {
    match x.as_ref() {
        Expr::Not(v) => {
            (if matches!(v.as_ref(),Expr::Bin(op,_,_)if matches!(op.as_str(),"and"|"or")) {
                3
            } else {
                1
            }) + nots(v)
        }
        Expr::Bin(op, _, _) if matches!(op.as_str(), "and" | "or") => {
            let xs = flat(x, op);
            let xs = if op == "or" { group_or(xs) } else { xs };
            xs.iter().map(nots).sum()
        }
        _ => 0,
    }
}
fn negate_full(x: &E) -> E {
    match x.as_ref() {
        Expr::Not(x) => tru(x),
        Expr::Bin(op, l, r) if matches!(op.as_str(), "and" | "or") => bin(
            if op == "and" { "or" } else { "and" },
            negate_full(l),
            negate_full(r),
        ),
        Expr::Bin(op, l, r) if op == "==" => bin("~=", l.clone(), r.clone()),
        Expr::Bin(op, l, r) if op == "~=" => bin("==", l.clone(), r.clone()),
        _ => not(x.clone()),
    }
}
pub fn tru(x: &E) -> E {
    match x.as_ref() {
        Expr::Not(x) => match x.as_ref() {
            Expr::Not(x) => tru(x),
            Expr::Bin(op, _, _) if matches!(op.as_str(), "and" | "or") => {
                let full = negate_full(x);
                let base = not(tru(x));
                if nots(&full) < nots(&base) {
                    full
                } else {
                    base
                }
            }
            Expr::Bin(op, l, r) if op == "==" => bin("~=", l.clone(), r.clone()),
            Expr::Bin(op, l, r) if op == "~=" => bin("==", l.clone(), r.clone()),
            _ => not(tru(x)),
        },
        Expr::Bin(op, l, r) if matches!(op.as_str(), "and" | "or") => bin(op, tru(l), tru(r)),
        _ => x.clone(),
    }
}
pub struct Printer {
    pub gen: Generator,
    pub reserved: BTreeSet<String>,
    pub names: BTreeMap<String, String>,
    pub annotate: bool,
    pub used_keys: BTreeSet<String>,
    children: BTreeMap<usize, Function>,
    pub fn_text: BTreeMap<i64, String>,
}
impl Printer {
    pub fn new(
        gen: Generator,
        reserved: BTreeSet<String>,
        names: BTreeMap<String, String>,
        annotate: bool,
    ) -> Self {
        let mut reserved = reserved;
        reserved.extend(
            KEYWORDS
                .split_whitespace()
                .chain(GLOBALS.split_whitespace())
                .map(str::to_owned),
        );
        Self {
            gen,
            reserved,
            names,
            annotate,
            used_keys: BTreeSet::new(),
            children: BTreeMap::new(),
            fn_text: BTreeMap::new(),
        }
    }
    pub fn name_of(&mut self, w: WebId, sc: &Rc<Scope>) -> String {
        if let Some(n) = &self.gen.ctx.webs[w].name {
            return n.clone();
        }
        let key = wkey(&self.gen.ctx.webs[w]);
        let mapped = self.names.get(&key).filter(|s| !s.is_empty()).cloned();
        if mapped.is_some() {
            self.used_keys.insert(key);
        }
        let hint = self.gen.ctx.webs[w].hint.clone();
        let base = if let Some(m) = mapped {
            m
        } else {
            match hint {
                Some(Hint::Name(s)) => s,
                Some(Hint::Clone(b)) => format!("{}Clone", self.name_of(b, sc)),
                None => match self.gen.ctx.webs[w].kind {
                    "param" => "argument",
                    "loop" => "index",
                    _ => "value",
                }
                .into(),
            }
        };
        let mut base: String = base
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        if base.is_empty() {
            base = "value".into();
        }
        if base.as_bytes()[0].is_ascii_digit() {
            base.insert(0, '_');
        }
        let mut candidate = base.clone();
        let mut i = 2;
        while self.reserved.contains(&candidate) || sc.visible(&candidate) {
            candidate = format!("{base}{i}");
            i += 1;
        }
        sc.used.borrow_mut().insert(candidate.clone());
        self.gen.ctx.webs[w].name = Some(candidate.clone());
        candidate
    }
    fn dn(&mut self, w: WebId, sc: &Rc<Scope>) -> String {
        let name = self.name_of(w, sc);
        if self.annotate {
            format!("{name}<@{}>", wkey(&self.gen.ctx.webs[w]))
        } else {
            name
        }
    }
    fn key_str(&self, k: &str) -> String {
        if identifier(k) {
            format!(".{k}")
        } else {
            format!("[{}]", lua_str(&latin_bytes(k)))
        }
    }
    fn atom(&mut self, x: &E, sc: &Rc<Scope>, ind: &str) -> Result<String> {
        if matches!(x.as_ref(), Expr::Vararg(_)) {
            return Ok("(...)".into());
        }
        if matches!(
            x.as_ref(),
            Expr::Bin(..) | Expr::Not(_) | Expr::Len(_) | Expr::Neg(_)
        ) {
            return Ok(format!("({})", self.ex(x, sc, ind, 0)?));
        }
        let s = self.ex(x, sc, ind, 100)?;
        if x.is_const()
            || matches!(
                x.as_ref(),
                Expr::Func { .. } | Expr::Table(_) | Expr::Multi(_)
            )
        {
            Ok(format!("({s})"))
        } else {
            Ok(s)
        }
    }
    fn cut(x: &E) -> E {
        if matches!(x.as_ref(), Expr::Call { .. }) {
            paren(x.clone())
        } else {
            x.clone()
        }
    }
    fn rhs(&mut self, x: &E, sc: &Rc<Scope>, ind: &str) -> Result<String> {
        if matches!(x.as_ref(), Expr::Vararg(_)) {
            Ok("...".into())
        } else {
            self.ex(x, sc, ind, 0)
        }
    }
    pub fn ex(&mut self, x: &E, sc: &Rc<Scope>, ind: &str, p: i32) -> Result<String> {
        Ok(match x.as_ref() {
            Expr::Str(b) => lua_str(b),
            Expr::Num(n, _) => {
                let s = fmt_num(*n);
                if s.starts_with('-') && p > 5 {
                    format!("({s})")
                } else {
                    s
                }
            }
            Expr::Bool(v) => v.to_string(),
            Expr::Nil => "nil".into(),
            Expr::Imp(path) => {
                let mut s = path.first().ok_or_else(|| err("empty import"))?.clone();
                for k in &path[1..] {
                    s.push_str(&self.key_str(k));
                }
                s
            }
            Expr::Var(w) => self.name_of(*w, sc),
            Expr::Idx(o, k) => {
                let object = self.atom(o, sc, ind)?;
                match k {
                    Key::Str(s) => object + &self.key_str(s),
                    Key::Expr(k) => {
                        if let Expr::Str(b) = k.as_ref() {
                            let s: String = b.iter().copied().map(char::from).collect();
                            object + &self.key_str(&s)
                        } else {
                            format!("{object}[{}]", self.ex(k, sc, ind, 0)?)
                        }
                    }
                }
            }
            Expr::Multi(x) => self.ex(x, sc, ind, p)?,
            Expr::Vararg(single) => if *single { "(...)" } else { "..." }.into(),
            Expr::Paren(x) => format!("({})", self.ex(x, sc, ind, 0)?),
            Expr::Conditional(c, y, n) => {
                let text = format!(
                    "if {} then {} else {}",
                    self.ex(c, sc, ind, 0)?,
                    self.ex(y, sc, ind, 0)?,
                    self.ex(n, sc, ind, 0)?
                );
                if p != 0 {
                    format!("({text})")
                } else {
                    text
                }
            }
            Expr::Call { func, args, method } => {
                let mut texts = vec![];
                for (i, arg) in args.iter().enumerate() {
                    let arg = if i + 1 == args.len() {
                        Self::cut(arg)
                    } else {
                        arg.clone()
                    };
                    if matches!(arg.as_ref(), Expr::Func { .. })
                        && method
                            .as_ref()
                            .is_some_and(|m| matches!(m.as_str(), "Connect" | "Once" | "connect"))
                        && i + 1 == args.len()
                    {
                        let key = if let Expr::Idx(_, Key::Str(k)) = func.as_ref() {
                            Some(k.as_str())
                        } else {
                            None
                        };
                        let hints = event_params(key.unwrap_or(""));
                        texts.push(self.func(&arg, sc, ind, hints)?);
                    } else {
                        texts.push(self.ex(&arg, sc, ind, 0)?);
                    }
                }
                let object = self.atom(func, sc, ind)?;
                if let Some(m) = method {
                    if identifier(m) {
                        format!("{object}:{m}({})", texts.join(", "))
                    } else {
                        let mut xs = vec![object.clone()];
                        xs.extend(texts);
                        format!("{object}[{}]({})", lua_str(&latin_bytes(m)), xs.join(", "))
                    }
                } else {
                    format!("{object}({})", texts.join(", "))
                }
            }
            Expr::Bin(op, l, r) => {
                if matches!(op.as_str(), "<" | "<=")
                    && matches!(l.as_ref(), Expr::Num(..))
                    && !r.is_const()
                {
                    return self.ex(
                        &bin(if op == "<" { ">" } else { ">=" }, r.clone(), l.clone()),
                        sc,
                        ind,
                        p,
                    );
                }
                if op == "or" {
                    let ops = flat(x, "or");
                    let grouped = group_or(ops.clone());
                    if grouped.len() < ops.len() {
                        return self.ex(&chain("or", &grouped), sc, ind, p);
                    }
                }
                let q = prec(op);
                let (lp, rp) = if matches!(op.as_str(), "^" | "..") {
                    (q + 1, q)
                } else if matches!(op.as_str(), "and" | "or")
                    && matches!(r.as_ref(),Expr::Bin(k,_,_)if k==op)
                {
                    (q, q)
                } else {
                    (q, q + 1)
                };
                let s = format!(
                    "{} {op} {}",
                    self.ex(l, sc, ind, lp)?,
                    self.ex(r, sc, ind, rp)?
                );
                if q < p {
                    format!("({s})")
                } else {
                    s
                }
            }
            Expr::Len(x) => format!("#{}", self.ex(x, sc, ind, 7)?),
            Expr::Not(x) => format!("not {}", self.ex(x, sc, ind, 7)?),
            Expr::Neg(x) => {
                let mut s = self.ex(x, sc, ind, 7)?;
                if s.starts_with('-') {
                    s = format!("({s})");
                }
                format!("-{s}")
            }
            Expr::Table(t) => {
                let ind2 = format!("{ind}\t");
                let mut parts = vec![];
                for (k, v) in &t.fields {
                    if let Some(v) = v {
                        let key = if identifier(k) {
                            k.clone()
                        } else {
                            format!("[{}]", lua_str(&latin_bytes(k)))
                        };
                        parts.push(format!("{key} = {}", self.ex(v, sc, &ind2, 0)?));
                    }
                }
                for (i, x) in t.items.iter().enumerate() {
                    parts.push(self.ex(
                        &if i + 1 == t.items.len() {
                            Self::cut(x)
                        } else {
                            x.clone()
                        },
                        sc,
                        &ind2,
                        0,
                    )?);
                }
                if parts.is_empty() {
                    "{}".into()
                } else {
                    let one = format!("{{{}}}", parts.join(", "));
                    if one.chars().count() > 110 && parts.len() > 1 {
                        format!(
                            "{{\n{}\n{ind}}}",
                            parts
                                .iter()
                                .map(|s| format!("{ind}\t{s}"))
                                .collect::<Vec<_>>()
                                .join(",\n")
                        )
                    } else {
                        one
                    }
                }
            }
            Expr::Func { .. } => self.func(x, sc, ind, None)?,
            Expr::Upv(_) => return Err(err("cannot print unresolved upvalue")),
        })
    }
    fn child(&mut self, x: &E) -> Result<Function> {
        let key = Rc::as_ptr(x) as usize;
        if let Some(f) = self.children.get(&key) {
            return Ok(f.clone());
        }
        let (pid, caps) = if let Expr::Func { pid, caps } = x.as_ref() {
            (*pid, caps.clone())
        } else {
            return Err(err("expected closure"));
        };
        let f = self.gen.run(pid, caps)?;
        self.children.insert(key, f.clone());
        Ok(f)
    }
    fn func(
        &mut self,
        x: &E,
        sc: &Rc<Scope>,
        ind: &str,
        hints: Option<Vec<&str>>,
    ) -> Result<String> {
        let child = self.child(x)?;
        self.render_fn(&child, sc, ind, hints, "function")
    }
    fn render_fn(
        &mut self,
        f: &Function,
        sc: &Rc<Scope>,
        ind: &str,
        hints: Option<Vec<&str>>,
        prefix: &str,
    ) -> Result<String> {
        let sc = Scope::new(Some(sc.clone()), true);
        let mut names = vec![];
        for (i, &w) in f.params.iter().enumerate() {
            if let Some(h) = hints.as_ref().and_then(|h| h.get(i)) {
                if self.gen.ctx.webs[w].name.is_none() {
                    self.gen.ctx.webs[w].hint = Some(Hint::Name((*h).into()));
                }
            }
            names.push(self.dn(w, &sc));
        }
        if f.p.va {
            names.push("...".into());
        }
        let head = format!("{prefix}({})", names.join(", "));
        let body = self.block(&f.body, &sc, &format!("{ind}\t"))?;
        let text = if body.is_empty() {
            format!("{head} end")
        } else {
            format!("{head}\n{}\n{ind}end", body.join("\n"))
        };
        if self.annotate {
            self.fn_text.insert(f.pid, text.clone());
        }
        Ok(text)
    }
    fn forin_hint(&self, xs: &[E], i: usize) -> Option<Hint> {
        let x = xs.first()?;
        let x = if let Expr::Multi(x) = x.as_ref() {
            x
        } else {
            x
        };
        let name = match x.as_ref() {
            Expr::Imp(p) if p.len() == 1 => Some(p[0].as_str()),
            Expr::Call { func, .. } => {
                if let Expr::Imp(p) = func.as_ref() {
                    if p.len() == 1 {
                        Some(p[0].as_str())
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
            _ => None,
        };
        let hint = match name {
            Some("ipairs") => ["index", "value"].get(i).copied(),
            Some("pairs" | "next") => ["key", "value"].get(i).copied(),
            _ => None,
        };
        hint.map(|s| Hint::Name(s.into()))
    }
    pub fn block(&mut self, body: &[Stmt], sc: &Rc<Scope>, ind: &str) -> Result<Vec<String>> {
        let mut out = vec![];
        let mut prev = None;
        let mut i = 0;
        while i < body.len() {
            let s = &body[i];
            let lines = if let Some((targets, n)) = self.multi_assign(body, i) {
                let value = if let Stmt::LocalN(_, x) = s {
                    x
                } else {
                    unreachable!()
                };
                let mut xs = vec![];
                for t in targets {
                    xs.push(self.ex(&t, sc, ind, 0)?);
                }
                i += n;
                vec![format!(
                    "{ind}{} = {}",
                    xs.join(", "),
                    self.ex(value, sc, ind, 0)?
                )]
            } else {
                self.stmt(s, sc, ind, &body[i + 1..])?
            };
            i += 1;
            if lines.is_empty() {
                continue;
            }
            let n: usize = lines.iter().map(|s| s.matches('\n').count() + 1).sum();
            let kind = if n == 1 {
                "small"
            } else if !ind.is_empty()
                && matches!(s,Stmt::If(_,t,e)if e.is_empty()&&n<=4&&(1..=2).contains(&t.len())&&matches!(t.last(),Some(Stmt::Return(_)|Stmt::Break|Stmt::Continue)))
            {
                "guard"
            } else {
                "big"
            };
            if let Some(p) = prev {
                if kind == "big" || p == "big" || kind == "guard" && p == "guard" {
                    out.push(String::new());
                }
            }
            out.extend(lines);
            prev = Some(kind);
        }
        Ok(out)
    }
    fn multi_assign(&self, body: &[Stmt], i: usize) -> Option<(Vec<E>, usize)> {
        let ws = if let Stmt::LocalN(ws, _) = &body[i] {
            ws
        } else {
            return None;
        };
        if ws.len() < 2 || ws.iter().any(Option::is_none) {
            return None;
        }
        let mut targets = vec![];
        for (k, w) in ws.iter().enumerate() {
            let w = w.unwrap();
            let s = body.get(i + 1 + k)?;
            let (t, v) = if let Stmt::Assign(t, v) = s {
                (t, v)
            } else {
                return None;
            };
            if !matches!(t.as_ref(),Expr::Var(t)if *t!=w)
                || !matches!(v.as_ref(),Expr::Var(v)if *v==w)
                || self.gen.ctx.webs[w].uses.len() != 1
                || self.gen.ctx.webs[w].captured
            {
                return None;
            }
            targets.push(t.clone());
        }
        Some((targets, ws.len()))
    }
    fn assign_hint(&mut self, w: WebId, body: &[Stmt]) -> Result<Option<Hint>> {
        for s in body {
            if let Stmt::Assign(t, x) = s {
                if matches!(t.as_ref(),Expr::Var(v)if *v==w) {
                    if let Some(h @ Hint::Name(_)) = suggest(x, &self.gen.ctx) {
                        return Ok(Some(h));
                    }
                }
            }
            let xs = match s {
                Stmt::Local(_, Some(x))
                | Stmt::Assign(_, x)
                | Stmt::Call(x)
                | Stmt::CallCond(x)
                | Stmt::If(x, _, _) => vec![x.clone()],
                _ => vec![],
            };
            for x in xs {
                if let Some(h) = self.assign_hint_expr(w, &x)? {
                    return Ok(Some(h));
                }
            }
            let subs: Vec<&Body> = match s {
                Stmt::If(_, t, e) => vec![t, e],
                Stmt::For(_, _, _, _, b) | Stmt::ForIn(_, _, b) | Stmt::While(_, b) => vec![b],
                _ => vec![],
            };
            for b in subs {
                if let Some(h) = self.assign_hint(w, b)? {
                    return Ok(Some(h));
                }
            }
        }
        Ok(None)
    }
    fn assign_hint_expr(&mut self, w: WebId, x: &E) -> Result<Option<Hint>> {
        if let Expr::Func { caps, .. } = x.as_ref() {
            if caps.contains(&w) {
                let child = self.child(x)?;
                return self.assign_hint(w, &child.body);
            }
            return Ok(None);
        }
        for x in children(x) {
            if let Some(h) = self.assign_hint_expr(w, &x)? {
                return Ok(Some(h));
            }
        }
        Ok(None)
    }
    fn stmt(&mut self, s: &Stmt, sc: &Rc<Scope>, ind: &str, rest: &[Stmt]) -> Result<Vec<String>> {
        let ind2 = format!("{ind}\t");
        Ok(match s {
            Stmt::Local(w, x) => {
                if let Some(x) = x {
                    if !matches!(x.as_ref(), Expr::Nil) {
                        if let Some(h) = suggest(x, &self.gen.ctx) {
                            self.gen.ctx.webs[*w].hint = Some(h);
                        }
                    }
                }
                if self.gen.ctx.webs[*w].hint.is_none() {
                    self.gen.ctx.webs[*w].hint = self.assign_hint(*w, rest)?;
                }
                let name = self.dn(*w, sc);
                if x.as_ref().is_none_or(|x| matches!(x.as_ref(), Expr::Nil)) {
                    vec![format!("{ind}local {name}")]
                } else {
                    let x = x.as_ref().unwrap();
                    if matches!(x.as_ref(), Expr::Func { .. }) {
                        let child = self.child(x)?;
                        vec![format!(
                            "{ind}{}",
                            self.render_fn(
                                &child,
                                sc,
                                ind,
                                None,
                                &format!("local function {name}")
                            )?
                        )]
                    } else {
                        vec![format!("{ind}local {name} = {}", self.rhs(x, sc, ind)?)]
                    }
                }
            }
            Stmt::Assign(t, x) => {
                let t = self.ex(t, sc, ind, 0)?;
                vec![format!("{ind}{t} = {}", self.rhs(x, sc, ind)?)]
            }
            Stmt::Call(x) => {
                let mut text = self.ex(x, sc, ind, 0)?;
                if text.starts_with('(') {
                    text.insert(0, ';');
                }
                vec![format!("{ind}{text}")]
            }
            Stmt::CallCond(x) => {
                if is_boolish(x) {
                    vec![
                        format!("{ind}if {} then", self.ex(&tru(x), sc, ind, 0)?),
                        format!("{ind}end"),
                    ]
                } else {
                    vec![format!("{ind}local _ = {}", self.ex(x, sc, ind, 0)?)]
                }
            }
            Stmt::Return(xs) => {
                if xs.is_empty() {
                    vec![format!("{ind}return")]
                } else {
                    let mut out = vec![];
                    for (i, x) in xs.iter().enumerate() {
                        out.push(self.ex(
                            &if i + 1 == xs.len() {
                                Self::cut(x)
                            } else {
                                x.clone()
                            },
                            sc,
                            ind,
                            0,
                        )?);
                    }
                    vec![format!("{ind}return {}", out.join(", "))]
                }
            }
            Stmt::If(c, t, el) => {
                let mut out = vec![format!("{ind}if {} then", self.ex(&tru(c), sc, ind, 0)?)];
                out.extend(self.block(t, &Scope::new(Some(sc.clone()), false), &ind2)?);
                let mut el = el;
                while el.len() == 1 {
                    if let Stmt::If(c, t, e) = &el[0] {
                        out.push(format!(
                            "{ind}elseif {} then",
                            self.ex(&tru(c), sc, ind, 0)?
                        ));
                        out.extend(self.block(t, &Scope::new(Some(sc.clone()), false), &ind2)?);
                        el = e;
                    } else {
                        break;
                    }
                }
                if !el.is_empty() {
                    out.push(format!("{ind}else"));
                    out.extend(self.block(el, &Scope::new(Some(sc.clone()), false), &ind2)?);
                }
                out.push(format!("{ind}end"));
                out
            }
            Stmt::Break => vec![format!("{ind}break")],
            Stmt::Continue => vec![format!("{ind}continue")],
            Stmt::While(c, b) => {
                let mut out = vec![format!("{ind}while {} do", self.ex(&tru(c), sc, ind, 0)?)];
                out.extend(self.block(b, &Scope::new(Some(sc.clone()), false), &ind2)?);
                out.push(format!("{ind}end"));
                out
            }
            Stmt::LocalN(ws, x) => {
                let mut names = vec![];
                for w in ws {
                    names.push(if let Some(w) = w {
                        self.dn(*w, sc)
                    } else {
                        "_".into()
                    });
                }
                vec![format!(
                    "{ind}local {} = {}",
                    names.join(", "),
                    self.ex(x, sc, ind, 0)?
                )]
            }
            Stmt::ForIn(ws, xs, b) => {
                let inner = Scope::new(Some(sc.clone()), false);
                let mut names = vec![];
                for (i, &w) in ws.iter().enumerate() {
                    if self.gen.ctx.webs[w].uses.is_empty() {
                        names.push("_".into());
                    } else {
                        if self.gen.ctx.webs[w].hint.is_none() {
                            self.gen.ctx.webs[w].hint = self.forin_hint(xs, i);
                        }
                        names.push(self.dn(w, &inner));
                    }
                }
                let mut values = vec![];
                for (i, x) in xs.iter().enumerate() {
                    values.push(self.ex(
                        &if i + 1 == xs.len() {
                            Self::cut(x)
                        } else {
                            x.clone()
                        },
                        sc,
                        ind,
                        0,
                    )?);
                }
                if xs.len() == 3 && matches!(xs[2].as_ref(), Expr::Nil) {
                    values.truncate(2);
                }
                let mut out = vec![format!(
                    "{ind}for {} in {} do",
                    names.join(", "),
                    values.join(", ")
                )];
                out.extend(self.block(b, &inner, &ind2)?);
                out.push(format!("{ind}end"));
                out
            }
            Stmt::For(w, i, l, step, b) => {
                let inner = Scope::new(Some(sc.clone()), false);
                let name = self.dn(*w, &inner);
                let mut header = format!(
                    "{ind}for {name} = {}, {}",
                    self.ex(i, sc, ind, 0)?,
                    self.ex(l, sc, ind, 0)?
                );
                if !matches!(step.as_ref(),Expr::Num(v,_)if *v==1.0) {
                    header.push_str(&format!(", {}", self.ex(step, sc, ind, 0)?));
                }
                header.push_str(" do");
                let mut out = vec![header];
                out.extend(self.block(b, &inner, &ind2)?);
                out.push(format!("{ind}end"));
                out
            }
        })
    }
}
fn event_params(event: &str) -> Option<Vec<&'static str>> {
    Some(match event {
        "InputBegan" | "InputChanged" | "InputEnded" => vec!["input", "gameProcessed"],
        "Touched" | "TouchEnded" => vec!["hit"],
        "Heartbeat" | "RenderStepped" => vec!["deltaTime"],
        "Stepped" => vec!["time", "deltaTime"],
        "PlayerAdded" | "PlayerRemoving" => vec!["player"],
        "CharacterAdded" => vec!["character"],
        "ChildAdded" | "ChildRemoved" => vec!["child"],
        "DescendantAdded" => vec!["descendant"],
        "FocusLost" => vec!["enterPressed", "inputThatCausedFocusLoss"],
        "MouseMoved" => vec!["x", "y"],
        "Changed" => vec!["value"],
        "OnClientEvent" | "Died" => vec![],
        _ => return None,
    })
}

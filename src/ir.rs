//! Native IR shared only by the graph, generation, postprocessing and printing passes.
use crate::error::{Error, Result};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

pub type WebId = usize;
pub type E = Rc<Expr>;
pub type Body = Vec<Stmt>;
pub type Reg = i64;
pub type DefSet = BTreeSet<usize>;
pub fn err(message: impl Into<String>) -> Error {
    Error::new("decompile", message)
}
pub fn e(expr: Expr) -> E {
    Rc::new(expr)
}
pub fn var(w: WebId) -> E {
    e(Expr::Var(w))
}
pub fn number(n: impl Into<f64>) -> E {
    e(Expr::Num(n.into(), false))
}
pub fn nil() -> E {
    e(Expr::Nil)
}
pub fn bin(op: &str, l: E, r: E) -> E {
    e(Expr::Bin(op.into(), l, r))
}
pub fn not(x: E) -> E {
    e(Expr::Not(x))
}
pub fn idx(o: E, k: Key) -> E {
    e(Expr::Idx(o, k))
}
pub fn multi(x: E) -> E {
    e(Expr::Multi(x))
}
pub fn paren(x: E) -> E {
    e(Expr::Paren(x))
}
#[derive(Clone, Debug, PartialEq)]
pub enum Key {
    Str(String),
    Expr(E),
}
#[derive(Clone, Debug, PartialEq)]
pub struct Table {
    pub items: Vec<E>,
    pub fields: Vec<(String, Option<E>)>,
    pub pending: Vec<String>,
    pub fromnew: bool,
}
impl Table {
    pub fn new(items: Vec<E>) -> Self {
        Self {
            items,
            fields: vec![],
            pending: vec![],
            fromnew: false,
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Str(Vec<u8>),
    Num(f64, bool),
    Bool(bool),
    Nil,
    Imp(Vec<String>),
    Var(WebId),
    Upv(usize),
    Idx(E, Key),
    Call {
        func: E,
        args: Vec<E>,
        method: Option<String>,
    },
    Bin(String, E, E),
    Not(E),
    Len(E),
    Neg(E),
    Table(Table),
    Func {
        pid: i64,
        caps: Vec<WebId>,
    },
    Multi(E),
    Vararg(bool),
    Paren(E),
    Conditional(E, E, E),
}
impl Expr {
    pub fn is_const(&self) -> bool {
        matches!(
            self,
            Self::Str(_) | Self::Num(..) | Self::Bool(_) | Self::Nil
        )
    }
}
#[derive(Clone, Debug, PartialEq)]
pub enum Stmt {
    Local(WebId, Option<E>),
    LocalN(Vec<Option<WebId>>, E),
    Assign(E, E),
    Call(E),
    CallCond(E),
    If(E, Body, Body),
    While(E, Body),
    For(WebId, E, E, E, Body),
    ForIn(Vec<WebId>, Vec<E>, Body),
    Return(Vec<E>),
    Break,
    Continue,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Hint {
    Name(String),
    Clone(WebId),
}
#[derive(Clone, Debug)]
pub struct Def {
    pub id: usize,
    pub node: Option<usize>,
    pub reg: Reg,
}
#[derive(Clone, Debug)]
pub struct Web {
    pub kind: &'static str,
    pub defs: Vec<Def>,
    pub uses: Vec<(usize, Reg)>,
    pub captured: bool,
    pub hint: Option<Hint>,
    pub name: Option<String>,
    pub declared: bool,
    pub owner: Option<usize>,
    pub noinline: bool,
    pub keep: bool,
    pub ctor: bool,
    pub param_index: usize,
    pub pid: i64,
}
impl Web {
    pub fn new(kind: &'static str, owner: Option<usize>, pid: i64) -> Self {
        Self {
            kind,
            defs: vec![],
            uses: vec![],
            captured: false,
            hint: None,
            name: None,
            declared: false,
            owner,
            noinline: false,
            keep: false,
            ctor: false,
            param_index: 0,
            pid,
        }
    }
}
#[derive(Clone, Debug, Default)]
pub struct Args {
    pub a: i64,
    pub b: i64,
    pub c: i64,
    pub rf: i64,
    pub aux: i64,
    pub child: i64,
    pub caps: Vec<(i64, i64)>,
}
impl Args {
    pub fn parse(v: &Value) -> Result<Self> {
        let get = |s: &str| -> Result<i64> { v.get(s).map_or(Ok(0), |x| integer(x, s)) };
        Ok(Self {
            a: get("A")?,
            b: get("B")?,
            c: get("C")?,
            rf: get("rf")?,
            aux: get("aux")?,
            ..Self::default()
        })
    }
}
pub fn integer(v: &Value, what: &str) -> Result<i64> {
    v.as_i64()
        .or_else(|| v.as_u64().and_then(|n| i64::try_from(n).ok()))
        .or_else(|| {
            v.as_f64()
                .filter(|f| {
                    f.is_finite()
                        && f.fract() == 0.0
                        && *f >= i64::MIN as f64
                        && *f < i64::MAX as f64
                })
                .map(|f| f as i64)
        })
        .ok_or_else(|| err(format!("invalid integer {what}: {v}")))
}
#[derive(Clone, Debug)]
pub struct Record {
    pub kind: String,
    pub ins: Vec<(String, Args)>,
    pub ctrl: Option<(String, Args)>,
    pub nxt: i64,
    pub alt: i64,
}
#[derive(Clone, Debug)]
pub struct Proto {
    pub entry: i64,
    pub np: usize,
    pub nup: usize,
    pub va: bool,
    pub kids: Vec<i64>,
    pub consts: BTreeMap<i64, Value>,
    pub decoded: Vec<(i64, Record)>,
}
impl Proto {
    pub fn parse(v: &Value) -> Result<Self> {
        let get = |k: &str| -> Result<usize> {
            let x = integer(
                v.get(k).ok_or_else(|| err(format!("missing proto {k}")))?,
                k,
            )?;
            usize::try_from(x).map_err(|_| err(format!("negative proto {k}")))
        };
        let mut consts = BTreeMap::new();
        if let Some(cs) = v["consts"].as_object() {
            for (k, x) in cs {
                consts.insert(
                    k.parse().map_err(|_| err("invalid constant id"))?,
                    x.clone(),
                );
            }
        } else if let Some(cs) = v["consts"].as_array() {
            for (k, x) in cs.iter().enumerate() {
                consts.insert(k as i64, x.clone());
            }
        } else {
            return Err(err("missing constants"));
        }
        let instruction = |x: &Value| -> Result<(String, Args)> {
            let ar = x
                .as_array()
                .filter(|a| a.len() == 2)
                .ok_or_else(|| err("invalid decoded instruction"))?;
            Ok((
                ar[0]
                    .as_str()
                    .ok_or_else(|| err("invalid instruction name"))?
                    .into(),
                Args::parse(&ar[1])?,
            ))
        };
        let mut decoded = vec![];
        for (k, d) in v["decoded"]
            .as_object()
            .ok_or_else(|| err("missing decoded records"))?
        {
            let rid = k.parse().map_err(|_| err("invalid record id"))?;
            let ins = d["ins"]
                .as_array()
                .ok_or_else(|| err("missing record instructions"))?
                .iter()
                .map(&instruction)
                .collect::<Result<_>>()?;
            let ctrl = if d["ctrl"].is_null() {
                None
            } else {
                Some(instruction(&d["ctrl"])?)
            };
            decoded.push((
                rid,
                Record {
                    kind: d["kind"].as_str().unwrap_or("").into(),
                    ins,
                    ctrl,
                    nxt: integer(&d["nxt"], "nxt")?,
                    alt: integer(&d["alt"], "alt")?,
                },
            ));
        }
        Ok(Self {
            entry: integer(&v["entry"], "entry")?,
            np: get("np")?,
            nup: get("nup")?,
            va: v["va"]
                .as_bool()
                .unwrap_or_else(|| v["va"].as_i64().unwrap_or(0) != 0),
            kids: v["kids"]
                .as_array()
                .ok_or_else(|| err("missing child list"))?
                .iter()
                .map(|x| integer(x, "child"))
                .collect::<Result<_>>()?,
            consts,
            decoded,
        })
    }
    pub fn constant(&self, k: i64) -> Result<E> {
        kconst(
            self.consts
                .get(&k)
                .ok_or_else(|| err(format!("missing constant {k}")))?,
        )
    }
    pub fn kbytes(&self, k: i64) -> Result<String> {
        let c = self
            .consts
            .get(&k)
            .ok_or_else(|| err(format!("missing constant {k}")))?;
        if c[0] != "s" {
            return Err(err("expected string constant"));
        }
        Ok(bytes(&c[1])?.into_iter().map(char::from).collect())
    }
}
pub fn bytes(v: &Value) -> Result<Vec<u8>> {
    v.as_array()
        .ok_or_else(|| err("expected byte array"))?
        .iter()
        .map(|n| {
            n.as_u64()
                .and_then(|n| u8::try_from(n).ok())
                .ok_or_else(|| err("invalid byte"))
        })
        .collect()
}
pub fn kconst(c: &Value) -> Result<E> {
    Ok(match c[0].as_str() {
        Some("s") => e(Expr::Str(bytes(&c[1])?)),
        Some("n") => e(Expr::Num(crate::model::decode_number(&c[1])?, false)),
        Some("b") => e(Expr::Bool(
            c[1].as_bool()
                .unwrap_or_else(|| c[1].as_i64().unwrap_or(0) != 0),
        )),
        _ => nil(),
    })
}
#[derive(Clone, Debug)]
pub struct Node {
    pub kind: String,
    pub a: Args,
    pub rid: i64,
    pub succ: Vec<usize>,
    pub reads: Vec<Reg>,
    pub writes: Vec<Reg>,
    pub preds: Vec<usize>,
    pub blk: usize,
    pub topbase: Option<Reg>,
}
#[derive(Clone, Debug)]
pub enum Term {
    Goto(usize),
    Ret(Vec<E>),
    Cond(E, usize, usize),
    For(WebId, E, E, E, usize, usize),
    ForNLoop(usize, usize),
    ForGPrep(Vec<E>, usize),
    ForGLoop(Vec<WebId>, usize, usize),
}
impl Term {
    pub fn succ(&self) -> Vec<usize> {
        match self {
            Self::Goto(b) | Self::ForGPrep(_, b) => vec![*b],
            Self::Ret(_) => vec![],
            Self::Cond(_, a, b)
            | Self::For(_, _, _, _, a, b)
            | Self::ForNLoop(a, b)
            | Self::ForGLoop(_, a, b) => vec![*a, *b],
        }
    }
}
#[derive(Clone, Debug)]
pub struct Block {
    pub nodes: Vec<usize>,
    pub succ: Vec<usize>,
    pub pred: Vec<usize>,
    pub stmts: Body,
    pub term: Term,
    pub dead: bool,
}
impl Default for Block {
    fn default() -> Self {
        Self {
            nodes: vec![],
            succ: vec![],
            pred: vec![],
            stmts: vec![],
            term: Term::Ret(vec![]),
            dead: false,
        }
    }
}
#[derive(Clone, Debug)]
pub struct Function {
    pub id: usize,
    pub pid: i64,
    pub p: Proto,
    pub upvals: Vec<WebId>,
    pub body: Body,
    pub params: Vec<WebId>,
    pub webs: Vec<WebId>,
    pub nodes: Vec<Node>,
    pub blocks: Vec<Block>,
    pub entry: usize,
    pub entry_block: usize,
    pub use_web: BTreeMap<(usize, Reg), WebId>,
    pub def_web: BTreeMap<(usize, Reg), WebId>,
    pub defs: Vec<Def>,
    pub node_defs: BTreeMap<(usize, Reg), usize>,
    pub read_defs: BTreeMap<(usize, Reg), DefSet>,
}
impl Function {
    pub fn new(id: usize, pid: i64, p: Proto, upvals: Vec<WebId>) -> Self {
        Self {
            id,
            pid,
            p,
            upvals,
            body: vec![],
            params: vec![],
            webs: vec![],
            nodes: vec![],
            blocks: vec![],
            entry: 0,
            entry_block: 0,
            use_web: BTreeMap::new(),
            def_web: BTreeMap::new(),
            defs: vec![],
            node_defs: BTreeMap::new(),
            read_defs: BTreeMap::new(),
        }
    }
}
pub struct Context {
    pub protos: BTreeMap<i64, Proto>,
    pub webs: Vec<Web>,
    pub next_fn: usize,
}
impl Context {
    pub fn web(&mut self, kind: &'static str, owner: Option<usize>, pid: i64) -> WebId {
        let id = self.webs.len();
        self.webs.push(Web::new(kind, owner, pid));
        id
    }
}
pub fn children(x: &E) -> Vec<E> {
    match x.as_ref() {
        Expr::Idx(o, k) => {
            let mut a = vec![o.clone()];
            if let Key::Expr(k) = k {
                a.push(k.clone());
            }
            a
        }
        Expr::Call { func, args, .. } => {
            let mut a = vec![func.clone()];
            a.extend(args.iter().cloned());
            a
        }
        Expr::Bin(_, l, r) => vec![l.clone(), r.clone()],
        Expr::Not(x) | Expr::Len(x) | Expr::Neg(x) | Expr::Multi(x) | Expr::Paren(x) => {
            vec![x.clone()]
        }
        Expr::Table(t) => t
            .items
            .iter()
            .cloned()
            .chain(t.fields.iter().filter_map(|(_, v)| v.clone()))
            .collect(),
        Expr::Conditional(c, y, n) => vec![c.clone(), y.clone(), n.clone()],
        _ => vec![],
    }
}

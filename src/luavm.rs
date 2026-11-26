//! Bounded native interpreter for the handler subset of the Luau AST.
use serde_json::Value;
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    fmt,
    rc::{Rc, Weak},
    time::Instant,
};

pub type Cell = Rc<RefCell<LuaValue>>;
pub type Cells = Rc<RefCell<BTreeMap<String, Cell>>>;
pub type VmResult<T> = std::result::Result<T, VmError>;
type Host = dyn Fn(&mut Interp, Vec<LuaValue>) -> VmResult<Vec<LuaValue>>;

#[derive(Clone)]
pub enum LuaValue {
    Nil,
    Bool(bool),
    Num(f64),
    Str(Vec<u8>),
    Table(Rc<RefCell<LuaTable>>),
    Func(Rc<Function>),
    Host(String, Rc<Host>, Vec<LuaValue>),
    Opaque(String),
}
impl fmt::Debug for LuaValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.repr())
    }
}
#[derive(Debug, Clone)]
pub enum VmError {
    Lua(LuaValue),
    Unsupported(String),
    StepLimit,
    Return(Vec<LuaValue>),
    Break,
    Continue,
}
impl fmt::Display for VmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lua(v) => f.write_str(&v.string()),
            Self::Unsupported(s) => write!(f, "unsupported: {s}"),
            Self::StepLimit => f.write_str("handler execution exceeded its limit"),
            Self::Return(_) => f.write_str("return outside function"),
            Self::Break => f.write_str("break outside loop"),
            Self::Continue => f.write_str("continue outside loop"),
        }
    }
}
impl std::error::Error for VmError {}
pub fn lua_error(msg: impl AsRef<str>) -> VmError {
    VmError::Lua(LuaValue::bytes(msg.as_ref().as_bytes()))
}
pub fn unsupported(msg: impl Into<String>) -> VmError {
    VmError::Unsupported(msg.into())
}

#[derive(Default)]
pub struct LuaTable {
    pub entries: Vec<(LuaValue, LuaValue)>,
    pub mt: Option<LuaValue>,
}
pub struct Function {
    pub node: Value,
    pub cells: Cells,
}
type WeakTable = Weak<RefCell<LuaTable>>;
type WeakCells = Weak<RefCell<BTreeMap<String, Cell>>>;
#[derive(Default)]
struct HeapRegistry {
    next: u64,
    tables: Vec<(u64, WeakTable)>,
    cells: Vec<(u64, WeakCells)>,
}
thread_local! {static HEAP:RefCell<HeapRegistry>=RefCell::new(HeapRegistry::default());}
/// Reclaim a discarded classification trial, including table/closure cycles.
/// Roots include persistent globals and values retained beyond the trial.
pub struct TrialHeap {
    start: u64,
    roots: Vec<LuaValue>,
}
impl TrialHeap {
    pub fn new(roots: Vec<LuaValue>) -> Self {
        Self {
            start: HEAP.with(|h| h.borrow().next),
            roots,
        }
    }
}
impl Drop for TrialHeap {
    fn drop(&mut self) {
        fn mark(v: &LuaValue, tables: &mut BTreeSet<usize>, cells: &mut BTreeSet<usize>) {
            match v {
                LuaValue::Table(t) => {
                    if !tables.insert(Rc::as_ptr(t) as usize) {
                        return;
                    }
                    let t = t.borrow();
                    for (k, v) in &t.entries {
                        mark(k, tables, cells);
                        mark(v, tables, cells);
                    }
                    if let Some(mt) = &t.mt {
                        mark(mt, tables, cells);
                    }
                }
                LuaValue::Func(f) => {
                    if !cells.insert(Rc::as_ptr(&f.cells) as usize) {
                        return;
                    }
                    for c in f.cells.borrow().values() {
                        mark(&c.borrow(), tables, cells);
                    }
                }
                LuaValue::Host(_, _, roots) => {
                    for root in roots {
                        mark(root, tables, cells);
                    }
                }
                _ => {}
            }
        }
        let (mut live_tables, mut live_cells) = (BTreeSet::new(), BTreeSet::new());
        for root in &self.roots {
            mark(root, &mut live_tables, &mut live_cells);
        }
        let (tables, cells) = HEAP.with(|h| {
            let h = h.borrow();
            (
                h.tables
                    .iter()
                    .filter(|(id, _)| *id >= self.start)
                    .map(|(_, w)| w.clone())
                    .collect::<Vec<_>>(),
                h.cells
                    .iter()
                    .filter(|(id, _)| *id >= self.start)
                    .map(|(_, w)| w.clone())
                    .collect::<Vec<_>>(),
            )
        });
        for t in tables {
            if let Some(t) = t.upgrade() {
                if !live_tables.contains(&(Rc::as_ptr(&t) as usize)) {
                    let mut t = t.borrow_mut();
                    t.entries.clear();
                    t.mt = None;
                }
            }
        }
        for c in cells {
            if let Some(c) = c.upgrade() {
                if !live_cells.contains(&(Rc::as_ptr(&c) as usize)) {
                    c.borrow_mut().clear();
                }
            }
        }
        HEAP.with(|h| {
            let mut h = h.borrow_mut();
            h.tables.retain(|(_, w)| w.strong_count() > 0);
            h.cells.retain(|(_, w)| w.strong_count() > 0);
        });
    }
}
#[derive(Clone, Default)]
pub struct Env {
    pub cells: Cells,
    pub varargs: Vec<LuaValue>,
}
impl Env {
    pub fn fork(&self) -> Self {
        Self {
            cells: Rc::new(RefCell::new(self.cells.borrow().clone())),
            varargs: self.varargs.clone(),
        }
    }
    pub fn get(&self, loc: &str) -> Option<LuaValue> {
        self.cells.borrow().get(loc).map(|v| v.borrow().clone())
    }
    pub fn declare(&mut self, loc: String, value: LuaValue) {
        self.cells
            .borrow_mut()
            .insert(loc, Rc::new(RefCell::new(value)));
    }
    pub fn set(&mut self, loc: String, value: LuaValue) {
        let c = self.cells.borrow().get(&loc).cloned();
        if let Some(c) = c {
            *c.borrow_mut() = value;
        } else {
            self.declare(loc, value);
        }
    }
}
pub fn location(local: &Value) -> String {
    local["location"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| local["location"].to_string())
}
pub fn ty(n: &Value) -> &str {
    n["type"].as_str().unwrap_or("")
}
pub fn array(n: &Value) -> &[Value] {
    n.as_array().map(Vec::as_slice).unwrap_or(&[])
}
pub fn unparen(mut n: &Value) -> &Value {
    while ty(n) == "AstExprGroup" {
        n = &n["expr"];
    }
    n
}
pub fn walk<'a>(n: &'a Value, out: &mut Vec<&'a Value>) {
    match n {
        Value::Object(m) => {
            if n.get("type").is_some() {
                out.push(n);
            }
            for v in m.values() {
                walk(v, out);
            }
        }
        Value::Array(a) => {
            for v in a {
                walk(v, out);
            }
        }
        _ => {}
    }
}
pub fn tuple(parts: &[String]) -> String {
    format!(
        "({}{})",
        parts.join(", "),
        if parts.len() == 1 { "," } else { "" }
    )
}
pub fn quoted(s: &str) -> String {
    quoted_bytes(s.as_bytes())
}
pub fn quoted_bytes(s: &[u8]) -> String {
    let delimiter = if s.contains(&b'\'') && !s.contains(&b'"') {
        b'"'
    } else {
        b'\''
    };
    let mut r = String::new();
    r.push(delimiter as char);
    for &b in s {
        match b {
            b'\\' => r.push_str("\\\\"),
            b'\n' => r.push_str("\\n"),
            b'\r' => r.push_str("\\r"),
            b'\t' => r.push_str("\\t"),
            b if b == delimiter => {
                r.push('\\');
                r.push(b as char);
            }
            32..=126 | 161..=172 | 174..=255 => r.push(b as char),
            _ => r.push_str(&format!("\\x{b:02x}")),
        }
    }
    r.push(delimiter as char);
    r
}
impl LuaValue {
    pub fn bytes(v: &[u8]) -> Self {
        Self::Str(v.to_vec())
    }
    pub fn table() -> Self {
        let t = Rc::new(RefCell::new(LuaTable::default()));
        HEAP.with(|h| {
            let mut h = h.borrow_mut();
            let id = h.next;
            h.next += 1;
            h.tables.push((id, Rc::downgrade(&t)));
        });
        Self::Table(t)
    }
    pub fn opaque(parts: Vec<String>) -> Self {
        Self::Opaque(tuple(&parts))
    }
    pub fn host(
        name: &str,
        f: impl Fn(&mut Interp, Vec<Self>) -> VmResult<Vec<Self>> + 'static,
    ) -> Self {
        Self::Host(name.to_owned(), Rc::new(f), vec![])
    }
    pub fn host_with_roots(
        name: &str,
        roots: Vec<Self>,
        f: impl Fn(&mut Interp, Vec<Self>) -> VmResult<Vec<Self>> + 'static,
    ) -> Self {
        Self::Host(name.to_owned(), Rc::new(f), roots)
    }
    pub fn function(node: Value, cells: Cells) -> Self {
        HEAP.with(|h| {
            let mut h = h.borrow_mut();
            let id = h.next;
            h.next += 1;
            h.cells.push((id, Rc::downgrade(&cells)));
        });
        Self::Func(Rc::new(Function { node, cells }))
    }
    pub fn is_nil(&self) -> bool {
        matches!(self, Self::Nil)
    }
    pub fn truthy(&self) -> bool {
        !matches!(self, Self::Nil | Self::Bool(false))
    }
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Nil => "nil",
            Self::Bool(_) => "boolean",
            Self::Num(_) => "number",
            Self::Str(_) => "string",
            Self::Table(_) => "table",
            _ => "function",
        }
    }
    pub fn number(&self) -> VmResult<f64> {
        match self {
            Self::Num(n) => Ok(*n),
            Self::Str(s) => std::str::from_utf8(s)
                .ok()
                .and_then(|s| s.trim().parse().ok())
                .ok_or_else(|| lua_error("attempt to perform arithmetic on a string value")),
            _ => Err(lua_error(format!(
                "attempt to perform arithmetic on a {} value",
                self.type_name()
            ))),
        }
    }
    pub fn integer(&self) -> VmResult<i64> {
        let n = self.number()?;
        if !n.is_finite() {
            return Err(lua_error("number has no integer representation"));
        }
        Ok(n as i64)
    }
    pub fn string(&self) -> String {
        match self {
            Self::Nil => "nil".into(),
            Self::Bool(b) => b.to_string(),
            Self::Num(n) => format_number(*n),
            Self::Str(b) => b.iter().map(|&b| b as char).collect(),
            _ => format!("<{}>", self.type_name()),
        }
    }
    pub fn repr(&self) -> String {
        match self {
            Self::Nil => "None".into(),
            Self::Bool(b) => if *b { "True" } else { "False" }.into(),
            Self::Num(n) => repr_number(*n),
            Self::Str(s) => quoted_bytes(s),
            Self::Opaque(k) => format!("U{k}"),
            Self::Table(_) => "<table>".into(),
            Self::Func(f) => format!("<function {}>", location(&f.node)),
            Self::Host(n, _, _) => format!("<function {n}>"),
        }
    }
    pub fn key(&self) -> String {
        self.key_seen(&mut Vec::new())
    }
    fn key_seen(&self, seen: &mut Vec<usize>) -> String {
        match self {
            Self::Opaque(k) => k.clone(),
            Self::Table(t) => {
                let id = Rc::as_ptr(t) as usize;
                if seen.contains(&id) {
                    return "('cycle',)".into();
                }
                seen.push(id);
                let mut items: Vec<_> = t
                    .borrow()
                    .entries
                    .iter()
                    .map(|(k, v)| tuple(&[k.key_seen(seen), v.key_seen(seen)]))
                    .collect();
                items.sort();
                seen.pop();
                tuple(&[quoted("tbl"), tuple(&items)])
            }
            Self::Func(f) => tuple(&[quoted("fn"), quoted(&location(&f.node))]),
            Self::Host(n, _, _) => tuple(&[quoted("fn"), quoted(n)]),
            _ => tuple(&[
                quoted("lit"),
                quoted(match self {
                    Self::Nil => "NoneType",
                    Self::Bool(_) => "bool",
                    Self::Num(n) if n.fract() != 0.0 || n.abs() >= 9007199254740992.0 => "float",
                    Self::Num(_) => "int",
                    _ => "str",
                }),
                self.repr(),
            ]),
        }
    }
    pub fn get(&self, k: &Self) -> VmResult<Self> {
        if let Self::Table(t) = self {
            Ok(t.borrow()
                .entries
                .iter()
                .find(|(key, _)| lua_eq(key, k))
                .map(|(_, v)| v.clone())
                .unwrap_or(Self::Nil))
        } else {
            Err(lua_error(format!(
                "attempt to index a {} value",
                self.type_name()
            )))
        }
    }
    pub fn set(&self, k: Self, v: Self) -> VmResult<()> {
        if k.is_nil() {
            return Err(lua_error("index is nil"));
        }
        if matches!(k,Self::Num(n) if n.is_nan()) {
            return Err(lua_error("index is NaN"));
        }
        if let Self::Table(t) = self {
            let mut t = t.borrow_mut();
            if let Some(i) = t.entries.iter().position(|(key, _)| lua_eq(key, &k)) {
                if v.is_nil() {
                    t.entries.remove(i);
                } else {
                    t.entries[i].1 = v;
                }
            } else if !v.is_nil() {
                t.entries.push((k, v));
            }
            Ok(())
        } else {
            Err(lua_error("table expected"))
        }
    }
    pub fn length(&self) -> VmResult<usize> {
        match self {
            Self::Str(s) => Ok(s.len()),
            Self::Table(_) => {
                let mut n = 0;
                while !self.get(&Self::Num((n + 1) as f64))?.is_nil() {
                    n += 1;
                }
                Ok(n)
            }
            _ => Err(lua_error(format!(
                "attempt to get length of a {} value",
                self.type_name()
            ))),
        }
    }
    pub fn items(&self) -> VmResult<Vec<(Self, Self)>> {
        if let Self::Table(t) = self {
            let n = self.length()?;
            let mut arr = Vec::new();
            for i in 1..=n {
                let k = Self::Num(i as f64);
                arr.push((k.clone(), self.get(&k)?));
            }
            let mut rest: Vec<_> = t
                .borrow()
                .entries
                .iter()
                .filter(
                    |(k, _)| !matches!(k,Self::Num(x) if x.fract()==0.0 && *x>=1.0 && *x<=n as f64),
                )
                .cloned()
                .collect();
            rest.sort_by_key(|(k, _)| match k {
                Self::Bool(b) => format!("('bool', {})", if *b { "True" } else { "False" }),
                _ => k.repr(),
            });
            arr.extend(rest);
            Ok(arr)
        } else {
            Err(lua_error("table expected"))
        }
    }
    pub fn clear(&self) -> VmResult<()> {
        if let Self::Table(t) = self {
            t.borrow_mut().entries.clear();
            Ok(())
        } else {
            Err(lua_error("table expected"))
        }
    }
}
fn repr_number(n: f64) -> String {
    if !n.is_finite() || n == 0.0 || n.fract() == 0.0 && n.abs() < 9007199254740992.0 {
        return format_number(n);
    }
    let scientific = format!("{n:e}");
    let (mantissa, exponent) = scientific.split_once('e').unwrap();
    let exponent: i32 = exponent.parse().unwrap();
    if !(-4..16).contains(&exponent) {
        format!(
            "{mantissa}e{}{:02}",
            if exponent < 0 { '-' } else { '+' },
            exponent.abs()
        )
    } else if n.fract() == 0.0 {
        format!("{n}.0")
    } else {
        n.to_string()
    }
}
fn format_number(n: f64) -> String {
    if n.is_nan() {
        return "nan".into();
    }
    if n == f64::INFINITY {
        return "inf".into();
    }
    if n == f64::NEG_INFINITY {
        return "-inf".into();
    }
    if n == 0.0 {
        return "0".into();
    }
    if n.fract() == 0.0 && n.abs() < 9007199254740992.0 {
        return format!("{n:.0}");
    }
    let scientific = format!("{n:.13e}");
    let (mantissa, exponent) = scientific.split_once('e').unwrap();
    let exponent: i32 = exponent.parse().unwrap();
    if !(-4..14).contains(&exponent) {
        let m = mantissa.trim_end_matches('0').trim_end_matches('.');
        format!(
            "{m}e{}{:02}",
            if exponent < 0 { '-' } else { '+' },
            exponent.abs()
        )
    } else {
        let s = format!("{:.*}", (13 - exponent).max(0) as usize, n);
        if s.contains('.') {
            s.trim_end_matches('0').trim_end_matches('.').into()
        } else {
            s
        }
    }
}
pub fn lua_eq(a: &LuaValue, b: &LuaValue) -> bool {
    use LuaValue::*;
    match (a, b) {
        (Nil, Nil) => true,
        (Bool(a), Bool(b)) => a == b,
        (Num(a), Num(b)) => a == b,
        (Str(a), Str(b)) => a == b,
        (Opaque(a), Opaque(b)) => a == b,
        (Table(a), Table(b)) => Rc::ptr_eq(a, b),
        (Func(a), Func(b)) => Rc::ptr_eq(a, b),
        (Host(_, a, _), Host(_, b, _)) => Rc::ptr_eq(a, b),
        _ => false,
    }
}
pub fn arith(op: &str, a: LuaValue, b: LuaValue) -> VmResult<LuaValue> {
    if matches!(a, LuaValue::Opaque(_)) || matches!(b, LuaValue::Opaque(_)) {
        return Ok(LuaValue::opaque(vec![quoted(op), a.key(), b.key()]));
    }
    let a = a.number()?;
    let b = b.number()?;
    let n = match op {
        "Add" => a + b,
        "Sub" => a - b,
        "Mul" => a * b,
        "Div" => a / b,
        "FloorDiv" => (a / b).floor(),
        "Mod" => a - (a / b).floor() * b,
        "Pow" => {
            let v = a.powf(b);
            if v.is_infinite() || b < 0.0 && a == 0.0 {
                f64::INFINITY
            } else {
                v
            }
        }
        _ => return Err(unsupported(op)),
    };
    Ok(LuaValue::Num(n))
}
pub fn compare(op: &str, a: &LuaValue, b: &LuaValue) -> VmResult<bool> {
    if op == "CompareEq" {
        return Ok(lua_eq(a, b));
    }
    if op == "CompareNe" {
        return Ok(!lua_eq(a, b));
    }
    let (op, a, b) = match op {
        "CompareGt" => ("CompareLt", b, a),
        "CompareGe" => ("CompareLe", b, a),
        _ => (op, a, b),
    };
    if matches!(a, LuaValue::Opaque(_)) || matches!(b, LuaValue::Opaque(_)) {
        let key = tuple(&[quoted(op), a.key(), b.key()]);
        let bytes: Vec<_> = key
            .chars()
            .map(|c| u8::try_from(c as u32).unwrap_or(b'?'))
            .collect();
        return Ok(md5_first(&bytes) & 1 != 0);
    }
    match (a, b) {
        (LuaValue::Num(a), LuaValue::Num(b)) => Ok(if op == "CompareLt" { a < b } else { a <= b }),
        (LuaValue::Str(a), LuaValue::Str(b)) => Ok(if op == "CompareLt" { a < b } else { a <= b }),
        _ => Err(lua_error(format!(
            "attempt to compare {} with {}",
            a.type_name(),
            b.type_name()
        ))),
    }
}
// The reference uses the first MD5 byte to order otherwise opaque values.
fn md5_first(bytes: &[u8]) -> u8 {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5,
        9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10,
        15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    let mut data = bytes.to_vec();
    let bits = (data.len() as u64) * 8;
    data.push(128);
    while data.len() % 64 != 56 {
        data.push(0);
    }
    data.extend(bits.to_le_bytes());
    let mut h = [0x67452301u32, 0xefcdab89, 0x98badcfe, 0x10325476];
    for chunk in data.as_chunks::<64>().0 {
        let mut m = [0u32; 16];
        for (i, w) in chunk.as_chunks::<4>().0.iter().enumerate() {
            m[i] = u32::from_le_bytes(*w);
        }
        let [mut a, mut b, mut c, mut d] = h;
        for (i, shift) in S.iter().enumerate() {
            let (f, g) = match i {
                0..=15 => ((b & c) | (!b & d), i),
                16..=31 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                32..=47 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let k = (((i + 1) as f64).sin().abs() * 4294967296.0).floor() as u32;
            let next = b.wrapping_add(
                a.wrapping_add(f)
                    .wrapping_add(k)
                    .wrapping_add(m[g])
                    .rotate_left(*shift),
            );
            a = d;
            d = c;
            c = b;
            b = next;
        }
        for (i, x) in [a, b, c, d].iter().enumerate() {
            h[i] = h[i].wrapping_add(*x);
        }
    }
    h[0] as u8
}

pub struct Interp {
    pub globals: LuaValue,
    pub steps: usize,
    pub max_steps: usize,
    pub deadline: Option<Instant>,
    pub unbound: Option<LuaValue>,
    pub log: Vec<String>,
    depth: usize,
}
impl Interp {
    pub fn new(max_steps: usize) -> Self {
        let mut vm = Self {
            globals: LuaValue::table(),
            steps: 0,
            max_steps,
            deadline: None,
            unbound: None,
            log: Vec::new(),
            depth: 0,
        };
        vm.install();
        vm
    }
    pub fn global(&self, k: &str) -> LuaValue {
        self.globals
            .get(&LuaValue::bytes(k.as_bytes()))
            .unwrap_or(LuaValue::Nil)
    }
    fn install(&mut self) {
        for (owner, names) in [
            (
                "table",
                &[
                    "create", "unpack", "pack", "clear", "move", "concat", "insert",
                ][..],
            ),
            (
                "bit32",
                &[
                    "band", "bor", "bxor", "bnot", "lshift", "rshift", "arshift", "lrotate",
                    "rrotate", "extract",
                ][..],
            ),
            ("string", &["byte", "char", "sub"][..]),
            ("math", &["floor", "ceil", "max", "min", "abs"][..]),
        ] {
            let t = LuaValue::table();
            for name in names {
                let full = format!("{owner}.{name}");
                let call = full.clone();
                let f = LuaValue::host(&full, move |vm, a| vm.builtin(&call, a));
                t.set(LuaValue::bytes(name.as_bytes()), f).unwrap();
            }
            if owner == "math" {
                t.set(LuaValue::bytes(b"huge"), LuaValue::Num(f64::INFINITY))
                    .unwrap();
            }
            self.globals
                .set(LuaValue::bytes(owner.as_bytes()), t)
                .unwrap();
        }
        for name in [
            "type",
            "error",
            "pcall",
            "select",
            "tonumber",
            "tostring",
            "getmetatable",
            "setmetatable",
            "next",
            "rawget",
            "rawequal",
            "rawset",
        ] {
            let call = name.to_owned();
            self.globals
                .set(
                    LuaValue::bytes(name.as_bytes()),
                    LuaValue::host(name, move |vm, a| vm.builtin(&call, a)),
                )
                .unwrap();
        }
        self.globals
            .set(
                LuaValue::bytes(b"unpack"),
                self.global("table")
                    .get(&LuaValue::bytes(b"unpack"))
                    .unwrap(),
            )
            .unwrap();
    }
    pub fn tick(&mut self) -> VmResult<()> {
        self.steps += 1;
        if self.steps > self.max_steps || self.deadline.is_some_and(|d| Instant::now() > d) {
            Err(VmError::StepLimit)
        } else {
            Ok(())
        }
    }
    fn builtin(&mut self, name: &str, a: Vec<LuaValue>) -> VmResult<Vec<LuaValue>> {
        use LuaValue::*;
        let arg = |i: usize| a.get(i).cloned().unwrap_or(Nil);
        let int = |i: usize, default: i64| -> VmResult<i64> {
            if arg(i).is_nil() {
                Ok(default)
            } else {
                arg(i).integer()
            }
        };
        let one = |v| Ok(vec![v]);
        match name {
            "table.create" => one(LuaValue::table()),
            "table.unpack" => {
                let t = arg(0);
                if !matches!(t, Table(_)) {
                    return Err(lua_error("table expected"));
                }
                let i = int(1, 1)?;
                let j = int(2, t.length()? as i64)?;
                if j >= i && j.checked_sub(i).is_none_or(|distance| distance >= 8000) {
                    return Err(lua_error("too many results to unpack"));
                }
                (i..=j).map(|k| t.get(&Num(k as f64))).collect()
            }
            "table.pack" => {
                let t = LuaValue::table();
                for (i, v) in a.iter().enumerate() {
                    t.set(Num((i + 1) as f64), v.clone())?;
                }
                t.set(Self::s("n"), Num(a.len() as f64))?;
                one(t)
            }
            "table.clear" => {
                arg(0).clear()?;
                Ok(vec![])
            }
            "table.move" => {
                let src = arg(0);
                let f = int(1, 0)?;
                let e = int(2, 0)?;
                let t = int(3, 0)?;
                let dst = if arg(4).is_nil() { src.clone() } else { arg(4) };
                let count = if e < f {
                    0
                } else {
                    e.checked_sub(f)
                        .and_then(|n| n.checked_add(1))
                        .ok_or(VmError::StepLimit)?
                };
                let backward = t > f && t <= e && lua_eq(&src, &dst);
                for q in 0..count {
                    self.tick()?;
                    let i = if backward { count - q - 1 } else { q };
                    dst.set(Num(t as f64 + i as f64), src.get(&Num((f + i) as f64))?)?;
                }
                one(dst)
            }
            "table.concat" => {
                let t = arg(0);
                let sep = if arg(1).is_nil() {
                    vec![]
                } else {
                    match arg(1) {
                        Str(s) => s,
                        _ => return Err(lua_error("string expected")),
                    }
                };
                let mut out = vec![];
                for k in int(2, 1)?..=int(3, t.length()? as i64)? {
                    self.tick()?;
                    let v = t.get(&Num(k as f64))?;
                    if !matches!(v, Str(_) | Num(_)) {
                        return Err(lua_error(format!(
                            "invalid value (at index {k}) in table for 'concat'"
                        )));
                    }
                    if k > int(2, 1)? {
                        out.extend(&sep);
                    }
                    match v {
                        Str(s) => out.extend(s),
                        v => out.extend(v.string().as_bytes()),
                    }
                }
                one(Str(out))
            }
            "table.insert" => {
                let t = arg(0);
                if arg(2).is_nil() {
                    t.set(Num((t.length()? + 1) as f64), arg(1))?;
                } else {
                    let pos = int(1, 0)?;
                    for i in (pos..=t.length()? as i64).rev() {
                        self.tick()?;
                        t.set(Num((i + 1) as f64), t.get(&Num(i as f64))?)?;
                    }
                    t.set(arg(1), arg(2))?;
                }
                Ok(vec![])
            }
            "type" => one(Self::s(arg(0).type_name())),
            "error" => Err(VmError::Lua(arg(0))),
            "pcall" => match self.call(arg(0), a.iter().skip(1).cloned().collect()) {
                Ok(mut r) => {
                    r.insert(0, Bool(true));
                    Ok(r)
                }
                Err(VmError::Lua(e)) => Ok(vec![Bool(false), e]),
                Err(e) => Err(e),
            },
            "select" => {
                if lua_eq(&arg(0), &Self::s("#")) {
                    return one(Num(a.len().saturating_sub(1) as f64));
                }
                let mut i = int(0, 0)?;
                if i < 0 {
                    i += a.len() as i64;
                }
                let len = a.len() as i64 - 1;
                let start = i - 1;
                let start = if start < 0 {
                    (len + start).max(0)
                } else {
                    start
                };
                Ok(a.iter().skip(1 + start as usize).cloned().collect())
            }
            "tonumber" => one(match arg(0) {
                Num(n) => Num(n),
                Str(s) => std::str::from_utf8(&s)
                    .ok()
                    .and_then(|s| s.trim().parse::<f64>().ok())
                    .map(Num)
                    .unwrap_or(Nil),
                _ => Nil,
            }),
            "tostring" => one(match arg(0) {
                Str(s) => Str(s),
                v => Self::s(&v.string()),
            }),
            "getmetatable" => one(if let Table(t) = arg(0) {
                t.borrow().mt.clone().unwrap_or(Nil)
            } else {
                Nil
            }),
            "setmetatable" => {
                let t = arg(0);
                if let Table(r) = &t {
                    r.borrow_mut().mt = Some(arg(1));
                    one(t)
                } else {
                    Err(lua_error("table expected"))
                }
            }
            "rawget" => one(arg(0).get(&arg(1))?),
            "rawset" => {
                arg(0).set(arg(1), arg(2))?;
                one(arg(0))
            }
            "rawequal" => one(Bool(lua_eq(&arg(0), &arg(1)))),
            "next" => {
                let items = arg(0).items()?;
                let pos = if arg(1).is_nil() {
                    Some(0)
                } else {
                    items
                        .iter()
                        .position(|(k, _)| lua_eq(k, &arg(1)))
                        .map(|i| i + 1)
                };
                Ok(pos
                    .and_then(|p| items.get(p))
                    .map(|(k, v)| vec![k.clone(), v.clone()])
                    .unwrap_or_else(|| vec![Nil]))
            }
            "string.byte" => {
                let Str(s) = arg(0) else {
                    return Err(lua_error("string expected"));
                };
                let i = int(1, 1)?;
                let j = int(2, i)?.min(s.len() as i64);
                let i = i.max(1);
                Ok((i..=j).map(|k| Num(s[(k - 1) as usize] as f64)).collect())
            }
            "string.char" => {
                let mut out = vec![];
                for v in a {
                    let n = v.integer()?;
                    if !(0..=255).contains(&n) {
                        return Err(lua_error("invalid value for string.char"));
                    }
                    out.push(n as u8);
                }
                one(Str(out))
            }
            "string.sub" => {
                let Str(s) = arg(0) else {
                    return Err(lua_error("string expected"));
                };
                let len = s.len() as i64;
                let mut i = int(1, 1)?;
                let mut j = int(2, len)?;
                if i < 0 {
                    i = (len + i + 1).max(1);
                }
                if j < 0 {
                    j = len + j + 1;
                }
                i = i.max(1);
                let end = if j < 0 { (len + j).max(0) } else { j.min(len) };
                let start = (i - 1).min(len);
                one(Str(if end <= start {
                    vec![]
                } else {
                    s[start as usize..end as usize].to_vec()
                }))
            }
            "math.floor" => one(Num(arg(0).number()?.floor())),
            "math.ceil" => one(Num(arg(0).number()?.ceil())),
            "math.abs" => one(Num(arg(0).number()?.abs())),
            "math.max" | "math.min" => {
                let mut r = a
                    .first()
                    .cloned()
                    .ok_or_else(|| lua_error("value expected"))?;
                for v in a.iter().skip(1) {
                    if compare(
                        if name == "math.max" {
                            "CompareLt"
                        } else {
                            "CompareGt"
                        },
                        &r,
                        v,
                    )? {
                        r = v.clone();
                    }
                }
                one(r)
            }
            name if name.starts_with("bit32.") => {
                let u32_value = |v: &LuaValue| -> VmResult<u32> {
                    let n = v.number()?;
                    if !n.is_finite() {
                        return Err(lua_error("number has no integer representation"));
                    }
                    Ok(n.trunc().rem_euclid(4294967296.0) as u32)
                };
                let u = |i: usize| u32_value(&arg(i));
                let r = match &name[6..] {
                    "band" | "bor" | "bxor" => {
                        let mut r = if name == "bit32.band" { u32::MAX } else { 0 };
                        for v in &a {
                            let n = u32_value(v)?;
                            r = match name {
                                "bit32.band" => r & n,
                                "bit32.bor" => r | n,
                                _ => r ^ n,
                            };
                        }
                        r
                    }
                    "bnot" => !u(0)?,
                    "lshift" | "rshift" => {
                        let x = u(0)?;
                        let n = int(1, 0)?;
                        if !(-31..=31).contains(&n) {
                            0
                        } else if (name == "bit32.lshift") == (n >= 0) {
                            x.wrapping_shl(n.unsigned_abs() as u32)
                        } else {
                            x.wrapping_shr(n.unsigned_abs() as u32)
                        }
                    }
                    "arshift" => {
                        let n = int(1, 0)?;
                        if n < 0 {
                            return Err(lua_error("negative shift count"));
                        }
                        ((u(0)? as i32) >> (n.min(31) as u32)) as u32
                    }
                    "lrotate" => u(0)?.rotate_left(u(1)? & 31),
                    "rrotate" => u(0)?.rotate_right(u(1)? & 31),
                    "extract" => {
                        let f = int(1, 0)?;
                        let w = int(2, 1)?;
                        if !(0..32).contains(&f) || !(1..=32).contains(&w) || f + w > 32 {
                            return Err(lua_error("invalid field/width"));
                        }
                        (u(0)? >> f) & if w == 32 { u32::MAX } else { (1u32 << w) - 1 }
                    }
                    _ => return Err(unsupported(name)),
                };
                one(Num(r as f64))
            }
            _ => Err(unsupported(name)),
        }
    }
    fn s(s: &str) -> LuaValue {
        LuaValue::bytes(s.as_bytes())
    }
    pub fn call(&mut self, f: LuaValue, args: Vec<LuaValue>) -> VmResult<Vec<LuaValue>> {
        if self.depth >= 200 {
            return Err(VmError::StepLimit);
        }
        self.tick()?;
        self.depth += 1;
        let result = (|| match f {
            LuaValue::Func(f) => {
                if ty(&f.node) != "AstExprFunction" || !f.node["args"].is_array() {
                    return Err(unsupported("invalid function AST"));
                }
                let mut env = Env {
                    cells: Rc::new(RefCell::new(f.cells.borrow().clone())),
                    varargs: vec![],
                };
                let ps = array(&f.node["args"]);
                for (i, p) in ps.iter().enumerate() {
                    env.declare(location(p), args.get(i).cloned().unwrap_or(LuaValue::Nil));
                }
                if f.node["vararg"].as_bool().unwrap_or(false) {
                    env.varargs = args.into_iter().skip(ps.len()).collect();
                }
                match self.exec_block(&f.node["body"], &mut env) {
                    Err(VmError::Return(v)) => Ok(v),
                    Ok(()) => Ok(vec![]),
                    Err(e) => Err(e),
                }
            }
            LuaValue::Host(_, f, _) => f(self, args),
            LuaValue::Opaque(k) => {
                let ak = tuple(&args.iter().map(LuaValue::key).collect::<Vec<_>>());
                self.log
                    .push(tuple(&[quoted("call"), k.clone(), ak.clone()]));
                Ok((0..4)
                    .map(|i| {
                        LuaValue::opaque(vec![quoted("call"), k.clone(), ak.clone(), i.to_string()])
                    })
                    .collect())
            }
            v => Err(lua_error(format!(
                "attempt to call a {} value",
                v.type_name()
            ))),
        })();
        self.depth -= 1;
        result
    }
    pub fn exec_block(&mut self, b: &Value, env: &mut Env) -> VmResult<()> {
        let statements = b["body"]
            .as_array()
            .ok_or_else(|| unsupported("invalid block AST"))?;
        for s in statements {
            self.exec_stat(s, env)?;
        }
        Ok(())
    }
    pub fn exec_stat(&mut self, s: &Value, env: &mut Env) -> VmResult<()> {
        self.tick()?;
        match ty(s) {
            "AstStatBlock" => self.exec_block(s, env),
            "AstStatLocal" | "AstStatAssign" => {
                let vs = self.eval_list(array(&s["values"]), env)?;
                for (i, t) in array(&s["vars"]).iter().enumerate() {
                    let v = vs.get(i).cloned().unwrap_or(LuaValue::Nil);
                    if ty(s) == "AstStatLocal" {
                        env.declare(location(t), v);
                    } else {
                        self.assign(t, v, env)?;
                    }
                }
                Ok(())
            }
            "AstStatCompoundAssign" => {
                let a = self.eval(&s["var"], env)?;
                let b = self.eval(&s["value"], env)?;
                let v = self.binop(s["op"].as_str().unwrap_or(""), a, b)?;
                self.assign(&s["var"], v, env)
            }
            "AstStatExpr" => {
                self.eval_multi(&s["expr"], env)?;
                Ok(())
            }
            "AstStatIf" => {
                if self.eval(&s["condition"], env)?.truthy() {
                    self.exec_block(&s["thenbody"], env)
                } else if !s["elsebody"].is_null() {
                    if ty(&s["elsebody"]) == "AstStatIf" {
                        self.exec_stat(&s["elsebody"], env)
                    } else {
                        self.exec_block(&s["elsebody"], env)
                    }
                } else {
                    Ok(())
                }
            }
            "AstStatReturn" => Err(VmError::Return(self.eval_list(array(&s["list"]), env)?)),
            "AstStatFor" => {
                let a = self.eval(&s["from"], env)?;
                let b = self.eval(&s["to"], env)?;
                let st = if s["step"].is_null() {
                    LuaValue::Num(1.0)
                } else {
                    self.eval(&s["step"], env)?
                };
                if !matches!(
                    (&a, &b, &st),
                    (LuaValue::Num(_), LuaValue::Num(_), LuaValue::Num(_))
                ) {
                    return Err(lua_error("'for' initial value must be a number"));
                }
                let (mut i, end, step) = (a.number()?, b.number()?, st.number()?);
                if step == 0.0 {
                    return Err(lua_error("'for' step is zero"));
                }
                while step > 0.0 && i <= end || step < 0.0 && i >= end {
                    self.tick()?;
                    env.declare(location(&s["var"]), LuaValue::Num(i));
                    match self.exec_block(&s["body"], env) {
                        Err(VmError::Break) => break,
                        Err(VmError::Continue) | Ok(()) => {}
                        Err(e) => return Err(e),
                    }
                    i += step;
                }
                Ok(())
            }
            "AstStatForIn" => {
                let vals = self.eval_list(array(&s["values"]), env)?;
                let f = vals.first().cloned().unwrap_or(LuaValue::Nil);
                if matches!(f, LuaValue::Table(_)) {
                    for (k, v) in f.items()? {
                        self.tick()?;
                        self.bind_vars(array(&s["vars"]), &[k, v], env);
                        match self.exec_block(&s["body"], env) {
                            Err(VmError::Break) => break,
                            Err(VmError::Continue) | Ok(()) => {}
                            Err(e) => return Err(e),
                        }
                    }
                } else {
                    let state = vals.get(1).cloned().unwrap_or(LuaValue::Nil);
                    let mut ctl = vals.get(2).cloned().unwrap_or(LuaValue::Nil);
                    loop {
                        self.tick()?;
                        let r = self.call(f.clone(), vec![state.clone(), ctl])?;
                        ctl = r.first().cloned().unwrap_or(LuaValue::Nil);
                        if ctl.is_nil() {
                            break;
                        }
                        self.bind_vars(array(&s["vars"]), &r, env);
                        match self.exec_block(&s["body"], env) {
                            Err(VmError::Break) => break,
                            Err(VmError::Continue) | Ok(()) => {}
                            Err(e) => return Err(e),
                        }
                    }
                }
                Ok(())
            }
            "AstStatWhile" | "AstStatRepeat" => {
                let repeat = ty(s) == "AstStatRepeat";
                loop {
                    if !repeat && !self.eval(&s["condition"], env)?.truthy() {
                        break;
                    }
                    self.tick()?;
                    match self.exec_block(&s["body"], env) {
                        Err(VmError::Break) => break,
                        Err(VmError::Continue) | Ok(()) => {}
                        Err(e) => return Err(e),
                    }
                    if repeat && self.eval(&s["condition"], env)?.truthy() {
                        break;
                    }
                }
                Ok(())
            }
            "AstStatBreak" => Err(VmError::Break),
            "AstStatContinue" => Err(VmError::Continue),
            "AstStatLocalFunction" => {
                let loc = location(&s["name"]);
                env.declare(loc.clone(), LuaValue::Nil);
                env.set(
                    loc,
                    LuaValue::function(s["func"].clone(), env.cells.clone()),
                );
                Ok(())
            }
            "AstStatFunction" => self.assign(
                &s["name"],
                LuaValue::function(s["func"].clone(), env.cells.clone()),
                env,
            ),
            "AstStatTypeAlias" | "AstStatTypeFunction" => Ok(()),
            t => Err(unsupported(t)),
        }
    }
    fn bind_vars(&self, vars: &[Value], vals: &[LuaValue], env: &mut Env) {
        for (i, v) in vars.iter().enumerate() {
            env.declare(location(v), vals.get(i).cloned().unwrap_or(LuaValue::Nil));
        }
    }
    pub fn assign(&mut self, t: &Value, v: LuaValue, env: &mut Env) -> VmResult<()> {
        match ty(t) {
            "AstExprLocal" => {
                env.set(location(&t["local"]), v);
                Ok(())
            }
            "AstExprGlobal" => self
                .globals
                .set(Self::s(t["global"].as_str().unwrap_or("")), v),
            "AstExprIndexName" | "AstExprIndexExpr" => {
                let o = self.eval(&t["expr"], env)?;
                let k = if ty(t) == "AstExprIndexName" {
                    Self::s(t["index"].as_str().unwrap_or(""))
                } else {
                    self.eval(&t["index"], env)?
                };
                self.setindex(o, k, v)
            }
            t => Err(unsupported(format!("assign {t}"))),
        }
    }
    pub fn setindex(&mut self, o: LuaValue, k: LuaValue, v: LuaValue) -> VmResult<()> {
        if let LuaValue::Opaque(key) = o {
            self.log
                .push(tuple(&[quoted("set"), key, k.key(), v.key()]));
            Ok(())
        } else {
            o.set(k, v)
        }
    }
    pub fn index(&self, o: LuaValue, k: LuaValue) -> VmResult<LuaValue> {
        match o {
            LuaValue::Opaque(key) => Ok(LuaValue::opaque(vec![quoted("idx"), key, k.key()])),
            LuaValue::Str(_) => self.global("string").get(&k),
            v => v.get(&k),
        }
    }
    pub fn eval_list(&mut self, exprs: &[Value], env: &mut Env) -> VmResult<Vec<LuaValue>> {
        let mut out = vec![];
        for (i, e) in exprs.iter().enumerate() {
            if i + 1 == exprs.len() {
                out.extend(self.eval_multi(e, env)?);
            } else {
                out.push(self.eval(e, env)?);
            }
        }
        Ok(out)
    }
    pub fn eval_multi(&mut self, e: &Value, env: &mut Env) -> VmResult<Vec<LuaValue>> {
        match ty(e) {
            "AstExprCall" => self.eval_call(e, env),
            "AstExprVarargs" => Ok(env.varargs.clone()),
            _ => Ok(vec![self.eval(e, env)?]),
        }
    }
    fn eval_call(&mut self, e: &Value, env: &mut Env) -> VmResult<Vec<LuaValue>> {
        self.tick()?;
        let f = &e["func"];
        let (mut args, func) = if e["self"].as_bool().unwrap_or(false) {
            let o = self.eval(&f["expr"], env)?;
            let fn_ = self.index(o.clone(), Self::s(f["index"].as_str().unwrap_or("")))?;
            (vec![o], fn_)
        } else {
            (vec![], self.eval(f, env)?)
        };
        args.extend(self.eval_list(array(&e["args"]), env)?);
        self.call(func, args)
    }
    pub fn eval(&mut self, e: &Value, env: &mut Env) -> VmResult<LuaValue> {
        use LuaValue::*;
        match ty(e) {
            "AstExprLocal" => env
                .get(&location(&e["local"]))
                .or_else(|| self.unbound.clone())
                .ok_or_else(|| unsupported(format!("unbound local {}", e["local"]["name"]))),
            "AstExprConstantNumber" => crate::model::decode_number(&e["value"])
                .map(Num)
                .map_err(|e| unsupported(e.message)),
            "AstExprConstantString" => Ok(Str(if let Some(s) = e["value"].as_str() {
                s.chars()
                    .map(|c| {
                        u8::try_from(c as u32).map_err(|_| unsupported("AST string is not Latin1"))
                    })
                    .collect::<VmResult<Vec<_>>>()?
            } else {
                array(&e["value"])
                    .iter()
                    .map(|v| {
                        v.as_u64()
                            .filter(|&n| n <= 255)
                            .map(|n| n as u8)
                            .ok_or_else(|| unsupported("invalid byte string"))
                    })
                    .collect::<VmResult<Vec<_>>>()?
            })),
            "AstExprConstantBool" => Ok(Bool(e["value"].as_bool().unwrap_or(false))),
            "AstExprConstantNil" => Ok(Nil),
            "AstExprGroup" | "AstExprTypeAssertion" => self.eval(&e["expr"], env),
            "AstExprGlobal" => Ok(self.global(e["global"].as_str().unwrap_or(""))),
            "AstExprIndexName" | "AstExprIndexExpr" => {
                let o = self.eval(&e["expr"], env)?;
                let k = if ty(e) == "AstExprIndexName" {
                    Self::s(e["index"].as_str().unwrap_or(""))
                } else {
                    self.eval(&e["index"], env)?
                };
                self.index(o, k)
            }
            "AstExprCall" => Ok(self.eval_call(e, env)?.into_iter().next().unwrap_or(Nil)),
            "AstExprVarargs" => Ok(env.varargs.first().cloned().unwrap_or(Nil)),
            "AstExprBinary" => {
                let op = e["op"].as_str().unwrap_or("");
                let a = self.eval(&e["left"], env)?;
                if op == "And" {
                    return if a.truthy() {
                        self.eval(&e["right"], env)
                    } else {
                        Ok(a)
                    };
                }
                if op == "Or" {
                    return if a.truthy() {
                        Ok(a)
                    } else {
                        self.eval(&e["right"], env)
                    };
                }
                let b = self.eval(&e["right"], env)?;
                self.binop(op, a, b)
            }
            "AstExprUnary" => {
                let v = self.eval(&e["expr"], env)?;
                match e["op"].as_str().unwrap_or("") {
                    "Not" => Ok(Bool(!v.truthy())),
                    "Minus" => match v {
                        Opaque(k) => Ok(LuaValue::opaque(vec![quoted("neg"), k])),
                        v => Ok(Num(-v.number()?)),
                    },
                    "Len" => match v {
                        Opaque(k) => Ok(LuaValue::opaque(vec![quoted("len"), k])),
                        v => Ok(Num(v.length()? as f64)),
                    },
                    op => Err(unsupported(op)),
                }
            }
            "AstExprFunction" => Ok(LuaValue::function(e.clone(), env.cells.clone())),
            "AstExprTable" => {
                let t = LuaValue::table();
                let items = array(&e["items"]);
                let mut n = 0;
                for (i, it) in items.iter().enumerate() {
                    if !it["key"].is_null() {
                        let k = self.eval(&it["key"], env)?;
                        let v = self.eval(&it["value"], env)?;
                        t.set(k, v)?;
                    } else {
                        let vs = if i + 1 == items.len() {
                            self.eval_multi(&it["value"], env)?
                        } else {
                            vec![self.eval(&it["value"], env)?]
                        };
                        for v in vs {
                            n += 1;
                            t.set(Num(n as f64), v)?;
                        }
                    }
                }
                Ok(t)
            }
            "AstExprIfElse" => {
                let branch = if self.eval(&e["condition"], env)?.truthy() {
                    "trueExpr"
                } else {
                    "falseExpr"
                };
                self.eval(&e[branch], env)
            }
            t => Err(unsupported(t)),
        }
    }
    pub fn binop(&self, op: &str, a: LuaValue, b: LuaValue) -> VmResult<LuaValue> {
        match op {
            "Add" | "Sub" | "Mul" | "Div" | "FloorDiv" | "Mod" | "Pow" => arith(op, a, b),
            "Concat" => {
                if matches!(a, LuaValue::Opaque(_)) || matches!(b, LuaValue::Opaque(_)) {
                    return Ok(LuaValue::opaque(vec![quoted("concat"), a.key(), b.key()]));
                }
                let mut out = vec![];
                for v in [a, b] {
                    match v {
                        LuaValue::Str(s) => out.extend(s),
                        LuaValue::Num(_) => out.extend(v.string().as_bytes()),
                        v => {
                            return Err(lua_error(format!(
                                "attempt to concatenate a {} value",
                                v.type_name()
                            )))
                        }
                    }
                }
                Ok(LuaValue::Str(out))
            }
            "CompareEq" | "CompareNe" | "CompareLt" | "CompareLe" | "CompareGt" | "CompareGe" => {
                Ok(LuaValue::Bool(compare(op, &a, &b)?))
            }
            _ => Err(unsupported(op)),
        }
    }
}

#[cfg(test)]
mod hash_tests {
    #[test]
    fn opaque_order_hash_uses_standard_md5_vectors() {
        for (text, first) in [
            ("", 0xd4),
            ("a", 0x0c),
            ("abc", 0x90),
            ("message digest", 0xf9),
        ] {
            assert_eq!(super::md5_first(text.as_bytes()), first);
        }
    }
}

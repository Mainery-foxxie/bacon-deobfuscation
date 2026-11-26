//! Native reference operations and deterministic trial-state generation.
use crate::luavm::{
    arith, compare, lua_eq, lua_error, quoted, tuple, Interp, LuaValue as V, VmResult,
};
use std::collections::{BTreeMap, BTreeSet};

pub const NXT0: i64 = 1000;
pub const ALT0: i64 = 2000;
pub const NREG: i64 = 24;
pub type Fields = BTreeMap<String, f64>;
pub fn opaque(name: &str, indices: &[i64]) -> V {
    let mut p = vec![quoted(name)];
    p.extend(indices.iter().map(ToString::to_string));
    V::opaque(p)
}
fn n(x: impl Into<f64>) -> V {
    V::Num(x.into())
}
pub fn specs() -> Vec<String> {
    let mut names: Vec<String> = [
        "NOP",
        "MOVE",
        "LOADNIL",
        "LOADN",
        "LOADK",
        "GETIMPORT",
        "GETUPVAL",
        "SETUPVAL",
        "GETTABLEKS",
        "SETTABLEKS",
        "GETTABLE",
        "SETTABLE",
        "GETTABLEN",
        "SETTABLEN",
        "SELF",
        "OR",
        "ORK",
        "AND",
        "ANDK",
        "NOT",
        "MINUS",
        "LENGTH",
        "CONCAT",
        "NEWTABLE",
        "DUPTABLE",
        "JUMP",
        "JUMPIF",
        "JUMPIFNOT",
        "JUMPIFEQ",
        "JUMPIFNOTEQ",
        "JUMPIFLE",
        "JUMPIFLT",
        "JUMPIFNOTLT",
        "JUMPIFNOTLE",
        "JUMPXEQK",
        "JUMPXEQKB",
        "JUMPXEQKNIL",
        "LOADB",
        "FORNPREP",
        "FORNLOOP",
        "FORGPREP",
        "FORGLOOP",
        "CALL",
        "RETURN",
        "SETLIST",
        "GETVARARGS",
        "TAILCALL",
        "SUBRK",
        "DIVRK",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    for base in ["ADD", "SUB", "MUL", "DIV", "MOD", "POW", "IDIV"] {
        names.push(base.into());
        names.push(format!("{base}K"));
    }
    for b in 1..7 {
        for c in 1..5 {
            names.push(format!("CALL_{b}_{c}"));
        }
    }
    names.sort();
    names
}
pub fn is_ctrl(name: &str) -> bool {
    matches!(
        name,
        "JUMP"
            | "JUMPIF"
            | "JUMPIFNOT"
            | "JUMPIFEQ"
            | "JUMPIFNOTEQ"
            | "JUMPIFLE"
            | "JUMPIFLT"
            | "JUMPIFNOTLT"
            | "JUMPIFNOTLE"
            | "JUMPXEQK"
            | "JUMPXEQKB"
            | "JUMPXEQKNIL"
            | "LOADB"
            | "FORNPREP"
            | "FORNLOOP"
            | "FORGPREP"
            | "FORGLOOP"
            | "RETURN"
            | "TAILCALL"
    )
}
pub fn has_normalize(name: &str) -> bool {
    name == "CALL" || name.starts_with("CALL_")
}
pub fn domain_fields(name: &str) -> &'static [&'static str] {
    match name {
        "CALL" => &["B", "C"],
        "RETURN" => &["B"],
        _ => &[],
    }
}

// CPython's integer-seeded MT19937 and _randbelow; field draws match the reference.
pub struct Rng {
    mt: [u32; 624],
    index: usize,
}
impl Rng {
    pub fn new(seed: u64) -> Self {
        let mut r = Self {
            mt: [0; 624],
            index: 624,
        };
        r.mt[0] = 19650218;
        for i in 1..624 {
            r.mt[i] = 1812433253u32
                .wrapping_mul(r.mt[i - 1] ^ (r.mt[i - 1] >> 30))
                .wrapping_add(i as u32);
        }
        let key = if seed >> 32 != 0 {
            vec![seed as u32, (seed >> 32) as u32]
        } else {
            vec![seed as u32]
        };
        let (mut i, mut j) = (1, 0);
        for _ in 0..624.max(key.len()) {
            r.mt[i] = (r.mt[i] ^ ((r.mt[i - 1] ^ (r.mt[i - 1] >> 30)).wrapping_mul(1664525)))
                .wrapping_add(key[j])
                .wrapping_add(j as u32);
            i += 1;
            j += 1;
            if i >= 624 {
                r.mt[0] = r.mt[623];
                i = 1;
            }
            if j >= key.len() {
                j = 0;
            }
        }
        for _ in 0..623 {
            r.mt[i] = (r.mt[i] ^ ((r.mt[i - 1] ^ (r.mt[i - 1] >> 30)).wrapping_mul(1566083941)))
                .wrapping_sub(i as u32);
            i += 1;
            if i >= 624 {
                r.mt[0] = r.mt[623];
                i = 1;
            }
        }
        r.mt[0] = 0x80000000;
        r
    }
    fn word(&mut self) -> u32 {
        if self.index >= 624 {
            for i in 0..624 {
                let y = (self.mt[i] & 0x80000000) | (self.mt[(i + 1) % 624] & 0x7fffffff);
                self.mt[i] =
                    self.mt[(i + 397) % 624] ^ (y >> 1) ^ if y & 1 != 0 { 0x9908b0df } else { 0 };
            }
            self.index = 0;
        }
        let mut y = self.mt[self.index];
        self.index += 1;
        y ^= y >> 11;
        y ^= (y << 7) & 0x9d2c5680;
        y ^= (y << 15) & 0xefc60000;
        y ^= y >> 18;
        y
    }
    fn bits(&mut self, k: u32) -> u64 {
        if k <= 32 {
            (self.word() >> (32 - k)) as u64
        } else {
            self.word() as u64 | ((self.word() >> (64 - k)) as u64) << 32
        }
    }
    pub fn below(&mut self, n: u64) -> u64 {
        assert!(n > 0);
        let k = 64 - n.leading_zeros();
        loop {
            let r = self.bits(k);
            if r < n {
                return r;
            }
        }
    }
    pub fn range(&mut self, a: i64, b: i64) -> i64 {
        a + self.below((b - a) as u64) as i64
    }
    fn choose<T: Clone>(&mut self, p: &[T]) -> T {
        p[self.below(p.len() as u64) as usize].clone()
    }
}

pub struct State {
    pub rng: Rng,
    pub trial: usize,
    pub name: String,
    pub r: V,
    pub up: V,
    pub f: Fields,
    pub top: V,
    pub tag: V,
    pub nxt: V,
    pub alt: V,
    pub rec: V,
    pub clr: bool,
    pub prof: V,
    pub tmpl: Option<V>,
    pub ret: Option<Vec<V>>,
    pub nvar: usize,
    pub varargs: V,
    pub iter_nil: bool,
    pub bad_iter: bool,
    pub fixed: BTreeSet<String>,
    pub log: Vec<String>,
}
impl State {
    pub fn new(
        name: &str,
        seed: u64,
        domain: Option<&[Fields]>,
        variant: Option<&Fields>,
        trial: usize,
    ) -> VmResult<Self> {
        let mut rng = Rng::new(seed);
        let r = V::table();
        for i in 0..NREG {
            r.set(V::Num(i as f64), opaque("r", &[i]))?;
        }
        let up = V::table();
        for i in 0..12 {
            let cont = V::table();
            cont.set(V::Num((100 + i) as f64), opaque("uv", &[i]))?;
            let bx = V::table();
            bx.set(n(1), cont)?;
            bx.set(n(2), V::Num((100 + i) as f64))?;
            up.set(V::Num(i as f64), bx)?;
        }
        let mut pool: Vec<i64> = (0..12).collect();
        let mut regs = vec![];
        for i in 0..3 {
            let j = rng.below((12 - i) as u64) as usize;
            regs.push(pool[j]);
            pool[j] = pool[11 - i];
        }
        let mut f = Fields::new();
        for (k, v) in [
            ("op", 0),
            ("ef", rng.range(0, 256)),
            ("A", regs[0]),
            ("B", regs[1]),
            ("C", regs[2]),
            ("RF", rng.range(0, 40)),
            ("HF", rng.range(0, 1i64 << 32)),
            ("AUX", rng.range(0, 40)),
        ] {
            f.insert(k.into(), v as f64);
        }
        let top = V::Num(rng.range(3, 12) as f64);
        let clr = rng.range(0, 2) != 0;
        let prof = V::table();
        for i in 1..4 {
            prof.set(n(i), V::Num(rng.range(0, 4) as f64))?;
        }
        let mut vr = Rng::new(seed ^ 0x5bd1e995);
        let nvar = vr.range(0, 5) as usize;
        let varargs = V::table();
        for i in 0..nvar {
            varargs.set(V::Num((i + 1) as f64), opaque("va", &[i as i64]))?;
        }
        let mut fixed = BTreeSet::new();
        if let Some(dom) = domain.filter(|d| !d.is_empty()) {
            let d = rng.choose(dom);
            for (k, v) in d {
                f.insert(k.clone(), v);
                fixed.insert(k);
            }
        }
        if let Some(v) = variant {
            for (k, v) in v {
                f.insert(k.clone(), *v);
                fixed.insert(k.clone());
            }
        }
        let mut st = Self {
            rng,
            trial,
            name: name.into(),
            r,
            up,
            f,
            top,
            tag: opaque("tmpl-tag", &[]),
            nxt: V::Num(NXT0 as f64),
            alt: V::Num(ALT0 as f64),
            rec: V::table(),
            clr,
            prof,
            tmpl: None,
            ret: None,
            nvar,
            varargs,
            iter_nil: false,
            bad_iter: false,
            fixed,
            log: vec![],
        };
        st.prep()?;
        st.fill_rec()?;
        Ok(st)
    }
    pub fn field(&self, k: &str) -> f64 {
        self.f[k]
    }
    fn setf(&mut self, k: &str, v: f64) {
        if !self.fixed.contains(k) {
            self.f.insert(k.into(), v);
        }
    }
    pub fn reg(&self, i: f64) -> VmResult<V> {
        self.r.get(&V::Num(i))
    }
    pub fn setreg(&self, i: f64, v: V) -> VmResult<()> {
        self.r.set(V::Num(i), v)
    }
    fn cyc(&self, p: &[V], salt: usize) -> V {
        p[(self.trial + salt) % p.len()].clone()
    }
    fn fill_rec(&self) -> VmResult<()> {
        for i in 0..8 {
            let p = V::table();
            let vals = [
                0.0,
                self.field("ef"),
                self.field("A"),
                self.field("B"),
                self.field("C"),
                self.field("RF"),
                self.field("HF"),
                self.field("AUX"),
                NXT0 as f64,
                ALT0 as f64,
            ];
            for (j, v) in vals.iter().enumerate() {
                p.set(V::Num((j + 1) as f64), V::Num(*v))?;
            }
            self.rec.set(n(11 + i), p)?;
        }
        Ok(())
    }
    pub fn kget(&self, i: f64) -> V {
        if i == self.field("RF") && matches!(self.name.as_str(), "DUPTABLE" | "NEWTABLE") {
            if let Some(t) = &self.tmpl {
                return t.clone();
            }
        }
        opaque("K", &[i as i64])
    }
    fn truth_prep(&self) -> VmResult<()> {
        let p = vec![
            V::Nil,
            V::Bool(false),
            V::Bool(true),
            n(0),
            n(1),
            V::bytes(b"x"),
            opaque("t", &[1]),
            opaque("t", &[2]),
        ];
        self.setreg(self.field("B"), self.cyc(&p, 0))
    }
    fn prep(&mut self) -> VmResult<()> {
        let name = self.name.clone();
        let name = name.as_str();
        match name {
            "MOVE" | "OR" | "ORK" | "AND" | "ANDK" | "NOT" => self.truth_prep()?,
            "CONCAT" => {
                let b = self.rng.range(0, 6) as f64;
                self.setf("B", b);
                let c = self.field("B") + self.rng.range(1, 5) as f64;
                self.setf("C", c);
                let a = self.rng.range(0, 20) as f64;
                self.setf("A", a);
                if self.rng.range(0, 2) != 0 {
                    for i in self.field("B") as i64..=self.field("C") as i64 {
                        let v = self.rng.choose(&[
                            V::bytes(b"a"),
                            V::bytes(b"bc"),
                            n(7),
                            V::bytes(b"d"),
                        ]);
                        self.setreg(i as f64, v)?;
                    }
                }
            }
            "NEWTABLE" | "DUPTABLE" => {
                let items = V::table();
                let count = self.rng.range(1, 4);
                for i in 0..count {
                    let pair = V::table();
                    pair.set(n(1), V::Num((200 + i) as f64))?;
                    let v = if self.rng.range(0, 3) != 0 {
                        300 + i
                    } else {
                        -1
                    };
                    pair.set(n(2), V::Num(v as f64))?;
                    items.set(V::Num((i + 1) as f64), pair)?;
                }
                let t = V::table();
                t.set(n(1), self.tag.clone())?;
                t.set(n(2), items)?;
                self.tmpl = Some(t);
            }
            "JUMP" => {
                let p = vec![
                    V::Nil,
                    V::Bool(false),
                    V::Bool(true),
                    n(0),
                    n(1),
                    n(5),
                    V::bytes(b"x"),
                    opaque("j", &[1]),
                    opaque("j", &[2]),
                ];
                for (i, k) in ["A", "B", "C", "AUX"].iter().enumerate() {
                    self.setreg(self.field(k), self.cyc(&p, 2 * i))?;
                }
                if self.trial.is_multiple_of(3) {
                    self.setreg(self.field("AUX"), self.reg(self.field("A"))?)?;
                }
            }
            "JUMPIF" | "JUMPIFNOT" => {
                let p = vec![
                    V::Nil,
                    V::Bool(false),
                    V::Bool(true),
                    n(0),
                    n(1),
                    V::bytes(b"x"),
                    opaque("t", &[1]),
                ];
                self.setreg(self.field("A"), self.cyc(&p, 0))?;
            }
            "JUMPIFEQ" | "JUMPIFNOTEQ" | "JUMPIFLE" | "JUMPIFLT" | "JUMPIFNOTLT"
            | "JUMPIFNOTLE" => {
                let aux = self.rng.range(12, 20) as f64;
                self.setf("AUX", aux);
                let a = self.field("A");
                let b = self.field("AUX");
                if !matches!(name, "JUMPIFEQ" | "JUMPIFNOTEQ") {
                    self.setreg(a, V::Num((self.trial % 4) as f64))?;
                    self.setreg(b, V::Num(((self.trial / 4 + self.trial / 2) % 4) as f64))?;
                } else {
                    let p = vec![
                        V::Nil,
                        V::Bool(false),
                        n(1),
                        n(2),
                        V::bytes(b"x"),
                        opaque("t", &[1]),
                        opaque("t", &[2]),
                    ];
                    self.setreg(a, self.cyc(&p, 0))?;
                    self.setreg(
                        b,
                        if !self.trial.is_multiple_of(2) {
                            self.cyc(&p, 3)
                        } else {
                            self.reg(a)?
                        },
                    )?;
                }
            }
            "JUMPXEQK" => {
                let idx = self.rng.range(0, 30);
                let f = (((self.trial % 2) as u64) << 31) | idx as u64;
                self.setf("AUX", f as f64);
                let p = vec![
                    V::Nil,
                    V::Bool(false),
                    V::Bool(true),
                    n(1),
                    n(2),
                    V::bytes(b"x"),
                    opaque("K", &[idx]),
                    opaque("K", &[idx]),
                ];
                self.setreg(self.field("A"), self.cyc(&p, self.trial / 2))?;
            }
            "JUMPXEQKB" | "JUMPXEQKNIL" => {
                let f = (((self.trial % 2) as u64) << 31)
                    | if name == "JUMPXEQKB" {
                        ((self.trial / 2) % 2) as u64
                    } else {
                        0
                    };
                self.setf("AUX", f as f64);
                let p = vec![
                    V::Nil,
                    V::Bool(false),
                    V::Bool(true),
                    n(1),
                    opaque("t", &[1]),
                ];
                self.setreg(self.field("A"), self.cyc(&p, self.trial / 2))?;
            }
            "LOADB" => {
                let b = self.rng.range(0, 2) as f64;
                self.setf("B", b);
                let x = self.rng.range(1, 9);
                let c = self.rng.choose(&[0, 0, x]);
                self.setf("C", c as f64);
            }
            "FORNPREP" | "FORNLOOP" => {
                let a = self.rng.range(0, 10) as f64;
                self.setf("A", a);
                let a = self.field("A");
                let lim = self.rng.range(0, 8) as f64;
                let step = self.rng.choose(&[1.0, 1.0, 2.0, -1.0, -2.0, 0.5, 3.0]);
                let idx = self.rng.range(-3, 9) as f64;
                self.setreg(a, V::Num(lim))?;
                self.setreg(a + 1.0, V::Num(step))?;
                self.setreg(a + 2.0, V::Num(idx))?;
            }
            "FORGPREP" => {
                self.bad_iter = self.rng.range(0, 4) == 0;
                self.setreg(
                    self.field("A"),
                    if self.bad_iter {
                        n(5)
                    } else {
                        opaque("iterfn", &[])
                    },
                )?;
            }
            "FORGLOOP" => {
                let a = self.rng.range(0, 12) as f64;
                self.setf("A", a);
                let aux = self.rng.choose(&[1, 2, 3, 2, 1]);
                self.setf("AUX", aux as f64);
                self.iter_nil = self.rng.range(0, 3) == 0;
                let nil = self.iter_nil;
                self.setreg(
                    self.field("A"),
                    V::host("iter", move |_, _| {
                        Ok(if nil {
                            vec![V::Nil]
                        } else {
                            vec![opaque("it", &[1]), opaque("it", &[2]), opaque("it", &[3])]
                        })
                    }),
                )?;
            }
            "CALL" | "TAILCALL" => {
                let a = self.rng.range(0, 8) as f64;
                self.setf("A", a);
                let a = self.field("A");
                let b = self.rng.range(0, 6) as f64;
                self.setf("B", b);
                let c = self.rng.range(0, 5) as f64;
                self.setf("C", c);
                self.setreg(a, opaque("fn", &[a as i64]))?;
                self.top = V::Num(if self.field("B") == 0.0 {
                    a + self.rng.range(0, 5) as f64
                } else {
                    self.rng.range(0, 12) as f64
                });
            }
            "RETURN" => {
                let a = self.rng.range(0, 8) as f64;
                self.setf("A", a);
                let a = self.field("A");
                let b = self.rng.range(0, 6) as f64;
                self.setf("B", b);
                self.top = V::Num(if self.field("B") == 0.0 {
                    a + self.rng.range(0, 5) as f64
                } else {
                    self.rng.range(0, 12) as f64
                });
            }
            "SETLIST" => {
                let a = self.rng.range(0, 6) as f64;
                self.setf("A", a);
                let b = self.rng.range(6, 12) as f64;
                self.setf("B", b);
                let c = self.rng.range(0, 5) as f64;
                self.setf("C", c);
                let aux = self.rng.range(1, 5) as f64;
                self.setf("AUX", aux);
                self.setreg(self.field("A"), V::table())?;
                self.top = V::Num(self.field("B") + self.rng.range(0, 4) as f64);
            }
            "GETVARARGS" => {
                let a = self.rng.range(0, 8) as f64;
                self.setf("A", a);
                let b = self.rng.range(0, 5) as f64;
                self.setf("B", b);
                self.top = V::Num(self.rng.range(0, 12) as f64);
            }
            name if name.starts_with("CALL_") => {
                let a = self.rng.range(0, 8) as f64;
                self.setf("A", a);
                self.setreg(self.field("A"), opaque("fn", &[self.field("A") as i64]))?;
            }
            _ => {}
        }
        Ok(())
    }
    pub fn normalize(&mut self) -> VmResult<()> {
        if self.name == "CALL" {
            let a = self.field("A");
            let c = self.field("C");
            if c != 0.0 {
                for i in (a + c - 1.0) as i64..(a + 8.0) as i64 {
                    self.setreg(i as f64, V::Nil)?;
                }
                self.top = n(0);
            }
        } else if self.name.starts_with("CALL_") {
            let (_, c) = fixed_call(&self.name).unwrap();
            let a = self.field("A");
            for i in (a + c as f64 - 1.0) as i64..(a + 8.0) as i64 {
                self.setreg(i as f64, V::Nil)?;
            }
        }
        Ok(())
    }
    pub fn snapshot(&self) -> String {
        let mut out = vec![self.r.key(), self.up.key()];
        let tail = self.name == "TAILCALL";
        out.push(tuple(&self.log));
        if !tail {
            out.push(self.nxt.key());
        }
        if matches!(self.name.as_str(), "RETURN" | "TAILCALL") {
            out.push(
                self.ret
                    .as_ref()
                    .map(|v| tuple(&v.iter().map(V::key).collect::<Vec<_>>()))
                    .unwrap_or_else(|| "None".into()),
            );
            out.push(self.rec.key());
        }
        if matches!(self.name.as_str(), "CALL" | "GETVARARGS") {
            out.push(self.top.key());
        }
        tuple(&out)
    }
    pub fn run(&mut self) -> VmResult<()> {
        let name = self.name.clone();
        let (a, b, c, rf, aux) = (
            self.field("A"),
            self.field("B"),
            self.field("C"),
            self.field("RF"),
            self.field("AUX"),
        );
        let mut vm = Interp::new(60000);
        match name.as_str() {
            "NOP" => {}
            "MOVE" => self.setreg(a, self.reg(b)?)?,
            "LOADNIL" => self.setreg(a, V::Nil)?,
            "LOADN" => self.setreg(a, V::Num(rf))?,
            "LOADK" => self.setreg(a, self.kget(rf))?,
            "GETIMPORT" => self.setreg(a, opaque("imp", &[aux as i64]))?,
            "GETUPVAL" | "SETUPVAL" => {
                let bx = self.up.get(&V::Num(b))?;
                let cont = bx.get(&n(1))?;
                let k = bx.get(&n(2))?;
                if name == "GETUPVAL" {
                    self.setreg(a, cont.get(&k)?)?;
                } else {
                    cont.set(k, self.reg(a)?)?;
                }
            }
            "GETTABLEKS" | "GETTABLE" | "GETTABLEN" | "SELF" => {
                let o = self.reg(b)?;
                let k = match name.as_str() {
                    "GETTABLEKS" | "SELF" => self.kget(aux),
                    "GETTABLE" => self.reg(c)?,
                    _ => V::Num(c + 1.0),
                };
                if name == "SELF" {
                    self.setreg(a + 1.0, o.clone())?;
                }
                self.setreg(a, vm.index(o, k)?)?;
            }
            "SETTABLEKS" | "SETTABLE" | "SETTABLEN" => {
                let k = match name.as_str() {
                    "SETTABLEKS" => self.kget(aux),
                    "SETTABLE" => self.reg(c)?,
                    _ => V::Num(c + 1.0),
                };
                vm.setindex(self.reg(b)?, k, self.reg(a)?)?;
            }
            "OR" | "ORK" | "AND" | "ANDK" => {
                let v = self.reg(b)?;
                let other = if name.ends_with('K') {
                    self.kget(c)
                } else {
                    self.reg(c)?
                };
                self.setreg(
                    a,
                    if name.starts_with("OR") {
                        if v.truthy() {
                            v
                        } else {
                            other
                        }
                    } else if v.truthy() {
                        other
                    } else {
                        v
                    },
                )?;
            }
            "NOT" => self.setreg(a, V::Bool(!self.reg(b)?.truthy()))?,
            "MINUS" => {
                let v = self.reg(b)?;
                self.setreg(
                    a,
                    if let V::Opaque(k) = v {
                        V::opaque(vec![quoted("neg"), k])
                    } else {
                        V::Num(-v.number()?)
                    },
                )?;
            }
            "LENGTH" => {
                let V::Opaque(k) = self.reg(b)? else {
                    return Err(lua_error("opaque expected"));
                };
                self.setreg(a, V::opaque(vec![quoted("len"), k]))?;
            }
            "CONCAT" => {
                let mut vals = vec![];
                for i in b as i64..=c as i64 {
                    vals.push(self.reg(i as f64)?);
                }
                let all = vals.iter().all(|v| matches!(v, V::Str(_) | V::Num(_)));
                let mut acc = vals
                    .first()
                    .cloned()
                    .ok_or_else(|| lua_error("empty concat"))?;
                if all {
                    let mut bytes = vec![];
                    for v in vals {
                        match v {
                            V::Str(s) => bytes.extend(s),
                            v => bytes.extend(v.string().bytes()),
                        }
                    }
                    acc = V::Str(bytes);
                } else {
                    for v in vals.iter().skip(1) {
                        acc = V::opaque(vec![quoted("concat"), acc.key(), v.key()]);
                    }
                }
                self.setreg(a, acc)?;
            }
            "NEWTABLE" => self.setreg(a, V::table())?,
            "DUPTABLE" => {
                let t = V::table();
                let template = self
                    .tmpl
                    .as_ref()
                    .ok_or_else(|| lua_error("missing template"))?;
                for (_, p) in template.get(&n(2))?.items()? {
                    let v = p.get(&n(2))?.number()?;
                    if v >= 0.0 {
                        t.set(self.kget(p.get(&n(1))?.number()?), self.kget(v))?;
                    }
                }
                self.setreg(a, t)?;
            }
            "JUMP" => self.nxt = self.alt.clone(),
            "JUMPIF" | "JUMPIFNOT" => {
                if self.reg(a)?.truthy() == (name == "JUMPIF") {
                    self.nxt = self.alt.clone();
                }
            }
            "JUMPIFEQ" | "JUMPIFNOTEQ" | "JUMPIFLE" | "JUMPIFLT" | "JUMPIFNOTLT"
            | "JUMPIFNOTLE" => {
                let op = match name.as_str() {
                    "JUMPIFEQ" => "CompareEq",
                    "JUMPIFNOTEQ" => "CompareNe",
                    "JUMPIFLE" | "JUMPIFNOTLE" => "CompareLe",
                    _ => "CompareLt",
                };
                let v = compare(op, &self.reg(a)?, &self.reg(aux)?)?;
                if v != matches!(name.as_str(), "JUMPIFNOTLT" | "JUMPIFNOTLE") {
                    self.nxt = self.alt.clone();
                }
            }
            "JUMPXEQK" | "JUMPXEQKB" | "JUMPXEQKNIL" => {
                let v = self.reg(a)?;
                let x = aux as u64;
                let eq = match name.as_str() {
                    "JUMPXEQK" => lua_eq(&v, &self.kget((x & 0xffffff) as f64)),
                    "JUMPXEQKB" => matches!(v,V::Bool(b) if b==(x&1!=0)),
                    _ => v.is_nil(),
                };
                if eq != (x >> 31 & 1 != 0) {
                    self.nxt = self.alt.clone();
                }
            }
            "LOADB" => {
                self.setreg(a, V::Bool(b == 1.0))?;
                if c != 0.0 {
                    self.nxt = self.alt.clone();
                }
            }
            "FORNPREP" | "FORNLOOP" => {
                let lim = self.reg(a)?.number()?;
                let step = self.reg(a + 1.0)?.number()?;
                let mut idx = self.reg(a + 2.0)?.number()?;
                let jump = if name == "FORNLOOP" {
                    idx += step;
                    self.setreg(a + 2.0, V::Num(idx))?;
                    step > 0.0 && idx <= lim || step <= 0.0 && idx >= lim
                } else {
                    step > 0.0 && idx > lim || step <= 0.0 && idx < lim
                };
                if jump {
                    self.nxt = self.alt.clone();
                }
            }
            "FORGPREP" => {
                if self.bad_iter {
                    return Err(lua_error("invalid iterator"));
                }
                self.nxt = self.alt.clone();
            }
            "FORGLOOP" => {
                let res = if self.iter_nil {
                    vec![V::Nil]
                } else {
                    vec![opaque("it", &[1]), opaque("it", &[2]), opaque("it", &[3])]
                };
                for i in 0..(aux as i64 & 255) {
                    self.setreg(
                        a + 3.0 + i as f64,
                        res.get(i as usize).cloned().unwrap_or(V::Nil),
                    )?;
                }
                if !self.reg(a + 3.0)?.is_nil() {
                    self.setreg(a + 2.0, self.reg(a + 3.0)?)?;
                    self.nxt = self.alt.clone();
                }
            }
            "RETURN" => {
                let last = if b == 0.0 {
                    self.top.number()?
                } else {
                    a + b - 2.0
                };
                self.ret = Some(
                    (a as i64..=last as i64)
                        .map(|i| self.reg(i as f64))
                        .collect::<VmResult<_>>()?,
                );
                if self.clr {
                    self.rec.clear()?;
                }
            }
            "SETLIST" => {
                let count = if c == 0.0 {
                    self.top.number()? - b + 1.0
                } else {
                    c - 1.0
                };
                let t = self.reg(a)?;
                for i in 0..count as i64 {
                    t.set(V::Num(aux + i as f64), self.reg(b + i as f64)?)?;
                }
            }
            "GETVARARGS" => {
                let count = if b == 0.0 {
                    self.top = V::Num(a + self.nvar as f64 - 1.0);
                    self.nvar as i64
                } else {
                    b as i64 - 1
                };
                for i in 0..count {
                    self.setreg(a + i as f64, self.varargs.get(&V::Num((i + 1) as f64))?)?;
                }
            }
            name if name == "CALL" || name == "TAILCALL" || name.starts_with("CALL_") => {
                let fixed = fixed_call(name);
                let (b, c) = fixed.map(|(b, c)| (b as f64, c as f64)).unwrap_or((b, c));
                let count = if b == 0.0 {
                    self.top.number()? - a
                } else {
                    b - 1.0
                };
                let args = (0..count as i64)
                    .map(|i| self.reg(a + 1.0 + i as f64))
                    .collect::<VmResult<Vec<_>>>()?;
                let res = vm.call(self.reg(a)?, args)?;
                if name == "TAILCALL" {
                    self.ret = Some(res);
                    if self.clr {
                        self.rec.clear()?;
                    }
                } else if c == 0.0 {
                    for (i, v) in res.iter().enumerate() {
                        self.setreg(a + i as f64, v.clone())?;
                    }
                    self.top = V::Num(a + res.len() as f64 - 1.0);
                } else {
                    for i in 0..c as i64 - 1 {
                        self.setreg(a + i as f64, res.get(i as usize).cloned().unwrap_or(V::Nil))?;
                    }
                }
            }
            name => {
                let (base, mode) = if let Some(base) = name.strip_suffix("RK") {
                    (base, 2)
                } else if let Some(base) = name.strip_suffix('K') {
                    (base, 1)
                } else {
                    (name, 0)
                };
                let op = match base {
                    "ADD" => "Add",
                    "SUB" => "Sub",
                    "MUL" => "Mul",
                    "DIV" => "Div",
                    "MOD" => "Mod",
                    "POW" => "Pow",
                    "IDIV" => "FloorDiv",
                    _ => return Err(crate::luavm::unsupported(name)),
                };
                let left = if mode == 2 {
                    self.kget(b)
                } else {
                    self.reg(b)?
                };
                let right = if mode == 1 {
                    self.kget(c)
                } else {
                    self.reg(c)?
                };
                self.setreg(a, arith(op, left, right)?)?;
            }
        }
        self.log = vm.log;
        Ok(())
    }
}
pub fn fixed_call(name: &str) -> Option<(i64, i64)> {
    let tail = name.strip_prefix("CALL_")?;
    let (b, c) = tail.split_once('_')?;
    Some((b.parse().ok()?, c.parse().ok()?))
}

#[cfg(test)]
mod random_tests {
    #[test]
    fn integer_seeded_mt_matches_cpython_word_stream() {
        let mut rng = super::Rng::new(0);
        for expected in [3626764237, 1654615998, 3255389356, 3823568514, 1806341205] {
            assert_eq!(rng.word(), expected);
        }
    }
}

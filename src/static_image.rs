//! Data-only decoder for the explicitly pinned d20c9817ff3b VM image.
use crate::{
    astutil::{lkey, unlatin1, unparen},
    error::{Error, Result},
    loader::{array, ty},
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
pub const SOURCE_HASH: &str = "e63785897ea15eb312fe87814fda3e1cc2655e4ba81febb11a83e6a22f473b76";
fn err(s: impl Into<String>) -> Error {
    Error::new("image", s)
}
pub fn rol(value: u32, count: i64, bits: u32) -> u32 {
    let mask = if bits == 32 {
        u32::MAX
    } else {
        (1u32 << bits) - 1
    };
    let count = count.rem_euclid(bits as i64) as u32;
    let v = value & mask;
    (((v as u64) << count) | ((v as u64) >> ((bits - count) % bits))) as u32 & mask
}
pub fn shift(mut value: u32, steps: [u32; 3]) -> u32 {
    value ^= value.wrapping_shl(steps[0]);
    value ^= value.wrapping_shr(steps[1]);
    value ^ value.wrapping_shl(steps[2])
}
pub fn record_index(value: u32, family: u32, salt: u32, rotate: u32) -> u32 {
    if family == 0 {
        value
    } else if family == 1 {
        value ^ salt
    } else {
        rol(value ^ salt, -(rotate as i64), 32)
    }
}
pub fn field_value(value: u32, width: u32, keys: (u32, u32, u32)) -> u32 {
    let bits = width * 8;
    let mask = if bits == 32 {
        u32::MAX
    } else {
        (1u32 << bits) - 1
    };
    rol(
        value.wrapping_sub(keys.1 & mask) & mask,
        -((keys.2 % bits) as i64),
        bits,
    ) ^ (keys.0 & mask)
}
pub fn opcode_value(value: u32, family: u32, affine: (u32, u32, u32, u32)) -> u32 {
    let (add, inverse, xor, rotation) = affine;
    match family {
        0 => value.wrapping_sub(add).wrapping_mul(inverse) & 255,
        1 => (value.wrapping_sub(add) & 255) ^ xor,
        _ => rol(value ^ xor, -(rotation as i64), 8).wrapping_sub(add) & 255,
    }
}
pub fn mix_value(fields: [i64; 6]) -> u32 {
    let [a, b, c, rf, hf, aux] = fields;
    let value = aux as u32
        ^ rf as u32
        ^ (a as u32).wrapping_add(1).wrapping_mul(2412427463)
        ^ (b as u32).wrapping_add(1).wrapping_mul(2697606547)
        ^ (c as u32).wrapping_add(1).wrapping_mul(3154191535);
    let constant = rol(873872955, 23, 32);
    match hf & 3 {
        0 => value ^ constant,
        1 => rol(
            value.wrapping_add(constant),
            1 + (a + c + 4).rem_euclid(31),
            32,
        ),
        2 => shift(value, [13, 17, 5]) ^ constant,
        _ => rol(
            shift(value ^ constant, [7, 9, 13]).wrapping_add(2412427463),
            1 + (b + 4).rem_euclid(31),
            32,
        ),
    }
}
pub fn checksum(data: &[u8], start: usize) -> u32 {
    let (mut a, mut b) = (
        rol(2048000001, 15, 32) as u64,
        4198473167u32.wrapping_add(96539108) as u64,
    );
    for byte in data.get(start..).unwrap_or(&[]) {
        a = (a + *byte as u64 * (1287708779 ^ 1287708798)) % 65413;
        b = (b + a) % 65413;
    }
    ((b * 65536 + a) as u32) ^ rol(3257154185, 20, 32)
}
pub fn matches_source(data: &[u8]) -> bool {
    matches_hash(data, SOURCE_HASH)
}
fn matches_hash(data: &[u8], expected: &str) -> bool {
    format!("{:x}", Sha256::digest(data)) == expected
}
fn unsigned(v: &Value) -> Result<u32> {
    v.as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| err("invalid unsigned image integer"))
}
fn number(v: &Value) -> Result<usize> {
    v.as_u64()
        .and_then(|n| usize::try_from(n).ok())
        .ok_or_else(|| err("invalid image length"))
}
pub fn decode_page(
    page: &Value,
    alphabet: &str,
    shifts: &[[u32; 3]],
    max_bytes: usize,
) -> Result<Vec<u8>> {
    let alphabet: Vec<_> = alphabet.chars().collect();
    if alphabet.len() != 85 || alphabet.iter().collect::<BTreeSet<_>>().len() != 85 {
        return Err(err("invalid transport alphabet"));
    }
    let (length, size, flags) = (
        number(&page["2"])?,
        number(&page["3"])?,
        unsigned(&page["4"])?,
    );
    if length.max(size) > max_bytes {
        return Err(err("page length exceeds limit"));
    }
    let encoded = if let Some(parts) = page["1"].as_object() {
        let order = page["8"]
            .as_object()
            .ok_or_else(|| err("invalid page segment order"))?;
        if order.len() != parts.len()
            || order.keys().cloned().collect::<BTreeSet<_>>()
                != (1..=parts.len()).map(|i| i.to_string()).collect()
        {
            return Err(err("invalid page segment order"));
        }
        let indices = order
            .values()
            .map(number)
            .collect::<Result<BTreeSet<_>>>()?;
        if indices != (1..=parts.len()).collect() {
            return Err(err("page segment order is not a permutation"));
        }
        let mut text = String::new();
        for i in 1..=parts.len() {
            let key = number(&order[&i.to_string()])?;
            text.push_str(
                parts
                    .get(&key.to_string())
                    .and_then(Value::as_str)
                    .ok_or_else(|| err("invalid page segment"))?,
            );
        }
        text
    } else {
        page["1"]
            .as_str()
            .ok_or_else(|| err("invalid base85 page length"))?
            .to_owned()
    };
    let chars: Vec<_> = encoded.chars().collect();
    if chars.len() % 5 != 0 || chars.len() != length.div_ceil(4) * 5 {
        return Err(err("invalid base85 page length"));
    }
    let (selector, mode, marker) = (((flags >> 8) & 7) as usize, (flags >> 11) & 3, flags >> 13);
    if selector >= shifts.len() || mode > 2 || marker > 1 {
        return Err(err("unsupported transport flags"));
    }
    if shifts[selector].iter().any(|n| *n >= 32) {
        return Err(err("invalid cipher shifts"));
    }
    let (mut state, multiplier, offset) = (
        unsigned(&page["5"])?,
        unsigned(&page["6"])?,
        unsigned(&page["7"])?,
    );
    let mut output = Vec::with_capacity(size);
    let (mut consumed, mut phase, mut remaining, mut low) = (0usize, 0, 0usize, 0usize);
    for group in chars.chunks(5) {
        let mut value = 0u64;
        for c in group {
            let digit = alphabet
                .iter()
                .position(|a| a == c)
                .ok_or_else(|| err("invalid base85 character"))?;
            value = value * 85 + digit as u64;
        }
        if value > u32::MAX as u64 {
            return Err(err("base85 word overflow"));
        }
        for byte in (value as u32).to_be_bytes() {
            if consumed == length {
                break;
            }
            consumed += 1;
            state = shift(state, shifts[selector]);
            let key = (state
                ^ (consumed as u32)
                    .wrapping_mul(multiplier)
                    .wrapping_add(offset)) as u8;
            let byte = match mode {
                0 => byte ^ key,
                1 => byte.wrapping_sub(key),
                _ => byte.wrapping_add(key),
            } ^ (flags as u8);
            match phase {
                0 => {
                    let copy = if marker == 1 { byte >= 128 } else { byte < 128 };
                    remaining = (byte % 128) as usize + if copy { 3 } else { 1 };
                    phase = if copy { 1 } else { 3 };
                }
                1 => {
                    low = byte as usize;
                    phase = 2;
                }
                2 => {
                    let distance = low + byte as usize * 256 + 1;
                    if distance > output.len() || output.len() + remaining > size {
                        return Err(err("invalid page backreference"));
                    }
                    for _ in 0..remaining {
                        output.push(output[output.len() - distance]);
                    }
                    remaining = 0;
                    phase = 0;
                }
                _ => {
                    if output.len() >= size {
                        return Err(err("page output exceeds declared length"));
                    }
                    output.push(byte);
                    remaining -= 1;
                    if remaining == 0 {
                        phase = 0;
                    }
                }
            }
        }
    }
    if consumed != length || phase != 0 || output.len() != size {
        return Err(err("page ended with incomplete data"));
    }
    Ok(output)
}
pub fn literal(node: &Value) -> Result<Value> {
    let node = unparen(node);
    match ty(node) {
        "AstExprConstantNumber" | "AstExprConstantString" | "AstExprConstantBool" => {
            let v = &node["value"];
            if let Some(f) = v.as_f64() {
                if f.fract() == 0.0 && f >= i64::MIN as f64 && f < (i64::MAX as f64) {
                    return Ok(json!(f as i64));
                }
            }
            Ok(v.clone())
        }
        "AstExprTable" => {
            let mut result = json!({});
            let mut index = 0;
            for item in array(&node["items"]) {
                let key = if item["key"].is_null() {
                    index += 1;
                    index.to_string()
                } else {
                    let k = literal(&item["key"])?;
                    match k {
                        Value::String(s) => s,
                        Value::Number(n) => n.to_string(),
                        Value::Bool(b) => {
                            if b {
                                "1".into()
                            } else {
                                "0".into()
                            }
                        }
                        _ => return Err(err("invalid literal table key")),
                    }
                };
                if result.get(&key).is_some() {
                    return Err(err("duplicate literal table key"));
                }
                result[key] = literal(&item["value"])?;
            }
            Ok(result)
        }
        _ => Err(err("nonliteral node in image data")),
    }
}
pub struct Reader {
    pub data: Vec<u8>,
    pub position: usize,
}
impl Reader {
    pub fn new(data: Vec<u8>) -> Self {
        Self { data, position: 0 }
    }
    pub fn take(&mut self, length: usize) -> Result<Vec<u8>> {
        let end = self
            .position
            .checked_add(length)
            .filter(|end| *end <= self.data.len())
            .ok_or_else(|| err("truncated image data"))?;
        let out = self.data[self.position..end].to_vec();
        self.position = end;
        Ok(out)
    }
    pub fn uint(&mut self, length: usize) -> Result<u32> {
        if length > 4 {
            return Err(err("integer width exceeds four bytes"));
        }
        let mut bytes = [0u8; 4];
        bytes[..length].copy_from_slice(&self.take(length)?);
        Ok(u32::from_le_bytes(bytes))
    }
    pub fn varint(&mut self) -> Result<u32> {
        let mut value = 0u64;
        for index in 0..5 {
            let byte = self.uint(1)?;
            value += (byte as u64 & 127) << (index * 7);
            if value > u32::MAX as u64 {
                return Err(err("varint overflow"));
            }
            if byte < 128 {
                return Ok(value as u32);
            }
        }
        Err(err("unterminated varint"))
    }
    pub fn finish(&self) -> Result<()> {
        if self.position != self.data.len() {
            return Err(err("unconsumed image bytes"));
        }
        Ok(())
    }
    fn count(&mut self) -> Result<usize> {
        let count = self.varint()? as usize;
        if count > self.data.len().saturating_sub(self.position) {
            return Err(err("image element count exceeds remaining bytes"));
        }
        Ok(count)
    }
}
#[derive(Clone)]
struct Section {
    page: i64,
    offset: usize,
    size: usize,
    logical: u32,
}
#[derive(Clone, Default)]
struct Proto {
    paged: bool,
    page: u32,
    size: usize,
    check: u32,
    pages: BTreeMap<u32, (u32, usize, u32)>,
    data: Vec<u8>,
    vf: u32,
    salt: u32,
    rotate: u32,
    affine: (u32, u32, u32, u32),
    order: usize,
    byte_keys: Vec<(u32, u32)>,
    field_keys: Vec<(u32, u32, u32)>,
    records: BTreeMap<u32, Section>,
    constants: BTreeMap<u32, Section>,
    kids: Vec<u32>,
    entry: u32,
    stack: u32,
    params: u32,
    upvalues: u32,
    vararg: bool,
    df: u32,
    reg_shift: u32,
}
struct StaticImage<'a> {
    loader: &'a Value,
    pages: Value,
    shifts: Vec<[u32; 3]>,
    alphabet: String,
    cache: BTreeMap<u32, Vec<u8>>,
    decoded_bytes: usize,
    key: u32,
    main: u32,
    protos: BTreeMap<u32, Proto>,
}
impl<'a> StaticImage<'a> {
    fn new(loader: &'a Value) -> Result<Self> {
        let s = array(&loader["root"]["body"]);
        let expr = |i: usize| -> Result<&Value> {
            s.get(i)
                .and_then(|s| array(&s["values"]).first())
                .ok_or_else(|| err("static image declaration missing"))
        };
        let pages = literal(expr(2)?)?;
        let steps = literal(expr(4)?)?;
        let mut shifts = vec![];
        let map = steps
            .as_object()
            .ok_or_else(|| err("invalid image shift table"))?;
        for i in 1..=map.len() {
            let p = &steps[i.to_string()];
            shifts.push([unsigned(&p["1"])?, unsigned(&p["2"])?, unsigned(&p["3"])?]);
        }
        let alphabet = literal(expr(5)?)?
            .as_str()
            .ok_or_else(|| err("invalid image alphabet"))?
            .to_owned();
        if !pages.is_object() {
            return Err(err("invalid image page table"));
        }
        Ok(Self {
            loader,
            pages,
            shifts,
            alphabet,
            cache: BTreeMap::new(),
            decoded_bytes: 0,
            key: 0,
            main: 0,
            protos: BTreeMap::new(),
        })
    }
    fn page(&mut self, index: u32, size: Option<usize>, check: Option<u32>) -> Result<Vec<u8>> {
        if !self.cache.contains_key(&index) {
            let page = self
                .pages
                .get(index.to_string())
                .ok_or_else(|| err("page reference is missing"))?;
            let data = decode_page(page, &self.alphabet, &self.shifts, 3 * 1024 * 1024)?;
            self.decoded_bytes += data.len();
            if self.decoded_bytes > 32 * 1024 * 1024 {
                return Err(err("total static image exceeds limit"));
            }
            self.cache.insert(index, data);
        }
        let data = self.cache.get(&index).unwrap();
        if size.map(|s| s != data.len()).unwrap_or(false)
            || check.map(|c| checksum(data, 0) != c).unwrap_or(false)
        {
            return Err(err("page length or checksum mismatch"));
        }
        Ok(data.clone())
    }
    fn metadata(&mut self) -> Result<()> {
        let mut r = Reader::new(self.page(1, None, None)?);
        if r.uint(4)? != 327239619u32.wrapping_add(553305879) {
            return Err(err("image magic mismatch"));
        }
        let check = r.uint(4)?;
        if checksum(&r.data, 8) != check {
            return Err(err("image header checksum mismatch"));
        }
        let fingerprint = rol(400639111, 31, 32)
            ^ (3077271321 ^ 781395780)
            ^ 655474375u32.wrapping_add(2132074348)
            ^ (762645833 ^ 1271823234);
        self.key = 209879755u32.wrapping_add(641415351)
            ^ rol(rol(3086240084, 6, 32), 16, 32)
            ^ rol(2622446568, 27, 32)
            ^ 2773452296u32.wrapping_add(2402060498)
            ^ check
            ^ fingerprint;
        if ((r.uint(1)? ^ 33) * 195 + 160) % 256 != 182 {
            return Err(err("unsupported image version"));
        }
        let (count, main, pages) = (r.varint()?, r.varint()?, r.varint()?);
        let page_count = self.pages.as_object().unwrap().len() as u32;
        if count == 0 || count > 10000 || main >= count || pages.checked_add(1) != Some(page_count)
        {
            return Err(err("invalid prototype/page count"));
        }
        let mut used = BTreeSet::new();
        for pid in 0..count {
            let (paged, page, size, check) =
                (r.uint(1)?, r.varint()?, r.varint()? as usize, r.uint(4)?);
            if paged > 1 || !(2..=page_count).contains(&page) || !used.insert(page) {
                return Err(err("invalid prototype page"));
            }
            let mut children = BTreeMap::new();
            for index in 0..r.count()? {
                let p = (r.varint()?, r.varint()? as usize, r.uint(4)?);
                if !(2..=page_count).contains(&p.0) || !used.insert(p.0) {
                    return Err(err("duplicate image data page"));
                }
                children.insert(index as u32, p);
            }
            self.protos.insert(
                pid,
                Proto {
                    paged: paged == 1,
                    page,
                    size,
                    check,
                    pages: children,
                    ..Proto::default()
                },
            );
        }
        r.finish()?;
        if used != (2..=page_count).collect() {
            return Err(err("unreferenced image pages"));
        }
        self.main = main;
        Ok(())
    }
    fn proto(&mut self, pid: u32, p: &mut Proto) -> Result<()> {
        let mut r = Reader::new(self.page(p.page, Some(p.size), Some(p.check))?);
        if p.paged && r.uint(1)? != 1 {
            return Err(err("invalid paged prototype version"));
        }
        p.stack = r.uint(1)?;
        p.params = r.uint(1)?;
        p.upvalues = r.uint(1)?;
        let flags = r.uint(1)?;
        p.vf = (flags >> 1) & 3;
        let cache = (flags >> 3) & 3;
        p.salt = r.uint(4)?;
        let header = r.uint(1)?;
        if p.vf > 2 || cache > 2 || header & 128 != 0 {
            return Err(err("invalid prototype flags"));
        }
        p.rotate = header & 31;
        p.vararg = flags & 1 != 0;
        p.entry = r.varint()?;
        let count = r.varint()?;
        if count > 1000000 {
            return Err(err("instruction count exceeds limit"));
        }
        let factor = pid.wrapping_add(1);
        let state = shift(
            1581107056u32.wrapping_add(3594405738) ^ factor.wrapping_mul(3896966113 ^ 3502236359),
            [13, 17, 5],
        );
        p.reg_shift = if p.stack >= 220 { 0 } else { state % 15 + 1 };
        let state = shift(
            rol(3062701827, 10, 32) ^ factor.wrapping_mul(373618271u32.wrapping_add(2061450517)),
            [13, 17, 5],
        );
        let odd = ((state >> 8) & 255) | 1;
        let inverse = (1u32..256)
            .step_by(2)
            .find(|n| n.wrapping_mul(odd) & 255 == 1)
            .unwrap();
        p.affine = (
            state & 255,
            inverse,
            (state >> 16) & 255,
            1 + ((state >> 24) & 255) % 7,
        );
        p.order = (shift(
            rol(3660872718, 8, 32) ^ factor.wrapping_mul(1608508304u32.wrapping_add(779605132)),
            [13, 17, 5],
        ) & 3) as usize;
        p.df = shift(
            1547655761u32.wrapping_add(3627857033)
                ^ factor.wrapping_mul(3808998020u32.wrapping_add(1626062670)),
            [13, 17, 5],
        ) & 3;
        let mut state = (1436517690 ^ 1642302432)
            ^ 1096098286u32.wrapping_add(3643197036)
            ^ factor.wrapping_mul(41803);
        let mut next = || {
            state = shift(state, [13, 17, 5]);
            state
        };
        for _ in 0..3 {
            let v = next();
            p.byte_keys.push((v & 255, (v >> 8) & 255));
        }
        for _ in 0..3 {
            p.field_keys.push((next(), next(), 1 + next() % 31));
        }
        for _ in 0..count {
            let rid = r.varint()?;
            let logical = record_index(rid, p.vf, p.salt, p.rotate);
            let (page, offset) = if p.paged {
                (r.varint()? as i64, Some(r.varint()? as usize))
            } else {
                (-1, None)
            };
            let size = r.varint()? as usize;
            let offset = offset.unwrap_or(r.position);
            if logical >= count || p.records.contains_key(&rid) || !(4..=224).contains(&size) {
                return Err(err("invalid instruction record"));
            }
            p.records.insert(
                rid,
                Section {
                    page,
                    offset,
                    size,
                    logical,
                },
            );
            if !p.paged {
                r.take(size)?;
            }
        }
        if !p.records.contains_key(&p.entry) {
            return Err(err("entry record missing"));
        }
        let constant_count = r.count()?;
        for cid in 0..constant_count {
            let (page, offset) = if p.paged {
                (r.varint()? as i64, Some(r.varint()? as usize))
            } else {
                (-1, None)
            };
            let size = r.varint()? as usize;
            let offset = offset.unwrap_or(r.position);
            p.constants.insert(
                cid as u32,
                Section {
                    page,
                    offset,
                    size,
                    logical: 0,
                },
            );
            if !p.paged {
                r.take(size)?;
            }
        }
        for _ in 0..r.count()? {
            let child = r.varint()?;
            if !self.protos.contains_key(&child) && child != pid {
                return Err(err("child prototype missing"));
            }
            p.kids.push(child);
        }
        r.finish()?;
        p.data = r.data;
        Ok(())
    }
    fn section(&mut self, p: &Proto, s: &Section) -> Result<Vec<u8>> {
        let data = if s.page == -1 {
            p.data.clone()
        } else {
            let (page, size, check) = *p
                .pages
                .get(&(s.page as u32))
                .ok_or_else(|| err("prototype data page missing"))?;
            self.page(page, Some(size), Some(check))?
        };
        let end = s
            .offset
            .checked_add(s.size)
            .filter(|end| *end <= data.len())
            .ok_or_else(|| err("section extends outside data"))?;
        Ok(data[s.offset..end].to_vec())
    }
    fn decrypt(data: &[u8], mut state: u32, steps: [u32; 3], vf: u32) -> Reader {
        let result = data
            .iter()
            .enumerate()
            .map(|(i, byte)| {
                state = shift(state, steps);
                let key = state as u8;
                match vf {
                    2 => (rol(*byte as u32, -(((i + key as usize) % 7 + 1) as i64), 8) as u8) ^ key,
                    1 => byte.wrapping_sub(key).wrapping_sub((i * 29 + 17) as u8),
                    _ => *byte ^ key,
                }
            })
            .collect();
        Reader::new(result)
    }
    fn constant(&mut self, pid: u32, p: &Proto, cid: u32) -> Result<Value> {
        let seed = rol(1040674074, 25, 32)
            ^ rol(1692189232, 13, 32)
            ^ self.key
            ^ pid.wrapping_add(1).wrapping_mul(44601)
            ^ cid.wrapping_add(1).wrapping_mul(319);
        let data = self.section(p, &p.constants[&cid])?;
        let mut r = Self::decrypt(&data, seed, [7, 9, 13], p.vf);
        let kind = r.uint(1)?;
        let value = match kind {
            1 => {
                let mut a = vec![];
                for _ in 0..r.count()? {
                    a.push(r.varint()?);
                }
                json!(["i", a])
            }
            2 => {
                let size = r.varint()? as usize;
                json!(["s", r.take(size)?])
            }
            7 => {
                let bytes: [u8; 8] = r.take(8)?.try_into().unwrap();
                let n = f64::from_le_bytes(bytes);
                json!(["n", crate::model::encode_number(n)])
            }
            8 => {
                let mut a = vec![];
                for _ in 0..r.count()? {
                    let k = r.varint()?;
                    let v = r.uint(4)? as i32;
                    a.push(json!([k, v]));
                }
                json!(["t", a])
            }
            0 => json!(["b", r.uint(1)? != 0]),
            5 => json!(["n", r.uint(4)?]),
            4 => json!(["n", r.varint()?]),
            _ => return Err(err(format!("unsupported constant tag {kind}"))),
        };
        r.finish()?;
        Ok(value)
    }
    // Keep the binary record's field context explicit at both decoding sites.
    #[allow(clippy::too_many_arguments)]
    fn value(
        &self,
        r: &mut Reader,
        p: &Proto,
        pid: u32,
        logical: u32,
        name: usize,
        present: u32,
        width: u32,
        jump_width: u32,
        op_xor: u32,
    ) -> Result<i64> {
        if name == 0 {
            return Ok((opcode_value(r.uint(1)?, p.vf, p.affine) ^ op_xor) as i64);
        }
        if name == 1 {
            return Ok(r.uint(1)? as i64);
        }
        let flag = 1u32 << (name - 2);
        if present & flag == 0 {
            return Ok(if name >= 8 { u32::MAX as i64 } else { 0 });
        }
        if name < 5 {
            let (add, xor) = p.byte_keys[name - 2];
            return Ok(((r.uint(1)?.wrapping_sub(add) & 255) ^ xor) as i64);
        }
        if name < 8 {
            let value = field_value(r.uint(width as usize)?, width, p.field_keys[name - 5]);
            return Ok(if name != 7 && value as u64 >= 1u64 << (width * 8 - 1) {
                value as i64 - (1i64 << (width * 8))
            } else {
                value as i64
            });
        }
        let alternate = u32::from(name == 9);
        let state = rol(62295327, 18, 32)
            ^ (711279068 ^ 1474777020)
            ^ self.key
            ^ pid.wrapping_add(1).wrapping_mul(15317)
            ^ logical.wrapping_add(1).wrapping_mul(41587)
            ^ alternate.wrapping_add(1).wrapping_mul(34557);
        let mask = if jump_width == 4 {
            u32::MAX
        } else {
            (1u32 << (jump_width * 8)) - 1
        };
        Ok((r.uint(jump_width as usize)? ^ (shift(state, [13, 17, 5]) & mask)) as i64)
    }
    fn raw(&self, p: &Proto, fields: &[i64; 10]) -> Result<Value> {
        let perm = array(&self.loader["perm"][p.df.to_string()]);
        if perm.len() != 10 {
            return Err(err("invalid instruction field permutation"));
        }
        let mut table = [0i64; 10];
        let mut used = BTreeSet::new();
        for (name, pos) in perm.iter().enumerate() {
            let at = number(pos)?;
            if !(1..=10).contains(&at) || !used.insert(at) {
                return Err(err("invalid instruction field permutation"));
            }
            table[at - 1] = fields[name];
        }
        Ok(json!(table))
    }
    fn record(
        &mut self,
        pid: u32,
        p: &Proto,
        rid: u32,
        remap: &BTreeMap<u8, u8>,
        remap_xor: u8,
    ) -> Result<Value> {
        let desc = &p.records[&rid];
        let logical = desc.logical;
        let seed = (1384073472 ^ 1711491034)
            ^ (3570431865 ^ 2879290169)
            ^ self.key
            ^ pid.wrapping_add(1).wrapping_mul(38303)
            ^ logical.wrapping_add(1).wrapping_mul(15243);
        let data = self.section(p, desc)?;
        let mut r = Self::decrypt(&data, shift(seed, [7, 4, 6]), [13, 17, 5], p.vf);
        let mask = r.uint(1)?;
        let header = r.uint(1)?;
        let (count, width, jump_width) = (
            header & 15,
            ((header >> 4) & 3) + 1,
            ((header >> 6) & 3) + 1,
        );
        if count == 1 {
            return Err(err("invalid fused record arity"));
        }
        let orders = [
            [0, 1, 2, 3, 4, 5, 6, 7, 8, 9],
            [8, 2, 0, 5, 1, 9, 4, 7, 3, 6],
            [7, 4, 1, 3, 0, 9, 6, 8, 2, 5],
            [5, 9, 0, 3, 8, 7, 1, 6, 4, 2],
        ];
        let salt = (3443762138 ^ 1550291118)
            ^ logical
                .wrapping_add(1)
                .wrapping_mul(rol(1673691136, 17, 32))
            ^ self.key;
        let op_xor = (salt ^ (salt >> 16)) & 255;
        let mut f = [0i64; 10];
        for name in orders[p.order] {
            f[name] = self.value(
                &mut r, p, pid, logical, name, mask, width, jump_width, op_xor,
            )?;
        }
        let junk = self.loader["junk_op"]
            .as_i64()
            .ok_or_else(|| err("junk opcode missing"))?;
        if f[0] != junk {
            for name in [8, 9] {
                if f[name] != u32::MAX as i64 {
                    let v = f[name] as u32;
                    f[name] = match p.vf {
                        0 => v,
                        1 => v ^ p.salt,
                        _ => rol(v, p.rotate as i64, 32) ^ p.salt,
                    } as i64;
                }
            }
        }
        let op = f[0] as u32;
        let semantic = (((op ^ 41) << 2 | (op ^ 41) >> 6) + 172) & 255;
        if let Some(mapped) = remap.get(&(semantic as u8)) {
            f[0] = (mapped ^ remap_xor) as i64;
        }
        let mut parts = vec![];
        if count != 0 {
            let mut first = f;
            first[0] = 0;
            first[1] = 0;
            first[8] = u32::MAX as i64;
            first[9] = u32::MAX as i64;
            parts.push(self.raw(p, &first)?);
            let part_orders = [
                [1, 2, 3, 4, 5, 6, 7],
                [2, 5, 1, 4, 7, 3, 6],
                [7, 4, 1, 3, 6, 2, 5],
                [5, 3, 7, 6, 4, 2, 1],
            ];
            for _ in 1..count {
                let present = r.uint(1)?;
                let mut part = [0; 10];
                part[8] = u32::MAX as i64;
                part[9] = u32::MAX as i64;
                for name in part_orders[p.order] {
                    part[name] = self.value(
                        &mut r, p, pid, logical, name, present, width, jump_width, op_xor,
                    )?;
                }
                parts.push(self.raw(p, &part)?);
            }
        }
        r.finish()?;
        for name in [8, 9] {
            if f[name] != u32::MAX as i64
                && (name == 9 || f[0] != junk)
                && !p.records.contains_key(&(f[name] as u32))
            {
                return Err(err("instruction jump target is missing"));
            }
        }
        Ok(json!({"sem":semantic,"T":self.raw(p,&f)?,"parts":parts}))
    }
    fn table(&self, role: &str) -> Result<Value> {
        let local = &self.loader["role_local"][role];
        let st = &self.loader["local_decl"][lkey(local)][0];
        let i = array(&st["vars"])
            .iter()
            .position(|v| v == local)
            .ok_or_else(|| err(format!("image table {role} declaration missing")))?;
        literal(
            array(&st["values"])
                .get(i)
                .ok_or_else(|| err("image table initializer missing"))?,
        )
    }
    fn decode(mut self) -> Result<Value> {
        self.metadata()?;
        let ids: Vec<_> = self.protos.keys().copied().collect();
        for pid in &ids {
            let mut p = self.protos.remove(pid).unwrap();
            self.proto(*pid, &mut p)?;
            self.protos.insert(*pid, p);
        }
        let mut d = json!({"main":self.main,"xorkey":self.table("XORKEY")?,"simple":[],"pat1":{},"pat2":{},"ded":{},"protos":{}});
        let simple = self.table("SIMPLESET")?;
        d["simple"] = json!(simple
            .as_object()
            .ok_or_else(|| err("invalid simple table"))?
            .keys()
            .map(|k| k.parse::<i64>().map_err(|_| err("invalid simple opcode")))
            .collect::<Result<BTreeSet<_>>>()?);
        for (role, key) in [("P1", "pat1"), ("P2", "pat2")] {
            let t = self.table(role)?;
            for (k, v) in t.as_object().ok_or_else(|| err("invalid pattern table"))? {
                d[key][k] = json!(unlatin1(
                    v.as_str().ok_or_else(|| err("invalid pattern bytes"))?
                )?);
            }
        }
        for ti in 1..=6 {
            let items = &self.loader["ded_decl"][ti.to_string()];
            if !items.is_array() {
                return Err(err("dedicated image table missing"));
            }
            d["ded"][ti.to_string()] = json!(array(items)
                .iter()
                .map(|p| p[0]
                    .as_i64()
                    .ok_or_else(|| err("invalid dedicated image opcode")))
                .collect::<Result<BTreeSet<_>>>()?);
        }
        let remaps_node = array(&self.loader["MOD"]["body"]["body"])
            .iter()
            .find(|s| {
                ty(s) == "AstStatLocal"
                    && s["location"].as_str().unwrap_or("").starts_with("50,697")
            })
            .ok_or_else(|| err("pinned opcode remap declaration missing"))?;
        let remaps = literal(
            array(&remaps_node["values"])
                .first()
                .ok_or_else(|| err("opcode remap data missing"))?,
        )?;
        for pid in ids {
            let p = self.protos[&pid].clone();
            let mut constants = json!({});
            for cid in p.constants.keys() {
                constants[cid.to_string()] = self.constant(pid, &p, *cid)?;
            }
            let (mut remap, mut xor) = (BTreeMap::new(), 0);
            if let Some(text) = remaps.get(pid.to_string()) {
                let bytes = unlatin1(text.as_str().ok_or_else(|| err("invalid opcode remap"))?)?;
                if bytes.is_empty() || bytes.len() % 2 != 1 {
                    return Err(err("invalid opcode remap length"));
                }
                xor = bytes[0];
                for pair in bytes[1..].chunks(2) {
                    remap.insert(pair[0], pair[1]);
                }
            }
            let mut records = json!({});
            for rid in p.records.keys() {
                let mut record = self.record(pid, &p, *rid, &remap, xor)?;
                let sem = record["sem"].as_u64().unwrap() as u32;
                let mut ded = "-".to_string();
                for ti in 1..=6 {
                    let key = sem + if ti == 6 { pid * 256 } else { 0 };
                    if array(&d["ded"][ti.to_string()])
                        .iter()
                        .any(|v| v.as_u64() == Some(key as u64))
                    {
                        ded = ti.to_string();
                        break;
                    }
                }
                record["ded"] = json!(ded);
                record["pat"] = d["pat1"]
                    .get(sem.to_string())
                    .or_else(|| d["pat2"].get((sem + pid * 256).to_string()))
                    .cloned()
                    .unwrap_or(Value::Null);
                let perm = array(&self.loader["perm"][p.df.to_string()]);
                let values = (0..10)
                    .map(|i| {
                        let at = number(&perm[i])?;
                        record["T"][at - 1]
                            .as_i64()
                            .ok_or_else(|| err("invalid raw image field"))
                    })
                    .collect::<Result<Vec<_>>>()?;
                let mix = mix_value([
                    values[2], values[3], values[4], values[5], values[6], values[7],
                ]);
                record["mix"] = json!(mix);
                if self.loader["junk_op"].as_i64() == Some(values[0])
                    && !p.records.contains_key(&((values[8] as u32) ^ mix))
                {
                    return Err(err("junk jump target missing"));
                }
                records[rid.to_string()] = record;
            }
            d["protos"][pid.to_string()] = json!({"idx":pid,"np":p.params,"va":u8::from(p.vararg),"nup":p.upvalues,"ms":p.stack,"entry":p.entry,"df":p.df,"vf":p.vf,"nrec":p.records.len(),"rs":p.reg_shift,"kids":p.kids,"consts":constants,"recs":records});
        }
        Ok(d)
    }
}
pub fn decode(loader: &Value) -> Result<Value> {
    let text = loader["text"]
        .as_str()
        .ok_or_else(|| err("loader text missing"))?;
    if !matches_source(&unlatin1(text)?) {
        return Err(err("source does not match pinned static image profile"));
    }
    StaticImage::new(loader)?.decode()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn image(loader: &Value) -> StaticImage<'_> {
        StaticImage {
            loader,
            pages: json!({}),
            shifts: vec![[13, 17, 5]],
            alphabet: String::new(),
            cache: BTreeMap::new(),
            decoded_bytes: 0,
            key: 0,
            main: 0,
            protos: BTreeMap::new(),
        }
    }
    fn encrypt(data: &[u8], mut state: u32, steps: [u32; 3]) -> Vec<u8> {
        data.iter()
            .map(|byte| {
                state = shift(state, steps);
                *byte ^ (state as u8)
            })
            .collect()
    }
    fn encrypt_family(data: &[u8], mut state: u32, steps: [u32; 3], family: u32) -> Vec<u8> {
        data.iter()
            .enumerate()
            .map(|(i, byte)| {
                state = shift(state, steps);
                let key = state as u8;
                match family {
                    2 => (byte ^ key).rotate_left(((i + key as usize) % 7 + 1) as u32),
                    1 => byte.wrapping_add(key).wrapping_add((i * 29 + 17) as u8),
                    _ => byte ^ key,
                }
            })
            .collect()
    }

    #[test]
    fn source_profile_matches_only_the_expected_normalized_bytes() {
        // Independently calculated SHA256 of ASCII "return 1\n".
        let hash = "0805bfdc02e872ed322a4a4e440ae985f3a4335f4dd59f3d373dfaf2a68a4a3c";
        assert!(matches_hash(b"return 1\n", hash));
        assert!(!matches_hash(b"return 2\n", hash));
        let bytes = crate::astutil::normalize_source(b"\xef\xbb\xbf\xef\xbb\xbfreturn 1\r\n");
        assert!(matches_hash(&bytes, hash));
        assert!(!matches_source(&bytes));
        let error = decode(&json!({"text":"return 1\n"})).unwrap_err();
        assert_eq!(error.code, "image");
        assert!(error.message.contains("source does not match"));
    }

    #[test]
    fn metadata_checks_magic_checksum_versions_counts_and_page_references() {
        let loader = json!({});
        let make = |tail: &[u8], pages: Value| {
            let mut data = 880545498u32.to_le_bytes().to_vec();
            data.extend(checksum(tail, 0).to_le_bytes());
            data.extend(tail);
            let mut image = image(&loader);
            image.pages = pages;
            image.cache.insert(1, data);
            image
        };
        // version 19, one proto, main 0, one payload page; the descriptor is data only.
        let tail = [19, 1, 0, 1, 0, 2, 4, 0, 0, 0, 0, 0];
        let mut valid = make(&tail, json!({"1":null,"2":null}));
        valid.metadata().unwrap();
        assert_eq!(valid.main, 0);
        assert_eq!(valid.protos.len(), 1);
        let proto = &valid.protos[&0];
        assert!(!proto.paged);
        assert_eq!((proto.page, proto.size, proto.check), (2, 4, 0));
        assert!(proto.pages.is_empty());
        for (offset, value, message) in [
            (0, 18, "version"),
            (1, 0, "count"),
            (2, 1, "count"),
            (3, 2, "count"),
            (4, 2, "prototype page"),
            (5, 1, "prototype page"),
            (11, 1, "count exceeds"),
        ] {
            let mut bad = tail;
            bad[offset] = value;
            let error = make(&bad, json!({"1":null,"2":null}))
                .metadata()
                .unwrap_err();
            assert_eq!(error.code, "image");
            assert!(error.message.contains(message), "{error}");
        }
        for offset in [0, 4] {
            let mut bad = make(&tail, json!({"1":null,"2":null}));
            bad.cache.get_mut(&1).unwrap()[offset] ^= 1;
            assert!(bad.metadata().is_err());
        }
        let mut unused = tail;
        unused[3] = 2;
        assert!(make(&unused, json!({"1":null,"2":null,"3":null}))
            .metadata()
            .unwrap_err()
            .message
            .contains("unreferenced"));
        let mut trailing = tail.to_vec();
        trailing.push(0);
        assert!(make(&trailing, json!({"1":null,"2":null}))
            .metadata()
            .is_err());
        let duplicate = [19, 2, 0, 2, 0, 2, 4, 0, 0, 0, 0, 0, 0, 2, 4, 0, 0, 0, 0, 0];
        assert!(make(&duplicate, json!({"1":null,"2":null,"3":null}))
            .metadata()
            .is_err());
    }

    #[test]
    fn proto_checks_flags_records_entry_children_and_full_consumption() {
        let loader = json!({});
        let mut image = image(&loader);
        let mut parse = |data: Vec<u8>| -> Result<Proto> {
            let mut p = Proto {
                page: 2,
                size: data.len(),
                check: checksum(&data, 0),
                ..Proto::default()
            };
            image.cache.insert(2, data);
            image.proto(0, &mut p)?;
            Ok(p)
        };
        // stack, parameters, upvalues, flags, salt, rotation, entry/count, record, constants, children.
        let data = vec![220, 1, 0, 1, 0, 0, 0, 0, 0, 0, 1, 0, 4, 0, 0, 0, 0, 0, 1, 0];
        let p = parse(data.clone()).unwrap();
        assert_eq!(
            (p.stack, p.params, p.upvalues, p.entry, p.reg_shift),
            (220, 1, 0, 0, 0)
        );
        assert!(p.vararg);
        assert_eq!(p.kids, vec![0]);
        assert_eq!(p.records.len(), 1);
        assert_eq!(
            (
                p.records[&0].offset,
                p.records[&0].size,
                p.records[&0].logical
            ),
            (13, 4, 0)
        );
        assert!(p.constants.is_empty());
        for flags in [0, 2, 4, 8, 16] {
            let mut accepted = data.clone();
            accepted[3] = flags;
            let p = parse(accepted).unwrap();
            assert!(!p.vararg);
            assert_eq!(p.vf, u32::from((flags >> 1) & 3));
        }
        for (offset, value) in [(3, 6), (3, 24), (8, 128), (9, 1), (11, 1), (12, 3), (19, 1)] {
            let mut bad = data.clone();
            bad[offset] = value;
            assert!(parse(bad).is_err(), "offset {offset}");
        }
        let mut trailing = data.clone();
        trailing.push(0);
        assert!(parse(trailing).is_err());
        for end in 0..data.len() {
            assert!(parse(data[..end].to_vec()).is_err(), "truncated at {end}");
        }
    }

    #[test]
    fn static_record_decodes_families_orders_signed_fields_fused_parts_and_remaps() {
        let loader = json!({"perm":{"0":[10,9,8,7,6,5,4,3,2,1]},"junk_op":255});
        let mut image = image(&loader);
        let salt = (3443762138u32 ^ 1550291118) ^ 1673691136u32.rotate_left(17);
        let op_xor = ((salt ^ (salt >> 16)) & 255) as u8;
        let seed = shift(
            (1384073472u32 ^ 1711491034) ^ (3570431865 ^ 2879290169) ^ 38303 ^ 15243,
            [7, 4, 6],
        );
        for family in 0..3 {
            for (order, names) in [
                [0, 1, 2, 3, 4, 5, 6, 7, 8, 9],
                [8, 2, 0, 5, 1, 9, 4, 7, 3, 6],
                [7, 4, 1, 3, 0, 9, 6, 8, 2, 5],
                [5, 9, 0, 3, 8, 7, 1, 6, 4, 2],
            ]
            .into_iter()
            .enumerate()
            {
                let op = 7 ^ op_xor;
                let op = if family == 2 { op.rotate_left(1) } else { op };
                let mut plain = vec![63, 50]; // six fields, 32-bit operands, two fused parts.
                for name in names {
                    match name {
                        0 => plain.push(op),
                        1 => plain.push(3),
                        2 => plain.push(10),
                        3 => plain.push(20),
                        4 => plain.push(30),
                        5 => plain.extend((-3i32).to_le_bytes()),
                        6 => plain.extend((-1i32).to_le_bytes()),
                        7 => plain.extend(0x80000000u32.to_le_bytes()),
                        _ => {}
                    }
                }
                plain.extend([0, 9]); // no optional fields; EF is always present.
                let mut p = Proto {
                    vf: family,
                    order,
                    affine: (0, 1, 0, 1),
                    byte_keys: vec![(0, 0); 3],
                    field_keys: vec![(0, 0, 0); 3],
                    data: encrypt_family(&plain, seed, [13, 17, 5], family),
                    ..Proto::default()
                };
                p.records.insert(
                    0,
                    Section {
                        page: -1,
                        offset: 0,
                        size: plain.len(),
                        logical: 0,
                    },
                );
                let mut expected = json!({"sem":100,"T":[4294967295u32,4294967295u32,2147483648u32,-1,-3,30,20,10,3,7],"parts":[[4294967295u32,4294967295u32,2147483648u32,-1,-3,30,20,10,0,0],[4294967295u32,4294967295u32,0,0,0,0,0,0,9,0]]});
                assert_eq!(
                    image.record(0, &p, 0, &BTreeMap::new(), 0).unwrap(),
                    expected,
                    "family {family}, order {order}"
                );
                expected["T"][9] = json!(52);
                assert_eq!(
                    image
                        .record(0, &p, 0, &BTreeMap::from([(100, 55)]), 3)
                        .unwrap(),
                    expected
                );
                plain[1] = 49; // arity one is never valid.
                p.data = encrypt_family(&plain, seed, [13, 17, 5], family);
                assert!(image.record(0, &p, 0, &BTreeMap::new(), 0).is_err());
            }
        }
    }
    #[test]
    fn static_constant_reader_preserves_every_supported_tag_and_number() {
        let l = json!({});
        let mut image = image(&l);
        let mut vectors = vec![
            (vec![1, 2, 7, 9], json!(["i", [7, 9]])),
            (vec![2, 3, 0, 128, 255], json!(["s", [0, 128, 255]])),
            (vec![8, 1, 7, 253, 255, 255, 255], json!(["t", [[7, -3]]])),
            (vec![0, 0], json!(["b", false])),
            (vec![0, 1], json!(["b", true])),
            (vec![5, 120, 86, 52, 18], json!(["n", 0x12345678])),
            (vec![4, 172, 2], json!(["n", 300])),
        ];
        for n in [1.25, -0.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let mut bytes = vec![7];
            bytes.extend(n.to_le_bytes());
            vectors.push((bytes, json!(["n", crate::model::encode_number(n)])));
        }
        for (plain, want) in vectors {
            let seed = rol(1040674074, 25, 32) ^ rol(1692189232, 13, 32) ^ 44601 ^ 319;
            let data = encrypt(&plain, seed, [7, 9, 13]);
            let mut p = Proto {
                data,
                ..Proto::default()
            };
            p.constants.insert(
                0,
                Section {
                    page: -1,
                    offset: 0,
                    size: plain.len(),
                    logical: 0,
                },
            );
            let got = image.constant(0, &p, 0).unwrap();
            assert_eq!(got, want);
            if plain[0] == 7
                && f64::from_le_bytes(plain[1..].try_into().unwrap()).is_sign_negative()
            {
                assert!(crate::model::decode_number(&got[1])
                    .unwrap()
                    .is_sign_negative());
            }
            p.constants.get_mut(&0).unwrap().size -= 1;
            assert!(image.constant(0, &p, 0).is_err());
        }
        for plain in [vec![3], vec![1, 128], vec![0, 1, 0]] {
            let seed = rol(1040674074, 25, 32) ^ rol(1692189232, 13, 32) ^ 44601 ^ 319;
            let mut p = Proto {
                data: encrypt(&plain, seed, [7, 9, 13]),
                ..Proto::default()
            };
            p.constants.insert(
                0,
                Section {
                    page: -1,
                    offset: 0,
                    size: plain.len(),
                    logical: 0,
                },
            );
            assert!(image.constant(0, &p, 0).is_err());
        }
    }
    #[test]
    fn static_record_reader_maps_raw_fields_and_rejects_bad_sections() {
        let l = json!({"perm":{"0":[1,2,3,4,5,6,7,8,9,10]},"junk_op":255});
        let mut image = image(&l);
        let salt = (3443762138u32 ^ 1550291118) ^ rol(1673691136, 17, 32);
        let op_xor = ((salt ^ (salt >> 16)) & 255) as u8;
        let plain = [0, 0, op_xor, 0];
        let seed = shift(
            (1384073472u32 ^ 1711491034) ^ (3570431865 ^ 2879290169) ^ 38303 ^ 15243,
            [7, 4, 6],
        );
        let mut p = Proto {
            data: encrypt(&plain, seed, [13, 17, 5]),
            affine: (0, 1, 0, 1),
            ..Proto::default()
        };
        p.records.insert(
            7,
            Section {
                page: -1,
                offset: 0,
                size: 4,
                logical: 0,
            },
        );
        let record = image.record(0, &p, 7, &BTreeMap::new(), 0).unwrap();
        assert_eq!(
            record,
            json!({"sem":80,"T":[0,0,0,0,0,0,0,0,4294967295u32,4294967295u32],"parts":[]})
        );
        p.records.get_mut(&7).unwrap().offset = 1;
        assert!(image.record(0, &p, 7, &BTreeMap::new(), 0).is_err());
    }
}

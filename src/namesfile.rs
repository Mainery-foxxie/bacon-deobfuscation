use crate::error::{Error, Result};
use regex::Regex;
use std::collections::BTreeMap;

pub fn parse_names(data: &[u8]) -> Result<BTreeMap<String, String>> {
    let err = |s: &str| Error::new("bad_names", s);
    let text = std::str::from_utf8(data.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(data))
        .map_err(|_| err("the names file must be UTF-8"))?;
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|_| err("the names file is not valid JSON"))?;
    let obj = value
        .as_object()
        .ok_or_else(|| err("the names file must be a JSON object: {\"12:34\": \"name\"}"))?;
    if obj.len() > 20000 {
        return Err(err("the names file has too many entries"));
    }
    let key = Regex::new(r"^\d{1,6}:(p\d{1,3}|\d{1,6})$").unwrap();
    let name = Regex::new(r"^[A-Za-z_][A-Za-z0-9_]{0,39}$").unwrap();
    let mut out = BTreeMap::new();
    for (k, v) in obj {
        if !key.is_match(k) {
            return Err(err(&format!(
                "unknown key: {:?} (valid examples: \"12:34\" or \"12:p1\")",
                k.chars().take(30).collect::<String>()
            )));
        }
        let v = v.as_str().filter(|s| name.is_match(s)).ok_or_else(|| {
            err(&format!(
                "invalid name for {k} (letters/digits/_ only, must not start with a digit, max 40)"
            ))
        })?;
        out.insert(k.clone(), v.to_owned());
    }
    Ok(out)
}

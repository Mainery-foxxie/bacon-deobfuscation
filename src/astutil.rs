//! Byte-preserving boundaries around the official Luau AST tool.
use crate::error::{Error, Result};
use serde::Deserialize;
use serde_json::Value;
use std::{env, io::Write, path::PathBuf, process::Command, time::Duration};

pub fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| char::from(b)).collect()
}
pub fn unlatin1(text: &str) -> Result<Vec<u8>> {
    text.chars()
        .map(|c| {
            u8::try_from(c as u32)
                .map_err(|_| Error::new("source", "character outside Latin1 byte range"))
        })
        .collect()
}
pub(crate) fn is_blank_source(data: &[u8]) -> bool {
    data.iter().all(|byte| matches!(byte, b'\t'..=b'\r' | b' '))
}
pub fn normalize_source(mut data: &[u8]) -> Vec<u8> {
    while data.starts_with(b"\xef\xbb\xbf") {
        data = &data[3..];
    }
    let mut out = Vec::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        if data[i] == b'\r' {
            out.push(b'\n');
            i += 1;
            if data.get(i) == Some(&b'\n') {
                i += 1;
            }
        } else {
            out.push(data[i]);
            i += 1;
        }
    }
    out
}
pub fn find_bin(name: &str) -> Result<PathBuf> {
    find_bin_at(
        name,
        env::var_os("BGDEOBF_LUAU_DIR").map(PathBuf::from),
        env::current_exe().ok(),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")),
        env::var_os("PATH")
            .map(|paths| env::split_paths(&paths).collect())
            .unwrap_or_default(),
    )
}
fn find_bin_at(
    name: &str,
    override_dir: Option<PathBuf>,
    executable: Option<PathBuf>,
    root: PathBuf,
    paths: Vec<PathBuf>,
) -> Result<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(p) = override_dir {
        dirs.push(p);
    }
    if let Some(dir) = executable.as_deref().and_then(std::path::Path::parent) {
        dirs.push(dir.join("lua_bin"));
        dirs.push(dir.join("luau_bin"));
    }
    dirs.push(root.join("lua_bin"));
    dirs.push(root.join("luau_bin"));
    dirs.extend(paths);
    for dir in dirs {
        for ext in if cfg!(windows) {
            vec![".exe", ""]
        } else {
            vec![""]
        } {
            let path = dir.join(format!("{name}{ext}"));
            if !path.is_file() {
                continue;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if path.metadata()?.permissions().mode() & 0o111 == 0 {
                    continue;
                }
            }
            return Ok(path.canonicalize()?);
        }
    }
    Err(Error::new(
        "setup",
        format!("Luau binary not found: {name} (set BGDEOBF_LUAU_DIR)"),
    ))
}
fn tool(name: &str, data: &[u8], timeout: Duration) -> Result<(PathBuf, crate::process::Capture)> {
    let path = find_bin(name)?;
    let mut file = tempfile::Builder::new().suffix(".lua").tempfile()?;
    file.write_all(data)?;
    file.flush()?;
    let mut cmd = Command::new(&path);
    cmd.arg(file.path());
    let capture = crate::process::capture(
        cmd,
        &crate::process::Limits {
            timeout,
            memory_mb: 1024,
            cpu_seconds: timeout.as_secs().max(1),
            max_output_bytes: 256 * 1024 * 1024,
            new_group: true,
        },
    )?;
    Ok((path, capture))
}
pub fn parse_ast(text: &[u8]) -> Result<Value> {
    let (_, capture) = tool("luau-ast", text, Duration::from_secs(120))?;
    if capture.status == "timeout" {
        return Err(Error::new("timeout", "luau-ast took longer than 120 s"));
    }
    if capture.status != "ok" || capture.code != Some(0) {
        let msg = latin1(&capture.err);
        let msg = msg.split_whitespace().collect::<Vec<_>>().join(" ");
        let short: String = msg.chars().take(300).collect();
        return Err(Error::new(
            if msg.contains("Parse error") {
                "syntax"
            } else {
                "unsupported"
            },
            format!("luau-ast failed: {short}"),
        ));
    }
    let text = latin1(&capture.out);
    let mut reader = serde_json::Deserializer::from_str(&text);
    // Valid VM handler ladders routinely exceed JSON's generic 128-container limit.
    reader.disable_recursion_limit();
    let mut value = Value::deserialize(&mut reader).map_err(|e| {
        Error::new(
            "unsupported",
            format!("luau-ast output could not be read: {e}"),
        )
    })?;
    reader
        .end()
        .map_err(|e| Error::new("unsupported", format!("luau-ast trailing output: {e}")))?;
    if value["root"]["type"] != "AstStatBlock" {
        return Err(Error::new(
            "unsupported",
            "luau-ast output has no root block",
        ));
    }
    Ok(value["root"].take())
}
pub fn check_tools() -> Vec<(String, bool, String)> {
    ["luau", "luau-ast", "luau-compile"]
        .iter()
        .map(|name| {
            let result = tool(name, b"print(\"ok\")\n", Duration::from_secs(30));
            let (ok, msg) = match result {
                Ok((p, c)) if c.status == "ok" && c.code == Some(0) => {
                    (true, p.to_string_lossy().into_owned())
                }
                Ok((_, c)) => (
                    false,
                    if c.status == "timeout" {
                        "not responding".into()
                    } else {
                        format!("failed to run ({}; exit {:?})", c.status, c.code)
                    },
                ),
                Err(e) => (false, e.to_string()),
            };
            (name.to_string(), ok, msg)
        })
        .collect()
}
pub fn parse_loc(loc: &str) -> Result<(usize, usize, usize, usize)> {
    let fail = || Error::new("source", format!("invalid source location: {loc}"));
    let (a, b) = loc.split_once(" - ").ok_or_else(fail)?;
    let (l1, c1) = a.split_once(',').ok_or_else(fail)?;
    let (l2, c2) = b.split_once(',').ok_or_else(fail)?;
    Ok((
        l1.parse().map_err(|_| fail())?,
        c1.parse().map_err(|_| fail())?,
        l2.parse().map_err(|_| fail())?,
        c2.parse().map_err(|_| fail())?,
    ))
}
pub struct Src {
    pub bytes: Vec<u8>,
    pub starts: Vec<usize>,
}
impl Src {
    pub fn new(text: &str) -> Result<Self> {
        let bytes = unlatin1(text)?;
        let mut starts = vec![0];
        for (i, b) in bytes.iter().enumerate() {
            if *b == b'\n' {
                starts.push(i + 1);
            }
        }
        Ok(Self { bytes, starts })
    }
    pub fn off(&self, line: usize, col: usize) -> Result<usize> {
        let start = *self
            .starts
            .get(line)
            .ok_or_else(|| Error::new("source", "source line outside input"))?;
        let end = self
            .starts
            .get(line + 1)
            .map(|n| n - 1)
            .unwrap_or(self.bytes.len());
        let at = start
            .checked_add(col)
            .filter(|at| *at <= end)
            .ok_or_else(|| Error::new("source", "source column outside line"))?;
        Ok(at)
    }
    pub fn span_loc(&self, loc: &str) -> Result<(usize, usize)> {
        let (l, c, l2, c2) = parse_loc(loc)?;
        let a = self.off(l, c)?;
        let b = self.off(l2, c2)?;
        if a > b {
            return Err(Error::new("source", "reversed source span"));
        }
        Ok((a, b))
    }
    pub fn span(&self, node: &Value) -> Result<(usize, usize)> {
        self.span_loc(
            node["location"]
                .as_str()
                .ok_or_else(|| Error::new("source", "node has no source location"))?,
        )
    }
    pub fn slice(&self, node: &Value) -> Result<String> {
        let (a, b) = self.span(node)?;
        Ok(latin1(&self.bytes[a..b]))
    }
}
pub fn kids(node: &Value) -> Vec<&Value> {
    let mut out = vec![];
    if let Some(obj) = node.as_object() {
        for v in obj.values() {
            if v.is_object() && v.get("type").is_some() {
                out.push(v);
            } else if let Some(items) = v.as_array() {
                for item in items {
                    if let Some(obj) = item.as_object() {
                        if obj.contains_key("type") {
                            out.push(item);
                        } else {
                            for value in obj.values() {
                                if value.is_object() && value.get("type").is_some() {
                                    out.push(value);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    out
}
pub fn walk(node: &Value) -> Vec<(&Value, Option<&Value>)> {
    let mut out = vec![];
    let mut pending = vec![(node, None)];
    while let Some((n, p)) = pending.pop() {
        out.push((n, p));
        for child in kids(n).into_iter().rev() {
            pending.push((child, Some(n)));
        }
    }
    out
}
pub fn lkey(local: &Value) -> &str {
    local["location"].as_str().unwrap_or("")
}
pub fn unparen(mut node: &Value) -> &Value {
    while node["type"] == "AstExprGroup" {
        node = &node["expr"];
    }
    node
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relocated_tools_work_without_checkout_or_path_and_override_wins() {
        let temp = tempfile::tempdir().unwrap();
        let distribution = temp.path().join("relocated distribution");
        let tools = distribution.join("lua_bin");
        let override_dir = temp.path().join("override");
        let checkout = temp.path().join("absent checkout");
        std::fs::create_dir_all(&tools).unwrap();
        std::fs::create_dir(&override_dir).unwrap();
        let name = if cfg!(windows) { "luau.exe" } else { "luau" };
        for dir in [&tools, &override_dir] {
            let path = dir.join(name);
            std::fs::write(&path, b"fixture").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
        }
        let executable = Some(distribution.join("bgdeobf.exe"));
        assert_eq!(
            find_bin_at("luau", None, executable.clone(), checkout.clone(), vec![]).unwrap(),
            tools.join(name).canonicalize().unwrap(),
        );
        assert_eq!(
            find_bin_at(
                "luau",
                Some(override_dir.clone()),
                executable,
                checkout.clone(),
                vec![]
            )
            .unwrap(),
            override_dir.join(name).canonicalize().unwrap(),
        );
        assert_eq!(
            find_bin_at("luau", None, None, checkout.clone(), vec![tools.clone()]).unwrap(),
            tools.join(name).canonicalize().unwrap(),
        );
        let error = find_bin_at("missing", None, None, checkout, vec![]).unwrap_err();
        assert_eq!(error.code, "setup");
        assert!(error.message.contains("BGDEOBF_LUAU_DIR"));
    }
}

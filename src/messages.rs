use regex::Regex;
use serde_json::Value;

fn n(v: &Value, k: &str) -> u64 {
    v[k].as_u64().unwrap_or(0)
}
fn s<'a>(v: &'a Value, k: &str, default: &'a str) -> &'a str {
    v[k].as_str().unwrap_or(default)
}
fn yes(v: &Value, k: &str) -> bool {
    v[k].as_bool() == Some(true)
}
fn list<'a>(v: &'a Value, k: &str) -> &'a [Value] {
    v[k].as_array().map(Vec::as_slice).unwrap_or(&[])
}

pub fn scrub(text: &str) -> String {
    let text =
        Regex::new(r"(?:\bscript\.lua|(?:[A-Za-z]:)?[\\/][^\n:]*?\.lua)\((\d+),\s*(\d+)\):\s*")
            .unwrap()
            .replace_all(text, "line $1:$2: ");
    let text = Regex::new(r"(?:\bscript\.lua|(?:[A-Za-z]:)?[\\/][^\n:]*?\.lua):(\d+):\s*")
        .unwrap()
        .replace_all(&text, "line $1: ")
        .into_owned();
    Regex::new(r#"(?:[A-Za-z]:)?[\\/](?:[^\s\\/:()'"<>|*?]+[\\/])+[^\s\\/:()'"<>|*?]*"#)
        .unwrap()
        .replace_all(&text, "<path>")
        .into_owned()
}

pub fn explain(code: &str, detail: &str) -> String {
    let base=match code {
        "empty"=>"The file is empty.","too_big"=>"The file is too large.","binary"=>"This file isn't text (it looks like binary / bytecode).",
        "syntax"=>"This file isn't valid Luau (it may be truncated or copied incorrectly). Send the original file.",
        "setup"=>"The Luau tools were not found on this machine (check the lua_bin folder or BGDEOBF_LUAU_DIR).",
        "unsupported"=>"This script isn't a Bacon Guard / FORGE or MoonVeil build I recognize, or it uses parts I don't understand yet, so I won't guess.",
        "timeout"=>"The job went over the time limit and was stopped.","sandbox"=>"The loader couldn't be opened in the sandbox (the script is damaged, truncated, or an unsupported version).",
        "static_only"=>"This file supports static decompilation only. Run the command without the sandbox option.",
        "image"=>"The static image failed validation. No script was run and no source was returned.","decode"=>"The VM contents were read, but some instructions could not be translated.",
        "decompile"=>"The VM contents were read, but the decompiler failed to rebuild the code.","memory"=>"Ran out of memory while processing this file.","cancelled"=>"The job was cancelled.",_=>"An internal error occurred."
    };
    if !detail.is_empty()
        && matches!(
            code,
            "syntax" | "unsupported" | "sandbox" | "image" | "decode" | "decompile" | "setup"
        )
    {
        format!("{base}\nTechnical detail: {}", scrub(detail))
    } else {
        base.into()
    }
}

pub fn host_list(domains: &[Value], markdown: bool) -> String {
    let re = Regex::new(r"^[a-z0-9][a-z0-9.-]{0,120}$").unwrap();
    let plain: Vec<_> = domains
        .iter()
        .filter_map(Value::as_str)
        .filter(|s| re.is_match(s))
        .collect();
    let odd = domains.len() - plain.len();
    let mut text = plain
        .iter()
        .take(4)
        .map(|s| {
            if markdown {
                format!("`{s}`")
            } else {
                (*s).to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    if plain.len() > 4 {
        text.push_str(" ...");
    }
    if odd > 0 {
        if !text.is_empty() {
            text.push_str(" and ");
        }
        text.push_str(&format!("{odd} host name(s) with unusual characters"));
    }
    if text.is_empty() {
        "unknown hosts".into()
    } else {
        text
    }
}

pub fn trace_lines(t: &Value, markdown: bool) -> Vec<String> {
    let mut parts = vec![format!("{} events", n(t, "events"))];
    if n(t, "requests") > 0 {
        parts.push(format!(
            "{} request(s) to {}",
            n(t, "requests"),
            host_list(list(t, "domains"), markdown)
        ));
    }
    for (key, label) in [
        ("remotes", "remote call(s)"),
        (
            "payloads",
            "text(s) captured (loadstring / files / requests)",
        ),
        ("instances", "instances created"),
        ("errors", "error(s)"),
    ] {
        if n(t, key) > 0 {
            parts.push(format!("{} {label}", n(t, key)));
        }
    }
    let mut out = vec![format!("Fake Roblox run: {}", parts.join(", "))];
    let mut findings = Vec::new();
    for f in list(t, "findings").iter().filter_map(Value::as_str) {
        if !findings.contains(&f) {
            findings.push(f);
        }
    }
    if !findings.is_empty() {
        out.push(format!(
            "Worth a look: {}",
            findings.into_iter().take(6).collect::<Vec<_>>().join("; ")
        ));
    }
    out.push(format!(
        "Run ended: {}{}",
        s(t, "reason", "unknown"),
        if yes(t, "finished") {
            ""
        } else {
            " (the log may be incomplete)"
        }
    ));
    out
}

pub fn verification_status(info: &Value) -> Option<String> {
    if yes(info, "sandbox_only") || yes(info, "trace_only") || info["kind"] == "moonveil" {
        return None;
    }
    let v = &info["verification"];
    if v["delivered_syntax"] == "failed" {
        return Some("FAILED".into());
    }
    let f = &info["fidelity"];
    let functions = n(f, "functions");
    let accepted = n(f, "identical")
        .saturating_add(n(f, "reordered"))
        .saturating_add(n(f, "cosmetic"));
    let checked_delivery = !yes(info, "annotated")
        && yes(v, "raw_applies_to_delivery")
        && ["raw_syntax", "delivered_syntax"]
            .iter()
            .all(|k| v[*k] == "passed")
        && functions > 0
        && functions == n(f, "functions_recompiled")
        && f["differing"].as_array().is_some_and(Vec::is_empty);
    let proofs = n(f, "semantic_equivalent");
    Some(
        if checked_delivery && v["raw_bytecode"]=="passed" && proofs==0 && functions==accepted {
            "BYTECODE CHECK PASSED"
        } else if checked_delivery && v["raw_bytecode"]=="differences found" && v["raw_semantic"]=="passed" && proofs>0 && accepted.checked_add(proofs)==Some(functions){
            "RAW FIDELITY PASSED WITH PROOFS"
        } else {
            "PARTIAL"
        }
        .into(),
    )
}

pub fn verification_report(info: &Value) -> String {
    let Some(status) = verification_status(info) else {
        return String::new();
    };
    let v = &info["verification"];
    let f = &info["fidelity"];
    let mut out = vec![format!("Status: {status}")];
    if info["unpacking"] == "static" {
        out.extend([
            format!(
                "Normalized source SHA-256: {}",
                s(info, "source_sha256", "unknown")
            ),
            "Unpacking: bounded static image decoding; loader and application not run.".into(),
        ]);
    }
    out.push(format!(
        "{}: {}",
        if yes(info, "annotated") {
            "Raw Lua syntax (unannotated copy)"
        } else {
            "Raw Lua syntax"
        },
        s(v, "raw_syntax", "not run")
    ));
    out.push(format!(
        "Delivered Lua syntax: {}",
        s(v, "delivered_syntax", "not run")
    ));
    out.push(format!(
        "Raw bytecode comparison: {}",
        s(v, "raw_bytecode", "not run")
    ));
    if f.get("functions_recompiled").is_some() {
        out.push(format!(
            "Functions: {} decoded, {} recompiled; {} accepted, {} differ",
            n(f, "functions"),
            n(f, "functions_recompiled"),
            n(f, "identical")
                .saturating_add(n(f, "reordered"))
                .saturating_add(n(f, "cosmetic"))
                .saturating_add(n(f, "semantic_equivalent")),
            list(f, "differing").len()
        ));
        out.push(format!(
            "Accepted categories: {} identical, {} reordered, {} cosmetic{}",
            n(f, "identical"),
            n(f, "reordered"),
            n(f, "cosmetic"),
            proof_suffix(f)
        ));
    } else {
        out.push("Function comparison coverage: unavailable".into());
    }
    out.push(if yes(info,"annotated") {"Scope: checks apply to the unannotated copy; the delivered annotated file is not runnable."} else if v["raw_applies_to_delivery"]==false {"Scope: the raw bytecode comparison does not cover the cleaned file."} else if yes(v,"raw_applies_to_delivery") {"Scope: raw checks apply to the delivered Lua, where available."} else {"Scope: unknown; delivery coverage was not recorded."}.into());
    let trace = s(v, "fake_trace", "not run");
    out.push(format!(
        "Fake Roblox trace: {}",
        if trace == "observed" {
            "observed (simulated only; not a behavior comparison)"
        } else {
            trace
        }
    ));
    out.extend([
        "Real Roblox / executor: not tested".into(),
        "These checks are not a guarantee of semantic equivalence.".into(),
        "Names inferred from the code are generated, not recovered original local names.".into(),
    ]);
    out.join("\n") + "\n"
}

pub fn verdict(info: &Value) -> Vec<String> {
    verdict_markdown(info, false)
}
fn proof_suffix(f: &Value) -> String {
    let count = n(f, "semantic_equivalent");
    if count == 0 {
        String::new()
    } else {
        format!(", {count} proven equivalent")
    }
}
pub fn verdict_markdown(info: &Value, markdown: bool) -> Vec<String> {
    let mut out = Vec::new();
    let b = &info["banner"];
    if yes(info, "sandbox_only") {
        out.push("Ran the script as given in the fake Roblox (nothing was deobfuscated)".into());
        if b.is_object() {
            out.push(format!("It carries a Bacon Guard v{} banner: the log shows the protected script itself, so most of it is the VM. Deobfuscate it first (or use the trace option together with deobfuscation) to read what it does.",s(b,"version","?")));
        }
    } else if yes(info, "trace_only") || info["kind"] == "moonveil" {
        let mv = &info["moonveil"];
        let version = s(mv, "version", "");
        out.push(if yes(mv,"assumed"){"No MoonVeil banner found; the script was handled as a MoonVeil one because that was asked for".into()}else{format!("Detected: MoonVeil{} (private VM protector)",if !version.is_empty() && version!="?" {format!(" {version}")}else{String::new()})});
        out.push("This is a behaviour reconstruction, NOT the original source: MoonVeil can't be reversed to source. The script was run in a sandbox with fake Roblox objects and what it did was written down".into());
        out.push(format!(
            "{} calls seen in {} threads/callbacks, {} output lines",
            n(info, "calls"),
            n(info, "threads"),
            n(info, "lines")
        ));
        let downloads = list(info, "downloads");
        if !downloads.is_empty() {
            out.push(format!(
                "It downloads and runs {} other script{} (not fetched, so not included): {}",
                downloads.len(),
                if downloads.len() == 1 { "" } else { "s" },
                downloads
                    .iter()
                    .filter_map(Value::as_str)
                    .take(3)
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if yes(info, "syntax_ok") {
            out.push("Output syntax is valid".into());
        }
    } else {
        out.push(if b.is_object() {
            format!(
                "Detected: Bacon Guard v{} ({}), build {}",
                s(b, "version", "?"),
                s(b, "protection", "?"),
                s(b, "build", "?")
            )
        } else {
            "No Bacon Guard banner, but the VM structure was recognized".into()
        });
        out.push(format!(
            "{} functions, {} VM instructions, {} output lines",
            n(info, "protos"),
            n(info, "records"),
            n(info, "lines")
        ));
        let c = &info["cleaned"];
        if c.is_object() && !c.as_object().unwrap().is_empty() {
            let mut what = Vec::new();
            if n(c, "strings") > 0 {
                what.push(format!("{} encrypted strings decrypted", n(c, "strings")));
            }
            if n(c, "numbers") > 0 {
                what.push(format!("{} number puzzles solved", n(c, "numbers")));
            }
            if n(c, "checks") > 0 {
                what.push(format!("{} always-true / always-false checks removed ({} branches that could never run)",n(c,"checks"),n(c,"arms")));
            }
            out.push(format!(
                "Cleanup of the protector's source-level junk: {}; the raw decompile had {} lines",
                if what.is_empty() {
                    "nothing to fold".into()
                } else {
                    what.join(", ")
                },
                n(info, "raw_lines")
            ));
        }
        out.push(format!(
            "Verification: {}",
            verification_status(info).unwrap_or_else(|| "PARTIAL".into())
        ));
        let f = &info["fidelity"];
        if f.is_object() {
            let mut line = format!(
                "Bytecode check{}: {}/{} functions identical",
                if c.is_object() {
                    " of the raw decompile"
                } else {
                    ""
                },
                n(f, "identical"),
                n(f, "functions")
            );
            if n(f, "reordered") > 0 {
                line.push_str(&format!(", {} with the same instructions in a different order (only plain reads and calculations moved)",n(f,"reordered")));
            }
            if n(f, "cosmetic") > 0 {
                line.push_str(&format!(", {} with a cosmetic difference (an extra `return`, or a number loaded at another place)",n(f,"cosmetic")));
            }
            line.push_str(&proof_suffix(f));
            if !list(f, "differing").is_empty() {
                line.push_str(&format!(
                    ", {} differ (check manually)",
                    list(f, "differing").len()
                ));
            }
            out.push(line);
        } else if yes(info, "syntax_ok") {
            out.push("Output syntax is valid (bytecode check unavailable)".into());
        }
        if yes(info, "annotated") {
            out.push(format!("Annotated copy: the `<@…>` keys are for building a names file, so this version is not runnable as is{}",if f.is_object()||yes(info,"syntax_ok"){" (the checks ran on a clean copy)"}else{""}));
        }
    }
    if info["trace"].is_object() {
        out.extend(trace_lines(&info["trace"], markdown));
    }
    for warning in list(info, "warnings").iter().filter_map(Value::as_str) {
        out.push(format!("Note: {}", scrub(warning)));
    }
    out
}

pub fn explain_fetch(code: &str, detail: &str, max_mb: f64) -> String {
    match code {
        "html"=>"That URL points to a web page, not a raw file. Use the **Raw** link (e.g. raw.githubusercontent.com or pastebin.com/raw/...).".into(),
        "too_big"=>format!("The file at that URL is too large (max {max_mb} MB)."),"empty"=>"The file at that URL is empty.".into(),"timeout"=>"The server at that URL took too long to respond.".into(),
        "loader"=>format!("That URL gives a loader that only runs inside Roblox (the paste host's \"Secure Runtime Envelope\"), not the script itself. I asked for the script link written in it, but {}. That link is signed for one IP address and expires quickly, and the host may give it only to a real Roblox client. Try the original raw URL again, or attach the script file to the command (the README shows how to dump it yourself from an executor).",if detail.is_empty(){"it did not give the script"}else{detail.trim_end_matches('.')}),
        _=>{let head=match code{"bad_url"=>"That URL can't be used","blocked"=>"That URL was rejected","net"=>"Couldn't fetch that URL",_=>"Failed to fetch the URL"};if detail.is_empty(){format!("{head}.")}else{format!("{head}: {}.",detail.trim_end_matches('.'))}}
    }
}

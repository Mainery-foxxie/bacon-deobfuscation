use crate::error::{Error, Result};
use crate::model::{Options, Output, Progress};
use crate::{
    astutil, classify, clean, decode, dump, fidelity, loader, messages, moonveil, patch, process,
    render, source_cleanup, static_image, tracer,
};
use regex::Regex;
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

fn banner(text: &str) -> Option<Value> {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    let pattern = PATTERN.get_or_init(|| {
        Regex::new(r"Bacon Guard v([0-9.]+)\s*\|\s*([^|\n]*?)\s*\|\s*Build\s+([0-9a-fA-F]+)")
            .expect("constant banner pattern")
    });
    let prefix: String = text.chars().take(4000).collect();
    let captures = pattern.captures(&prefix)?;
    Some(json!({"version":captures[1],"protection":captures[2],"build":captures[3]}))
}

fn validate(data: &[u8], max_bytes: usize) -> Result<()> {
    if astutil::is_blank_source(data) {
        return Err(Error::new("empty", "the file is empty"));
    }
    if data.len() > max_bytes {
        return Err(Error::new(
            "too_big",
            format!("the file is larger than {max_bytes} bytes"),
        ));
    }
    if data.iter().take(4096).any(|byte| *byte == 0) {
        return Err(Error::new(
            "binary",
            "the file does not look like a text script",
        ));
    }
    Ok(())
}

fn warning(info: &mut Value, message: impl Into<String>) {
    if let Some(warnings) = info.get_mut("warnings").and_then(Value::as_array_mut) {
        warnings.push(Value::String(message.into()));
    }
}

fn seconds(start: Instant) -> Value {
    json!((start.elapsed().as_secs_f64() * 100.0).round() / 100.0)
}

fn stage(progress: Progress<'_>, name: &str) -> Instant {
    if let Some(note) = progress {
        note(name);
    }
    Instant::now()
}

fn layout_permutations(layout: &Value) -> Result<Vec<String>> {
    let permutations = layout["perm"]
        .as_object()
        .filter(|perms| !perms.is_empty())
        .ok_or_else(|| Error::new("unsupported", "no record layout found"))?;
    let mut remarks = Vec::new();
    for (id, permutation) in permutations {
        let fields = permutation
            .as_array()
            .ok_or_else(|| Error::new("unsupported", format!("record layout {id} is malformed")))?;
        let mut sorted: Vec<_> = fields.iter().filter_map(Value::as_i64).collect();
        sorted.sort_unstable();
        if fields.len() != 10 || sorted != (1..=10).collect::<Vec<_>>() {
            return Err(Error::new(
                "unsupported",
                format!("record layout {id} is not a permutation of 10 fields"),
            ));
        }
        let fused = layout["fused_perm"][id].as_array();
        let simple = layout["simple_perm"][id].as_array();
        let (Some(fused), Some(simple)) = (fused, simple) else {
            return Err(Error::new(
                "unsupported",
                format!("record layout {id} has no fused/simple reader"),
            ));
        };
        for (list, size) in [(fused, 6), (simple, 7)] {
            let values: BTreeSet<_> = list.iter().filter_map(Value::as_i64).collect();
            if list.len() != size
                || values.len() != size
                || values.iter().any(|value| !(1..=10).contains(value))
            {
                return Err(Error::new(
                    "unsupported",
                    format!("record layout {id} has a malformed fused/simple reader"),
                ));
            }
        }
        let expected_fused: Vec<_> = [2, 3, 4, 5, 6, 7]
            .iter()
            .map(|index| fields[*index].clone())
            .collect();
        let expected_simple: Vec<_> = [0, 2, 3, 4, 5, 6, 7]
            .iter()
            .map(|index| fields[*index].clone())
            .collect();
        if fused != &expected_fused || simple != &expected_simple {
            remarks.push(format!("record layout {id} differs from the known pattern"));
        }
    }
    Ok(remarks)
}

fn clean_tool_command(name: &str) -> Result<Command> {
    let mut command = Command::new(astutil::find_bin(name)?);
    command.env_clear().stdin(Stdio::null());
    for key in ["PATH", "SYSTEMROOT", "LANG", "LC_ALL"] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    Ok(command)
}

pub fn compile_check(text: &str) -> Result<Value> {
    let mut source = tempfile::Builder::new()
        .prefix("bgdeobf_compile_")
        .suffix(".lua")
        .tempfile()?;
    source.write_all(text.as_bytes())?;
    source.flush()?;
    let mut command = clean_tool_command("luau-compile")?;
    command
        .args(["--text", "-O1", "-g0", "--dump-constants"])
        .arg(source.path());
    let captured = process::capture(
        command,
        &process::Limits {
            timeout: Duration::from_secs(60),
            memory_mb: 1024,
            cpu_seconds: 62,
            max_output_bytes: 128 * 1024 * 1024,
            new_group: true,
        },
    )?;
    if captured.status == "ok" && captured.code == Some(0) {
        return Ok(json!({"ok":true,"message":"","bytecode":astutil::latin1(&captured.out)}));
    }
    let stderr = astutil::latin1(&captured.err);
    if stderr.contains("Exceeded allowed recursion depth") {
        return Ok(
            json!({"ok":null,"message":"compile check unavailable: compiler recursion-depth limit","bytecode":null}),
        );
    }
    static SYNTAX: OnceLock<Regex> = OnceLock::new();
    let syntax = SYNTAX.get_or_init(|| {
        Regex::new(r"\(\d+,\s*\d+\):\s*SyntaxError:").expect("constant syntax pattern")
    });
    if captured.code == Some(1) && syntax.is_match(&stderr) {
        let scrubbed = messages::scrub(
            &stderr.replace(&source.path().to_string_lossy().to_string(), "<script>"),
        );
        let errors: Vec<_> = scrubbed
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect();
        let mut message = errors
            .iter()
            .take(3)
            .map(|line| line.chars().take(200).collect::<String>())
            .collect::<Vec<_>>()
            .join("; ");
        if errors.len() > 3 {
            message.push_str(&format!(" (+{} more)", errors.len() - 3));
        }
        if message.is_empty() {
            message = "unknown error".into();
        }
        return Ok(json!({"ok":false,"message":message,"bytecode":null}));
    }
    Ok(
        json!({"ok":null,"message":format!("compile check unavailable: compiler {} (exit {:?})",captured.status,captured.code),"bytecode":null}),
    )
}

pub fn detect(data: &[u8], max_bytes: usize) -> Result<Value> {
    fn answer(
        kind: &str,
        confidence: f64,
        label: String,
        banner: Option<Value>,
        detail: String,
        supported: bool,
    ) -> Value {
        json!({"kind":kind,"confidence":confidence,"label":label,"banner":banner,"detail":messages::scrub(&detail),"supported":supported})
    }
    if let Err(error) = validate(data, max_bytes) {
        let label = match error.code.as_str() {
            "empty" => "empty file",
            "too_big" => "file too large",
            _ => "not a text script",
        };
        return Ok(answer(
            "unknown",
            0.0,
            label.into(),
            None,
            if error.code == "binary" {
                "the file looks like binary / bytecode".into()
            } else {
                error.message
            },
            false,
        ));
    }
    let normalized = astutil::normalize_source(data);
    let text = astutil::latin1(&normalized);
    if let Some(found) = moonveil::detect(&text) {
        return Ok(answer("moonveil", 1.0,
            format!("MoonVeil {} (behaviour trace only)", found["version"].as_str().unwrap_or("?")), None,
            "MoonVeil cannot be turned back into source; a behaviour reconstruction is written instead".into(), true));
    }
    let found = banner(&text);
    let label = found.as_ref().map(|value| {
        format!(
            "Bacon Guard v{} ({}), build {}",
            value["version"].as_str().unwrap_or("?"),
            value["protection"].as_str().unwrap_or("?"),
            value["build"].as_str().unwrap_or("?")
        )
    });
    let result = loader::analyze(&normalized).and_then(|layout| layout_permutations(&layout));
    match result {
        Ok(remarks) => {
            let mut details = vec!["the VM layout of the loader was recognized".to_string()];
            details.extend(remarks);
            Ok(answer(
                "bacon_guard",
                1.0,
                label.unwrap_or_else(|| "Bacon Guard-style VM (no banner)".into()),
                found,
                details.join("; "),
                true,
            ))
        }
        Err(error) if error.code == "setup" || error.code == "timeout" => Err(error),
        Err(error) => {
            let detail = if error.code == "syntax" {
                format!("not valid Luau (damaged or truncated?): {}", error.message)
            } else {
                format!("the loader layout is not recognized: {}", error.message)
            };
            if found.is_some() {
                Ok(answer(
                    "bacon_guard",
                    0.5,
                    label.unwrap_or_default(),
                    found,
                    format!("the banner is there, but {detail}"),
                    false,
                ))
            } else {
                Ok(answer(
                    "unknown",
                    0.0,
                    "not a Bacon Guard / FORGE script".into(),
                    None,
                    detail,
                    false,
                ))
            }
        }
    }
}

fn trace_options(options: &Options, lines_of: &str) -> Value {
    let mut value = options
        .trace_opts
        .clone()
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    let maximum = value["max_bytes"]
        .as_u64()
        .unwrap_or(0)
        .max(options.max_bytes as u64);
    value["max_bytes"] = json!(maximum);
    value["lines_of"] = json!(lines_of);
    value
}

fn sandbox(data: &[u8], options: &Options, progress: Progress<'_>) -> Result<Output> {
    let start = Instant::now();
    let normalized = astutil::normalize_source(data);
    if static_image::matches_source(&normalized) {
        return Err(Error::new(
            "static_only",
            "this image supports static decompilation only",
        ));
    }
    let mut info = json!({"warnings":[],"sandbox_only":true});
    if let Some(value) = banner(&astutil::latin1(&normalized)) {
        info["banner"] = value;
    }
    stage(progress, "sandbox");
    let trace = tracer::run(data, &trace_options(options, "the script as given"))?;
    info["trace"] = trace["summary"].clone();
    info["timings"] = json!({"total":seconds(start)});
    Ok(Output {
        text: None,
        info,
        trace: Some(trace),
        raw: None,
    })
}

fn coverage(image: &Value, layout: &Value, mapping: &Value) -> Result<()> {
    let prototypes = image["protos"]
        .as_object()
        .ok_or_else(|| Error::new("decode", "unpacked image has no prototypes"))?;
    let mut missing = BTreeSet::new();
    for (id, prototype) in prototypes {
        let pid: i64 = id
            .parse()
            .map_err(|_| Error::new("decode", "invalid prototype id"))?;
        let layout_key = prototype["df"]
            .as_i64()
            .ok_or_else(|| Error::new("decode", "function has no record layout"))?
            .to_string();
        let permutation = layout["perm"][&layout_key].as_array().ok_or_else(|| {
            Error::new(
                "unsupported",
                format!(
                    "a function uses record layout {layout_key} that the loader does not define"
                ),
            )
        })?;
        let records = prototype["recs"]
            .as_object()
            .ok_or_else(|| Error::new("decode", "function has no records"))?;
        for record in records.values() {
            if record["ded"].as_str().is_some_and(|value| value != "-") || record["ded"].is_number()
            {
                let handler = record["ded"]
                    .as_i64()
                    .or_else(|| record["ded"].as_str().and_then(|value| value.parse().ok()))
                    .ok_or_else(|| Error::new("decode", "invalid dedicated-handler id"))?;
                let semantic = record["sem"]
                    .as_i64()
                    .ok_or_else(|| Error::new("decode", "record has no semantic id"))?;
                let key = if handler == 6 {
                    semantic
                        .checked_add(
                            pid.checked_mul(256)
                                .ok_or_else(|| Error::new("decode", "prototype id overflow"))?,
                        )
                        .ok_or_else(|| Error::new("decode", "handler id overflow"))?
                } else {
                    semantic
                };
                if mapping["ded"].get(format!("{handler}:{key}")).is_none() {
                    missing.insert(format!("dedicated handler {handler}/{key}"));
                }
            } else if let Some(pattern) = record["pat"].as_array() {
                let xor = image["xorkey"]
                    .as_u64()
                    .ok_or_else(|| Error::new("decode", "image has no XOR key"))?;
                for byte in pattern {
                    let op = byte
                        .as_u64()
                        .ok_or_else(|| Error::new("decode", "invalid pattern byte"))?
                        ^ xor;
                    if mapping["simple"].get(op.to_string()).is_none() {
                        missing.insert(format!("operation {op} (pattern)"));
                    }
                }
            } else {
                let index = permutation
                    .first()
                    .and_then(Value::as_u64)
                    .and_then(|value| value.checked_sub(1))
                    .ok_or_else(|| Error::new("decode", "invalid opcode field"))?
                    as usize;
                let op = record["T"]
                    .get(index)
                    .ok_or_else(|| Error::new("decode", "short instruction record"))?;
                if op == &layout["junk_op"] {
                    continue;
                }
                let key = op
                    .as_i64()
                    .ok_or_else(|| Error::new("decode", "invalid opcode"))?
                    .to_string();
                if ["hot", "ctrl", "simple"]
                    .iter()
                    .all(|kind| mapping[kind].get(&key).is_none())
                {
                    missing.insert(format!("operation {key}"));
                }
            }
        }
    }
    if !missing.is_empty() {
        return Err(Error::new(
            "unsupported",
            format!(
                "this build uses handlers that are not understood yet: {}",
                missing.into_iter().take(8).collect::<Vec<_>>().join(", ")
            ),
        ));
    }
    Ok(())
}

fn add_fidelity(info: &mut Value, program: &Value, report: &Value, listing: &str) -> bool {
    let scoped =
        report.get("semantic_equivalent").is_some() || report.get("semantic_unresolved").is_some();
    let replay = if scoped {
        fidelity::semantic_ids(program, listing, report)
    } else {
        None
    };
    let trustworthy = !scoped || replay.is_some();
    info["verification"]["raw_bytecode"] = json!("incomplete");
    let original = report["functions_orig"].as_u64().unwrap_or(0);
    let new = report["functions_new"].as_u64().unwrap_or(0);
    let cosmetic = if trustworthy {
        report["cosmetic"].as_array().cloned().unwrap_or_default()
    } else {
        vec![]
    };
    let differing: Vec<_> = report["differing"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|id| !cosmetic.contains(id))
        .cloned()
        .collect();
    info["fidelity"] = json!({"functions":original,"functions_recompiled":new,"identical":report["identical"],"reordered":report["reordered"],"cosmetic":cosmetic.len(),"differing":differing});
    let Some(prototypes) = program["protos"].as_object() else {
        return false;
    };
    let Some(matches) = report["match"].as_object() else {
        return false;
    };
    let matched: Option<BTreeSet<String>> = matches
        .values()
        .map(|id| {
            id.as_i64().map(|value| value.to_string()).or_else(|| {
                id.as_str()
                    .and_then(|value| value.parse::<i64>().ok())
                    .map(|value| value.to_string())
            })
        })
        .collect();
    let complete = original > 0
        && original == new
        && original as usize == prototypes.len()
        && matches.len() == prototypes.len()
        && prototypes.keys().all(|key| matches.contains_key(key))
        && matched.is_some_and(|ids| ids.len() == prototypes.len());
    if complete && !differing.is_empty() {
        info["verification"]["raw_bytecode"] = json!("differences found");
    } else if complete
        && trustworthy
        && report["identical"]
            .as_u64()
            .and_then(|n| n.checked_add(report["reordered"].as_u64()?))
            .and_then(|n| n.checked_add(cosmetic.len() as u64))
            == Some(original)
    {
        info["verification"]["raw_bytecode"] = json!("passed");
    }
    if scoped {
        let ids = replay.filter(|_| complete).unwrap_or_default();
        let unresolved: Vec<_> = differing
            .iter()
            .filter(|id| !id.as_u64().is_some_and(|n| ids.contains(&n)))
            .collect();
        info["fidelity"]["semantic_equivalent"] = json!(ids.len());
        info["fidelity"]["differing"] = json!(unresolved);
        info["verification"]["raw_semantic"] = json!(if !ids.is_empty() && unresolved.is_empty() {
            "passed"
        } else {
            "incomplete"
        });
    }
    complete
}

fn clean_delivery(
    text: &mut String,
    info: &mut Value,
    options: &Options,
    is_static: bool,
    check: impl Fn(&str) -> Result<Value>,
) -> Option<String> {
    if !options.clean || options.annotate {
        return None;
    }
    let verified = info["verification"]["raw_bytecode"] == "passed";
    let infer_names = options.names.as_ref().is_none_or(|names| names.is_empty());
    let cleanup = (|| -> Result<(String, Value)> {
        let (mut cleaned, mut stats) = if is_static {
            source_cleanup::clean_helpers_source(text, infer_names)?
        } else {
            let (mut cleaned, mut stats) = clean::clean_source(
                text,
                Duration::from_secs(options.sandbox_timeout.min(30)),
                options.sandbox_mem_mb.min(512),
                verified,
            )?;
            if infer_names {
                let (renamed, count) = source_cleanup::rename_locals_source(&cleaned)?;
                cleaned = renamed;
                if count > 0 {
                    stats["names"] = json!(count);
                }
            }
            (cleaned, stats)
        };
        let (stripped, protection) = source_cleanup::strip_protection_source(&cleaned, verified)?;
        cleaned = stripped;
        if protection["statements"].as_u64().unwrap_or(0) > 0 {
            stats["protection"] = protection["statements"].clone();
            if let Some(counts) = protection.as_object() {
                for (key, count) in counts {
                    stats[key] =
                        json!(stats[key].as_u64().unwrap_or(0) + count.as_u64().unwrap_or(0));
                }
            }
        }
        Ok((cleaned, stats))
    })();
    match cleanup {
        Ok((cleaned, stats)) if cleaned != *text => match check(&cleaned) {
            Ok(check) if check["ok"] == true => {
                info["raw_lines"] = info["lines"].clone();
                info["lines"] = json!(cleaned.bytes().filter(|byte| *byte == b'\n').count());
                info["cleaned"] = stats;
                info["verification"]["delivered_syntax"] = json!("passed");
                info["verification"]["raw_applies_to_delivery"] = json!(false);
                return Some(std::mem::replace(text, cleaned));
            }
            Ok(check) if check["ok"] == false => warning(
                info,
                format!(
                    "the cleanup was dropped because its result had a syntax error: {}",
                    check["message"].as_str().unwrap_or("unknown error")
                ),
            ),
            Ok(_) => {}
            Err(error) => warning(
                info,
                format!(
                    "the cleanup was skipped: {}",
                    messages::scrub(&error.message)
                ),
            ),
        },
        Ok(_) => {}
        Err(error) => warning(
            info,
            format!(
                "the cleanup was skipped: {}",
                messages::scrub(&error.message)
            ),
        ),
    }
    None
}

pub fn run(data: &[u8], options: &Options, progress: Progress<'_>) -> Result<Output> {
    validate(data, options.max_bytes)?;
    if options.sandbox_only {
        return sandbox(data, options, progress);
    }
    let start = Instant::now();
    let source = astutil::normalize_source(data);
    let source_text = astutil::latin1(&source);
    let found_banner = banner(&source_text);
    let found_moonveil = moonveil::detect(&source_text);
    let found_moonveil = if found_moonveil.is_none() && options.moonveil {
        if found_banner.is_some() {
            return Err(Error::new("unsupported","this is a Bacon Guard / FORGE script, not a MoonVeil one; use the normal command for it"));
        }
        Some(json!({"version":"?","assumed":true}))
    } else {
        found_moonveil
    };
    if let Some(found) = found_moonveil {
        stage(progress, "analyse");
        let mut output = moonveil::deobfuscate(data, &found, options)?;
        if options.trace {
            stage(progress, "sandbox");
            match tracer::run(data, &trace_options(options, "the script as given")) {
                Ok(trace) => {
                    output.info["trace"] = trace["summary"].clone();
                    output.trace = Some(trace);
                }
                Err(error) => warning(
                    &mut output.info,
                    format!(
                        "the fake Roblox report could not be made: {}",
                        messages::scrub(&error.message)
                    ),
                ),
            }
        }
        return Ok(output);
    }
    let mut info = json!({"warnings":[]});
    if let Some(value) = found_banner {
        info["banner"] = value;
    }
    let mut timings = Map::new();
    let step = stage(progress, "analyse");
    let layout = loader::analyze(&source)?;
    for remark in layout_permutations(&layout)? {
        warning(&mut info, remark);
    }
    timings.insert("analyse".into(), seconds(step));

    let step = stage(progress, "unpack");
    let is_static = static_image::matches_source(&source);
    let image = if is_static {
        let value = static_image::decode(&layout)?;
        info["unpacking"] = json!("static");
        use sha2::{Digest, Sha256};
        info["source_sha256"] = json!(format!("{:x}", Sha256::digest(&source)));
        value
    } else {
        let script = patch::build_dump_script(&layout)?;
        let output = crate::sandbox::run_luau(
            &script,
            Duration::from_secs(options.sandbox_timeout),
            options.sandbox_mem_mb,
            128,
        )?;
        dump::parse_dump(&astutil::latin1(&output))?
    };
    timings.insert("unpack".into(), seconds(step));
    let prototypes = image["protos"]
        .as_object()
        .ok_or_else(|| Error::new("decode", "image has no prototypes"))?;
    info["protos"] = json!(prototypes.len());
    info["records"] = json!(prototypes
        .values()
        .map(|value| value["recs"].as_object().map_or(0, Map::len))
        .sum::<usize>());

    let step = stage(progress, "classify");
    let mapping = classify::classify(
        &layout,
        &image,
        Duration::from_secs(options.classify_timeout),
    )?;
    coverage(&image, &layout, &mapping)?;
    info["handlers"] = json!({"simple":mapping["simple"].as_object().map_or(0,Map::len),"hot":mapping["hot"].as_object().map_or(0,Map::len),"ctrl":mapping["ctrl"].as_object().map_or(0,Map::len),"ded":mapping["ded"].as_object().map_or(0,Map::len)});
    timings.insert("classify".into(), seconds(step));

    let step = stage(progress, "decompile");
    let program = decode::decode_program(&image, &mapping, &layout)?;
    let rendered = render::render(&program, options.names.as_ref(), options.annotate)?;
    let mut text = rendered["text"]
        .as_str()
        .ok_or_else(|| Error::new("decompile", "the decompiler returned no source"))?
        .to_string();
    info["lines"] = json!(text.bytes().filter(|byte| *byte == b'\n').count());
    info["duplicated_blocks"] = json!(rendered["log"].as_array().map_or(0, Vec::len));
    timings.insert("decompile".into(), seconds(step));

    info["verification"] = json!({"raw_syntax":"not run","delivered_syntax":"not run","raw_bytecode":"not run","raw_applies_to_delivery":!options.annotate,"fake_trace":"not run"});
    if options.annotate {
        info["annotated"] = json!(true);
        info["verification"]["delivered_syntax"] = json!("not runnable");
    }
    let mut clean_render: Option<String> = None;
    if options.verify || (options.trace && options.annotate) {
        clean_render = if options.annotate {
            render::render(&program, options.names.as_ref(), false)
                .ok()
                .and_then(|result| result["text"].as_str().map(str::to_string))
        } else {
            Some(text.clone())
        };
    }
    if options.verify {
        let step = stage(progress, "verify");
        info["verification"]["raw_bytecode"] = json!("incomplete");
        let checked = match clean_render.as_deref() {
            Some(source) => compile_check(source).unwrap_or_else(|error|json!({"ok":null,"message":format!("compile check unavailable: {}",messages::scrub(&error.message)),"bytecode":null})),
            None => json!({"ok":null,"message":"the result could not be verified (no clean copy to check)","bytecode":null}),
        };
        let syntax_status = match checked["ok"].as_bool() {
            Some(true) => "passed",
            Some(false) => "failed",
            None => "unavailable",
        };
        info["verification"]["raw_syntax"] = json!(syntax_status);
        if !options.annotate {
            info["verification"]["delivered_syntax"] = json!(syntax_status);
        }
        match checked["ok"].as_bool() {
            Some(true) => {
                info["syntax_ok"] = json!(true);
                if let Some(bytecode) = checked["bytecode"].as_str() {
                    match fidelity::compare_for_source(&program, bytecode, &source) {
                        Ok(report) => {
                            add_fidelity(&mut info, &program, &report, bytecode);
                        }
                        Err(error) => warning(
                            &mut info,
                            format!("fidelity check failed: {}", messages::scrub(&error.message)),
                        ),
                    }
                }
            }
            Some(false) => warning(
                &mut info,
                format!(
                    "the result has a syntax error: {}",
                    checked["message"].as_str().unwrap_or("unknown error")
                ),
            ),
            None => warning(
                &mut info,
                checked["message"]
                    .as_str()
                    .unwrap_or("compile check unavailable"),
            ),
        }
        timings.insert("verify".into(), seconds(step));
    }

    let mut raw = None;
    if options.clean && !options.annotate {
        let step = stage(progress, "clean");
        raw = clean_delivery(&mut text, &mut info, options, is_static, compile_check);
        timings.insert("clean".into(), seconds(step));
    }

    let mut trace = None;
    if options.trace && is_static {
        info["verification"]["fake_trace"] = json!("unavailable");
        warning(
            &mut info,
            "the fake Roblox run was skipped for the data-only static image path",
        );
    } else if options.trace {
        let step = stage(progress, "sandbox");
        info["verification"]["fake_trace"] = json!("unavailable");
        let run_text = if options.annotate {
            clean_render.as_deref()
        } else {
            Some(text.as_str())
        };
        if let Some(run_text) = run_text {
            match tracer::run(
                run_text.as_bytes(),
                &trace_options(options, "the deobfuscated script"),
            ) {
                Ok(report) => {
                    info["trace"] = report["summary"].clone();
                    info["verification"]["fake_trace"] = json!("observed");
                    trace = Some(report);
                }
                Err(error) => warning(
                    &mut info,
                    format!(
                        "the fake Roblox run failed: {}",
                        messages::scrub(&error.message)
                    ),
                ),
            }
        } else {
            warning(
                &mut info,
                "the fake Roblox run was skipped: there is no clean copy of the annotated script",
            );
        }
        timings.insert("sandbox".into(), seconds(step));
    }
    timings.insert("total".into(), seconds(start));
    info["timings"] = Value::Object(timings);
    Ok(Output {
        text: Some(text),
        info,
        trace,
        raw,
    })
}

pub type TraceFiles = Vec<(String, Vec<u8>)>;

pub fn fill_trace(trace: &Value, name: &str, stem: &str) -> Result<(String, TraceFiles)> {
    let report = trace["report"]
        .as_str()
        .ok_or_else(|| Error::new("internal", "trace has no report"))?
        .replace("@@NAME@@", name)
        .replace("@@STEM@@", stem);
    let mut files = Vec::new();
    let payloads = trace["payloads"]
        .as_array()
        .ok_or_else(|| Error::new("internal", "trace has no payload list"))?;
    for payload in payloads {
        let id = payload["id"]
            .as_u64()
            .ok_or_else(|| Error::new("internal", "invalid trace payload id"))?;
        let extension = payload["ext"]
            .as_str()
            .filter(|ext| matches!(*ext, "lua" | "txt"))
            .ok_or_else(|| Error::new("internal", "invalid trace payload extension"))?;
        let data = payload["data"]
            .as_array()
            .ok_or_else(|| Error::new("internal", "trace payload has no bytes"))?
            .iter()
            .map(|byte| {
                byte.as_u64()
                    .filter(|byte| *byte <= 255)
                    .map(|byte| byte as u8)
                    .ok_or_else(|| Error::new("internal", "invalid trace payload byte"))
            })
            .collect::<Result<Vec<_>>>()?;
        files.push((format!("{stem}_payload{id}.{extension}"), data));
    }
    Ok((report, files))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn delivery_info(status: &str) -> Value {
        json!({"warnings":[],"lines":3,"verification":{"raw_bytecode":status,"raw_applies_to_delivery":true,"delivered_syntax":"not run"}})
    }

    #[test]
    fn both_delivery_paths_infer_names_and_honor_explicit_names_or_disabled_cleanup() {
        let source =
            "local frame = Instance.new(\"Frame\")\nframe.Name=\"KeyPanel\"\nreturn frame\n";
        for is_static in [false, true] {
            for (names, infer_names) in [
                (None, true),
                (Some(std::collections::BTreeMap::new()), true),
                (
                    Some(std::collections::BTreeMap::from([(
                        "0:0".into(),
                        "frame".into(),
                    )])),
                    false,
                ),
            ] {
                let options = Options {
                    names,
                    verify: false,
                    ..Options::default()
                };
                let mut text = source.to_string();
                let mut info = delivery_info("not run");
                let raw = clean_delivery(&mut text, &mut info, &options, is_static, compile_check);
                if !infer_names {
                    assert_eq!(text, source);
                    assert!(raw.is_none() && info.get("cleaned").is_none());
                } else {
                    assert!(
                        text.contains("local keyPanel = Instance.new(\"Frame\")"),
                        "{text}"
                    );
                    assert_eq!(info["cleaned"]["names"], 1);
                    assert_eq!(raw.as_deref(), Some(source));
                    assert_eq!(info["verification"]["raw_applies_to_delivery"], false);
                    assert_eq!(info["verification"]["delivered_syntax"], "passed");
                }
            }
            for options in [
                Options {
                    clean: false,
                    ..Options::default()
                },
                Options {
                    annotate: true,
                    ..Options::default()
                },
            ] {
                let mut text = source.to_string();
                let mut info = delivery_info("passed");
                assert!(
                    clean_delivery(&mut text, &mut info, &options, is_static, |_| panic!(
                        "disabled cleanup must not compile"
                    ))
                    .is_none()
                );
                assert_eq!(text, source);
                assert!(info.get("cleaned").is_none());
            }
        }
    }

    #[test]
    fn both_delivery_paths_only_strip_protection_after_complete_verification() {
        let source = include_str!("../test/fixtures/delivery_protection.lua");
        for is_static in [false, true] {
            for status in ["passed", "not run", "incomplete", "differences found"] {
                let mut text = source.to_string();
                let mut info = delivery_info(status);
                clean_delivery(
                    &mut text,
                    &mut info,
                    &Options::default(),
                    is_static,
                    compile_check,
                );
                assert!(text.contains("application key failed"), "{text}");
                assert_eq!(
                    text.contains("getconstants"),
                    status != "passed",
                    "{status}: {text}"
                );
                assert_eq!(
                    text.contains("verifyEnvironment"),
                    status != "passed",
                    "{status}: {text}"
                );
                assert_eq!(info["verification"]["raw_bytecode"], status);
                assert_eq!(compile_check(&text).unwrap()["ok"], true);
            }
            for options in [
                Options {
                    clean: false,
                    ..Options::default()
                },
                Options {
                    annotate: true,
                    ..Options::default()
                },
            ] {
                let mut text = source.to_string();
                let mut info = delivery_info("passed");
                assert!(
                    clean_delivery(&mut text, &mut info, &options, is_static, |_| panic!(
                        "disabled cleanup must not compile"
                    ))
                    .is_none()
                );
                assert_eq!(text, source);
                assert_eq!(info["verification"]["raw_bytecode"], "passed");
                assert!(info.get("cleaned").is_none());
            }
        }
    }

    #[test]
    fn rejected_or_unavailable_cleanup_keeps_raw_text_and_verification() {
        let source =
            "local frame = Instance.new(\"Frame\")\nframe.Name=\"KeyPanel\"\nreturn frame\n";
        for is_static in [false, true] {
            for checked in [
                Ok(json!({"ok":false,"message":"injected syntax failure","bytecode":null})),
                Ok(json!({"ok":null,"message":"compiler unavailable","bytecode":null})),
                Err(Error::new("setup", "compiler missing")),
            ] {
                let mut text = source.to_string();
                let mut info = delivery_info("passed");
                assert!(clean_delivery(
                    &mut text,
                    &mut info,
                    &Options::default(),
                    is_static,
                    |_| checked.clone()
                )
                .is_none());
                assert_eq!(text, source);
                assert!(info.get("cleaned").is_none());
                assert_eq!(info["verification"]["raw_applies_to_delivery"], true);
                if checked.is_err() || checked.as_ref().is_ok_and(|value| value["ok"] == false) {
                    assert_eq!(info["warnings"].as_array().unwrap().len(), 1);
                }
            }
            let mut text = "local =\n".to_string();
            let mut info = delivery_info("passed");
            assert!(clean_delivery(
                &mut text,
                &mut info,
                &Options::default(),
                is_static,
                compile_check
            )
            .is_none());
            assert_eq!(text, "local =\n");
            assert!(info["warnings"][0]
                .as_str()
                .unwrap()
                .contains("cleanup was skipped"));
        }
    }

    #[test]
    fn static_delivery_composes_inline_strings_and_dispatch_cleanup_without_evaluation() {
        let options = Options {
            verify: false,
            trace: true,
            sandbox_timeout: 0,
            sandbox_mem_mb: 0,
            ..Options::default()
        };
        let inline = include_str!("../test/fixtures/engine_inline.lua");
        let mut text = inline.to_string();
        let mut info = delivery_info("not run");
        assert_eq!(
            clean_delivery(&mut text, &mut info, &options, true, compile_check).as_deref(),
            Some(inline)
        );
        assert_eq!(text.trim(), "return \"HI\"");
        assert_eq!(info["cleaned"]["strings"], 1);
        assert_eq!(info["verification"]["raw_bytecode"], "not run");
        assert_eq!(info["verification"]["raw_applies_to_delivery"], false);
        assert_eq!(info["verification"]["delivered_syntax"], "passed");

        let dispatch = include_str!("../test/fixtures/engine_dispatch.lua");
        let mut text = dispatch.to_string();
        let mut info = delivery_info("not run");
        assert_eq!(
            clean_delivery(&mut text, &mut info, &options, true, compile_check).as_deref(),
            Some(dispatch)
        );
        assert!(
            !text.contains("while state")
                && !text.contains("4610615")
                && !text.contains("os.clock()"),
            "{text}"
        );
        assert!(text.contains("print(\"condition kept\")"), "{text}");
        assert_eq!(info["cleaned"]["states"], 1);
        assert_eq!(info["cleaned"]["numbers"], 1);
        assert_eq!(info["cleaned"]["checks"], 1);
        assert_eq!(info["verification"]["delivered_syntax"], "passed");
        assert_eq!(info["verification"]["raw_applies_to_delivery"], false);
        for source in [inline, dispatch] {
            let mut text = source.to_string();
            let mut info = delivery_info("not run");
            assert!(clean_delivery(
                &mut text,
                &mut info,
                &Options {
                    clean: false,
                    ..options.clone()
                },
                true,
                |_| panic!("disabled cleanup must not compile")
            )
            .is_none());
            assert_eq!(text, source);
            assert!(info.get("cleaned").is_none());
        }
    }

    #[test]
    fn incomplete_fidelity_cannot_authorize_protection_removal() {
        let program = json!({"protos":{"1":{},"2":{}}});
        for matches in [
            json!({"1":0}),
            json!({"1":0,"2":0}),
            json!({"1":0,"2":null}),
        ] {
            let mut info = json!({"verification":{"raw_bytecode":"incomplete"}});
            let report = json!({"functions_orig":2,"functions_new":2,"identical":2,"reordered":0,"cosmetic":[],"differing":[],"match":matches});
            assert!(!add_fidelity(&mut info, &program, &report, ""));
            assert_eq!(info["verification"]["raw_bytecode"], "incomplete");
        }
    }

    #[test]
    fn semantic_equivalence_counts_toward_complete_fidelity() {
        // A two-prototype chunk: fresh reference capture of a boolean vs a reused value capture.
        let proto = |ops: &[(&str, u64, u64, u64)], kids: Vec<u64>, va: bool, up: u64| {
            let mut decoded = serde_json::Map::new();
            for (i, &(op, a, b, c)) in ops.iter().enumerate() {
                let item = json!([op,{"A":a,"B":b,"C":c,"rf":0,"aux":0,"Hf":0}]);
                let mut rec = json!({"rid":i,"kind":op,"sem":0,"ins":[item],"ctrl":null,"nxt":i+1,"alt":4294967295u64});
                if op == "RETURN" {
                    rec["ins"] = json!([]);
                    rec["ctrl"] = item;
                    rec["nxt"] = json!(4294967295u64);
                }
                decoded.insert(i.to_string(), rec);
            }
            json!({"entry":0,"np":0,"nup":up,"va":va,"kids":kids,"consts":{},"decoded":decoded})
        };
        let program = json!({"main":177,"protos":{"0":proto(&[("GETUPVAL",0,0,0),("RETURN",0,2,0)],vec![],false,1),"177":proto(&[("GETVARARGS",0,2,0),("NOT",0,0,0),("NEWCLOSURE",1,0,0),("CAPTURE",1,0,0),("CALL",1,1,0),("CLOSEUPVALS",0,0,0),("RETURN",1,0,0)],vec![0],true,0)}});
        let compiled = compile_check(
            "local arg=...; local value=not arg; return (function() return value end)()",
        )
        .unwrap();
        let listing = compiled["bytecode"]
            .as_str()
            .unwrap()
            .replace("Function 1 (", "Function 177 (");
        let listing = listing.as_str();
        let report = fidelity::proofs::compare_scoped(&program, listing, &[177]).unwrap();
        let proof = &report["semantic_equivalent"][0];
        let mut info = delivery_info("incomplete");
        assert!(add_fidelity(&mut info, &program, &report, listing));
        assert_eq!(info["verification"]["raw_bytecode"], "differences found");
        assert_eq!(info["verification"]["raw_semantic"], "passed");
        assert_eq!(info["fidelity"]["semantic_equivalent"], 1);
        assert_eq!(info["fidelity"]["differing"], json!([]));
        for (key, value) in [
            ("match", json!({"0":null,"177":177})),
            ("match", json!({"0":177,"177":177})),
            ("semantic_equivalent", json!([proof, proof])),
            (
                "semantic_equivalent",
                json!([{ "prototype":0,"function":0,"proofs":proof["proofs"] }]),
            ),
            (
                "semantic_equivalent",
                json!([{ "prototype":177,"function":0,"proofs":proof["proofs"] }]),
            ),
            ("cosmetic", json!([177])),
            ("differing", json!([0, 177])),
            ("identical", json!(0)),
            ("reordered_protos", json!([177])),
            (
                "semantic_unresolved",
                json!([{"prototype":177,"function":177,"reason":"unknown edge"}]),
            ),
        ] {
            let mut bad = report.clone();
            bad[key] = value;
            let mut info = delivery_info("incomplete");
            add_fidelity(&mut info, &program, &bad, listing);
            assert_ne!(info["verification"]["raw_semantic"], "passed", "{bad}");
        }
        for (key, value) in [
            ("kind", json!("maybe")),
            ("kind", json!("")),
            ("compiler_pc", json!(-1)),
            ("compiler_pc", json!(19267)),
            ("record", json!(82441)),
            ("evidence", json!({"unrelated":false})),
            ("evidence", json!({})),
        ] {
            let mut bad = report.clone();
            bad["semantic_equivalent"][0]["proofs"][0][key] = value;
            let mut info = delivery_info("incomplete");
            add_fidelity(&mut info, &program, &bad, listing);
            assert_ne!(info["verification"]["raw_semantic"], "passed", "{bad}");
        }
        for is_static in [true, false] {
            let mut text = include_str!("../test/fixtures/delivery_protection.lua").to_owned();
            clean_delivery(
                &mut text,
                &mut info,
                &Options::default(),
                is_static,
                compile_check,
            );
            assert!(text.contains("getconstants") && text.contains("verifyEnvironment"));
        }
    }

    #[test]
    fn complete_one_to_one_fidelity_is_required() {
        let program = json!({"protos":{"1":{},"2":{}}});
        let mut info = json!({"verification":{"raw_bytecode":"incomplete"}});
        let report = json!({"functions_orig":2,"functions_new":2,"identical":1,"reordered":1,"cosmetic":[],"differing":[],"match":{"1":0,"2":1}});
        assert!(add_fidelity(&mut info, &program, &report, ""));
        assert_eq!(info["verification"]["raw_bytecode"], "passed");
    }

    #[test]
    fn review_contradictory_report_never_grants_strict_cleanup() {
        let program = json!({"protos":{"0":{},"216":{}}});
        let report = json!({"functions_orig":2,"functions_new":2,"identical":1,"reordered":0,"reordered_protos":[],"cosmetic":[216],"differing":[216],"match":{"0":0,"216":216},"semantic_equivalent":[{"prototype":216,"function":216,"proofs":[{"kind":"false-seed-short-circuit","record":0,"compiler_pc":0,"evidence":{"fake":true}}]}],"semantic_unresolved":[]});
        for is_static in [false, true] {
            let mut info = delivery_info("incomplete");
            add_fidelity(&mut info, &program, &report, "");
            let mut text = include_str!("../test/fixtures/delivery_protection.lua").to_owned();
            clean_delivery(
                &mut text,
                &mut info,
                &Options::default(),
                is_static,
                compile_check,
            );
            assert!(
                text.contains("getconstants") && text.contains("verifyEnvironment"),
                "invalid proof stripped protection, strict status {}",
                info["verification"]["raw_bytecode"]
            );
            assert_ne!(info["verification"]["raw_bytecode"], "passed");
        }
    }

    #[test]
    fn review_unknown_witness_and_positions_never_count() {
        let program = json!({"protos":{"0":{},"216":{}}});
        let report = json!({"functions_orig":2,"functions_new":2,"identical":1,"reordered":0,"reordered_protos":[],"cosmetic":[],"differing":[216],"match":{"0":0,"216":216},"semantic_equivalent":[{"prototype":216,"function":216,"proofs":[{"kind":"false-seed-short-circuit","record":98765,"compiler_pc":12345,"evidence":{"unrelated":false}}]}],"semantic_unresolved":[]});
        let mut info = delivery_info("incomplete");
        add_fidelity(&mut info, &program, &report, "");
        assert_ne!(info["verification"]["raw_semantic"], "passed");
    }

    #[test]
    fn bytecode_status_requires_both_complete_function_sets_and_accepted_matches() {
        let program = json!({"protos":{"1":{}}});
        let base = json!({"functions_orig":1,"functions_new":1,"identical":1,"reordered":0,"cosmetic":[],"differing":[],"match":{"1":0}});
        for (field, value, expected) in [
            ("functions_new", json!(2), "incomplete"),
            ("match", json!({"1":null}), "incomplete"),
            ("differing", json!([1]), "differences found"),
            ("match", json!({}), "incomplete"),
            ("match", json!({"2":0}), "incomplete"),
            ("identical", json!(0), "incomplete"),
        ] {
            let mut report = base.clone();
            report[field] = value;
            let mut info = json!({"verification":{"raw_bytecode":"incomplete"}});
            add_fidelity(&mut info, &program, &report, "");
            assert_eq!(
                info["verification"]["raw_bytecode"], expected,
                "field {field}, report {report}"
            );
        }
        let mut info = json!({"verification":{"raw_bytecode":"incomplete"}});
        let report = json!({"functions_orig":0,"functions_new":0,"identical":0,"reordered":0,"cosmetic":[],"differing":[],"match":{}});
        assert!(!add_fidelity(&mut info, &json!({"protos":{}}), &report, ""));
        assert_eq!(info["verification"]["raw_bytecode"], "incomplete");
    }

    #[test]
    fn malformed_permutations_never_pass_layout_validation() {
        let valid = json!({"perm":{"0":[1,2,3,4,5,6,7,8,9,10]}, "fused_perm":{"0":[3,4,5,6,7,8]}, "simple_perm":{"0":[1,3,4,5,6,7,8]}});
        assert!(layout_permutations(&valid).unwrap().is_empty());
        for value in [
            json!([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, null]),
            json!([1, 2, 3, 4, 5, 6, 7, 8, 9]),
            json!([1, 2, 3, 4, 5, 6, 7, 8, 9, 9]),
        ] {
            let mut bad = valid.clone();
            bad["perm"]["0"] = value;
            assert!(
                layout_permutations(&bad).is_err(),
                "accepted malformed layout {}",
                bad["perm"]["0"]
            );
        }
    }
}

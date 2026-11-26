use crate::error::{Error, Result};
use crate::model::{Options, Output, Progress};
use crate::{astutil, engine, messages, namesfile, runner};
use clap::Parser;
use regex::Regex;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Mutex, OnceLock};
use std::time::{Duration, Instant};

#[derive(Debug, Parser)]
#[command(
    name = "bgdeobf",
    version,
    about = "Deobfuscate Bacon Guard / FORGE Luau scripts (MoonVeil: behaviour reconstruction only)."
)]
struct Arguments {
    #[arg(value_name = "INPUT")]
    input: Vec<PathBuf>,
    #[arg(long, help = "Check the bundled Luau tools")]
    check: bool,
    #[arg(long, help = "Detect the obfuscator without executing the script")]
    detect: bool,
    #[arg(short, long, value_name = "PATH")]
    output: Option<PathBuf>,
    #[arg(long, default_value_t = 1)]
    jobs: usize,
    #[arg(long, value_name = "JSON")]
    names: Option<PathBuf>,
    #[arg(long)]
    annotate: bool,
    #[arg(long)]
    no_verify: bool,
    #[arg(long)]
    no_clean: bool,
    #[arg(long)]
    trace: bool,
    #[arg(long)]
    sandbox: bool,
    #[arg(long)]
    explore: bool,
    #[arg(long, default_value_t = 0)]
    place_id: i64,
    #[arg(long)]
    user_id: Option<i64>,
    #[arg(long)]
    username: Option<String>,
    #[arg(long)]
    executor: Option<String>,
    #[arg(long, default_value_t = 0)]
    players: i64,
    #[arg(long, value_name = "JSON")]
    responses: Option<PathBuf>,
    #[arg(long, default_value_t = 30.0)]
    sandbox_time: f64,
    #[arg(long, default_value_t = 30)]
    sandbox_timeout: u64,
    #[arg(long, default_value_t = 240)]
    timeout: u64,
    #[arg(long, default_value_t = 3.0)]
    max_mb: f64,
    #[arg(
        long,
        help = "Debugging only: bypass the isolated worker and its resource limits"
    )]
    in_process: bool,
    #[arg(long)]
    json: bool,
    #[arg(short, long)]
    quiet: bool,
    #[arg(long, hide = true, value_name = "DIRECTORY")]
    worker: Option<PathBuf>,
}

fn argument_error(arguments: &Arguments) -> Option<&'static str> {
    if arguments.jobs == 0 {
        return Some("--jobs must be at least 1");
    }
    if arguments.input.len() > 1 && arguments.input.iter().any(|path| path == Path::new("-")) {
        return Some("- (standard input) cannot be combined with other inputs");
    }
    if arguments.jobs > 1 && arguments.in_process {
        return Some("--jobs needs the separate worker processes; leave out --in-process");
    }
    if arguments.trace && arguments.sandbox {
        return Some("--trace and --sandbox cannot be combined (--sandbox runs the script as given, --trace runs the deobfuscated result)");
    }
    if arguments.sandbox && (arguments.annotate || arguments.names.is_some()) {
        return Some("--sandbox does not deobfuscate, so --annotate and --names do not apply");
    }
    if !(arguments.trace || arguments.sandbox)
        && (arguments.explore
            || arguments.responses.is_some()
            || arguments.user_id.is_some_and(|value| value != 0)
            || arguments
                .username
                .as_ref()
                .is_some_and(|value| !value.is_empty())
            || arguments
                .executor
                .as_ref()
                .is_some_and(|value| !value.is_empty())
            || arguments.players != 0
            || arguments.place_id != 0)
    {
        return Some("--explore, --responses, --place-id, --user-id, --username, --executor and --players need --trace or --sandbox");
    }
    if !arguments.max_mb.is_finite()
        || arguments.max_mb < 0.0
        || arguments.max_mb * 1024.0 * 1024.0 > usize::MAX as f64
    {
        return Some("--max-mb must be a finite nonnegative size");
    }
    if !arguments.sandbox_time.is_finite() {
        return Some("--sandbox-time must be finite");
    }
    None
}

fn generated_name(name: &str) -> bool {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(||Regex::new(r"(?i)(\.deobf\.lua|_deobf\.lua|_annotated\.lua|_reconstructed\.lua|_display\.txt|_trace\.txt|_verification\.txt|_payload\d+\.(lua|txt))$").expect("constant generated-file pattern")).is_match(name)
}

fn path_key(path: &Path) -> String {
    let resolved = fs::canonicalize(path)
        .or_else(|_| {
            let absolute = if path.is_absolute() {
                path.to_path_buf()
            } else {
                std::env::current_dir()?.join(path)
            };
            if let (Some(parent), Some(name)) = (absolute.parent(), absolute.file_name()) {
                if let Ok(parent) = fs::canonicalize(parent) {
                    return Ok(parent.join(name));
                }
            }
            Ok::<_, io::Error>(absolute)
        })
        .unwrap_or_else(|_| path.to_path_buf());
    let text = resolved.to_string_lossy().to_string();
    if cfg!(windows) {
        text.to_lowercase()
    } else {
        text
    }
}

pub fn expand_inputs(paths: &[PathBuf]) -> (Vec<PathBuf>, Vec<String>) {
    let mut files = Vec::new();
    let mut problems = Vec::new();
    let mut seen = BTreeSet::new();
    for path in paths {
        let found = if path.is_dir() {
            match fs::read_dir(path) {
                Ok(entries) => {
                    let mut found: Vec<_> = entries
                        .filter_map(std::result::Result::ok)
                        .map(|entry| entry.path())
                        .filter(|file| {
                            let name = file.file_name().unwrap_or_default().to_string_lossy();
                            let extension = file
                                .extension()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .to_lowercase();
                            file.is_file()
                                && matches!(extension.as_str(), "lua" | "luau" | "txt")
                                && !generated_name(&name)
                        })
                        .collect();
                    found.sort_by(|left, right| left.file_name().cmp(&right.file_name()));
                    if found.is_empty() {
                        problems.push(format!(
                            "No .lua / .luau / .txt scripts found in {}",
                            path.display()
                        ));
                    }
                    found
                }
                Err(error) => {
                    problems.push(format!(
                        "Cannot read the folder {}: {error}",
                        path.display()
                    ));
                    Vec::new()
                }
            }
        } else {
            vec![path.clone()]
        };
        for file in found {
            if seen.insert(path_key(&file)) {
                files.push(file);
            }
        }
    }
    (files, problems)
}

fn read_input(input: &Path, max_bytes: usize) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let limit = max_bytes.saturating_add(1) as u64;
    if input == Path::new("-") {
        io::stdin().lock().take(limit).read_to_end(&mut bytes)?;
    } else {
        File::open(input)?.take(limit).read_to_end(&mut bytes)?;
    }
    Ok(bytes)
}

fn output_path(
    input: &Path,
    info: &Value,
    arguments: &Arguments,
    directory: Option<&Path>,
    taken: &Mutex<BTreeSet<String>>,
) -> Result<PathBuf> {
    if let Some(explicit) = &arguments.output {
        if directory.is_none() {
            return Ok(explicit.clone());
        }
    }
    let stem = if input == Path::new("-") {
        OsString::from("stdin")
    } else {
        input
            .file_stem()
            .unwrap_or(OsStr::new("script"))
            .to_os_string()
    };
    let suffix = if info["sandbox_only"] == true {
        "_trace.txt"
    } else if info["trace_only"] == true {
        "_reconstructed.lua"
    } else if info["annotated"] == true {
        "_annotated.lua"
    } else {
        ".deobf.lua"
    };
    let folder = if let Some(directory) = directory {
        directory.to_path_buf()
    } else if input == Path::new("-") {
        std::env::current_dir()?
    } else {
        let absolute = if input.is_absolute() {
            input.to_path_buf()
        } else {
            std::env::current_dir()?.join(input)
        };
        absolute.parent().unwrap_or(Path::new(".")).to_path_buf()
    };
    let mut reserved = taken
        .lock()
        .map_err(|_| Error::new("internal", "output reservation failed"))?;
    let mut number = 1;
    loop {
        let mut name = stem.clone();
        if number > 1 {
            name.push(format!("_{number}"));
        }
        name.push(suffix);
        let candidate = folder.join(name);
        if reserved.insert(path_key(&candidate)) {
            return Ok(candidate);
        }
        number += 1;
    }
}

fn trace_files(
    trace: &Value,
    input: &Path,
    output: &Path,
    report_is_output: bool,
) -> Result<Vec<PathBuf>> {
    let name = if input == Path::new("-") {
        "stdin".into()
    } else {
        input
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string()
    };
    let output_name = output.file_name().unwrap_or_default().to_string_lossy();
    let base_name = [
        ".deobf.lua",
        "_deobf.lua",
        "_annotated.lua",
        "_reconstructed.lua",
        "_trace.txt",
    ]
    .iter()
    .find_map(|suffix| output_name.strip_suffix(suffix))
    .map(str::to_string)
    .unwrap_or_else(|| {
        output
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string()
    });
    let folder = output.parent().unwrap_or(Path::new("."));
    let (report, payloads) = engine::fill_trace(trace, &name, &base_name)?;
    let report_path = if report_is_output {
        output.to_path_buf()
    } else {
        folder.join(format!("{base_name}_trace.txt"))
    };
    fs::write(&report_path, report.as_bytes())?;
    let mut paths = vec![report_path];
    for (name, data) in payloads {
        let path = folder.join(name);
        fs::write(&path, data)?;
        paths.push(path);
    }
    Ok(paths)
}

struct FileResult {
    file: PathBuf,
    output: Option<PathBuf>,
    info: Option<Value>,
    error: Option<Error>,
    unreadable: bool,
    seconds: f64,
    trace_files: Vec<PathBuf>,
}

fn process_file(
    input: &Path,
    options: &Options,
    arguments: &Arguments,
    directory: Option<&Path>,
    taken: &Mutex<BTreeSet<String>>,
    progress: Progress<'_>,
) -> FileResult {
    let start = Instant::now();
    let mut result = FileResult {
        file: input.to_path_buf(),
        output: None,
        info: None,
        error: None,
        unreadable: false,
        seconds: 0.0,
        trace_files: Vec::new(),
    };
    let data = match read_input(input, options.max_bytes) {
        Ok(data) => data,
        Err(error) => {
            result.unreadable = true;
            result.error = Some(Error::new(
                "unreadable",
                format!("Cannot read the file: {error}"),
            ));
            return result;
        }
    };
    let output = if arguments.in_process {
        engine::run(&data, options, progress)
    } else {
        runner::run(
            &data,
            options,
            Duration::from_secs(arguments.timeout),
            progress,
            None,
        )
    };
    let written = (|| -> Result<(PathBuf, Output, Vec<PathBuf>)> {
        let output = output?;
        let path = output_path(input, &output.info, arguments, directory, taken)?;
        let mut extra = Vec::new();
        if let Some(text) = &output.text {
            fs::write(&path, text.as_bytes()).map_err(|error| {
                Error::new("unwritable", format!("Cannot write the result: {error}"))
            })?;
        }
        if let Some(trace) = &output.trace {
            extra = trace_files(trace, input, &path, output.text.is_none()).map_err(|error| {
                Error::new(
                    "unwritable",
                    format!("Cannot write the result: {}", error.message),
                )
            })?;
        }
        Ok((path, output, extra))
    })();
    match written {
        Ok((path, output, extra)) => {
            result.output = Some(path);
            result.info = Some(output.info);
            result.trace_files = extra;
        }
        Err(error) => {
            result.error = Some(error);
        }
    }
    result.seconds = start.elapsed().as_secs_f64();
    result
}

fn short_summary(info: &Value) -> String {
    let prefix = messages::verification_status(info)
        .map(|status| format!("{status}; "))
        .unwrap_or_default();
    if info["fidelity"].is_object() {
        let fidelity = &info["fidelity"];
        let mut extra = Vec::new();
        for (key, label) in [
            ("reordered", "reordered"),
            ("cosmetic", "cosmetic"),
            ("semantic_equivalent", "proven equivalent"),
        ] {
            let count = fidelity[key].as_u64().unwrap_or(0);
            if count > 0 {
                extra.push(format!("{count} {label}"));
            }
        }
        let differing = fidelity["differing"].as_array().map_or(0, Vec::len);
        if differing > 0 {
            extra.push(format!("{differing} differ"));
        }
        format!(
            "{prefix}{}/{} functions identical{}",
            fidelity["identical"].as_u64().unwrap_or(0),
            fidelity["functions"].as_u64().unwrap_or(0),
            if extra.is_empty() {
                String::new()
            } else {
                format!(" ({})", extra.join(", "))
            }
        )
    } else {
        format!(
            "{prefix}{} functions{}",
            info["protos"].as_u64().unwrap_or(0),
            if info["syntax_ok"] == true {
                ", syntax ok"
            } else {
                ""
            }
        )
    }
}

fn print_json(value: &Value) {
    match serde_json::to_string_pretty(value) {
        Ok(text) => println!("{text}"),
        Err(error) => eprintln!("Cannot serialize the result: {error}"),
    }
}

fn detect_files(files: &[PathBuf], arguments: &Arguments, max_bytes: usize) -> i32 {
    let mut rows = Vec::new();
    let mut failed = false;
    for input in files {
        let data = match read_input(input, max_bytes) {
            Ok(data) => data,
            Err(error) => {
                eprintln!("Cannot read the file: {error}");
                failed = true;
                continue;
            }
        };
        match engine::detect(&data, max_bytes) {
            Ok(mut row) => {
                row["file"] = json!(input.to_string_lossy());
                failed |= row["supported"] != true;
                rows.push(row);
            }
            Err(error) => {
                eprintln!("FAILED: {}", messages::explain(&error.code, &error.message));
                failed = true;
            }
        }
    }
    if arguments.json {
        print_json(&Value::Array(rows));
    } else {
        for row in rows {
            let path = Path::new(row["file"].as_str().unwrap_or(""));
            let label = row["label"].as_str().unwrap_or("unknown");
            let detail = row["detail"].as_str().unwrap_or("");
            let label = if row["supported"] == true || detail.is_empty() {
                label.to_string()
            } else {
                format!("{label}: {detail}")
            };
            println!(
                "{}\t{}\t{:.2}\t{label}",
                path.file_name().unwrap_or_default().to_string_lossy(),
                row["kind"].as_str().unwrap_or("unknown"),
                row["confidence"].as_f64().unwrap_or(0.0)
            );
        }
    }
    if failed {
        2
    } else {
        0
    }
}

pub fn main() -> i32 {
    let arguments = match Arguments::try_parse() {
        Ok(arguments) => arguments,
        Err(error) => {
            let code = error.exit_code();
            let _ = error.print();
            return code;
        }
    };
    if let Some(directory) = &arguments.worker {
        return match runner::worker_main(directory) {
            Ok(()) => 0,
            Err(error) => {
                eprintln!("{}", messages::scrub(&error.message));
                2
            }
        };
    }
    if arguments.check {
        let mut bad = false;
        for (name, ok, message) in astutil::check_tools() {
            println!("{name:<13} {}  {message}", if ok { "OK  " } else { "FAIL" });
            bad |= !ok;
        }
        return i32::from(bad);
    }
    if arguments.input.is_empty() {
        eprintln!("error: give a file name, a folder, or use --check");
        return 2;
    }
    if let Some(message) = argument_error(&arguments) {
        eprintln!("error: {message}");
        return 2;
    }
    let max_bytes = (arguments.max_mb * 1024.0 * 1024.0) as usize;
    let responses = if let Some(path) = &arguments.responses {
        let response = fs::read(path)
            .map_err(|error| error.to_string())
            .and_then(|bytes| {
                serde_json::from_slice::<Value>(&bytes).map_err(|error| error.to_string())
            });
        match response {
            Ok(value)
                if value
                    .as_object()
                    .is_some_and(|values| values.values().all(Value::is_string)) =>
            {
                Some(value)
            }
            Ok(_) => {
                eprintln!("error: invalid --responses file: it must be one JSON object of strings: {{\"https://...\":\"text\"}}");
                return 2;
            }
            Err(error) => {
                eprintln!("error: invalid --responses file: {error}");
                return 2;
            }
        }
    } else {
        None
    };
    let (files, problems) = if arguments.input == [PathBuf::from("-")] {
        (arguments.input.clone(), Vec::new())
    } else {
        expand_inputs(&arguments.input)
    };
    for problem in problems {
        eprintln!("{problem}");
    }
    if files.is_empty() {
        return 1;
    }
    if arguments.detect {
        return detect_files(&files, &arguments, max_bytes);
    }
    let names = if let Some(path) = &arguments.names {
        match fs::read(path)
            .map_err(Error::from)
            .and_then(|data| namesfile::parse_names(&data))
        {
            Ok(names) => Some(names),
            Err(error) => {
                eprintln!("Invalid --names file: {}", error.message);
                return 1;
            }
        }
    } else {
        None
    };
    let options = Options {
        max_bytes,
        verify: !arguments.no_verify,
        names,
        annotate: arguments.annotate,
        trace: arguments.trace,
        sandbox_only: arguments.sandbox,
        clean: !arguments.no_clean,
        trace_opts: if arguments.trace || arguments.sandbox {
            Some(
                json!({"place_id":arguments.place_id,"user_id":arguments.user_id,"username":arguments.username,"executor":arguments.executor,"other_players":arguments.players,"responses":responses,"explore":arguments.explore,"max_time":arguments.sandbox_time,"timeout":arguments.sandbox_timeout}),
            )
        } else {
            None
        },
        ..Options::default()
    };
    let batch = files.len() > 1 || arguments.input.iter().any(|path| path.is_dir());
    let directory = if let Some(output) = &arguments.output {
        if batch && output.exists() && !output.is_dir() {
            eprintln!(
                "With several files or a folder, -o must be a folder ({} is a file)",
                output.display()
            );
            return 1;
        }
        if batch
            || output.is_dir()
            || output
                .as_os_str()
                .to_string_lossy()
                .ends_with(['/', std::path::MAIN_SEPARATOR])
        {
            Some(output.clone())
        } else {
            None
        }
    } else {
        None
    };
    if let Some(directory) = &directory {
        if let Err(error) = fs::create_dir_all(directory) {
            eprintln!("Cannot create the output folder: {error}");
            return 1;
        }
    }
    let taken = Mutex::new(
        files
            .iter()
            .filter(|path| path.as_path() != Path::new("-"))
            .map(|path| path_key(path))
            .collect(),
    );
    let start = Instant::now();
    if !batch {
        let note = |stage: &str| eprintln!("  ... {stage}");
        let progress: Progress<'_> = if arguments.quiet { None } else { Some(&note) };
        let result = process_file(
            &files[0],
            &options,
            &arguments,
            directory.as_deref(),
            &taken,
            progress,
        );
        if let Some(error) = result.error {
            if result.unreadable {
                eprintln!("{}", error.message);
                return 1;
            }
            eprintln!("FAILED: {}", messages::explain(&error.code, &error.message));
            return 2;
        }
        if let Some(info) = result.info {
            if arguments.json {
                print_json(&info);
            } else if !arguments.quiet {
                for line in messages::verdict(&info) {
                    println!("{line}");
                }
                let output = result.output.as_deref().unwrap_or(Path::new(""));
                println!(
                    "Done in {:.1} s -> {}",
                    start.elapsed().as_secs_f64(),
                    output.display()
                );
                for path in result.trace_files {
                    if path != output {
                        println!("  also written: {}", path.display());
                    }
                }
            }
        }
        return 0;
    }

    let count = files.len();
    let next = AtomicUsize::new(0);
    let mut results: Vec<Option<FileResult>> = (0..count).map(|_| None).collect();
    std::thread::scope(|scope| {
        let (send, receive) = mpsc::channel();
        for _ in 0..arguments.jobs.min(count) {
            let sender = send.clone();
            let files = &files;
            let next = &next;
            let options = &options;
            let arguments = &arguments;
            let directory = directory.as_deref();
            let taken = &taken;
            scope.spawn(move || loop {
                let index = next.fetch_add(1, Ordering::Relaxed);
                let Some(input) = files.get(index) else {
                    break;
                };
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    process_file(input, options, arguments, directory, taken, None)
                }))
                .unwrap_or_else(|_| FileResult {
                    file: input.clone(),
                    output: None,
                    info: None,
                    error: Some(Error::new("internal", "internal file-processing failure")),
                    unreadable: false,
                    seconds: 0.0,
                    trace_files: Vec::new(),
                });
                if sender.send((index, result)).is_err() {
                    break;
                }
            });
        }
        drop(send);
        for (shown, (index, result)) in receive.into_iter().enumerate() {
            if let Some(error) = &result.error {
                eprintln!(
                    "[{}/{}] FAILED: {}: {}",
                    shown + 1,
                    count,
                    result.file.display(),
                    if result.unreadable {
                        error.message.clone()
                    } else {
                        messages::explain(&error.code, &error.message)
                    }
                );
            } else if !arguments.quiet && !arguments.json {
                let info = result.info.as_ref().unwrap_or(&Value::Null);
                println!(
                    "[{}/{}] {} -> {}  {}  ({:.1} s)",
                    shown + 1,
                    count,
                    result
                        .file
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy(),
                    result
                        .output
                        .as_ref()
                        .and_then(|path| path.file_name())
                        .unwrap_or_default()
                        .to_string_lossy(),
                    short_summary(info),
                    result.seconds
                );
                for warning in info["warnings"].as_array().into_iter().flatten() {
                    println!("      Note: {}", warning.as_str().unwrap_or(""));
                }
            }
            results[index] = Some(result);
        }
    });
    let failed = results
        .iter()
        .filter(|result| result.as_ref().is_none_or(|result| result.error.is_some()))
        .count();
    if arguments.json {
        let rows=results.iter().flatten().map(|result|json!({"file":result.file.to_string_lossy(),"ok":result.error.is_none(),"output":result.output.as_ref().map(|path|path.to_string_lossy()),"error":result.error.as_ref().map(|error|messages::explain(&error.code,&error.message)),"code":result.error.as_ref().map(|error|error.code.as_str()),"info":result.info})).collect();
        print_json(&Value::Array(rows));
    } else if !arguments.quiet {
        println!(
            "Done: {} of {} files in {:.1} s{}; results {}",
            count - failed,
            count,
            start.elapsed().as_secs_f64(),
            if failed > 0 {
                format!(" ({failed} failed)")
            } else {
                String::new()
            },
            directory
                .as_ref()
                .map(|path| format!("are in {}", path.display()))
                .unwrap_or_else(|| "are next to each input file".into())
        );
    }
    if failed > 0 {
        2
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semantic_proof_summary() {
        let info = json!({"verification":{"raw_syntax":"passed","delivered_syntax":"passed","raw_bytecode":"differences found","raw_semantic":"passed","raw_applies_to_delivery":true},"fidelity":{"functions":2,"functions_recompiled":2,"identical":1,"reordered":0,"cosmetic":0,"semantic_equivalent":1,"differing":[]}});
        assert_eq!(
            short_summary(&info),
            "RAW FIDELITY PASSED WITH PROOFS; 1/2 functions identical (1 proven equivalent)"
        );
    }

    #[test]
    fn incompatible_modes_remain_errors() {
        for args in [
            vec!["bgdeobf", "fixture.lua", "--jobs", "0"],
            vec!["bgdeobf", "fixture.lua", "--trace", "--sandbox"],
            vec!["bgdeobf", "fixture.lua", "--sandbox", "--annotate"],
            vec!["bgdeobf", "fixture.lua", "--jobs", "2", "--in-process"],
            vec!["bgdeobf", "fixture.lua", "--explore"],
        ] {
            let arguments = Arguments::try_parse_from(args).unwrap();
            assert!(argument_error(&arguments).is_some());
        }
    }

    #[test]
    fn explicit_debug_mode_is_retained_without_becoming_the_default() {
        let normal = Arguments::try_parse_from(["bgdeobf", "fixture.lua"]).unwrap();
        assert!(!normal.in_process);
        let debugging =
            Arguments::try_parse_from(["bgdeobf", "fixture.lua", "--in-process"]).unwrap();
        assert!(debugging.in_process && argument_error(&debugging).is_none());
    }

    #[test]
    fn batch_outputs_never_replace_named_inputs() {
        let directory = tempfile::tempdir().unwrap();
        let input = directory.path().join("x.lua");
        let protected = directory.path().join("x.deobf.lua");
        fs::write(&input, b"fixture").unwrap();
        fs::write(&protected, b"keep").unwrap();
        let arguments = Arguments::try_parse_from(["bgdeobf", "x.lua"]).unwrap();
        let taken = Mutex::new(
            [path_key(&input), path_key(&protected)]
                .into_iter()
                .collect(),
        );
        let path = output_path(&input, &json!({}), &arguments, None, &taken).unwrap();
        assert_eq!(path.file_name().unwrap(), "x_2.deobf.lua");
        assert_eq!(fs::read(protected).unwrap(), b"keep");
    }
}

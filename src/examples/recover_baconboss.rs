//! Restartable analysis of a supplied sample; never runs its application instructions.
use bgdeobf::{
    astutil, classify, clean, decode, dump, engine, fidelity, loader, patch, refspec, render,
    sandbox, source_cleanup,
};
use serde_json::{json, Value};
use std::{
    env,
    error::Error,
    fs,
    path::PathBuf,
    time::{Duration, Instant},
};

fn main() -> Result<(), Box<dyn Error>> {
    run(&env::args_os().collect::<Vec<_>>())
}

fn run(args: &[std::ffi::OsString]) -> Result<(), Box<dyn Error>> {
    assert!(
        args.len() >= 4,
        "recover_baconboss <unpack|probe|recover|verify|compare-saved|partial> <input> <artifacts>"
    );
    let artifacts = PathBuf::from(&args[3]);
    fs::create_dir_all(&artifacts)?;
    if args[1] == "compare-saved" {
        let source = astutil::normalize_source(&fs::read(&args[2])?);
        let program = serde_json::from_slice(&fs::read(artifacts.join("program.json"))?)?;
        let compiled: Value =
            serde_json::from_slice(&fs::read(artifacts.join("raw-compile.json"))?)?;
        if compiled["ok"] != true {
            return Err("saved compile did not pass".into());
        }
        let report = fidelity::compare_for_source(
            &program,
            compiled["bytecode"]
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or("missing saved compiler listing")?,
            &source,
        )?;
        let output = fs::File::create_new(artifacts.join("fidelity.json"))?;
        serde_json::to_writer_pretty(output, &report)?;
        return Ok(());
    }
    if args[1] == "unpack" {
        let source = astutil::normalize_source(&fs::read(&args[2])?);
        let layout = loader::analyze(&source)?;
        fs::write(artifacts.join("loader.json"), serde_json::to_vec(&layout)?)?;
        eprintln!("loader saved");
        let script = patch::build_dump_script(&layout)?;
        fs::write(artifacts.join("instrumented-loader.lua"), &script)?;
        let bytes = sandbox::run_luau(&script, Duration::from_secs(45), 1024, 128)?;
        fs::write(artifacts.join("vm-dump.txt"), &bytes)?;
        let image = dump::parse_dump(&astutil::latin1(&bytes))?;
        assert!(!image["protos"]
            .as_object()
            .ok_or("missing prototypes")?
            .is_empty());
        fs::write(artifacts.join("image.json"), serde_json::to_vec(&image)?)?;
        eprintln!(
            "image saved: {} prototypes",
            image["protos"].as_object().unwrap().len()
        );
    } else if args[1] == "partial" {
        let raw = fs::read_to_string(artifacts.join("RzbWcraN6v.raw.lua"))?;
        let report: Value = serde_json::from_slice(&fs::read(artifacts.join("fidelity.json"))?)?;
        let cosmetic = report["cosmetic"]
            .as_array()
            .ok_or("missing cosmetic results")?;
        let proved = if report.get("semantic_equivalent").is_some()
            || report.get("semantic_unresolved").is_some()
        {
            let program: Value =
                serde_json::from_slice(&fs::read(artifacts.join("program.json"))?)?;
            let compiled: Value =
                serde_json::from_slice(&fs::read(artifacts.join("raw-compile.json"))?)?;
            if compiled["ok"] != true {
                return Err("saved compile did not pass".into());
            }
            fidelity::semantic_ids(
                &program,
                compiled["bytecode"]
                    .as_str()
                    .ok_or("missing saved compiler listing")?,
                &report,
            )
            .ok_or("saved report does not replay from the supplied artifacts")?
        } else {
            Default::default()
        };
        let unresolved: Vec<_> = report["differing"]
            .as_array()
            .ok_or("missing differences")?
            .iter()
            .filter(|id| {
                !cosmetic.contains(id) && !id.as_u64().is_some_and(|n| proved.contains(&n))
            })
            .collect();
        let (cleaned, stats) = clean::clean_source(&raw, Duration::from_secs(30), 512, false)?;
        fs::write(artifacts.join("cleanup-intermediate.lua"), &cleaned)?;
        fs::write(
            artifacts.join("cleanup-intermediate.json"),
            serde_json::to_vec_pretty(&stats)?,
        )?;
        let (cleaned, static_stats) = source_cleanup::clean_helpers_source(&cleaned, false)?;
        let (renamed, names) = source_cleanup::rename_locals_source(&cleaned)?;
        let delivered = format!(
            "-- PARTIAL DEOBFUSCATION: BaconBoss RzbWcraN6v / FORGE c92811be24ed.\n-- Syntax checked; raw fidelity: {} exact, {} reordered, {} cosmetic, {} proven equivalent, {} unresolved functions.\n-- Unresolved prototype IDs: {}. Protection checks retained; application not executed.\n-- Local names are generated. See partial-verification.json for limits.\n\n{}",
            report["identical"], report["reordered"], cosmetic.len(),proved.len(),unresolved.len(), json!(unresolved), renamed
        );
        let syntax = engine::compile_check(&delivered)?;
        let mut evidence = json!({"cleaned": stats, "static_cleaned": static_stats, "names": names, "syntax": syntax["ok"], "message": syntax["message"], "raw_fidelity": report, "fully_verified": false, "application_executed": false, "protection_removed": false});
        assert_eq!(syntax["ok"], true, "partial syntax: {}", syntax["message"]);
        let (again, _) = clean::clean_source(&renamed, Duration::from_secs(30), 512, false)?;
        let (again, _) = source_cleanup::clean_helpers_source(&again, false)?;
        evidence["cleanup_idempotent"] = json!(again == renamed);
        fs::write(artifacts.join("RzbWcraN6v.partial.deobf.lua"), delivered)?;
        fs::write(
            artifacts.join("partial-verification.json"),
            serde_json::to_vec_pretty(&evidence)?,
        )?;
        eprintln!("syntax-checked partial source saved; fidelity differences preserved");
    } else if args[1] == "recover" || args[1] == "verify" {
        let layout: Value = serde_json::from_slice(&fs::read(artifacts.join("loader.json"))?)?;
        assert_eq!(
            astutil::latin1(&astutil::normalize_source(&fs::read(&args[2])?)),
            layout["text"].as_str().ok_or("missing loader source")?
        );
        let program = if args[1] == "recover" {
            let image: Value = serde_json::from_slice(&fs::read(artifacts.join("image.json"))?)?;
            let mapping = classify::classify(&layout, &image, Duration::from_secs(240))?;
            fs::write(
                artifacts.join("mapping.json"),
                serde_json::to_vec_pretty(&mapping)?,
            )?;
            eprintln!("classification saved; unknowns: {}", mapping["unknown"]);
            let program = decode::decode_program(&image, &mapping, &layout)?;
            fs::write(
                artifacts.join("program.json"),
                serde_json::to_vec(&program)?,
            )?;
            eprintln!("IR saved");
            program
        } else {
            serde_json::from_slice(&fs::read(artifacts.join("program.json"))?)?
        };
        let rendered = render::render(&program, None, false)?;
        let raw = rendered["text"].as_str().ok_or("missing raw source")?;
        fs::write(artifacts.join("RzbWcraN6v.raw.lua"), raw)?;
        let compiled = engine::compile_check(raw)?;
        fs::write(
            artifacts.join("raw-compile.json"),
            serde_json::to_vec(&compiled)?,
        )?;
        assert_eq!(compiled["ok"], true, "raw syntax: {}", compiled["message"]);
        let report = fidelity::compare_for_source(
            &program,
            compiled["bytecode"]
                .as_str()
                .ok_or("missing compiled bytecode")?,
            &astutil::normalize_source(&fs::read(&args[2])?),
        )?;
        fs::write(
            artifacts.join("fidelity.json"),
            serde_json::to_vec_pretty(&report)?,
        )?;
        eprintln!(
            "fidelity: {}",
            json!({"functions": report["functions_orig"], "new": report["functions_new"], "identical": report["identical"], "reordered": report["reordered"], "differing": report["differing"], "cosmetic": report["cosmetic"]})
        );
        let count = program["protos"]
            .as_object()
            .ok_or("missing IR prototypes")?
            .len();
        assert_eq!(report["functions_orig"].as_u64(), Some(count as u64));
        assert_eq!(report["functions_new"].as_u64(), Some(count as u64));
        let matched = report["match"]
            .as_object()
            .ok_or("missing fidelity mapping")?;
        assert_eq!(matched.len(), count);
        assert!(program["protos"]
            .as_object()
            .unwrap()
            .keys()
            .all(|key| matched.contains_key(key)));
        let targets: std::collections::BTreeSet<_> =
            matched.values().map(Value::to_string).collect();
        assert_eq!(targets.len(), count);
        let cosmetic = report["cosmetic"]
            .as_array()
            .ok_or("missing cosmetic results")?;
        assert!(
            report["differing"]
                .as_array()
                .ok_or("missing differing results")?
                .iter()
                .all(|id| cosmetic.contains(id)),
            "raw bytecode differs; inspect fidelity.json"
        );
        assert_eq!(
            report["identical"].as_u64().unwrap_or(0)
                + report["reordered"].as_u64().unwrap_or(0)
                + cosmetic.len() as u64,
            count as u64
        );
        let (cleaned, stats) = clean::clean_source(raw, Duration::from_secs(30), 512, true)?;
        let (renamed, names) = source_cleanup::rename_locals_source(&cleaned)?;
        let (delivered, protection) = source_cleanup::strip_protection_source(&renamed, true)?;
        let syntax = engine::compile_check(&delivered)?;
        fs::write(
            artifacts.join("cleanup.json"),
            serde_json::to_vec_pretty(
                &json!({"cleaned": stats, "names": names, "protection": protection, "syntax": syntax["ok"], "message": syntax["message"]}),
            )?,
        )?;
        assert_eq!(
            syntax["ok"], true,
            "delivered syntax: {}",
            syntax["message"]
        );
        fs::write(artifacts.join("RzbWcraN6v.cleaned.lua"), delivered)?;
        eprintln!("verified raw source and syntax-checked cleaned source saved");
    } else {
        assert_eq!(args[1], "probe");
        let layout: Value = serde_json::from_slice(&fs::read(artifacts.join("loader.json"))?)?;
        let image: Value = serde_json::from_slice(&fs::read(artifacts.join("image.json"))?)?;
        let domains = classify::collect_domains(&layout, &image)?;
        eprintln!(
            "domains: {}; seed tables: {}",
            domains.len(),
            layout["simple_table_items"].as_array().map_or(0, Vec::len)
        );
        for table in layout["simple_table_items"]
            .as_array()
            .into_iter()
            .flatten()
        {
            eprintln!(
                "seed table {}: {} entries, {} JSON bytes",
                table[0],
                table[1].as_array().map_or(0, Vec::len),
                serde_json::to_vec(table)?.len()
            );
        }
        let mut classifier = classify::Classifier::new(
            &layout,
            domains,
            Some(Instant::now() + Duration::from_secs(45)),
        );
        classifier.trials = 1; // Diagnostic timings only; not an accepted opcode mapping.
        for opcode in image["simple"]
            .as_array()
            .ok_or("missing simple opcode list")?
        {
            let opcode = classify::as_int(opcode)?;
            for spec in refspec::specs() {
                let started = Instant::now();
                eprintln!("try simple {opcode} / {spec}");
                let matched = classifier.matches(
                    &classify::Runner::Simple(opcode),
                    &spec,
                    None,
                    None,
                    true,
                )?;
                eprintln!("=> {matched}, {:.3}s", started.elapsed().as_secs_f64());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn semantic_saved_comparison_stops_before_cleanup() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("fixture.lua");
        fs::write(&input, "return").unwrap();
        let p = json!({"main":0,"protos":{"0":{"entry":0,"np":0,"nup":0,"va":false,"kids":[],"consts":{},"decoded":{"0":{"kind":"RETURN","ins":[],"ctrl":["RETURN",{"A":0,"B":1,"C":0,"rf":0,"aux":0}],"nxt":4294967295u64,"alt":4294967295u64}}}}});
        fs::write(dir.path().join("program.json"), p.to_string()).unwrap();
        let path = dir.path().join("raw-compile.json");
        let bc = "Function 0 (??):\nRETURN R0 0\n";
        fs::write(&path, json!({"ok":true,"bytecode":bc}).to_string()).unwrap();
        let args = [
            "example".into(),
            "compare-saved".into(),
            input.into_os_string(),
            dir.path().into(),
        ];
        run(&args).unwrap();
        let output = dir.path().join("fidelity.json");
        assert_eq!(
            serde_json::from_slice::<Value>(&fs::read(&output).unwrap()).unwrap(),
            fidelity::compare(&p, bc).unwrap()
        );
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 4);
        fs::remove_file(&output).unwrap();
        for bad in [
            json!({"ok":false,"bytecode":bc}),
            json!({"ok":true,"bytecode":null}),
        ] {
            fs::write(&path, bad.to_string()).unwrap();
            assert!(run(&args).is_err());
            assert!(!output.exists());
        }
    }
}

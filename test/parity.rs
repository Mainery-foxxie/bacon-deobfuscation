use bgdeobf::{astutil, classify, decode, dump, fidelity, loader, patch, render, sandbox};
use serde_json::Value;
use std::collections::BTreeMap;
use std::time::Duration;

fn reference() -> Value {
    serde_json::from_str(include_str!("fixtures/safe_vm_reference.json")).unwrap()
}

#[test]
fn native_layout_matches_the_independent_python_reference() {
    let expected = reference();
    let actual = loader::analyze(include_bytes!("samples/safe_vm.lua")).unwrap();
    for (key, value) in expected["layout"].as_object().unwrap() {
        assert_eq!(&actual[key], value, "loader field {key}");
    }
}

#[test]
fn native_unpack_classification_and_decoding_match_the_reference() {
    let expected = reference();
    let layout = loader::analyze(include_bytes!("samples/safe_vm.lua")).unwrap();
    let script = patch::build_dump_script(&layout).unwrap();
    let dumped = sandbox::run_luau(&script, Duration::from_secs(60), 1024, 128).unwrap();
    let image = dump::parse_dump(&astutil::latin1(&dumped)).unwrap();
    assert_eq!(image, expected["dump"], "unpacked image");
    let mapping = classify::classify(&layout, &image, Duration::from_secs(120)).unwrap();
    assert_eq!(mapping, expected["mapping"], "behavioral opcode mapping");
    let program = decode::decode_program(&image, &mapping, &layout).unwrap();
    assert_eq!(program, expected["program"], "complete normalized program");
}

#[test]
fn native_raw_annotated_and_mapped_renders_match_reference() {
    let expected = reference();
    let program = &expected["program"];
    let raw = render::render(program, None, false).unwrap();
    assert_eq!(raw["text"], expected["renders"]["raw"]);
    let annotated = render::render(program, None, true).unwrap();
    assert_eq!(annotated["text"], expected["renders"]["annotated"]);
    let names: BTreeMap<String, String> =
        serde_json::from_value(expected["renders"]["names"].clone()).unwrap();
    let named = render::render(program, Some(&names), false).unwrap();
    assert_eq!(named["text"], expected["renders"]["named"]);
}

#[test]
fn native_fidelity_matches_independently_compiled_bytecode() {
    let expected = reference();
    let scoped = fidelity::compare_for_source(
        &expected["program"],
        expected["bytecode"].as_str().unwrap(),
        include_bytes!("samples/safe_vm.lua"),
    )
    .unwrap();
    assert_eq!(scoped, expected["fidelity"]);
    let report =
        fidelity::compare(&expected["program"], expected["bytecode"].as_str().unwrap()).unwrap();
    for key in [
        "functions_orig",
        "functions_new",
        "identical",
        "reordered",
        "differing",
        "reordered_protos",
        "cosmetic",
        "dead_loads",
        "match",
    ] {
        assert_eq!(
            report[key], expected["fidelity"][key],
            "fidelity field {key}"
        );
    }
}

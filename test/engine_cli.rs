use bgdeobf::engine;
use bgdeobf::model::Options;
use serde_json::json;

#[test]
fn empty_inputs_fail_before_starting_luau() {
    for input in [b"".as_slice(), b" \r\n\t".as_slice()] {
        let error = engine::run(input, &Options::default(), None).unwrap_err();
        assert_eq!(error.code, "empty");
    }
}

#[test]
fn oversized_and_binary_inputs_keep_distinct_error_codes() {
    let options = Options {
        max_bytes: 2,
        ..Options::default()
    };
    assert_eq!(
        engine::run(b"abc", &options, None).unwrap_err().code,
        "too_big"
    );
    assert_eq!(
        engine::run(b"x\0x", &Options::default(), None)
            .unwrap_err()
            .code,
        "binary"
    );
}

#[test]
fn detection_rejects_invalid_inputs_without_claiming_support() {
    for input in [b"".as_slice(), b"x\0x".as_slice(), b"abc".as_slice()] {
        let result = engine::detect(input, 2).unwrap();
        assert_eq!(result["supported"], false);
        assert_eq!(result["confidence"], json!(0.0));
    }
}

#[test]
fn moonveil_detection_is_explicitly_behavior_only() {
    let result = engine::detect(
        b"-- This script was generated using MoonVeil v3\nprint('fixture')",
        1024,
    )
    .unwrap();
    assert_eq!(result["kind"], "moonveil");
    assert_eq!(result["supported"], true);
    assert!(result["detail"]
        .as_str()
        .unwrap()
        .contains("reconstruction"));
}

#[test]
fn compile_check_distinguishes_valid_source_from_syntax_errors() {
    let valid = engine::compile_check("return 41\n").unwrap();
    assert_eq!(valid["ok"], true);
    assert!(valid["bytecode"].as_str().unwrap().contains("RETURN"));
    let invalid = engine::compile_check("local =\n").unwrap();
    assert_eq!(invalid["ok"], false);
    assert!(invalid["bytecode"].is_null());
}

#[test]
fn safe_vm_fixture_decompiles_with_complete_raw_verification() {
    let source = include_bytes!("samples/safe_vm.lua");
    let output = engine::run(source, &Options::default(), None).unwrap();
    let text = output.text.as_deref().unwrap();
    assert!(text.contains("safe fixture"));
    assert_eq!(output.info["verification"]["raw_syntax"], "passed");
    assert_eq!(output.info["verification"]["raw_bytecode"], "passed");
    assert_eq!(output.info["verification"]["delivered_syntax"], "passed");
}

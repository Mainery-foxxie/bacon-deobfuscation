use bgdeobf::error::Error;
use bgdeobf::model::{decode_number, encode_number, Options, Output};
use serde_json::json;

#[test]
fn engine_options_retain_reference_defaults() {
    let options = Options::default();
    assert_eq!(options.max_bytes, 3 * 1024 * 1024);
    assert_eq!(options.sandbox_timeout, 60);
    assert_eq!(options.sandbox_mem_mb, 1024);
    assert_eq!(options.classify_timeout, 120);
    assert!(options.verify && options.clean);
    assert!(!options.annotate && !options.trace && !options.sandbox_only && !options.moonveil);
    assert!(options.names.is_none() && options.trace_opts.is_none());
}

#[test]
fn worker_options_and_results_round_trip_without_losing_bytes() {
    let options: Options = serde_json::from_value(json!({"verify": false, "trace": true})).unwrap();
    assert!(!options.verify && options.trace);
    assert_eq!(options.max_bytes, 3 * 1024 * 1024);
    let output = Output {
        text: Some("return 41\n".into()),
        info: json!({"verification":{"raw_bytecode":"incomplete"}}),
        trace: Some(json!({"payloads":[{"data":[0,128,255]}]})),
        raw: None,
    };
    let recovered: Output = serde_json::from_slice(&serde_json::to_vec(&output).unwrap()).unwrap();
    assert_eq!(recovered.text, output.text);
    assert_eq!(recovered.trace, output.trace);
    assert_eq!(recovered.info, output.info);
}

#[test]
fn errors_keep_machine_code_separate_from_display_text() {
    let error = Error::new("unsupported", "handler 74 is not understood");
    assert_eq!(error.code, "unsupported");
    assert_eq!(error.message, "handler 74 is not understood");
    assert_eq!(error.to_string(), error.message);
}

#[test]
fn numeric_constant_transport_preserves_nonfinite_values_and_negative_zero() {
    assert!(decode_number(&encode_number(f64::NAN)).unwrap().is_nan());
    assert_eq!(
        decode_number(&encode_number(f64::INFINITY)).unwrap(),
        f64::INFINITY
    );
    assert_eq!(
        decode_number(&encode_number(f64::NEG_INFINITY)).unwrap(),
        f64::NEG_INFINITY
    );
    let negative_zero = decode_number(&encode_number(-0.0)).unwrap();
    assert!(negative_zero == 0.0 && negative_zero.is_sign_negative());
    assert_eq!(encode_number(41.0), serde_json::json!(41.0));
    assert!(decode_number(&serde_json::json!("ordinary string")).is_err());
}

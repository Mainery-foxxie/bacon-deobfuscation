use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

use crate::error::{Error, Result};

pub type Progress<'a> = Option<&'a (dyn Fn(&str) + Send + Sync)>;

pub fn encode_number(number: f64) -> Value {
    if number.is_nan() {
        Value::String("nan".into())
    } else if number == f64::INFINITY {
        Value::String("inf".into())
    } else if number == f64::NEG_INFINITY {
        Value::String("-inf".into())
    } else {
        Value::Number(serde_json::Number::from_f64(number).expect("finite numeric constant"))
    }
}

pub fn decode_number(value: &Value) -> Result<f64> {
    value
        .as_f64()
        .or_else(|| match value.as_str() {
            Some("nan") => Some(f64::NAN),
            Some("inf") => Some(f64::INFINITY),
            Some("-inf") => Some(f64::NEG_INFINITY),
            _ => None,
        })
        .ok_or_else(|| Error::new("decode", "invalid numeric constant"))
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct Options {
    pub max_bytes: usize,
    pub sandbox_timeout: u64,
    pub sandbox_mem_mb: u64,
    pub classify_timeout: u64,
    pub verify: bool,
    pub names: Option<BTreeMap<String, String>>,
    pub annotate: bool,
    pub trace: bool,
    pub sandbox_only: bool,
    pub trace_opts: Option<Value>,
    pub moonveil: bool,
    pub clean: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            max_bytes: 3 * 1024 * 1024,
            sandbox_timeout: 60,
            sandbox_mem_mb: 1024,
            classify_timeout: 120,
            verify: true,
            names: None,
            annotate: false,
            trace: false,
            sandbox_only: false,
            trace_opts: None,
            moonveil: false,
            clean: true,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Output {
    pub text: Option<String>,
    pub info: Value,
    pub trace: Option<Value>,
    #[serde(default)]
    pub raw: Option<String>,
}

//! Fixed JSON render boundary for the native graph decompiler.
use crate::error::Result;
use crate::gen::Generator;
use crate::ir::{err, integer, Proto};
use crate::printer::{Printer, Scope};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

pub fn render(
    program: &Value,
    names: Option<&BTreeMap<String, String>>,
    annotate: bool,
) -> Result<Value> {
    let mut protos = BTreeMap::new();
    for (pid, p) in program["protos"]
        .as_object()
        .ok_or_else(|| err("missing program prototypes"))?
    {
        protos.insert(
            pid.parse().map_err(|_| err("invalid prototype id"))?,
            Proto::parse(p)?,
        );
    }
    let main = integer(&program["main"], "main prototype")?;
    let mut reserved = BTreeSet::new();
    for p in protos.values() {
        for (_, d) in &p.decoded {
            for (k, a) in &d.ins {
                if k == "GETIMPORT" {
                    reserved.insert(p.kbytes((a.aux >> 20) & 1023)?);
                }
            }
        }
    }
    let mut gen = Generator::new(protos);
    let nup = gen
        .ctx
        .protos
        .get(&main)
        .ok_or_else(|| err("missing main prototype"))?
        .nup;
    let upvals = (0..nup).map(|_| gen.ctx.web("var", None, -1)).collect();
    let root = gen.run(main, upvals)?;
    let mut printer = Printer::new(gen, reserved, names.cloned().unwrap_or_default(), annotate);
    let lines = printer.block(&root.body, &Scope::new(None, false), "")?;
    Ok(json!({"text":lines.join("\n")+"\n","log":printer.gen.log}))
}

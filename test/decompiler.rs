//! Native graph/render regressions. Path modules deliberately permit testing before parent integration.
#![allow(dead_code)]
#[path = "../src/decomp.rs"]
mod decomp;
#[path = "../src/error.rs"]
mod error;
#[path = "../src/fidelity.rs"]
mod fidelity;
#[path = "../src/gen.rs"]
mod gen;
#[path = "../src/ir.rs"]
mod ir;
#[path = "../src/model.rs"]
mod model;
#[path = "../src/post.rs"]
mod post;
#[path = "../src/printer.rs"]
mod printer;
#[path = "../src/render.rs"]
mod render;
use fidelity::proofs::compare_scoped;
use serde_json::{json, Value};
use std::collections::BTreeMap;

const NONE: u64 = 4294967295;
fn i(op: &str, a: Value, to: Option<usize>) -> Value {
    json!([op, a, to])
}
fn s(text: &str) -> Value {
    json!(["s", text.as_bytes()])
}
fn num(n: i64) -> Value {
    json!(["n", n])
}
fn imp(ids: &[u64]) -> u64 {
    ids.iter()
        .enumerate()
        .fold((ids.len() as u64) << 30, |v, (i, k)| {
            v | (k << (20 - 10 * i))
        })
}
fn proto(code: Vec<Value>, consts: Vec<Value>, np: usize, nup: usize, kids: Vec<u64>) -> Value {
    let mut decoded = serde_json::Map::new();
    for (rid, ins) in code.iter().enumerate() {
        let op = ins[0].as_str().unwrap();
        let mut a = json!({"A":0,"B":0,"C":0,"rf":0,"Hf":0,"aux":0});
        for (k, v) in ins[1].as_object().unwrap() {
            a[k] = v.clone();
        }
        let mut d =
            json!({"kind":op,"sem":0,"ins":[],"ctrl":null,"nxt":rid+1,"alt":NONE,"rid":rid});
        if op == "RETURN" {
            d["ctrl"] = json!([op, a]);
            d["nxt"] = json!(NONE);
        } else if decomp::is_cond(op) || matches!(op, "JUMP" | "FORNPREP" | "FORNLOOP" | "FORGLOOP")
        {
            d["ctrl"] = json!([op, a]);
            d["alt"] = ins[2].clone();
        } else if op == "FORGPREP" {
            d["ins"] = json!([[op, a.clone()]]);
            d["ctrl"] = json!(["JUMP", a]);
            d["alt"] = ins[2].clone();
        } else {
            d["ins"] = json!([[op, a]]);
        }
        decoded.insert(rid.to_string(), d);
    }
    let consts: serde_json::Map<String, Value> = consts
        .into_iter()
        .enumerate()
        .map(|(i, v)| (i.to_string(), v))
        .collect();
    json!({"entry":0,"np":np,"nup":nup,"va":false,"kids":kids,"consts":consts,"decoded":decoded})
}
fn program(protos: Vec<Value>) -> Value {
    let ps: serde_json::Map<String, Value> = protos
        .into_iter()
        .enumerate()
        .map(|(i, v)| (i.to_string(), v))
        .collect();
    json!({"main":0,"protos":ps})
}
fn output(p: &Value) -> String {
    let text = syntax_output(p);
    let report = fidelity::compare(p, &compile(&text)).unwrap();
    assert_eq!(
        report["differing"],
        json!([]),
        "{}\n{text}",
        report["report"]
    );
    assert_eq!(
        report["functions_orig"], report["functions_new"],
        "{report}"
    );
    text
}
fn syntax_output(p: &Value) -> String {
    let text = render::render(p, None, false).unwrap()["text"]
        .as_str()
        .unwrap()
        .to_owned();
    compile(&text);
    text
}
fn identical_output(p: &Value) -> String {
    let text = output(p);
    let report = fidelity::compare(p, &compile(&text)).unwrap();
    assert_eq!(
        report["identical"], report["functions_orig"],
        "{report}\n{text}"
    );
    text
}
fn main_code(code: Vec<Value>, cs: Vec<Value>) -> Value {
    program(vec![proto(code, cs, 0, 0, vec![])])
}

#[test]
fn noop_is_omitted() {
    let p = main_code(
        vec![
            i("NOP", json!({}), None),
            i("LOADN", json!({"A":0,"rf":7}), None),
            i("RETURN", json!({"A":0,"B":2}), None),
        ],
        vec![],
    );
    let text = output(&p);
    assert_eq!(text, "return 7\n");
    assert!(!compile(&text).contains("NOP"));
}
#[test]
fn fidelity_accepts_integral_lua_number_closure_ids_without_truncation() {
    let mut p = program(vec![
        proto(
            vec![
                i("DUPCLOSURE", json!({"rf":0}), None),
                i("RETURN", json!({"B":2}), None),
            ],
            vec![json!(["n", 1.0])],
            0,
            0,
            vec![],
        ),
        proto(
            vec![
                i("LOADN", json!({"rf":7}), None),
                i("RETURN", json!({"B":2}), None),
            ],
            vec![],
            0,
            0,
            vec![],
        ),
    ]);
    let compiled = compile(&syntax_output(&p));
    let report = fidelity::compare(&p, &compiled).unwrap();
    assert_eq!(report["functions_orig"], 2);
    assert_eq!(report["identical"], 2);
    assert_eq!(report["differing"], json!([]));
    for invalid in [json!(1.5), json!(-1.0), json!("inf")] {
        p["protos"]["0"]["consts"]["0"][1] = invalid;
        assert_eq!(
            fidelity::compare(&p, &compiled).unwrap_err().code,
            "fidelity"
        );
    }
}

#[test]
fn readable_parameters() {
    let p = program(vec![
        proto(
            vec![
                i("DUPCLOSURE", json!({"rf":0}), None),
                i("RETURN", json!({"B":2}), None),
            ],
            vec![num(1)],
            0,
            0,
            vec![],
        ),
        proto(
            vec![
                i("ADD", json!({"A":2,"B":0,"C":1}), None),
                i("RETURN", json!({"A":2,"B":2}), None),
            ],
            vec![],
            2,
            0,
            vec![],
        ),
    ]);
    assert_eq!(
        output(&p),
        "return function(argument, argument2)\n\treturn argument + argument2\nend\n"
    );
}
#[test]
fn descriptive_names_annotations_and_user_mapping() {
    let p = main_code(
        vec![
            i("NEWTABLE", json!({}), None),
            i("GETTABLEKS", json!({"A":1,"B":0,"aux":0}), None),
            i("SETTABLEKS", json!({"A":1,"B":0,"aux":1}), None),
            i("SETTABLEKS", json!({"A":1,"B":0,"aux":2}), None),
            i("RETURN", json!({"B":1}), None),
        ],
        vec![s("Password"), s("A"), s("B")],
    );
    let text = output(&p);
    assert!(text.contains("local password = items.Password"), "{text}");
    assert!(
        text.contains(".A = password") && text.contains(".B = password"),
        "{text}"
    );
    let annotated = render::render(&p, None, true).unwrap()["text"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(annotated.contains("password<@0:1>"), "{annotated}");
    let names = BTreeMap::from([("0:1".into(), "chosenByUser".into())]);
    let renamed = render::render(&p, Some(&names), false).unwrap();
    assert!(renamed["text"]
        .as_str()
        .unwrap()
        .contains("local chosenByUser = items.Password"));
    let renamed = renamed["text"].as_str().unwrap();
    assert!(renamed.contains(".A = chosenByUser") && renamed.contains(".B = chosenByUser"));
    compile(renamed);
}

macro_rules! op {($k:expr $(,$key:ident=$v:expr)* $(,)?)=>{i($k,json!({$(stringify!($key):$v),*}),None)}}
macro_rules! jump {($k:expr,$to:expr $(,$key:ident=$v:expr)* $(,)?)=>{i($k,json!({$(stringify!($key):$v),*}),Some($to))}}
fn tool(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("lua_bin")
        .join(format!("{name}{}", std::env::consts::EXE_SUFFIX))
}
fn source_file(text: &str) -> tempfile::NamedTempFile {
    use std::io::Write;
    let mut f = tempfile::Builder::new().suffix(".luau").tempfile().unwrap();
    f.write_all(text.as_bytes()).unwrap();
    f.flush().unwrap();
    f
}
fn compile(text: &str) -> String {
    let f = source_file(text);
    let out = std::process::Command::new(tool("luau-compile"))
        .args(["--text", "-O1", "-g0", "--dump-constants"])
        .arg(f.path())
        .output()
        .expect("official Luau compiler required for native decompiler regressions");
    assert!(
        out.status.success(),
        "{}\n{text}",
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout.into_iter().map(char::from).collect()
}
fn run(text: &str) -> String {
    let f = source_file(text);
    let out = std::process::Command::new(tool("luau"))
        .arg(f.path())
        .output()
        .expect("official Luau runtime required");
    assert!(
        out.status.success(),
        "{}\n{text}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().replace("\r\n", "\n")
}
fn re(pat: &str, text: &str) {
    assert!(
        regex::Regex::new(pat).unwrap().is_match(text),
        "expected {pat}\n{text}"
    );
}
fn function(code: Vec<Value>, cs: Vec<Value>, np: usize) -> Value {
    program(vec![
        proto(
            vec![op!("DUPCLOSURE", rf = 0), op!("RETURN", B = 2)],
            vec![num(1)],
            0,
            0,
            vec![],
        ),
        proto(code, cs, np, 0, vec![]),
    ])
}
#[test]
fn independent_reference_fixture_raw_annotated_and_names() {
    let f: Value = serde_json::from_str(include_str!("fixtures/safe_vm_reference.json")).unwrap();
    let p = &f["program"];
    let raw = render::render(p, None, false).unwrap();
    assert_eq!(raw["text"], f["renders"]["raw"]);
    assert!(raw["log"].as_array().unwrap().is_empty());
    let bytecode = compile(raw["text"].as_str().unwrap());
    let report = fidelity::compare(p, &bytecode).unwrap();
    assert_eq!(report, f["fidelity"]);
    assert_eq!(
        fidelity::compare(p, f["bytecode"].as_str().unwrap()).unwrap(),
        f["fidelity"]
    );
    assert_eq!(report["identical"], 0);
    assert_eq!(report["reordered"], 1);
    assert_eq!(
        report["match"].as_object().unwrap().len(),
        p["protos"].as_object().unwrap().len()
    );
    assert_eq!(
        render::render(p, None, true).unwrap()["text"],
        f["renders"]["annotated"]
    );
    let names: BTreeMap<String, String> =
        serde_json::from_value(f["renders"]["names"].clone()).unwrap();
    let named = render::render(p, Some(&names), false).unwrap();
    assert_eq!(named["text"], f["renders"]["named"]);
    compile(named["text"].as_str().unwrap());
    assert_eq!(run(named["text"].as_str().unwrap()), "safe fixture\t41\n");
    assert_eq!(run(raw["text"].as_str().unwrap()), "safe fixture\t41\n");
}

fn captured_vararg_fixture() -> Value {
    let leaf = proto(
        vec![op!("GETUPVAL"), op!("RETURN", B = 2)],
        vec![],
        0,
        1,
        vec![],
    );
    let parent = proto(
        vec![
            op!("GETVARARGS", B = 2),
            op!("NEWCLOSURE", A = 1),
            op!("CAPTURE", A = 1),
            op!("CALL", A = 1, B = 1),
            op!("CLOSEUPVALS"),
            op!("RETURN", A = 1),
        ],
        vec![],
        0,
        0,
        vec![0],
    );
    let mut p = program(vec![leaf, parent]);
    p["main"] = json!(1);
    p["protos"]["1"]["va"] = json!(true);
    p
}

#[test]
fn review_closure_rejects_stale_signed_zero() {
    let p = captured_vararg_fixture();
    let source = "local value = ...\nreturn (function() return value end)()";
    assert_eq!(
        run(
            r#"local dup=assert(loadstring('local v=...; return (function() return v end)()'))
local fresh=assert(loadstring('local v=...; local f=function() return v end; v=...; return f()'))
local neg=-1/math.huge; print(1/dup(0),1/dup(neg)); print(1/fresh(0),1/fresh(neg))"#
        ),
        "inf\tinf\ninf\t-inf\n"
    );
    assert_eq!(
        compare_scoped(&p, &compile(source), &[1]).unwrap()["semantic_equivalent"],
        json!([])
    );
}

#[test]
fn closure_lowering_equivalence() {
    let mut p = captured_vararg_fixture();
    p["protos"]["1"]["decoded"]["0"]["ins"]
        .as_array_mut()
        .unwrap()
        .push(json!(["NOT",{"A":0,"B":0,"C":0,"rf":0,"aux":0}]));
    let bc =
        compile("local arg = ...\nlocal value = not arg\nreturn (function() return value end)()");
    assert_eq!(
        fidelity::compare_for_source(&p, &bc, b"self-authored fixture").unwrap(),
        fidelity::compare(&p, &bc).unwrap()
    );
    let report = compare_scoped(&p, &bc, &[1]).unwrap();
    assert_eq!(report["differing"], json!([1]));
    assert_eq!(
        report["semantic_equivalent"][0]["proofs"][0]["kind"], "closure-allocation",
        "{report}"
    );
    for variant in [
        bc.replace("CAPTURE VAL R1", "CAPTURE VAL R0"),
        bc.replace("RETURN R0 1", "SETUPVAL R0 U0\nRETURN R0 1"),
        bc.replace("CALL R2 0 -1", "RETURN R2 1"),
        bc.replace("CALL R2 0 -1", "SETTABLE R2 R0 R2\nCALL R2 0 -1"),
        bc.replace("CALL R2 0 -1", "LOADNIL R1\nCALL R2 0 -1"),
        bc.replace("RETURN R0 1", "CALL R0 0 1\nRETURN R0 1"),
        bc.replace("DUPCLOSURE R2 K0", "DUPCLOSURE R2 K7"),
        bc.replace("K0: function", "K0: 42"),
        bc.replace("CALL R2 0 -1", "JUMPIFEQ R0 R2 L0\nL0: CALL R2 0 -1"),
        format!("{bc}\nFunction 2 (??):\nGETUPVAL R0 U0\nRETURN R0 1\n"),
    ] {
        assert_eq!(
            compare_scoped(&p, &variant, &[1]).unwrap()["semantic_equivalent"],
            json!([]),
            "{variant}"
        );
    }
    let mut nested = p.clone();
    let arg = nested["protos"]["0"]["decoded"]["0"]["ins"][0][1].clone();
    nested["protos"]["0"]["decoded"]["0"]["ins"]
        .as_array_mut()
        .unwrap()
        .extend([json!(["LOADNIL", arg]), json!(["SETUPVAL", arg])]);
    assert_eq!(
        compare_scoped(&nested, &bc, &[1]).unwrap()["semantic_equivalent"],
        json!([])
    );
    let mut malformed = p.clone();
    malformed["protos"]["1"]["decoded"]["2"]["ins"][0][1]
        .as_object_mut()
        .unwrap()
        .remove("B");
    assert_eq!(
        compare_scoped(&malformed, &bc, &[1]).unwrap()["semantic_equivalent"],
        json!([])
    );
    let mut write = p.clone();
    write["protos"]["1"]["decoded"]["3"]["ins"]
        .as_array_mut()
        .unwrap()
        .insert(0, json!(["LOADNIL", arg]));
    assert_eq!(
        compare_scoped(&write, &bc, &[1]).unwrap()["semantic_equivalent"],
        json!([])
    );
    let mut missing = p;
    missing["protos"]["1"]["decoded"]
        .as_object_mut()
        .unwrap()
        .remove("2");
    assert!(compare_scoped(&missing, &bc, &[1]).is_err());
}

#[test]
fn false_seed_short_circuit_equivalence() {
    let mut p = main_code(
        vec![
            op!("GETVARARGS", B = 3),
            op!("LOADB", A = 2),
            jump!("JUMPIF", 6, A = 2),
            op!("MOVE", A = 3),
            op!("CALL", A = 3, B = 1, C = 2),
            op!("MOVE", A = 2, B = 3),
            jump!("JUMPIF", 10, A = 2),
            op!("MOVE", A = 3, B = 1),
            op!("CALL", A = 3, B = 1, C = 2),
            op!("MOVE", A = 2, B = 3),
            op!("RETURN", A = 2, B = 2),
        ],
        vec![],
    );
    p["protos"]["0"]["va"] = json!(true);
    let original =
        "local a,b=...; local v=false; if not v then v=a() end; if not v then v=b() end; return v";
    let chain = "local a,b=...; return a() or b()";
    let driver = r#"
local object={}
for _,mode in {'object','false','nil','throw'} do
    local trace=''
    local ok,value=pcall(test,function() trace..='a'; if mode=='object' then return object end; if mode=='throw' then error('first') end; return false end,
        function() trace..='b'; if mode=='object' then error('unreachable') end; if mode=='nil' then return nil end; return false end)
    print(trace, if not ok then 'error' elseif value==object then 'object' else tostring(value))
end"#;
    for src in [original, chain] {
        assert_eq!(
            run(&format!("local function test(...) {src} end\n{driver}")),
            "a\tobject\nab\tfalse\nab\tnil\na\terror\n"
        );
    }
    let bc = compile(chain);
    let report = compare_scoped(&p, &bc, &[0]).unwrap();
    assert_eq!(report["differing"], json!([0]));
    assert_eq!(
        report["semantic_equivalent"][0]["proofs"][0]["kind"], "false-seed-short-circuit",
        "{report}"
    );
    for bad in [
        "local a,b=...; return b() or a()",
        "local a,b=...; return a(1) or b()",
        "local a,b=...; local x,y=a(),b(); return x or y",
        "local a,b=...; return (a() or b()) or false",
        "local a,b=...; return a() or b(), 2",
        "local a,b=...; return not not (a() or b())",
    ] {
        assert_eq!(
            compare_scoped(&p, &compile(bad), &[0]).unwrap()["semantic_equivalent"],
            json!([]),
            "{bad}"
        );
    }
    let mut side_effect = p;
    side_effect["protos"]["0"]["decoded"]["1"]["ins"]
        .as_array_mut()
        .unwrap()
        .insert(0, json!(["CALL",{"A":0,"B":1,"C":1,"rf":0,"aux":0,"Hf":0}]));
    assert_eq!(
        compare_scoped(&side_effect, &bc, &[0]).unwrap()["semantic_equivalent"],
        json!([])
    );
}

fn nested_accumulator_fixture() -> (Value, String) {
    // A self-authored chunk with the observed region boundaries and different incoming cells.
    let code = vec![
        op!("GETUPVAL"),
        op!("CALL", B = 1, C = 2),
        op!("GETUPVAL", A = 3, B = 2),
        jump!("JUMPIFNOT", 14, A = 3),
        op!("LOADB", A = 3),
        jump!("JUMPIF", 9, A = 3),
        op!("GETUPVAL", A = 4, B = 1),
        op!("CALL", A = 4, B = 2, C = 2),
        op!("MOVE", A = 3, B = 4),
        jump!("JUMPIF", 13, A = 3),
        op!("GETUPVAL", A = 4, B = 1),
        op!("CALL", A = 4, B = 1, C = 2),
        op!("MOVE", A = 3, B = 4),
        jump!("JUMPIF", 14, A = 3),
        op!("RETURN", B = 1),
    ];
    let mut inner = proto(code, vec![], 0, 3, vec![]);
    let mv = json!(["MOVE",{"A":0,"B":0,"C":0,"rf":0,"aux":0}]);
    inner["decoded"]["1"]["ins"]
        .as_array_mut()
        .unwrap()
        .extend(vec![mv; 20]);
    inner["decoded"]["6"]["ins"]
        .as_array_mut()
        .unwrap()
        .push(json!(["MOVE",{"A":5,"B":0,"C":0,"rf":0,"aux":0}]));
    let d = inner["decoded"].as_object_mut().unwrap();
    for (old, new) in [(4, 304099439), (13, 304099927)] {
        let o = old.to_string();
        let mut record = d.remove(&o).unwrap();
        record["rid"] = json!(new);
        d.insert(new.to_string(), record);
        for other in d.values_mut() {
            for k in ["nxt", "alt"] {
                if other[k] == old {
                    other[k] = json!(new);
                }
            }
        }
    }
    let root = proto(
        vec![
            op!("NEWCLOSURE"),
            op!("CAPTURE", A = 2),
            op!("CAPTURE", A = 2, B = 1),
            op!("CAPTURE", A = 2),
            op!("RETURN", B = 2),
        ],
        vec![],
        0,
        2,
        vec![216],
    );
    let p = json!({"main":217,"protos":{"216":inner,"217":root}});
    let parent =
        "Function 217 (??):\nNEWCLOSURE R0 P0\nCAPTURE UPVAL U0\nCAPTURE UPVAL U1\nCAPTURE UPVAL U0\nRETURN R0 1\n";
    let listing=format!("Function 216 (??):\nGETUPVAL R0 U0\nCALL R0 0 1\n{}GETUPVAL R3 U2\nJUMPIFNOT R3 L1\nGETUPVAL R3 U1\nMOVE R4 R0\nCALL R3 1 1\nJUMPIF R3 L0\n{}GETUPVAL R3 U1\nCALL R3 0 1\nL0: JUMPIF R3 L1\nL1: RETURN R0 0\n{parent}","MOVE R0 R0\n".repeat(20),"MOVE R4 R0\n".repeat(27));
    (p, listing)
}

#[test]
fn false_seed_rejects_changed_incoming_binding() {
    let (p, listing) = nested_accumulator_fixture();
    let control = compare_scoped(&p, &listing, &[216]).unwrap();
    assert_eq!(
        control["semantic_unresolved"][0]["region_witness"]["compiler_pc"], 24,
        "{control}"
    );
    // The approved production witness requires the observed complete incoming value trace,
    // not mere token shape or the use of the same register numbers at the artificial boundary.
    let changed = listing.replacen("GETUPVAL R0 U0", "GETUPVAL R0 U1", 1);
    let rejected = compare_scoped(&p, &changed, &[216]).unwrap();
    assert_eq!(rejected["semantic_equivalent"], json!([]));
    assert!(
        rejected["semantic_unresolved"][0]
            .get("region_witness")
            .is_none(),
        "{rejected}"
    );
}

#[test]
fn review_rejects_swapped_grandparent_capture() {
    let (mut p, mut listing) = nested_accumulator_fixture();
    p["main"] = json!(218);
    p["protos"]["218"] = proto(
        vec![
            op!("GETVARARGS", B = 3),
            op!("NEWCLOSURE", A = 2),
            op!("CAPTURE"),
            op!("CAPTURE", B = 1),
            op!("RETURN", A = 2, B = 2),
        ],
        vec![],
        0,
        0,
        vec![217],
    );
    p["protos"]["218"]["va"] = json!(true);
    listing.push_str("Function 218 (??):\nGETVARARGS R0 2\nNEWCLOSURE R2 P0\nCAPTURE VAL R1\nCAPTURE VAL R0\nRETURN R2 1\n");
    assert_eq!(
        compare_scoped(&p, &listing, &[216]).unwrap()["semantic_equivalent"],
        json!([])
    );
}

#[test]
fn review_rejects_unchecked_suffix_call() {
    let (mut p, listing) = nested_accumulator_fixture();
    p["protos"]["216"]["decoded"]["14"]["ins"] = json!([["GETUPVAL",{"A":1,"B":0,"C":0,"rf":0,"aux":0}],["CALL",{"A":1,"B":1,"C":2,"rf":0,"aux":0}]]);
    let changed = listing.replace(
        "L1: RETURN R0 0",
        "L1: GETUPVAL R1 U1\nCALLFB R1 0 1 [0]\nRETURN R0 0",
    );
    assert_eq!(
        compare_scoped(&p, &changed, &[216]).unwrap()["semantic_equivalent"],
        json!([])
    );
}

#[test]
fn review_rejects_unknown_compiled_signature() {
    let (mut p, listing) = nested_accumulator_fixture();
    p["protos"]["216"]["np"] = json!(1);
    assert_eq!(
        compare_scoped(&p, &listing, &[216]).unwrap()["semantic_equivalent"],
        json!([])
    );
}

#[test]
fn guarded_library_alias() {
    let p = main_code(
        vec![
            op!("GETIMPORT", aux = imp(&[0])),
            jump!("JUMPIFNOT", 3),
            op!("GETIMPORT", aux = imp(&[0, 1])),
            op!("GETIMPORT", A = 1, aux = imp(&[2])),
            op!("MOVE", A = 2, B = 0),
            op!("CALL", A = 1, B = 2, C = 1),
            op!("GETIMPORT", A = 1, aux = imp(&[2])),
            op!("MOVE", A = 2, B = 0),
            op!("CALL", A = 1, B = 2, C = 1),
            op!("RETURN", B = 1),
        ],
        vec![s("buffer"), s("len"), s("print")],
    );
    assert!(output(&p).contains("local len = buffer and buffer.len"));
}
#[test]
fn concat_slot_reuse_is_not_assignment() {
    let p = main_code(
        vec![
            op!("NEWTABLE"),
            op!("GETTABLEKS", A = 1, B = 0, aux = 0),
            op!("GETTABLEKS", A = 2, B = 0, aux = 6),
            jump!("JUMPIFNOT", 6, A = 2),
            op!("LOADN", A = 5, rf = 100),
            op!("SETTABLEKS", A = 5, B = 0, aux = 5),
            op!("LOADK", A = 4, rf = 2),
            op!("SELF", A = 7, B = 1, aux = 1),
            op!("CALL", A = 7, B = 2, C = 2),
            op!("MOVE", A = 5, B = 7),
            op!("LOADK", A = 6, rf = 3),
            op!("CONCAT", A = 3, B = 4, C = 6),
            op!("SETTABLEKS", A = 3, B = 0, aux = 4),
            op!("RETURN", B = 1),
        ],
        vec![
            s("Product"),
            s("upper"),
            s("PREFIX "),
            s("!"),
            s("Status"),
            s("Count"),
            s("Flag"),
        ],
    );
    let text = output(&p);
    re(r#"\.Status = "PREFIX " \.\. \w+:upper\(\) \.\. "!""#, &text);
    assert!(!compile(&text).contains("SETGLOBAL"));
}
#[test]
fn real_register_copy_reassigns_existing_variable() {
    let p = main_code(
        vec![
            op!("NEWTABLE"),
            op!("GETTABLEKS", A = 1, B = 0, aux = 0),
            op!("GETTABLEKS", A = 2, B = 0, aux = 1),
            op!("GETTABLEKS", A = 3, B = 0, aux = 2),
            jump!("JUMPIFNOT", 6, A = 3),
            op!("MOVE", A = 1, B = 2),
            op!("SETTABLEKS", A = 1, B = 0, aux = 3),
            op!("RETURN", B = 1),
        ],
        vec![s("A"), s("B"), s("Flag"), s("Out")],
    );
    let text = output(&p);
    assert_eq!(
        text.lines().filter(|s| s.starts_with("local ")).count(),
        3,
        "{text}"
    );
    re(r"(?m)^\t\w+ = \w+$", &text);
    assert!(!compile(&text).contains("SETGLOBAL"));
}
#[test]
fn dead_getters_stay_but_constants_and_copies_drop() {
    let cs = vec![s("Key"), s("Stock"), num(0), s("print")];
    let p = main_code(
        vec![
            op!("NEWTABLE"),
            op!("GETTABLEKS", A = 1, B = 0),
            op!("GETTABLEKS", A = 3, B = 0, aux = 1),
            op!("GETTABLE", A = 2, B = 3, C = 1),
            op!("ORK", A = 2, B = 2, C = 2),
            op!("GETIMPORT", A = 3, aux = imp(&[3])),
            op!("MOVE", A = 4, B = 1),
            op!("CALL", A = 3, B = 2, C = 1),
            op!("RETURN", B = 1),
        ],
        cs,
    );
    re(r"(?m)^local _ = \w+\.Stock\[\w+\] or 0$", &output(&p));
    let p = main_code(
        vec![
            op!("NEWTABLE"),
            op!("GETTABLEKS", A = 1, B = 0),
            op!("LOADN", A = 2, rf = 7),
            op!("MOVE", A = 3, B = 1),
            op!("GETIMPORT", A = 2, aux = imp(&[1])),
            op!("MOVE", A = 3, B = 1),
            op!("CALL", A = 2, B = 2, C = 1),
            op!("RETURN", B = 1),
        ],
        vec![s("Key"), s("print")],
    );
    let text = output(&p);
    assert!(!text.contains("_ =") && !text.contains("= 7"));
}
#[test]
fn dead_fallback_in_reassigned_web() {
    let p = function(
        vec![
            jump!("JUMPIFNOT", 3, A = 0),
            op!("GETTABLEKS", A = 1, B = 0),
            jump!("JUMPIF", 4, A = 1),
            op!("LOADN", A = 1, rf = 3),
            op!("GETIMPORT", A = 2, aux = imp(&[1])),
            op!("MOVE", A = 3, B = 0),
            op!("CALL", A = 2, B = 2, C = 1),
            op!("GETTABLEKS", A = 2, B = 0),
            op!("MOVE", A = 1, B = 2),
            op!("RETURN", A = 1, B = 2),
        ],
        vec![s("Value"), s("print")],
        1,
    );
    let text = output(&p);
    assert_eq!(
        run(&format!(
            "local f=(function()\n{text}end)()\nprint(f({{Value=5}}))"
        ))
        .lines()
        .last(),
        Some("5")
    );
}

#[test]
fn branch_split_list_constructor() {
    let p = main_code(
        vec![
            op!("GETIMPORT", aux = imp(&[0])),
            op!("NEWTABLE", A = 1, aux = 3),
            op!("GETIMPORT", A = 2, aux = imp(&[1])),
            op!("LOADK", A = 3, rf = 2),
            op!("CALL", A = 2, B = 2, C = 2),
            op!("LOADK", A = 3, rf = 3),
            op!("GETIMPORT", A = 4, aux = imp(&[4])),
            jump!("JUMPIFNOT", 10, A = 4),
            op!("GETIMPORT", A = 4, aux = imp(&[4])),
            op!("CALL", A = 4, B = 1, C = 2),
            op!("SETLIST", A = 1, B = 2, C = 4, aux = 1),
            op!("CALL", A = 0, B = 2, C = 1),
            op!("RETURN", B = 1),
        ],
        vec![s("pairs"), s("tostring"), s("x"), s("str"), s("flag")],
    );
    let text = output(&p);
    re(r#"\{tostring\("x"\), "str", \w*flag and flag\(\)\}"#, &text);
    let bc = compile(&text);
    assert!(bc.contains("SETLIST") && !bc.contains("SETTABLEN"));
}
#[test]
fn branch_list_open_call_and_or_call_order() {
    for open in [true, false] {
        let (code, cs) = if open {
            (
                vec![
                    op!("GETIMPORT", aux = imp(&[0])),
                    op!("NEWTABLE", A = 1, aux = 3),
                    op!("GETIMPORT", A = 2, aux = imp(&[1])),
                    op!("LOADK", A = 3, rf = 2),
                    op!("CALL", A = 2, B = 2, C = 2),
                    op!("GETIMPORT", A = 3, aux = imp(&[3])),
                    jump!("JUMPIFNOT", 9, A = 3),
                    op!("GETIMPORT", A = 3, aux = imp(&[3])),
                    op!("CALL", A = 3, B = 1, C = 2),
                    op!("GETIMPORT", A = 4, aux = imp(&[4])),
                    op!("CALL", A = 4, B = 1, C = 0),
                    op!("SETLIST", A = 1, B = 2, C = 0, aux = 1),
                    op!("CALL", B = 2, C = 1),
                    op!("RETURN", B = 1),
                ],
                vec![s("pairs"), s("tostring"), s("x"), s("flag"), s("f")],
            )
        } else {
            (
                vec![
                    op!("GETIMPORT", aux = imp(&[0])),
                    op!("NEWTABLE", A = 1, aux = 3),
                    op!("GETIMPORT", A = 2, aux = imp(&[2])),
                    op!("CALL", A = 2, B = 1, C = 2),
                    op!("GETIMPORT", A = 3, aux = imp(&[3])),
                    op!("CALL", A = 3, B = 1, C = 2),
                    op!("GETIMPORT", A = 4, aux = imp(&[1])),
                    jump!("JUMPIF", 10, A = 4),
                    op!("GETIMPORT", A = 4, aux = imp(&[4])),
                    op!("CALL", A = 4, B = 1, C = 2),
                    op!("SETLIST", A = 1, B = 2, C = 4, aux = 1),
                    op!("CALL", B = 2, C = 1),
                    op!("RETURN", B = 1),
                ],
                vec![s("pairs"), s("flag"), s("f"), s("g"), s("h")],
            )
        };
        let text = output(&main_code(code, cs));
        assert!(
            text.contains(if open {
                "{tostring(\"x\"), flag and flag(), f()}"
            } else {
                "{f(), g(), flag or h()}"
            }),
            "{text}"
        );
    }
}
#[test]
fn two_setlists_are_one_list() {
    let mut code = vec![
        op!("GETIMPORT", aux = imp(&[0])),
        op!("NEWTABLE", A = 1, aux = 19),
        op!("GETIMPORT", A = 2, aux = imp(&[1])),
        jump!("JUMPIFNOT", 6, A = 2),
        op!("GETIMPORT", A = 2, aux = imp(&[1])),
        op!("CALL", A = 2, B = 1, C = 2),
    ];
    for n in 0..15 {
        code.push(op!("LOADN", A = 3 + n, rf = n));
    }
    code.push(op!("SETLIST", A = 1, B = 2, C = 17, aux = 1));
    for n in 0..3 {
        code.push(op!("LOADN", A = 2 + n, rf = 15 + n));
    }
    code.extend([
        op!("SETLIST", A = 1, B = 2, C = 4, aux = 17),
        op!("CALL", B = 2, C = 1),
        op!("RETURN", B = 1),
    ]);
    let text = output(&main_code(code, vec![s("pairs"), s("flag")]));
    assert_eq!(
        text.trim(),
        format!(
            "pairs({{flag and flag(), {}}})",
            (0..18)
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )
    );
}

#[test]
fn vararg_fixed_open_table_count_and_return() {
    let p = main_code(
        vec![
            op!("GETVARARGS", B = 3),
            op!("NEWTABLE", A = 2),
            op!("GETVARARGS", A = 3, B = 0),
            op!("SETLIST", A = 2, B = 3, C = 0, aux = 1),
            op!("GETIMPORT", A = 3, aux = imp(&[0])),
            op!("MOVE", A = 4, B = 0),
            op!("MOVE", A = 5, B = 1),
            op!("LENGTH", A = 6, B = 2),
            op!("CALL", A = 3, B = 4, C = 1),
            op!("GETIMPORT", A = 3, aux = imp(&[0])),
            op!("GETIMPORT", A = 4, aux = imp(&[1])),
            op!("LOADK", A = 5, rf = 2),
            op!("GETVARARGS", A = 6, B = 0),
            op!("CALL", A = 4, B = 0, C = 0),
            op!("CALL", A = 3, B = 0, C = 1),
            op!("GETVARARGS", A = 3, B = 0),
            op!("RETURN", A = 3, B = 0),
        ],
        vec![s("print"), s("select"), s("#")],
    );
    let text = output(&p);
    re(r"(?m)^local \w+, \w+ = \.\.\.$", &text);
    assert!(
        text.contains("{...}")
            && text.contains("select(\"#\", ...)")
            && text.ends_with("return ...\n")
    );
}
#[test]
fn all_register_arithmetic_and_logic_forms() {
    let mut code = vec![
        op!("GETIMPORT", aux = imp(&[0])),
        op!("GETIMPORT", A = 1, aux = imp(&[1])),
    ];
    for (i, kind) in ["MOD", "DIV", "POW", "IDIV", "AND", "OR"]
        .into_iter()
        .enumerate()
    {
        code.push(op!(kind, A = i + 2, B = 0, C = 1));
    }
    code.push(op!("GETIMPORT", A = 8, aux = imp(&[2])));
    for i in 0..6 {
        code.push(op!("MOVE", A = 9 + i, B = 2 + i));
    }
    code.extend([op!("CALL", A = 8, B = 7, C = 1), op!("RETURN", B = 1)]);
    let text = output(&main_code(code, vec![s("x"), s("y"), s("print")]));
    for op in [" % ", " / ", " ^ ", " // ", " and ", " or "] {
        assert!(text.contains(op), "{text}");
    }
}
#[test]
fn numbers_roundtrip_and_nonfinite_contract() {
    assert_eq!(decomp::fmt_num(4503599627370496.0), "4503599627370496");
    assert_eq!(decomp::fmt_num(9007199254740991.0), "9007199254740991");
    assert_eq!(decomp::fmt_num(0.1), "0.1");
    assert_eq!(decomp::fmt_num(-3.0), "-3");
    assert_eq!(decomp::fmt_num(-0.0), "0");
    assert_eq!(
        decomp::fmt_num(2f64.powi(60)).parse::<f64>().unwrap(),
        2f64.powi(60)
    );
    for (value, expected) in [
        ("nan", "(0/0)"),
        ("inf", "math.huge"),
        ("-inf", "-math.huge"),
    ] {
        let p = main_code(
            vec![op!("LOADK", rf = 0), op!("RETURN", B = 2)],
            vec![json!(["n", value])],
        );
        // Nonfinite source spelling is reference-compatible, but is not an identical opcode skeleton.
        assert_eq!(syntax_output(&p), format!("return {expected}\n"));
    }
    let p = main_code(
        vec![
            op!("GETIMPORT", aux = imp(&[0])),
            op!("LOADK", A = 1, rf = 1),
            op!("LOADK", A = 2, rf = 2),
            op!("LOADK", A = 3, rf = 3),
            op!("CALL", B = 4, C = 1),
            op!("RETURN", B = 1),
        ],
        vec![
            s("print"),
            num(4503599627370496),
            num(9007199254740991),
            num(18014398509481984),
        ],
    );
    let text = output(&p);
    assert!(text.contains("4503599627370496") && text.contains("9007199254740991"));
}

#[test]
fn recursive_local_function() {
    let p = program(vec![
        proto(
            vec![
                op!("DUPCLOSURE", rf = 1),
                op!("CAPTURE", A = 0, B = 0),
                op!("GETIMPORT", A = 1, aux = imp(&[0])),
                op!("MOVE", A = 2, B = 0),
                op!("LOADN", A = 3, rf = 3),
                op!("CALL", A = 2, B = 2, C = 0),
                op!("CALL", A = 1, B = 0, C = 1),
                op!("RETURN", B = 1),
            ],
            vec![s("print"), num(1)],
            0,
            0,
            vec![],
        ),
        proto(
            vec![
                op!("LOADN", A = 1, rf = 0),
                jump!("JUMPIFNOTLT", 6, A = 1, aux = 0),
                op!("GETUPVAL", A = 1, B = 0),
                op!("SUBK", A = 2, B = 0, C = 0),
                op!("CALL", A = 1, B = 2, C = 0),
                op!("RETURN", A = 1, B = 0),
                op!("LOADN", A = 1, rf = 0),
                op!("RETURN", A = 1, B = 2),
            ],
            vec![num(1)],
            1,
            1,
            vec![],
        ),
    ]);
    let text = output(&p);
    let captures = regex::Regex::new(r"(?m)^local function (\w+)\((\w+)\)$")
        .unwrap()
        .captures(&text)
        .unwrap();
    assert!(
        text.contains(&format!("return {}({} - 1)", &captures[1], &captures[2])),
        "{text}"
    );
    assert!(
        !text.contains(&format!("local {};", &captures[1])),
        "{text}"
    );
    assert!(!text.contains("= function"));
    assert_eq!(run(&text), "0\n");
}
#[test]
fn value_capture_snapshot_of_register_and_parameter() {
    let p = program(vec![
        proto(
            vec![
                op!("GETIMPORT", aux = imp(&[1])),
                op!("CALL", B = 1, C = 2),
                op!("DUPCLOSURE", A = 1, rf = 0),
                op!("CAPTURE", A = 0, B = 0),
                op!("LOADN", rf = 9),
                op!("GETIMPORT", A = 2, aux = imp(&[3])),
                op!("LOADK", A = 3, rf = 2),
                op!("CALL", A = 2, B = 2, C = 1),
                op!("RETURN", B = 3),
            ],
            vec![num(1), s("read"), s("after"), s("print")],
            0,
            0,
            vec![],
        ),
        proto(
            vec![op!("GETUPVAL", A = 0, B = 0), op!("RETURN", B = 2)],
            vec![],
            0,
            1,
            vec![],
        ),
    ]);
    let text = output(&p);
    assert_eq!(
        run(&format!(
            "local read=function()return 7 end\nlocal v,f=(function()\n{text}end)()\nprint(v,f())"
        )),
        "after\n9\t7\n"
    );
    let p = program(vec![
        proto(
            vec![op!("DUPCLOSURE", rf = 0), op!("RETURN", B = 2)],
            vec![num(1)],
            0,
            0,
            vec![],
        ),
        proto(
            vec![
                op!("NEWCLOSURE", A = 1, rf = 0),
                op!("CAPTURE", A = 0, B = 0),
                op!("LOADN", A = 0, rf = 9),
                op!("RETURN", B = 3),
            ],
            vec![],
            1,
            0,
            vec![2],
        ),
        proto(
            vec![op!("GETUPVAL"), op!("RETURN", B = 2)],
            vec![],
            0,
            1,
            vec![],
        ),
    ]);
    let text = output(&p);
    assert_eq!(
        run(&format!(
            "local f=(function()\n{text}end)()\nlocal n,g=f(7)\nprint(n,g())"
        )),
        "9\t7\n"
    );
}
#[test]
fn dead_function_is_kept_and_discarded_condition_stays_a_condition() {
    let p = program(vec![
        proto(
            vec![
                op!("DUPCLOSURE", rf = 1),
                op!("GETIMPORT", A = 1, aux = imp(&[0])),
                op!("LOADK", A = 2, rf = 2),
                op!("CALL", A = 1, B = 2, C = 1),
                op!("RETURN", B = 1),
            ],
            vec![s("print"), num(1), s("hi")],
            0,
            0,
            vec![],
        ),
        proto(
            vec![op!("LOADN", rf = 1), op!("RETURN", B = 2)],
            vec![],
            0,
            0,
            vec![],
        ),
    ]);
    let text = output(&p);
    re(r"local function \w+\(\)\n\treturn 1\nend", &text);
    let p = function(
        vec![
            op!("GETIMPORT", A = 1, aux = imp(&[0])),
            op!("MOVE", A = 2, B = 0),
            op!("CALL", A = 1, B = 2, C = 2),
            jump!("JUMPXEQK", 7, A = 1, aux = 0x80000001u64),
            op!("LOADN", A = 1, rf = 0),
            jump!("JUMPIFNOTLE", 7, A = 1, aux = 0),
            jump!("JUMP", 7),
            op!("RETURN", B = 1),
        ],
        vec![s("type"), s("number")],
        1,
    );
    let text = output(&p);
    assert!(text.contains("if type("), "{text}");
}
#[test]
fn method_receiver_local_and_direct_expression() {
    let p = main_code(
        vec![
            op!("GETIMPORT", aux = imp(&[0])),
            op!("LOADK", A = 1, rf = 1),
            op!("CALL", B = 2, C = 2),
            op!("GETIMPORT", A = 1, aux = imp(&[2])),
            op!("LOADK", A = 2, rf = 3),
            op!("GETIMPORT", A = 5, aux = imp(&[4])),
            op!("SELF", A = 3, B = 0, aux = 5),
            op!("CALL", A = 3, B = 3, C = 0),
            op!("CALL", A = 1, B = 0, C = 1),
            op!("RETURN", B = 1),
        ],
        vec![
            s("g"),
            s("HttpService"),
            s("writefile"),
            s("name.json"),
            s("items"),
            s("JSONEncode"),
        ],
    );
    let text = identical_output(&p);
    re(r#"(?m)^local \w+ = g\("HttpService"\)$"#, &text);
    assert!(text.find("g(\"HttpService\")") < text.find("writefile("));
    let p = main_code(
        vec![
            op!("GETIMPORT", aux = imp(&[0])),
            op!("CALL", B = 1, C = 2),
            op!("SELF", A = 0, B = 0, aux = 1),
            op!("CALL", B = 2, C = 1),
            op!("RETURN", B = 1),
        ],
        vec![s("g"), s("m")],
    );
    assert_eq!(output(&p), "g():m()\n");
}

fn fields_by_assignment(n: usize) -> Value {
    let mut code = vec![op!("NEWTABLE", A = 0, B = 7)];
    let mut cs = vec![];
    for j in 0..n {
        cs.extend([s(&format!("f{j}")), s(&format!("k{j}"))]);
        code.extend([
            op!("GETIMPORT", A = 1, aux = imp(&[(2 * j) as u64])),
            op!("CALL", A = 1, B = 1, C = 2),
            op!("SETTABLEKS", A = 1, B = 0, aux = 2 * j + 1),
        ]);
    }
    code.push(op!("RETURN", B = 2));
    main_code(code, cs)
}
#[test]
fn newtable_fields_stay_assignments_until_template_limit() {
    for n in [3, 32, 33] {
        let text = output(&fields_by_assignment(n));
        let bc = compile(&text);
        if n <= 32 {
            re(r"(?m)^local \w+ = \{\}$", &text);
            assert!(
                text.contains(".k0 = f0()")
                    && text.contains(&format!(".k{} = f{}()", n - 1, n - 1))
            );
            assert!(bc.contains("NEWTABLE") && !bc.contains("DUPTABLE"));
        } else {
            re(
                r"(?m)^(local \w+ =|return) \{\n\tk0 = f0\(\),\n\tk1 = f1\(\),",
                &text,
            );
        }
    }
}
#[test]
fn template_constructor_folds_in_assignment_order() {
    let p = main_code(
        vec![
            op!("DUPTABLE", rf = 3),
            op!("GETIMPORT", A = 1, aux = imp(&[4])),
            op!("CALL", A = 1, B = 1, C = 2),
            op!("SETTABLEKS", A = 1, B = 0, aux = 0),
            op!("GETIMPORT", A = 1, aux = imp(&[5])),
            op!("CALL", A = 1, B = 1, C = 2),
            op!("SETTABLEKS", A = 1, B = 0, aux = 1),
            op!("RETURN", B = 2),
        ],
        vec![
            s("a"),
            s("b"),
            s("c"),
            json!(["t", [[0, -1], [1, -1]]]),
            s("f"),
            s("g"),
        ],
    );
    let text = output(&p);
    assert_eq!(text, "return {a = f(), b = g()}\n");
    assert!(compile(&text).contains("DUPTABLE"));
}
#[test]
fn shared_tail_or_and_becomes_one_expression() {
    let p = main_code(
        vec![
            op!("GETIMPORT", A = 1, aux = imp(&[0])),
            jump!("JUMPIF", 6, A = 1),
            op!("GETIMPORT", A = 0, aux = imp(&[1])),
            jump!("JUMPIFNOT", 7, A = 0),
            op!("GETIMPORT", A = 0, aux = imp(&[2])),
            jump!("JUMPIFNOT", 7, A = 0),
            op!("GETIMPORT", A = 0, aux = imp(&[3])),
            op!("GETIMPORT", A = 1, aux = imp(&[4])),
            op!("MOVE", A = 2, B = 0),
            op!("CALL", A = 1, B = 2, C = 1),
            op!("RETURN", B = 1),
        ],
        vec![s("a"), s("b"), s("c"), s("game"), s("print")],
    );
    assert!(output(&p).contains("= (a or b and c) and game"));
}
#[test]
fn table_value_test_keeps_fallback() {
    let p = function(
        vec![
            jump!("JUMPIFNOT", 5, A = 0),
            op!("MOVE", A = 2, B = 0),
            op!("NEWTABLE", A = 1, aux = 1),
            op!("SETLIST", A = 1, B = 2, C = 2, aux = 1),
            jump!("JUMPIF", 6, A = 1),
            op!("NEWTABLE", A = 1),
            op!("RETURN", A = 1, B = 2),
        ],
        vec![],
        1,
    );
    let text = output(&p);
    assert_eq!(
        run(&format!(
            "local f=(function()\n{text}end)()\nprint(#f(false),f(7)[1])"
        )),
        "0\t7\n"
    );
}
#[test]
fn distinct_fallback_loads_keep_distinct_evaluation() {
    let p = function(
        vec![
            jump!("JUMPIFNOT", 7),
            op!("GETIMPORT", A = 1, aux = imp(&[0])),
            op!("MOVE", A = 2, B = 0),
            op!("CALL", A = 1, B = 2, C = 2),
            jump!("JUMPIF", 8, A = 1),
            op!("LOADN", A = 1, rf = 0),
            jump!("JUMP", 8),
            op!("LOADN", A = 1, rf = 0),
            op!("GETIMPORT", A = 2, aux = imp(&[1])),
            op!("MOVE", A = 3, B = 1),
            op!("CALL", A = 2, B = 2, C = 1),
            op!("RETURN", A = 1, B = 2),
        ],
        vec![s("tonumber"), s("print")],
        1,
    );
    let text = output(&p);
    assert_eq!(
        run(&format!(
            "local f=(function()\n{text}end)()\nf(false)\nf('7')\nf('bad')"
        )),
        "0\n7\n0\n"
    );
}
#[test]
fn numeric_for_reserved_slot_reused_for_table() {
    let p = function(
        vec![
            op!("LOADN", A = 2, rf = 1),
            jump!("JUMPIFNOT", 27, A = 2),
            jump!("JUMPXEQK", 13, A = 2, aux = 0x80000000u64),
            op!("GETIMPORT", A = 6, aux = imp(&[1])),
            op!("CALL", A = 6, B = 1, C = 2),
            op!("MOVE", A = 3, B = 6),
            op!("LOADN", A = 4, rf = 1),
            op!("LOADN", A = 5, rf = 0),
            jump!("FORNPREP", 11, A = 3),
            op!("NOP"),
            jump!("FORNLOOP", 9, A = 3),
            op!("LOADN", A = 2, rf = 2),
            jump!("JUMP", 1),
            jump!("JUMPXEQK", 25, A = 2, aux = 0x80000003u64),
            jump!("JUMPIFNOT", 19, A = 1),
            op!("MOVE", A = 4, B = 1),
            op!("NEWTABLE", A = 3, aux = 1),
            op!("SETLIST", A = 3, B = 4, C = 2, aux = 1),
            jump!("JUMPIF", 20, A = 3),
            op!("NEWTABLE", A = 3),
            op!("GETIMPORT", A = 6, aux = imp(&[2])),
            op!("MOVE", A = 7, B = 3),
            op!("CALL", A = 6, B = 2, C = 1),
            op!("LOADN", A = 2, rf = 3),
            jump!("JUMP", 1),
            op!("LOADNIL", A = 2),
            jump!("JUMP", 1),
            op!("RETURN", B = 1),
        ],
        vec![num(1), s("get_limit"), s("print"), num(2)],
        2,
    );
    let text = output(&p);
    assert!(!compile(&text).contains("SETGLOBAL"));
    re(r"while ", &text);
    re(r"for \w+ = 0, get_limit\(\) do", &text);
}

#[test]
fn equality_preserves_metamethod_argument_order() {
    for (kind, expected) in [("JUMPIFEQ", "true\n"), ("JUMPIFNOTEQ", "false\n")] {
        let p = function(
            vec![
                jump!(kind, 3, A = 1, aux = 0),
                op!("LOADB", A = 2, B = 0),
                op!("RETURN", A = 2, B = 2),
                op!("LOADB", A = 2, B = 1),
                op!("RETURN", A = 2, B = 2),
            ],
            vec![],
            2,
        );
        let text = output(&p);
        let wrapped=format!("local f=(function()\n{text}end)()\nlocal mt={{__eq=function(a,b)return a.label=='right'end}}\nprint(f(setmetatable({{label='left'}},mt),setmetatable({{label='right'}},mt)))");
        assert_eq!(run(&wrapped), expected);
    }
}
#[test]
fn cached_field_in_merged_conditions_is_read_once() {
    let p = function(
        vec![
            jump!("JUMPIFNOT", 8),
            op!("GETTABLEKS", A = 2, B = 1),
            op!("LOADN", A = 3, rf = 0),
            jump!("JUMPIFNOTLT", 8, A = 3, aux = 2),
            op!("LOADN", A = 3, rf = 5),
            jump!("JUMPIFNOTLT", 8, A = 2, aux = 3),
            op!("LOADB", A = 4, B = 1),
            op!("RETURN", A = 4, B = 2),
            op!("LOADB", A = 4, B = 0),
            op!("RETURN", A = 4, B = 2),
        ],
        vec![s("Value")],
        2,
    );
    let text = output(&p);
    assert_eq!(run(&format!("local f=(function()\n{text}end)()\nlocal reads=0\nlocal obj=setmetatable({{}},{{__index=function()reads+=1;return reads==1 and 1 or 10 end}})\nprint(f(true,obj),reads)")),"true\t1\n");
}
#[test]
fn field_read_stays_before_observable_work() {
    for work in ["call", "getter", "arithmetic"] {
        let middle = match work {
            "call" => vec![op!("MOVE", A = 4, B = 2), op!("CALL", A = 4, B = 1, C = 2)],
            "getter" => vec![op!("NOP"), op!("GETTABLEKS", A = 4, B = 2)],
            _ => vec![op!("NOP"), op!("ADDK", A = 4, B = 2, C = 1)],
        };
        let mut code = vec![jump!("JUMPIFNOT", 7), op!("GETTABLEKS", A = 3, B = 1)];
        code.extend(middle);
        code.extend([
            jump!("JUMPIFNOTEQ", 7, A = 4, aux = 3),
            op!("LOADB", A = 5, B = 1),
            op!("RETURN", A = 5, B = 2),
            op!("LOADB", A = 5, B = 0),
            op!("RETURN", A = 5, B = 2),
        ]);
        let text = output(&function(code, vec![s("Value"), num(0)], 3));
        let action = match work {
            "call" => "mutate",
            "getter" => "setmetatable({}, {__index=mutate})",
            _ => "setmetatable({}, {__add=mutate})",
        };
        assert_eq!(run(&format!("local f=(function()\n{text}end)()\nlocal obj={{Value=1}}\nlocal function mutate()obj.Value=2;return 1 end\nprint(f(true,obj,{action}))")),"true\n");
    }
}
#[test]
fn fixed_and_open_last_call_results() {
    let cs = vec![s("print"), s("a"), s("f")];
    for open in [false, true] {
        let p = main_code(
            vec![
                op!("GETIMPORT", aux = imp(&[0])),
                op!("GETIMPORT", A = 1, aux = imp(&[1])),
                op!("GETIMPORT", A = 2, aux = imp(&[2])),
                op!("CALL", A = 2, B = 1, C = if open { 0 } else { 2 }),
                op!("CALL", B = if open { 0 } else { 3 }, C = 1),
                op!("RETURN", B = 1),
            ],
            cs.clone(),
        );
        assert_eq!(
            output(&p),
            if open {
                "print(a, f())\n"
            } else {
                "print(a, (f()))\n"
            }
        );
    }
    let p = main_code(
        vec![
            op!("NEWTABLE", C = 2),
            op!("GETIMPORT", A = 1, aux = imp(&[2])),
            op!("CALL", A = 1, B = 1, C = 2),
            op!("GETIMPORT", A = 2, aux = imp(&[2])),
            op!("CALL", A = 2, B = 1, C = 2),
            op!("SETLIST", B = 1, C = 3, aux = 1),
            op!("RETURN", B = 2),
        ],
        cs.clone(),
    );
    assert_eq!(output(&p), "return {f(), (f())}\n");
    let p = main_code(
        vec![
            op!("GETIMPORT", aux = imp(&[2])),
            op!("CALL", B = 1, C = 2),
            op!("RETURN", B = 2),
        ],
        cs,
    );
    assert_eq!(output(&p), "return (f())\n");
}
#[test]
fn and_value_retested_stays_local_while_direct_chain_does_not() {
    let cs = vec![s("a"), s("b"), s("print"), s("x")];
    for materialized in [true, false] {
        let p = main_code(
            vec![
                op!("GETIMPORT", aux = imp(&[0])),
                jump!("JUMPIFNOT", if materialized { 3 } else { 4 }),
                op!("GETIMPORT", aux = imp(&[1])),
                jump!("JUMPIF", 7),
                op!(
                    "GETIMPORT",
                    A = if materialized { 1 } else { 0 },
                    aux = imp(&[2])
                ),
                op!("LOADK", A = if materialized { 2 } else { 1 }, rf = 3),
                op!("CALL", A = if materialized { 1 } else { 0 }, B = 2, C = 1),
                op!("RETURN", B = 1),
            ],
            cs.clone(),
        );
        let text = output(&p);
        if materialized {
            re(r"(?m)^local \w+ = a and b$", &text);
            re(r"(?m)^if not \w+ then$", &text);
        } else {
            assert!(!text.contains("local "), "{text}");
        }
    }
}

fn metatable_program(template: bool) -> Value {
    program(vec![
        proto(
            vec![op!("DUPCLOSURE", rf = 0), op!("RETURN", B = 2)],
            vec![num(1)],
            0,
            0,
            vec![],
        ),
        proto(
            vec![
                op!("NEWTABLE", A = 2),
                if template {
                    op!("DUPTABLE", A = 3, rf = 3)
                } else {
                    op!("NEWTABLE", A = 3, B = 2)
                },
                op!("NEWCLOSURE", A = 4, rf = 0),
                op!("CAPTURE", A = 0, B = 0),
                op!("SETTABLEKS", A = 4, B = 3),
                op!("NEWCLOSURE", A = 4, rf = 1),
                op!("CAPTURE", A = 0, B = 0),
                op!("SETTABLEKS", A = 4, B = 3, aux = 1),
                op!("GETIMPORT", A = 1, aux = imp(&[2])),
                op!("CALL", A = 1, B = 3, C = 0),
                op!("RETURN", A = 1, B = 0),
            ],
            vec![
                s("__index"),
                s("__tostring"),
                s("setmetatable"),
                json!(["t", [[0, -1], [1, -1]]]),
            ],
            1,
            0,
            vec![2, 3],
        ),
        proto(
            vec![op!("GETUPVAL"), op!("RETURN", B = 2)],
            vec![],
            0,
            1,
            vec![],
        ),
        proto(
            vec![op!("GETUPVAL"), op!("RETURN", B = 2)],
            vec![],
            0,
            1,
            vec![],
        ),
    ])
}
#[test]
fn two_table_call_arguments_keep_creation_order() {
    for template in [false, true] {
        let text = output(&metatable_program(template));
        if template {
            assert!(
                text.contains("return setmetatable({}, {__index = function"),
                "{text}"
            );
            assert!(!text.replace("local function", "").contains("local"));
        } else {
            re(r"(?m)^\tlocal \w+ = \{\}\n\tlocal \w+ = \{\}\n", &text);
            re(r"return setmetatable\(\w+, \w+\)", &text);
        }
    }
}
#[test]
fn nested_table_list_constructor() {
    let p = main_code(
        vec![
            op!("NEWTABLE", C = 2),
            op!("DUPTABLE", A = 1, rf = 9),
            op!("NEWTABLE", A = 2, C = 2),
            op!("LOADK", A = 3, rf = 2),
            op!("LOADK", A = 4, rf = 3),
            op!("SETLIST", A = 2, B = 3, C = 3, aux = 1),
            op!("SETTABLEKS", A = 2, B = 1),
            op!("NEWTABLE", A = 2, C = 1),
            op!("LOADK", A = 3, rf = 4),
            op!("SETLIST", A = 2, B = 3, C = 2, aux = 1),
            op!("SETTABLEKS", A = 2, B = 1, aux = 1),
            op!("DUPTABLE", A = 2, rf = 9),
            op!("NEWTABLE", A = 3, C = 1),
            op!("LOADK", A = 4, rf = 5),
            op!("SETLIST", A = 3, B = 4, C = 2, aux = 1),
            op!("SETTABLEKS", A = 3, B = 2),
            op!("NEWTABLE", A = 3, C = 2),
            op!("LOADK", A = 4, rf = 6),
            op!("LOADK", A = 5, rf = 7),
            op!("SETLIST", A = 3, B = 4, C = 3, aux = 1),
            op!("SETTABLEKS", A = 3, B = 2, aux = 1),
            op!("SETLIST", A = 0, B = 1, C = 3, aux = 1),
            op!("RETURN", B = 2),
        ],
        vec![
            s("words"),
            s("paths"),
            s("a"),
            s("b"),
            s("c"),
            s("d"),
            s("e"),
            s("f"),
            s("g"),
            json!(["t", [[0, -1], [1, -1]]]),
        ],
    );
    let text = output(&p);
    assert!(text.contains("words = {\"a\", \"b\"}") && text.contains("paths = {\"e\", \"f\"}"));
    assert!(text.starts_with("return {"));
}
#[test]
fn list_around_table_built_by_assignments() {
    let p = main_code(
        vec![
            op!("NEWTABLE", C = 1),
            op!("NEWTABLE", A = 1),
            op!("LOADN", A = 2, rf = 1),
            op!("SETTABLEKS", A = 2, B = 1),
            op!("SETLIST", A = 0, B = 1, C = 2, aux = 1),
            op!("RETURN", B = 2),
        ],
        vec![s("x")],
    );
    // The reference deliberately asserts syntax/shape only: this moves the invisible outer table creation.
    re(r"(?m)^return \{\w+\}$", &syntax_output(&p));
}

#[test]
fn while_ending_in_generic_for() {
    let p = main_code(
        vec![
            op!("GETVARARGS", B = 2),
            op!("LENGTH", A = 1, B = 0),
            op!("LOADN", A = 2, rf = 0),
            jump!("JUMPIFNOTLT", 17, A = 2, aux = 1),
            op!("GETIMPORT", A = 1, aux = imp(&[0, 1])),
            op!("MOVE", A = 2, B = 0),
            op!("LOADN", A = 3, rf = 1),
            op!("CALL", A = 1, B = 3, C = 2),
            op!("GETIMPORT", A = 2, aux = imp(&[2])),
            op!("MOVE", A = 3, B = 1),
            op!("CALL", A = 2, B = 2, C = 4),
            jump!("FORGPREP", 15, A = 2),
            op!("GETIMPORT", A = 7, aux = imp(&[3])),
            op!("MOVE", A = 8, B = 6),
            op!("CALL", A = 7, B = 2, C = 1),
            jump!("FORGLOOP", 12, A = 2, aux = 2),
            jump!("JUMP", 1),
            op!("RETURN", B = 1),
        ],
        vec![s("table"), s("remove"), s("iter"), s("print")],
    );
    let text = output(&p);
    re(r"(?m)^while #\w+ > 0 do$", &text);
    re(r"(?m)^\tfor _, \w+ in iter\(\w+\) do$", &text);
}
#[test]
fn while_ending_in_numeric_for() {
    let p = main_code(
        vec![
            op!("GETVARARGS", B = 2),
            op!("LOADN", A = 1, rf = 0),
            jump!("JUMPIFNOTLT", 13, A = 1, aux = 0),
            op!("SUBK", A = 0, B = 0, C = 1),
            op!("LOADN", A = 3, rf = 1),
            op!("LOADN", A = 1, rf = 3),
            op!("LOADN", A = 2, rf = 1),
            jump!("FORNPREP", 12, A = 1),
            op!("GETIMPORT", A = 4, aux = imp(&[0])),
            op!("MOVE", A = 5, B = 3),
            op!("CALL", A = 4, B = 2, C = 1),
            jump!("FORNLOOP", 8, A = 1),
            jump!("JUMP", 1),
            op!("RETURN", B = 2),
        ],
        vec![s("print"), num(1)],
    );
    let text = output(&p);
    re(r"(?m)^while \w+ > 0 do$", &text);
    re(r"(?m)^\tfor \w+ = 1, 3 do$", &text);
    assert_eq!(
        run(&format!("local f=function(...)\n{text}end\nprint(f(1))")),
        "1\n2\n3\n0\n"
    );
}
#[test]
fn executor_and_imported_names_never_shadow_globals() {
    for name in [
        "getgc",
        "toclipboard",
        "runonactor",
        "hookmetamethod",
        "rconsoleprint",
        "crypt",
        "json",
        "info",
    ] {
        let p = main_code(
            vec![
                op!("NEWTABLE"),
                op!("GETTABLEKS", A = 1, B = 0),
                op!("SETTABLEKS", A = 1, B = 0, aux = 1),
                op!("SETTABLEKS", A = 1, B = 0, aux = 2),
                op!("RETURN", B = 1),
            ],
            vec![s(name), s("A"), s("B")],
        );
        let text = output(&p);
        let assignment = regex::Regex::new(&format!(r"(?m)^local (\w+) = \w+\.{name}$")).unwrap();
        let local = &assignment.captures(&text).unwrap()[1];
        assert_ne!(local, name, "{text}");
        assert!(
            text.contains(&format!(".A = {local}")) && text.contains(&format!(".B = {local}")),
            "{text}"
        );
        assert!(!text.contains(&format!("local {name} =")), "{text}");
    }
    for name in ["fetch", "v1"] {
        let p = main_code(
            vec![
                op!("GETIMPORT", aux = imp(&[0])),
                op!("LOADK", A = 1, rf = 1),
                op!("CALL", B = 2, C = 2),
                op!("GETIMPORT", A = 1, aux = imp(&[0])),
                op!("MOVE", A = 2, B = 0),
                op!("CALL", A = 1, B = 2, C = 1),
                op!("GETIMPORT", A = 1, aux = imp(&[0])),
                op!("MOVE", A = 2, B = 0),
                op!("CALL", A = 1, B = 2, C = 1),
                op!("RETURN", B = 1),
            ],
            vec![s(name), s("a")],
        );
        let text = output(&p);
        assert!(!text.contains(&format!("local {name} =")));
        let local = text
            .lines()
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap();
        assert_eq!(text.matches(&format!("{name}({local})\n")).count(), 2);
    }
}

#[test]
fn below_result_operand_stays_local_in_original_order() {
    let cs = vec![s("math"), s("pi"), s("print"), s("x"), num(2)];
    let p = main_code(
        vec![
            op!("GETIMPORT", aux = imp(&[0, 1])),
            op!("GETIMPORT", A = 1, aux = imp(&[2])),
            op!("LOADK", A = 2, rf = 3),
            op!("CALL", A = 1, B = 2, C = 1),
            op!("MULK", A = 1, B = 0, C = 4),
            op!("RETURN", A = 1, B = 2),
        ],
        cs.clone(),
    );
    let text = identical_output(&p);
    let local = regex::Regex::new(r"^local (\w+) = math\.pi")
        .unwrap()
        .captures(&text)
        .unwrap()[1]
        .to_owned();
    assert_eq!(
        text,
        format!("local {local} = math.pi\nprint(\"x\")\nreturn {local} * 2\n")
    );
    let p = main_code(
        vec![
            op!("GETIMPORT", aux = imp(&[2])),
            op!("LOADK", A = 1, rf = 3),
            op!("CALL", B = 2, C = 1),
            op!("GETIMPORT", aux = imp(&[0, 1])),
            op!("MULK", A = 0, B = 0, C = 4),
            op!("RETURN", B = 2),
        ],
        cs,
    );
    assert_eq!(identical_output(&p), "print(\"x\")\nreturn math.pi * 2\n");
}
#[test]
fn and_or_operand_below_result_stays_local() {
    let p = main_code(
        vec![
            op!("GETIMPORT", A = 1, aux = imp(&[0])),
            jump!("JUMPIFNOT", 4, A = 1),
            op!("GETIMPORT", A = 0, aux = imp(&[1])),
            jump!("JUMPIF", 5, A = 0),
            op!("GETIMPORT", aux = imp(&[2])),
            op!("GETIMPORT", A = 1, aux = imp(&[3])),
            op!("GETIMPORT", A = 3, aux = imp(&[4])),
            op!("CALL", A = 3, B = 1, C = 2),
            op!("ADD", A = 2, B = 0, C = 3),
            op!("CALL", A = 1, B = 2, C = 1),
            op!("RETURN", B = 1),
        ],
        vec![s("a"), s("b"), s("c"), s("print"), s("g")],
    );
    let text = identical_output(&p);
    let local = regex::Regex::new(r"^local (\w+) = a and b or c")
        .unwrap()
        .captures(&text)
        .unwrap()[1]
        .to_owned();
    assert_eq!(
        text,
        format!("local {local} = a and b or c\nprint({local} + g())\n")
    );
}
#[test]
fn and_or_computed_before_table_stays_before_table() {
    let cs = vec![s("a"), s("b"), s("c"), json!(["t", [[4, -1]]]), s("k")];
    let p = main_code(
        vec![
            op!("GETIMPORT", A = 1, aux = imp(&[0])),
            jump!("JUMPIFNOT", 4, A = 1),
            op!("GETIMPORT", A = 0, aux = imp(&[1])),
            jump!("JUMPIF", 5, A = 0),
            op!("GETIMPORT", aux = imp(&[2])),
            op!("DUPTABLE", A = 1, rf = 3),
            op!("SETTABLEKS", A = 0, B = 1, aux = 4),
            op!("RETURN", A = 1, B = 2),
        ],
        cs.clone(),
    );
    let text = identical_output(&p);
    let local = regex::Regex::new(r"^local (\w+) = a and b or c")
        .unwrap()
        .captures(&text)
        .unwrap()[1]
        .to_owned();
    assert_eq!(
        text,
        format!("local {local} = a and b or c\nreturn {{k = {local}}}\n")
    );
    let p = main_code(
        vec![
            op!("DUPTABLE", rf = 3),
            op!("GETIMPORT", A = 2, aux = imp(&[0])),
            jump!("JUMPIFNOT", 5, A = 2),
            op!("GETIMPORT", A = 1, aux = imp(&[1])),
            jump!("JUMPIF", 6, A = 1),
            op!("GETIMPORT", A = 1, aux = imp(&[2])),
            op!("SETTABLEKS", A = 1, B = 0, aux = 4),
            op!("RETURN", B = 2),
        ],
        cs,
    );
    assert_eq!(identical_output(&p), "return {k = a and b or c}\n");
}
#[test]
fn numeric_index_assignments_keep_preexisting_values() {
    let cs = vec![s("a"), s("b"), s("c")];
    let p = main_code(
        vec![
            op!("GETIMPORT", A = 1, aux = imp(&[0])),
            jump!("JUMPIFNOT", 4, A = 1),
            op!("GETIMPORT", A = 0, aux = imp(&[1])),
            jump!("JUMPIF", 5, A = 0),
            op!("GETIMPORT", aux = imp(&[2])),
            op!("NEWTABLE", A = 1, B = 0, aux = 1),
            op!("SETTABLEN", A = 0, B = 1, C = 0),
            op!("RETURN", A = 1, B = 2),
        ],
        cs.clone(),
    );
    let text = identical_output(&p);
    let caps = regex::Regex::new(r"^local (\w+) = a and b or c\nlocal (\w+) = \{\}\n")
        .unwrap()
        .captures(&text)
        .unwrap();
    let (v, t) = (&caps[1], &caps[2]);
    assert_eq!(
        text,
        format!("local {v} = a and b or c\nlocal {t} = {{}}\n{t}[1] = {v}\nreturn {t}\n")
    );
    assert!(compile(&text).contains("SETTABLEN"));
    let p = main_code(
        vec![
            op!("NEWTABLE", aux = 3),
            op!("GETIMPORT", A = 2, aux = imp(&[0])),
            jump!("JUMPIFNOT", 5, A = 2),
            op!("GETIMPORT", A = 1, aux = imp(&[1])),
            jump!("JUMPIF", 6, A = 1),
            op!("GETIMPORT", A = 1, aux = imp(&[2])),
            op!("SETTABLEN", A = 1, B = 0, C = 0),
            op!("LOADN", A = 1, rf = 7),
            op!("SETTABLEN", A = 1, B = 0, C = 2),
            op!("RETURN", B = 2),
        ],
        cs,
    );
    let text = identical_output(&p);
    let t = regex::Regex::new(r"^local (\w+) = \{\}")
        .unwrap()
        .captures(&text)
        .unwrap()[1]
        .to_owned();
    assert_eq!(
        text,
        format!("local {t} = {{}}\n{t}[1] = a and b or c\n{t}[3] = 7\nreturn {t}\n")
    );
}
#[test]
fn loop_assignment_before_return_updates_original_local() {
    let p = main_code(
        vec![
            op!("LOADB", B = 0),
            op!("LOADN", A = 3, rf = 1),
            op!("LOADN", A = 1, rf = 3),
            op!("LOADN", A = 2, rf = 1),
            jump!("FORNPREP", 12, A = 1),
            op!("GETIMPORT", A = 4, aux = imp(&[0])),
            op!("MOVE", A = 5, B = 3),
            op!("CALL", A = 4, B = 2, C = 2),
            jump!("JUMPIFNOT", 11, A = 4),
            op!("LOADB", B = 1),
            op!("RETURN", B = 2),
            jump!("FORNLOOP", 5, A = 1),
            op!("RETURN", B = 2),
        ],
        vec![s("f")],
    );
    let text = identical_output(&p);
    let local = regex::Regex::new(r"^local (\w+) = false")
        .unwrap()
        .captures(&text)
        .unwrap()[1]
        .to_owned();
    assert!(
        text.contains(&format!(
            "\t\t{local} = true\n\t\treturn {local}\n\tend\nend\n\nreturn {local}"
        )),
        "{text}"
    );
    assert_eq!(
        run(&format!(
            "local f=function(i)return i==2 end\nprint((function()\n{text}end)())"
        )),
        "true\n"
    );
}
#[test]
fn explicit_return_in_if_branch_is_retained() {
    for explicit in [true, false] {
        let mut code = vec![
            op!("GETVARARGS", B = 2),
            jump!("JUMPIF", if explicit { 6 } else { 5 }),
            op!("GETIMPORT", A = 1, aux = imp(&[0])),
            op!("LOADK", A = 2, rf = 1),
            op!("CALL", A = 1, B = 2, C = 1),
            op!("RETURN", B = 1),
        ];
        if explicit {
            code.push(op!("RETURN", B = 1));
        }
        let text = identical_output(&main_code(code, vec![s("error"), s("e")]));
        if explicit {
            assert!(text.contains("\treturn\nend\n"), "{text}");
        } else {
            assert!(!text.contains("return"), "{text}");
        }
    }
}
#[test]
fn blank_lines_around_blocks_only() {
    let p = main_code(
        vec![
            op!("GETIMPORT", aux = imp(&[0])),
            op!("LOADK", A = 1, rf = 1),
            op!("CALL", B = 2, C = 1),
            op!("GETIMPORT", aux = imp(&[0])),
            op!("LOADK", A = 1, rf = 2),
            op!("CALL", B = 2, C = 1),
            op!("LOADN", A = 2, rf = 1),
            op!("LOADN", rf = 3),
            op!("LOADN", A = 1, rf = 1),
            jump!("FORNPREP", 14),
            op!("GETIMPORT", A = 3, aux = imp(&[0])),
            op!("MOVE", A = 4, B = 2),
            op!("CALL", A = 3, B = 2, C = 1),
            jump!("FORNLOOP", 10),
            op!("GETIMPORT", aux = imp(&[0])),
            op!("LOADK", A = 1, rf = 3),
            op!("CALL", B = 2, C = 1),
            op!("GETIMPORT", aux = imp(&[0])),
            op!("LOADK", A = 1, rf = 4),
            op!("CALL", B = 2, C = 1),
            op!("RETURN", B = 1),
        ],
        vec![s("print"), s("a"), s("b"), s("c"), s("d")],
    );
    assert_eq!(identical_output(&p),"print(\"a\")\nprint(\"b\")\n\nfor index = 1, 3 do\n\tprint(index)\nend\n\nprint(\"c\")\nprint(\"d\")\n");
}
#[test]
fn adjacent_guards_have_one_blank_line() {
    let p = function(
        vec![
            jump!("JUMPIF", 2, A = 0),
            op!("RETURN", B = 1),
            jump!("JUMPIF", 4, A = 1),
            op!("RETURN", B = 1),
            op!("GETIMPORT", A = 2, aux = imp(&[0])),
            op!("LOADK", A = 3, rf = 1),
            op!("CALL", A = 2, B = 2, C = 1),
            op!("RETURN", B = 1),
        ],
        vec![s("print"), s("x")],
        2,
    );
    assert_eq!(identical_output(&p),"return function(argument, argument2)\n\tif not argument then\n\t\treturn\n\tend\n\n\tif not argument2 then\n\t\treturn\n\tend\n\tprint(\"x\")\nend\n");
}

#[test]
fn early_continue_shared_tail_emitted_once() {
    for guard in ["JUMPIF", "JUMPIFNOT"] {
        let p = function(
            vec![
                op!("LOADN", A = 2, rf = 1),
                jump!("JUMPIFNOT", 13, A = 2),
                jump!("JUMPIF", 8, A = 0),
                op!("GETIMPORT", A = 3, aux = imp(&[0])),
                op!("LOADK", A = 4, rf = 1),
                op!("CALL", A = 3, B = 2, C = 1),
                jump!(guard, 11, A = 1),
                jump!("JUMP", 8),
                op!("GETIMPORT", A = 3, aux = imp(&[0])),
                op!("LOADK", A = 4, rf = 2),
                op!("CALL", A = 3, B = 2, C = 1),
                op!("LOADNIL", A = 2),
                jump!("JUMP", 1),
                op!("RETURN", B = 1),
            ],
            vec![s("print"), s("checked"), s("tail")],
            2,
        );
        let text = output(&p);
        assert_eq!(text.matches("print(\"tail\")").count(), 1, "{text}");
        let (skip, stay) = if guard == "JUMPIF" {
            ("true", "false")
        } else {
            ("false", "true")
        };
        assert_eq!(run(&format!("local f=(function()\n{text}end)()\nf(true,{skip})\nf(true,{stay})\nf(false,{stay})\nf(false,{skip})")),"tail\ntail\nchecked\ntail\nchecked\n");
    }
}
#[test]
fn conditional_argument_keeps_earlier_format_string() {
    let p = function(
        vec![
            op!("GETIMPORT", A = 2, aux = imp(&[0, 1])),
            op!("LOADK", A = 3, rf = 2),
            jump!("JUMPIFNOT", 5, A = 1),
            op!("ADDK", A = 4, B = 0, C = 3),
            jump!("JUMP", 6),
            op!("SUBK", A = 4, B = 0, C = 3),
            op!("CALL", A = 2, B = 3, C = 2),
            op!("RETURN", A = 2, B = 2),
        ],
        vec![s("string"), s("format"), s("Value %d"), num(1)],
        2,
    );
    let text = output(&p);
    assert_eq!(
        run(&format!(
            "local f=(function()\n{text}end)()\nprint(f(5,true))\nprint(f(5,false))"
        )),
        "Value 6\nValue 4\n"
    );
}
#[test]
fn conditional_argument_false_nil_and_single_result() {
    let p = function(
        vec![
            op!("MOVE", A = 3, B = 1),
            op!("LOADK", A = 4, rf = 0),
            jump!("JUMPIFNOT", 6, A = 0),
            op!("MOVE", A = 5, B = 2),
            op!("CALL", A = 5, B = 1, C = 2),
            jump!("JUMP", 7),
            op!("LOADNIL", A = 5),
            op!("CALL", A = 3, B = 3, C = 2),
            op!("RETURN", A = 3, B = 2),
        ],
        vec![s("label")],
        3,
    );
    let text = output(&p);
    assert_eq!(run(&format!("local f=(function()\n{text}end)()\nlocal calls=0\nlocal function produce()calls+=1;return false,99 end\nlocal function sink(label,...)print(label,select('#',...),...);return 'done'end\nprint(f(true,sink,produce))\nprint(f(false,sink,produce))\nprint(calls)")),"label\t1\tfalse\ndone\nlabel\t1\tnil\ndone\n1\n");
}
#[test]
fn branch_can_change_captured_callee_argument_or_metamethod() {
    for operand in ["callee", "argument", "metamethod"] {
        let read = if operand == "argument" {
            vec![op!("MOVE", A = 1, B = 0), op!("GETUPVAL", A = 2, B = 0)]
        } else {
            vec![op!("GETUPVAL", A = 1, B = 0), op!("LOADK", A = 2, rf = 0)]
        };
        let mut code = vec![jump!("JUMPIFNOT", 4, A = 0)];
        code.extend(if operand == "metamethod" {
            vec![op!("ADDK", A = 3, B = 0, C = 1), op!("NOP")]
        } else {
            vec![op!("MOVE", A = 3, B = 0), op!("CALL", A = 3, B = 1, C = 2)]
        });
        code.extend([jump!("JUMP", 5), op!("LOADNIL", A = 3)]);
        code.extend(read);
        code.extend([op!("CALL", A = 1, B = 3, C = 1), op!("RETURN", B = 1)]);
        let p = program(vec![
            proto(
                vec![
                    op!("LOADNIL"),
                    op!("NEWCLOSURE", A = 1, rf = 0),
                    op!("CAPTURE", A = 1, B = 0),
                    op!("NEWCLOSURE", A = 2, rf = 1),
                    op!("CAPTURE", A = 1, B = 0),
                    op!("RETURN", A = 1, B = 3),
                ],
                vec![],
                0,
                0,
                vec![1, 2],
            ),
            proto(code, vec![s("label"), num(1)], 1, 1, vec![]),
            proto(
                vec![op!("SETUPVAL", A = 0, B = 0), op!("RETURN", B = 1)],
                vec![],
                1,
                1,
                vec![],
            ),
        ]);
        let text = output(&p);
        let (initial, changed, expected) = if operand == "argument" {
            ("10", "20", "20\tfalse\n")
        } else {
            (
                "function(...)print('old',...)end",
                "function(...)print('new',...)end",
                if operand == "metamethod" {
                    "new\tlabel\t7\n"
                } else {
                    "new\tlabel\tfalse\n"
                },
            )
        };
        let action = if operand == "argument" {
            "function(...)if select('#',...)==0 then set(20);return false end;print(...)end"
                .to_owned()
        } else if operand == "metamethod" {
            format!("setmetatable({{}},{{__add=function()set({changed});return 7 end}})")
        } else {
            format!("function()set({changed});return false end")
        };
        assert_eq!(
            run(&format!(
                "local f,set=(function()\n{text}end)()\nset({initial})\nf({action})"
            )),
            expected,
            "{text}"
        );
    }
}
#[test]
fn branch_can_change_global_callee_and_argument() {
    for operand in ["callee", "argument"] {
        let read = if operand == "callee" {
            vec![
                op!("GETIMPORT", A = 1, aux = imp(&[0])),
                op!("LOADK", A = 2, rf = 1),
            ]
        } else {
            vec![
                op!("GETIMPORT", A = 1, aux = imp(&[1])),
                op!("GETIMPORT", A = 2, aux = imp(&[0])),
            ]
        };
        let mut code = vec![
            jump!("JUMPIFNOT", 4),
            op!("MOVE", A = 3, B = 0),
            op!("CALL", A = 3, B = 1, C = 2),
            jump!("JUMP", 5),
            op!("LOADNIL", A = 3),
        ];
        code.extend(read);
        code.extend([op!("CALL", A = 1, B = 3, C = 1), op!("RETURN", B = 1)]);
        let cs = if operand == "callee" {
            vec![s("sink"), s("label")]
        } else {
            vec![s("status"), s("print")]
        };
        let text = output(&function(code, cs, 1));
        let (setup, action, expected) = if operand == "callee" {
            (
                "sink=function(...)print('old',...)end",
                "function()sink=function(...)print('new',...)end;return false end",
                "new\tlabel\tfalse\n",
            )
        } else {
            (
                "status='old'",
                "function()status='new';return false end",
                "new\tfalse\n",
            )
        };
        assert_eq!(run(&format!("{setup}\nlocal f=(function()\n{text}end)()\nf({action},function(...)print(...)end)")),expected,"{text}");
    }
}

fn imp_expr(s: &str) -> ir::E {
    ir::e(ir::Expr::Imp(vec![s.into()]))
}
fn call_expr(name: &str, args: Vec<ir::E>) -> ir::E {
    ir::e(ir::Expr::Call {
        func: imp_expr(name),
        args,
        method: None,
    })
}
fn print_body(g: gen::Generator, body: ir::Body) -> String {
    let mut p = printer::Printer::new(g, std::collections::BTreeSet::new(), BTreeMap::new(), false);
    let text = p
        .block(&body, &printer::Scope::new(None, false), "")
        .unwrap()
        .join("\n")
        + "\n";
    assert!(!compile(&text).contains("SETGLOBAL"), "{text}");
    text
}
#[test]
fn list_store_movement_requires_no_intervening_table_use() {
    use ir::*;
    let mut g = gen::Generator::new(BTreeMap::new());
    let w = g.ctx.web("var", None, -1);
    let table = e(Expr::Table(Table {
        fromnew: true,
        ..Table::new(vec![])
    }));
    let store = Stmt::Assign(idx(var(w), Key::Expr(e(Expr::Num(1.0, true)))), number(5.0));
    let uses = Stmt::Call(call_expr("consume", vec![var(w)]));
    let body = vec![Stmt::Local(w, Some(table.clone())), uses, store.clone()];
    assert_eq!(post::fold_lists(body.clone(), &g.ctx), body);
    let body = vec![
        Stmt::Local(w, Some(table.clone())),
        Stmt::Call(call_expr("consume", vec![])),
        store,
    ];
    let folded = post::fold_lists(body, &g.ctx);
    assert!(
        matches!(&folded[..],[Stmt::Call(_),Stmt::Local(_,Some(x))]if matches!(x.as_ref(),Expr::Table(t)if t.items.len()==1))
    );
    let body = vec![
        Stmt::Local(w, Some(table)),
        Stmt::Assign(idx(var(w), Key::Expr(number(1.0))), number(5.0)),
    ];
    assert_eq!(post::fold_lists(body.clone(), &g.ctx), body);
}
#[test]
fn unrepresentable_open_list_store_is_error() {
    use ir::*;
    let body = vec![Stmt::Assign(
        idx(var(0), Key::Expr(e(Expr::Num(1.0, true)))),
        multi(call_expr("f", vec![])),
    )];
    assert!(post::check_open_lists(&body).is_err());
}
#[test]
fn copied_multireturn_declarations_stay_in_branches() {
    use ir::*;
    for unused_middle in [false, true] {
        let mut g = gen::Generator::new(BTreeMap::new());
        let a = g.ctx.web("var", None, -1);
        let b = g.ctx.web("var", None, -1);
        g.ctx.webs[a].uses = vec![(0, 0)];
        g.ctx.webs[b].uses = vec![(0, 2)];
        let slots = if unused_middle {
            vec![Some(a), None, Some(b)]
        } else {
            vec![Some(a), Some(b)]
        };
        let branch = |name: &str| {
            vec![
                Stmt::LocalN(slots.clone(), call_expr(name, vec![])),
                Stmt::Call(call_expr("print", vec![var(a), var(b)])),
            ]
        };
        let body = vec![Stmt::If(
            imp_expr("flag"),
            branch("read_left"),
            branch("read_right"),
        )];
        assert_eq!(post::hoist_decls(body.clone()).unwrap(), body);
        let text = print_body(g, body);
        for (flag, expected, left, right) in [
            ("true", "false\t9\n", "false", "true"),
            ("false", "true\t8\n", "false", "true"),
        ] {
            let a = if unused_middle {
                format!("{left},nil,9")
            } else {
                format!("{left},9")
            };
            let b = if unused_middle {
                format!("{right},nil,8")
            } else {
                format!("{right},8")
            };
            assert_eq!(run(&format!("local flag={flag}\nlocal calls=0\nlocal function read_left()calls+=1;return {a},77 end\nlocal function read_right()calls+=1;return {b},88 end\n{text}\nprint(calls)")),format!("{expected}1\n"));
        }
    }
}
#[test]
fn copied_declarations_with_out_of_scope_read_are_hoisted() {
    use ir::*;
    let mut g = gen::Generator::new(BTreeMap::new());
    let w = g.ctx.web("var", None, -1);
    let body = vec![
        Stmt::If(
            imp_expr("flag"),
            vec![Stmt::Local(w, Some(number(1.0)))],
            vec![Stmt::Local(w, Some(number(2.0)))],
        ),
        Stmt::Call(call_expr("print", vec![var(w)])),
    ];
    let text = print_body(g, post::hoist_decls(body).unwrap());
    for (flag, expected) in [("true", "1\n"), ("false", "2\n")] {
        assert_eq!(run(&format!("local flag={flag}\n{text}")), expected);
    }
}
#[test]
fn ancestor_declaration_keeps_nested_write() {
    use ir::*;
    let mut g = gen::Generator::new(BTreeMap::new());
    let w = g.ctx.web("var", None, -1);
    let body = vec![
        Stmt::Local(w, Some(number(1.0))),
        Stmt::If(
            imp_expr("flag"),
            vec![Stmt::Local(w, Some(number(2.0)))],
            vec![],
        ),
        Stmt::Call(call_expr("print", vec![var(w)])),
    ];
    let body = post::post_passes(body, &mut g.ctx, None).unwrap();
    let text = print_body(g, body);
    for (flag, expected) in [("true", "2\n"), ("false", "1\n")] {
        assert_eq!(run(&format!("local flag={flag}\n{text}")), expected);
    }
}
#[test]
fn copied_results_do_not_hide_unrelated_sibling_local() {
    use ir::*;
    let mut g = gen::Generator::new(BTreeMap::new());
    let a = g.ctx.web("var", None, -1);
    let b = g.ctx.web("var", None, -1);
    let extra = g.ctx.web("var", None, -1);
    g.ctx.webs[a].uses = vec![(0, 0)];
    g.ctx.webs[b].uses = vec![(0, 1)];
    let body = vec![Stmt::If(
        imp_expr("flag"),
        vec![
            Stmt::LocalN(vec![Some(a), Some(b)], call_expr("read", vec![])),
            Stmt::Call(call_expr("print", vec![var(a), var(b), number(0.0)])),
        ],
        vec![
            Stmt::Local(extra, Some(bin("+", imp_expr("input"), number(1.0)))),
            Stmt::LocalN(vec![Some(a), Some(b)], call_expr("read", vec![])),
            Stmt::Call(call_expr("print", vec![var(a), var(b), var(extra)])),
        ],
    )];
    let body = post::post_passes(body, &mut g.ctx, None).unwrap();
    let text = print_body(g, body);
    for (flag, expected) in [("true", "3\t4\t0\n"), ("false", "3\t4\t42\n")] {
        assert_eq!(
            run(&format!(
                "local flag={flag}\nlocal input=41\nlocal function read()return 3,4 end\n{text}"
            )),
            expected
        );
    }
}
#[test]
fn copied_captures_stay_inside_loop() {
    use ir::*;
    let mut g = gen::Generator::new(BTreeMap::new());
    let w = g.ctx.web("var", None, -1);
    let index = g.ctx.web("loop", None, -1);
    let branch = || {
        vec![
            Stmt::Local(w, Some(var(index))),
            Stmt::Call(call_expr(
                "save",
                vec![e(Expr::Func {
                    pid: 1,
                    caps: vec![w],
                })],
            )),
        ]
    };
    let body = vec![Stmt::For(
        index,
        number(1.0),
        number(2.0),
        number(1.0),
        vec![Stmt::If(imp_expr("flag"), branch(), branch())],
    )];
    assert_eq!(post::hoist_decls(body.clone()).unwrap(), body);
}
#[test]
fn loop_shared_return_tail_is_not_duplicated() {
    use ir::*;
    let p = Proto::parse(&proto(vec![op!("RETURN", B = 1)], vec![], 0, 0, vec![])).unwrap();
    let mut f = Function::new(0, 1, p, vec![]);
    let c = e(Expr::Bool(true));
    let block = |term, pred| Block {
        term,
        pred,
        ..Block::default()
    };
    f.blocks = vec![
        block(Term::Goto(1), vec![]),
        block(Term::Cond(c.clone(), 2, 8), vec![0, 6]),
        block(Term::Cond(c.clone(), 3, 6), vec![1]),
        block(Term::Cond(c, 4, 5), vec![2]),
        block(Term::Goto(7), vec![3]),
        block(Term::Goto(7), vec![3]),
        block(Term::Goto(1), vec![2]),
        block(Term::Ret(vec![number(1.0)]), vec![4, 5]),
        block(Term::Ret(vec![number(0.0)]), vec![1]),
    ];
    let mut g = gen::Generator::new(BTreeMap::new());
    let body = g.structure(&f).unwrap();
    let mut returns = 0;
    post::walk_stmts(&body, &mut |s| {
        if matches!(s, Stmt::Return(_)) {
            returns += 1;
        }
    });
    assert_eq!(returns, 2, "{body:?}");
    assert!(g.log.is_empty(), "{:?}", g.log);
}

#[test]
fn all_constant_arithmetic_unary_and_index_forms() {
    // Each result remains live and is printed, so none of these instruction families can disappear as dead code.
    let kinds = [
        "ADDK",
        "SUBK",
        "MULK",
        "DIVK",
        "MODK",
        "POWK",
        "IDIVK",
        "SUBRK",
        "DIVRK",
        "ANDK",
        "ORK",
        "MINUS",
        "NOT",
        "LENGTH",
        "GETTABLEN",
    ];
    for kind in kinds {
        let (param, fields, constants) = match kind {
            "SUBRK" | "DIVRK" => ("4", json!({"A":1,"B":0,"C":0}), vec![num(12)]),
            "LENGTH" => ("'abcd'", json!({"A":1,"B":0}), vec![]),
            "GETTABLEN" => ("{7}", json!({"A":1,"B":0,"C":0}), vec![]),
            "MINUS" | "NOT" => ("4", json!({"A":1,"B":0}), vec![]),
            _ => ("4", json!({"A":1,"B":0,"C":0}), vec![num(3)]),
        };
        let text = output(&function(
            vec![i(kind, fields, None), op!("RETURN", A = 1, B = 2)],
            constants,
            1,
        ));
        let actual = run(&format!(
            "local f=(function()\n{text}end)()\nprint(f({param}))"
        ));
        let expected = match kind {
            "ADDK" => "7\n",
            "SUBK" => "1\n",
            "MULK" => "12\n",
            "DIVK" => "1.3333333333333333\n",
            "MODK" => "1\n",
            "POWK" => "64\n",
            "IDIVK" => "1\n",
            "SUBRK" => "8\n",
            "DIVRK" => "3\n",
            "ANDK" => "3\n",
            "ORK" => "4\n",
            "MINUS" => "-4\n",
            "NOT" => "false\n",
            "LENGTH" => "4\n",
            "GETTABLEN" => "7\n",
            _ => unreachable!(),
        };
        assert_eq!(actual, expected, "{kind}\n{text}");
    }
}

#[test]
fn dynamic_table_get_set_and_all_constant_comparisons() {
    let text = output(&function(
        vec![
            op!("SETTABLE", A = 2, B = 0, C = 1),
            op!("GETTABLE", A = 3, B = 0, C = 1),
            op!("RETURN", A = 3, B = 2),
        ],
        vec![],
        3,
    ));
    assert_eq!(
        run(&format!(
            "local f=(function()\n{text}end)()\nprint(f({{}},'x',false))"
        )),
        "false\n"
    );
    for (kind, aux, arg, expected) in [
        ("JUMPXEQKNIL", 0, "nil", "true\n"),
        ("JUMPXEQKNIL", 0x80000000u64, "nil", "false\n"),
        ("JUMPXEQKB", 1, "true", "true\n"),
        ("JUMPXEQKB", 0x80000001, "true", "false\n"),
        ("JUMPXEQK", 0, "42", "true\n"),
        ("JUMPXEQK", 0x80000000, "42", "false\n"),
    ] {
        let text = output(&function(
            vec![
                jump!(kind, 3, A = 0, aux = aux),
                op!("LOADB", A = 1, B = 0),
                op!("RETURN", A = 1, B = 2),
                op!("LOADB", A = 1, B = 1),
                op!("RETURN", A = 1, B = 2),
            ],
            vec![num(42)],
            1,
        ));
        assert_eq!(
            run(&format!(
                "local f=(function()\n{text}end)()\nprint(f({arg}))"
            )),
            expected,
            "{kind}\n{text}"
        );
    }
    for (kind, expected) in [
        ("JUMPIFLE", "true\n"),
        ("JUMPIFLT", "true\n"),
        ("JUMPIFNOTLE", "false\n"),
        ("JUMPIFNOTLT", "false\n"),
    ] {
        let text = output(&function(
            vec![
                jump!(kind, 3, A = 0, aux = 1),
                op!("LOADB", A = 2, B = 0),
                op!("RETURN", A = 2, B = 2),
                op!("LOADB", A = 2, B = 1),
                op!("RETURN", A = 2, B = 2),
            ],
            vec![],
            2,
        ));
        assert_eq!(
            run(&format!("local f=(function()\n{text}end)()\nprint(f(1,2))")),
            expected
        );
    }
}

#[test]
fn byte_strings_and_numeric_literals_are_lossless() {
    for bytes in [
        vec![0, 1, 9, 10, 13, 34, 92, 127, 128, 255, b'1'],
        "héllo 世界".as_bytes().to_vec(),
    ] {
        let literal = decomp::lua_str(&bytes);
        let printed = run(&format!(
            "local s={literal}\nfor i=1,#s do print(string.byte(s,i)) end"
        ));
        let roundtrip: Vec<u8> = printed.lines().map(|s| s.parse().unwrap()).collect();
        assert_eq!(roundtrip, bytes);
        let text = output(&main_code(
            vec![op!("LOADK", rf = 0), op!("RETURN", B = 2)],
            vec![json!(["s", bytes])],
        ));
        assert_eq!(text, format!("return {literal}\n"));
    }
}

#[test]
fn template_indices_and_constant_fields() {
    for template in [json!(["t", [[0, 1], [2, -1]]]), json!(["i", [0, 2]])] {
        let p = main_code(
            vec![
                op!("DUPTABLE", rf = 3),
                op!("LOADN", A = 1, rf = 9),
                op!("SETTABLEKS", A = 1, B = 0, aux = 2),
                op!("RETURN", B = 2),
            ],
            vec![s("a"), num(5), s("end"), template],
        );
        // Constant-valued templates do not arise from this compiler's constructor lowering.
        let text = syntax_output(&p);
        assert!(text.contains("[\"end\"] = 9"), "{text}");
    }
}

#[test]
fn non_constructor_open_list_is_rejected_at_render_boundary() {
    let p = function(
        vec![
            op!("GETIMPORT", A = 1, aux = imp(&[0])),
            op!("CALL", A = 1, B = 1, C = 0),
            op!("SETLIST", A = 0, B = 1, C = 0, aux = 1),
            op!("RETURN", B = 2),
        ],
        vec![s("f")],
        1,
    );
    assert!(render::render(&p, None, false)
        .unwrap_err()
        .message
        .contains("SETLIST"));
}

#[test]
fn generic_for_explicit_iterator_and_unused_result_slots() {
    let text = output(&function(
        vec![
            op!("LOADNIL", A = 2),
            jump!("FORGPREP", 5, A = 0),
            op!("GETIMPORT", A = 5, aux = imp(&[0])),
            op!("MOVE", A = 6, B = 4),
            op!("CALL", A = 5, B = 2, C = 1),
            jump!("FORGLOOP", 2, A = 0, aux = 2),
            op!("RETURN", B = 1),
        ],
        vec![s("print")],
        2,
    ));
    assert_eq!(
        run(&format!(
            "local f=(function()\n{text}end)()\nf(next,{{a=7}})"
        )),
        "7\n"
    );
}

#[test]
fn by_ref_capture_lifetime_ends_at_closeupvals() {
    let p = program(vec![
        proto(
            vec![
                op!("LOADN", rf = 1),
                op!("NEWCLOSURE", A = 1, rf = 0),
                op!("CAPTURE", A = 1, B = 0),
                op!("LOADN", rf = 2),
                op!("CLOSEUPVALS", A = 0),
                op!("LOADN", rf = 3),
                op!("RETURN", A = 1, B = 2),
            ],
            vec![],
            0,
            0,
            vec![1],
        ),
        proto(
            vec![op!("GETUPVAL"), op!("RETURN", B = 2)],
            vec![],
            0,
            1,
            vec![],
        ),
    ]);
    let text = output(&p);
    assert_eq!(
        run(&format!("local f=(function()\n{text}end)()\nprint(f())")),
        "2\n"
    );
}

#[test]
fn nested_upvalue_capture_and_vararg_function_metadata() {
    let mut p = program(vec![
        proto(
            vec![
                op!("LOADN", rf = 7),
                op!("NEWCLOSURE", A = 1, rf = 0),
                op!("CAPTURE", A = 1, B = 0),
                op!("RETURN", A = 1, B = 2),
            ],
            vec![],
            0,
            0,
            vec![1],
        ),
        proto(
            vec![
                op!("NEWCLOSURE", A = 0, rf = 0),
                op!("CAPTURE", A = 2, B = 0),
                op!("RETURN", B = 2),
            ],
            vec![],
            0,
            1,
            vec![2],
        ),
        proto(
            vec![
                op!("GETUPVAL", A = 0, B = 0),
                op!("GETVARARGS", A = 1, B = 0),
                op!("RETURN", A = 0, B = 0),
            ],
            vec![],
            0,
            1,
            vec![],
        ),
    ]);
    p["protos"]["2"]["va"] = json!(true);
    // This hand-built closure chain is semantically valid; the compiler propagates its constant capture.
    let text = syntax_output(&p);
    assert_eq!(
        run(&format!(
            "local f=(function()\n{text}end)()\nprint(f()(false,9))"
        )),
        "7\tfalse\t9\n"
    );
}

#[test]
fn event_parameter_hints_and_clone_names() {
    let mut g = gen::Generator::new(BTreeMap::new());
    let child = ir::Proto::parse(&proto(
        vec![op!("RETURN", A = 0, B = 3)],
        vec![],
        2,
        0,
        vec![],
    ))
    .unwrap();
    g.ctx.protos.insert(1, child);
    let event = ir::idx(imp_expr("inputService"), ir::Key::Str("InputBegan".into()));
    let closure = ir::e(ir::Expr::Func {
        pid: 1,
        caps: vec![],
    });
    let body = vec![ir::Stmt::Call(ir::e(ir::Expr::Call {
        func: event,
        args: vec![closure],
        method: Some("Connect".into()),
    }))];
    let text = print_body(g, body);
    assert!(
        text.contains(":Connect(function(input, gameProcessed)"),
        "{text}"
    );
    assert_eq!(gen::camel("UIListLayout").as_deref(), Some("uiListLayout"));
    assert_eq!(gen::camel("42 bad").as_deref(), Some("_42bad"));
    assert_eq!(gen::camel("HTTP").as_deref(), Some("http"));
}

#[test]
fn malformed_records_report_errors_instead_of_inventing_values() {
    let p = main_code(
        vec![op!("MOVE", A = 0, B = 9), op!("RETURN", B = 2)],
        vec![],
    );
    assert!(render::render(&p, None, false)
        .unwrap_err()
        .message
        .contains("undefined register R9"));
    let p = main_code(vec![jump!("JUMP", 0)], vec![]);
    assert!(render::render(&p, None, false)
        .unwrap_err()
        .message
        .contains("empty cycle"));
    let p = main_code(
        vec![op!("GETVARARGS", B = 0), op!("NOP"), op!("RETURN", B = 0)],
        vec![],
    );
    assert_eq!(output(&p), "return ...\n");
}

#[test]
fn explicit_parameter_and_local_names_keep_distinct_lexical_bindings() {
    let p = function(
        vec![
            op!("NEWTABLE", A = 1),
            op!("GETTABLEKS", A = 2, B = 1, aux = 0),
            op!("GETIMPORT", A = 3, aux = imp(&[1])),
            op!("MOVE", A = 4, B = 0),
            op!("MOVE", A = 5, B = 2),
            op!("CALL", A = 3, B = 3, C = 1),
            op!("GETIMPORT", A = 3, aux = imp(&[1])),
            op!("MOVE", A = 4, B = 0),
            op!("MOVE", A = 5, B = 2),
            op!("CALL", A = 3, B = 3, C = 1),
            op!("RETURN", A = 2, B = 2),
        ],
        vec![s("Password"), s("print")],
        1,
    );
    let raw = output(&p);
    let annotated = render::render(&p, None, true).unwrap();
    assert!(annotated["text"]
        .as_str()
        .unwrap()
        .contains("argument<@1:p1>"));
    assert!(annotated["text"]
        .as_str()
        .unwrap()
        .contains("password<@1:2>"));
    let names = BTreeMap::from([
        ("1:p1".into(), "chosen".into()),
        ("1:2".into(), "chosen".into()),
    ]);
    let named = render::render(&p, Some(&names), false).unwrap();
    let text = named["text"].as_str().unwrap();
    assert!(text.contains("function(chosen)"), "{text}");
    assert!(
        text.contains("local chosen2 =") && text.contains(".Password"),
        "{text}"
    );
    compile(text);
    let wrap = |body: &str| format!("local f=(function()\n{body}end)()\nprint(f(7))");
    assert_eq!(run(&wrap(text)), run(&wrap(&raw)));
    assert_eq!(run(&wrap(text)), "7\tnil\n7\tnil\nnil\n");
}

#[test]
fn large_virtual_record_ids_and_fused_instruction_records() {
    let mut p = main_code(vec![op!("LOADN", rf = 7), op!("RETURN", B = 2)], vec![]);
    let decoded = p["protos"]["0"]["decoded"].as_object_mut().unwrap();
    let mut first = decoded.shift_remove("0").unwrap();
    let last = decoded.shift_remove("1").unwrap();
    let large = 1u64 << 42;
    first["nxt"] = json!(large + 1);
    first["rid"] = json!(large);
    decoded.insert(large.to_string(), first);
    decoded.insert((large + 1).to_string(), last);
    p["protos"]["0"]["entry"] = json!(large);
    assert_eq!(output(&p), "return 7\n");
    let p = json!({"main":0,"protos":{"0":{"np":0,"nup":0,"va":0,"kids":[],"consts":{},"entry":large,"decoded":{large.to_string():{"rid":large,"kind":"HOT","sem":0,"ins":[["LOADN",{"A":0,"rf":3}],["LOADN",{"A":1,"rf":7}],["ADD",{"A":0,"B":0,"C":1}]],"ctrl":["RETURN",{"A":0,"B":2}],"nxt":NONE,"alt":NONE}}}}});
    assert_eq!(syntax_output(&p), "return 3 + 7\n");
}

#[test]
fn natural_loop_break_and_infinite_loop_virtual_exit() {
    let p = function(
        vec![
            op!("LOADN", A = 1, rf = 0),
            jump!("JUMPIFNOT", 6, A = 0),
            op!("ADDK", A = 1, B = 1, C = 0),
            jump!("JUMPIFNOTLT", 6, A = 1, aux = 0),
            jump!("JUMP", 1),
            op!("NOP"),
            op!("RETURN", A = 1, B = 2),
        ],
        vec![num(1)],
        1,
    );
    let text = output(&p);
    assert!(text.contains("break"), "{text}");
    assert_eq!(
        run(&format!(
            "local f=(function()\n{text}end)()\nprint(f(3))\nprint(f(false))"
        )),
        "3\n0\n"
    );
    let p = main_code(
        vec![
            op!("GETIMPORT", aux = imp(&[0])),
            op!("CALL", B = 1, C = 1),
            jump!("JUMP", 0),
        ],
        vec![s("tick")],
    );
    let text = output(&p);
    assert_eq!(text, "while true do\n\ttick()\nend\n");
}

#[test]
fn mapped_names_sibling_scopes_and_clone_hints_do_not_collide() {
    use ir::*;
    let mut g = gen::Generator::new(BTreeMap::new());
    let a = g.ctx.web("var", Some(0), 0);
    let clone = g.ctx.web("var", Some(0), 0);
    let other = g.ctx.web("var", Some(0), 0);
    g.ctx.webs[a].hint = Some(Hint::Name("frame".into()));
    let cloned = e(Expr::Call {
        func: var(a),
        args: vec![],
        method: Some("Clone".into()),
    });
    let body = vec![
        Stmt::Local(a, Some(call_expr("make", vec![]))),
        Stmt::Local(clone, Some(cloned)),
        Stmt::If(
            imp_expr("flag"),
            vec![
                Stmt::Local(other, Some(var(a))),
                Stmt::Call(call_expr("print", vec![var(other)])),
            ],
            vec![],
        ),
        Stmt::Call(call_expr("print", vec![var(a), var(clone)])),
    ];
    let text = print_body(g, body);
    assert!(text.contains("local makeClone = make:Clone()"), "{text}");
    assert_eq!(run(&format!("local function make()return {{Clone=function()return 9 end}}end\nlocal flag=true\n{text}" )).lines().last().unwrap().split('\t').next_back(),Some("9"));
}

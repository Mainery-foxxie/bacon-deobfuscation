use bgdeobf::{astutil, classify, decode, loader, luavm, refspec};
use luavm::{Env, Interp, LuaValue as V, VmError};
use serde_json::{json, Value};

fn n(x: i64) -> V {
    V::Num(x as f64)
}

#[test]
fn shuffled_simple_dispatch_arguments_keep_their_semantic_roles() {
    let source = include_str!("samples/safe_vm.lua")
        .replace(
            "simple(r, k, imp, e, u, op, a, b, c, rf, hf, aux)",
            "simple(k, e, aux, c, r, b, rf, hf, imp, op, a, u)",
        )
        .replace(
            "simple(regs, kget, imp, env, up, code, pa, pb, pc_, prf, phf, paux)",
            "simple(kget, env, paux, pc_, regs, pb, prf, phf, imp, code, pa, up)",
        )
        .replace(
            "simple(regs, kget, imp, env, up, op, a, b, c, rf, hf, aux)",
            "simple(kget, env, aux, c, regs, b, rf, hf, imp, op, a, up)",
        );
    let l = loader::analyze(source.as_bytes()).unwrap();
    assert_eq!(
        l["call_roles"]["SIMPLE"],
        json!(["KGET", "ENV", "AUX", "C", "REGS", "B", "RF", "HF", "IMP", "OP", "A", "UP"])
    );
    let mut classifier = classify::Classifier::new(&l, Default::default(), None);
    for (op, name) in [(10, "GETIMPORT"), (11, "LOADK"), (12, "MOVE")] {
        assert!(
            classifier
                .matches(&classify::Runner::Simple(op), name, None, None, true)
                .unwrap(),
            "{name}"
        );
    }
}

fn lit(x: i64) -> Value {
    json!({"type":"AstExprConstantNumber","value":x})
}
fn local(s: &str) -> Value {
    json!({"type":"AstExprLocal","local":{"name":s,"location":s}})
}
fn block(s: Vec<Value>) -> Value {
    json!({"type":"AstStatBlock","body":s})
}
fn decl(s: &str, value: Value) -> Value {
    json!({"type":"AstStatLocal","vars":[{"name":s,"location":s}],"values":[value]})
}
fn args() -> Value {
    json!({"A":13,"B":14,"C":15,"rf":7,"Hf":0,"aux":16})
}
fn layout() -> Value {
    json!({"perm":{"0":[1,2,3,4,5,6,7,8,9,10]},"fused_perm":{"0":[1,2,3,4,5,6]},"junk_op":255})
}
fn mapping() -> Value {
    json!({"simple":{"20":"LOADN","26":"MOVE","27":"GETUPVAL"},"hot":{"22":"CALL","23":"DUPCLOSURE"},"ctrl":{"24":"FORNPREP","25":"RETURN"},"ded":{},"junk":255,"unknown":[]})
}
fn rec(op: i64, a: i64, b: i64, c: i64, rf: i64, nxt: u64, parts: Value) -> Value {
    json!({"sem":0,"ded":"-","pat":null,"mix":null,"T":[op,0,a,b,c,rf,0,0,nxt,4294967295u64],"parts":parts})
}
fn dump(recs: Vec<Value>, rs: i64) -> Value {
    let records: serde_json::Map<String, Value> = recs
        .into_iter()
        .enumerate()
        .map(|(i, r)| (i.to_string(), r))
        .collect();
    json!({"main":0,"xorkey":7,"protos":{"0":{"df":0,"rs":rs,"np":0,"va":0,"nup":0,"kids":[],"consts":{},"entry":0,"recs":records}}})
}

#[test]
fn lua_table_identity_bool_keys_and_nil_deletion() {
    let t = V::table();
    t.set(V::Bool(true), n(7)).unwrap();
    t.set(n(1), n(8)).unwrap();
    assert!(luavm::lua_eq(&t.get(&V::Bool(true)).unwrap(), &n(7)));
    assert!(luavm::lua_eq(&t.get(&n(1)).unwrap(), &n(8)));
    assert!(!luavm::lua_eq(&t, &V::table()));
    assert!(luavm::lua_eq(&t, &t.clone()));
    t.set(n(1), V::Nil).unwrap();
    assert!(t.get(&n(1)).unwrap().is_nil());
    assert!(t.set(V::Nil, n(1)).is_err());
    assert!(t.set(V::Num(f64::NAN), n(1)).is_err());
}
#[test]
fn lua_arithmetic_truth_and_special_numbers() {
    assert!(n(0).truthy());
    assert!(V::bytes(b"").truthy());
    assert!(!V::Bool(false).truthy());
    assert!(luavm::lua_eq(
        &luavm::arith("Mod", n(-5), n(3)).unwrap(),
        &n(1)
    ));
    assert!(matches!(luavm::arith("Div",n(0),n(0)).unwrap(),V::Num(v) if v.is_nan()));
    assert!(luavm::arith("Add", V::Bool(true), n(1)).is_err());
}
#[test]
fn lua_multiple_returns_groups_closures_and_steps() {
    let mut vm = Interp::new(100);
    let mut env = Env::default();
    let f = json!({"type":"AstExprFunction","location":"f","args":[],"vararg":false,"body":block(vec![json!({"type":"AstStatReturn","list":[local("x"),lit(2)]})])});
    let call = json!({"type":"AstExprCall","func":local("f"),"args":[],"self":false});
    vm.exec_block(
        &block(vec![
            decl("x", lit(1)),
            decl("f", f),
            json!({"type":"AstStatAssign","vars":[local("x")],"values":[lit(3)]}),
        ]),
        &mut env,
    )
    .unwrap();
    let vs = vm.eval_multi(&call, &mut env).unwrap();
    assert_eq!(vs.len(), 2);
    assert!(luavm::lua_eq(&vs[0], &n(3)));
    let grouped = json!({"type":"AstExprGroup","expr":call});
    assert_eq!(vm.eval_multi(&grouped, &mut env).unwrap().len(), 1);
    let forever = json!({"type":"AstStatWhile","condition":{"type":"AstExprConstantBool","value":true},"body":block(vec![])});
    assert!(matches!(
        vm.exec_stat(&forever, &mut env),
        Err(VmError::StepLimit)
    ));
    assert!(matches!(
        vm.eval(&json!({"type":"Unknown"}), &mut env),
        Err(VmError::Unsupported(_))
    ));
}
#[test]
fn lua_host_bytes_unpack_overlap_and_pcall() {
    let mut vm = Interp::new(1000);
    let char_fn = vm.global("string").get(&V::bytes(b"char")).unwrap();
    let result = vm.call(char_fn, vec![n(0), n(128), n(255)]).unwrap();
    assert!(matches!(&result[0],V::Str(b) if b==&vec![0,128,255]));
    let t = V::table();
    for i in 1..=4 {
        t.set(n(i), n(i)).unwrap();
    }
    let f = vm.global("table").get(&V::bytes(b"move")).unwrap();
    vm.call(f, vec![t.clone(), n(1), n(3), n(2)]).unwrap();
    for (i, v) in [1, 1, 2, 3].into_iter().enumerate() {
        assert!(luavm::lua_eq(&t.get(&n(i as i64 + 1)).unwrap(), &n(v)));
    }
    let out = vm
        .call(
            vm.global("pcall"),
            vec![vm.global("error"), V::bytes(b"bad")],
        )
        .unwrap();
    assert!(matches!(&out[0], V::Bool(false)));
    assert_eq!(out[1].repr(), "'bad'");
}
#[test]
fn register_offsets_only_shift_register_operands() {
    for (name, fields) in [
        ("MOVE", vec!["A", "B"]),
        ("ADD", vec!["A", "B", "C"]),
        ("JUMPIFEQ", vec!["A", "aux"]),
        ("SUBRK", vec!["A", "C"]),
        ("GETUPVAL", vec!["A"]),
        ("RETURN", vec!["A"]),
        ("JUMP", vec![]),
    ] {
        let got = decode::unshift_args(name, &args(), 10).unwrap();
        for field in ["A", "B", "C", "rf", "Hf", "aux"] {
            assert_eq!(
                got[field].as_i64().unwrap(),
                args()[field].as_i64().unwrap() - if fields.contains(&field) { 10 } else { 0 },
                "{name}/{field}"
            );
        }
    }
    for kind in 0..=2 {
        let mut a = args();
        a["A"] = json!(kind);
        assert_eq!(
            decode::unshift_args("CAPTURE", &a, 10).unwrap(),
            json!({"A":kind,"B":if kind < 2 {4} else {14},"C":15,"rf":7,"Hf":0,"aux":16})
        );
    }
    assert!(decode::unshift_args("NOT_A_REAL_OP", &args(), 3).is_err());
}
#[test]
fn decode_offsets_and_preserve_metadata() {
    let d = dump(
        vec![
            rec(20, 3, 0, 0, 5, 1, json!([])),
            rec(26, 7, 4, 0, 0, 2, json!([])),
            rec(25, 5, 2, 0, 0, 4294967295, json!([])),
        ],
        3,
    );
    let out = decode::decode_program(&d, &mapping(), &layout()).unwrap();
    let p = &out["protos"]["0"];
    let ds = &p["decoded"];
    assert_eq!(p["recs"], d["protos"]["0"]["recs"]);
    assert_eq!(
        ds["0"]["ins"][0],
        json!(["LOADN",{"A":0,"B":0,"C":0,"rf":5,"Hf":0,"aux":0}])
    );
    assert_eq!(ds["0"]["ins"].as_array().unwrap().len(), 1);
    assert_eq!(ds["1"]["ins"][0][0], "MOVE");
    assert_eq!(ds["1"]["ins"][0][1]["A"], 4);
    assert_eq!(ds["1"]["ins"][0][1]["B"], 1);
    assert_eq!(ds["2"]["ctrl"][0], "RETURN");
    assert_eq!(ds["2"]["ctrl"][1]["A"], 2);
    assert_eq!(ds["2"]["ctrl"][1]["B"], 2);
    assert_eq!(ds["0"]["nxt"], 1);
    assert_eq!(ds["1"]["nxt"], 2);
    assert_eq!(ds["2"]["nxt"], decode::NONE);
}
#[test]
fn captures_expand_large_ids_and_use_capture_reader() {
    let d = dump(
        vec![rec(
            23,
            4,
            0,
            0,
            2,
            1,
            json!([
                [0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                [0, 0, 0, 3, 0, 0, 0, 0, 0, 0],
                [0, 0, 1, 5, 0, 0, 0, 0, 0, 0],
                [0, 0, 2, 1, 0, 0, 0, 0, 0, 0]
            ]),
        )],
        3,
    );
    let mut l = layout();
    l["cap_perm"] = json!({"0":[1,3,4,9]});
    let out = decode::decode_program(&d, &mapping(), &l).unwrap();
    let ds = &out["protos"]["0"]["decoded"];
    assert_eq!(ds["0"]["nxt"], json!(1u64 << 40));
    for (q, kind, b) in [(1u64, 0, 0), (2, 1, 2), (3, 2, 1)] {
        let r = &ds[(q << 40).to_string()];
        assert_eq!(r["ins"][0][0], "CAPTURE");
        assert_eq!(r["ins"][0][1]["A"], kind);
        assert_eq!(r["ins"][0][1]["B"], b);
    }
}
#[test]
fn fused_head_tail_loops_and_missing_parts() {
    for head in [true, false] {
        for pre in [true, false] {
            let lp = json!({"pre":if pre {json!([["LOADN",0,{},false]])}else{json!([])},"head":if head {json!([["FORGLOOP",2,{},true]])}else{json!([])},"body":[["MOVE",1,{},false]],"tail":if head {json!([])}else{json!([["FORNLOOP",2,{},true]])}});
            let mut m = mapping();
            m["ded"]["4:9"] = json!({"loop":lp});
            let mut r = rec(
                0,
                0,
                0,
                0,
                0,
                1,
                json!([[4, 0, 0, 1, 0, 0], [6, 7, 0, 0, 0, 0], [5, 0, 2, 0, 0, 0]]),
            );
            r["sem"] = json!(9);
            r["ded"] = json!("4");
            let out = decode::decode_program(&dump(vec![r.clone()], 0), &m, &layout()).unwrap();
            let ds = &out["protos"]["0"]["decoded"];
            assert_eq!(
                ds["0"]["nxt"],
                json!(if !head && pre { 1u64 << 40 } else { 1 })
            );
            assert_eq!(
                ds.as_object().unwrap().len(),
                if head {
                    3
                } else if pre {
                    2
                } else {
                    1
                }
            );
            r["parts"] = json!([]);
            assert!(decode::decode_program(&dump(vec![r], 0), &m, &layout()).is_err());
        }
    }
}
#[test]
fn split_by_part_reference_assertions() {
    let split = |items: &[(&str, &[usize])]| {
        classify::split_by_part(
            items
                .iter()
                .map(|(s, p)| (json!(s), p.iter().copied().collect()))
                .collect(),
        )
    };
    assert_eq!(
        split(&[("a", &[0]), ("x", &[]), ("b", &[1])]),
        Some(vec![
            (0, vec![json!("a"), json!("x")]),
            (1, vec![json!("b")])
        ])
    );
    assert_eq!(
        split(&[("decl", &[]), ("a", &[0]), ("a2", &[0]), ("b", &[1])]),
        Some(vec![
            (0, vec![json!("decl"), json!("a"), json!("a2")]),
            (1, vec![json!("b")])
        ])
    );
    assert!(split(&[("a", &[0]), ("ab", &[0, 1])]).is_none());
    assert!(split(&[("x", &[]), ("y", &[])]).is_none());
    assert_eq!(
        split(&[("a", &[0]), ("a2", &[0]), ("b", &[1]), ("b2", &[1])]),
        Some(vec![
            (0, vec![json!("a"), json!("a2")]),
            (1, vec![json!("b"), json!("b2")])
        ])
    );
    assert_eq!(split(&[]), Some(vec![]));
}
#[test]
fn unsupported_opcode_and_bad_pattern_are_explicit_errors() {
    assert!(decode::decode_program(
        &dump(vec![rec(999, 0, 0, 0, 0, 1, json!([]))], 0),
        &mapping(),
        &layout()
    )
    .is_err());
    let mut r = rec(20, 0, 0, 0, 0, 1, json!([]));
    r["pat"] = json!([1]);
    assert!(decode::decode_program(&dump(vec![r], 0), &mapping(), &layout()).is_err());
}

fn parse(text: &str) -> Value {
    astutil::parse_ast(text.as_bytes()).unwrap()
}
fn seed_loader(text: &str, roles: &[&str]) -> (Value, Value, String) {
    let root = parse(text);
    let body = &root["body"];
    let mut role_local = serde_json::Map::new();
    for (i, &r) in roles.iter().enumerate() {
        role_local.insert(r.into(), body[0]["vars"][i].clone());
    }
    let l = json!({"role_local":role_local,"junk_op":null,"frame_body":body,"roles":{},"funcs":{},"simple_table_items":[[body[1]["vars"][0],[]]]});
    (
        l,
        body[2]["func"]["body"].clone(),
        body[1]["vars"][0]["location"].as_str().unwrap().into(),
    )
}
const TEMPLATE_HANDLER: &str = r#"local R, KGET, A, RF, NXT, TOP
local _P = {}
local function handler()
 local tpl = KGET(RF)
 local t = {}
 if tpl[1] == _P then
  for i = 1, #tpl[2] do
   local pair = tpl[2][i]
   if pair[2] >= 0 then t[KGET(pair[1])] = KGET(pair[2]) end
  end
 end
 R[A] = t
end"#;
#[test]
fn template_tag_identity_and_constant_table_seeding() {
    let (mut l, body, tag_loc) =
        seed_loader(TEMPLATE_HANDLER, &["REGS", "KGET", "A", "RF", "NXT", "TOP"]);
    let mut c = classify::Classifier::new(&l, Default::default(), None);
    let st = refspec::State::new("DUPTABLE", 13, None, None, 0).unwrap();
    let env = c.seed(&st).unwrap();
    assert!(env.get(&tag_loc).is_none());
    assert!(luavm::lua_eq(c.interp.unbound.as_ref().unwrap(), &st.tag));
    assert!(c
        .matches(
            &classify::Runner::Block(body.clone()),
            "DUPTABLE",
            None,
            None,
            true
        )
        .unwrap());
    assert!(!c
        .matches(&classify::Runner::Block(body), "NEWTABLE", None, None, true)
        .unwrap());
    l["simple_table_items"][0][1] = json!([[7,{"type":"AstExprConstantNumber","value":10.0}],[8,{"type":"AstExprConstantString","value":"x"}],[9,{"type":"AstExprConstantBool","value":true}]]);
    let mut c = classify::Classifier::new(&l, Default::default(), None);
    let env = c.seed(&st).unwrap();
    let tab = env.get(&tag_loc).unwrap();
    assert!(luavm::lua_eq(&tab.get(&n(7)).unwrap(), &n(10)));
    assert!(luavm::lua_eq(&tab.get(&n(8)).unwrap(), &V::bytes(b"x")));
    assert!(luavm::lua_eq(&tab.get(&n(9)).unwrap(), &V::Bool(true)));
}
#[test]
fn base4_profile_detection_and_seeding() {
    let text = r#"local REGS, A, NXT, ALT, TOP
local profile=0
local variants={profile%4,math.floor(profile/4)%4,math.floor(profile/16)%4}
local function handler()
 if variants[1]==0 then NXT=ALT
 elseif variants[1]==1 then local target=ALT;NXT=target
 elseif variants[1]==2 then NXT,ALT=ALT,NXT
 elseif variants[1]==3 then repeat NXT=ALT until true
 else error('bad variant') end
end"#;
    let root = parse(text);
    let b = &root["body"];
    let mut roles = serde_json::Map::new();
    for (i, r) in ["REGS", "A", "NXT", "ALT", "TOP"].iter().enumerate() {
        roles.insert((*r).into(), b[0]["vars"][i].clone());
    }
    roles.insert("PROF".into(), b[1]["vars"][0].clone());
    let mut l =
        json!({"role_local":roles,"frame_body":b,"simple_table_items":[],"roles":{},"funcs":{}});
    let mut c = classify::Classifier::new(&l, Default::default(), None);
    assert_eq!(c.profile_tables.len(), 1);
    assert!(c
        .matches(
            &classify::Runner::Block(b[3]["func"]["body"].clone()),
            "JUMP",
            None,
            None,
            true
        )
        .unwrap());
    l["frame_body"] =
        parse(&text.replace("math.floor(profile/4)", "math.ceil(profile/4)"))["body"].clone();
    assert!(classify::Classifier::new(&l, Default::default(), None)
        .profile_tables
        .is_empty());
}
const INNER_LOOP: &str = r#"local function handler(rec, regs, dec, nxt, top)
 local p0=rec[11];local a=dec(p0)
 local p2=rec[13];local b,c=dec(p2)
 while true do
  regs[b]=regs[b]+1
  if regs[b]>c then return nxt,top end
  do local p1=rec[12];local x,y=dec(p1);do regs[y]=regs[x] end end
 end
end"#;
#[test]
fn fused_loop_hoists_inner_reads_and_keeps_unrelated_blocks() {
    use std::collections::{BTreeMap, BTreeSet};
    let root = parse(INNER_LOOP);
    let f = &root["body"][0]["func"];
    let body = luavm::array(&f["body"]["body"]);
    let r = luavm::location(&f["args"][0]);
    let (mut vp, mut po, mut reads) = (BTreeMap::new(), BTreeMap::new(), BTreeMap::new());
    for s in &body[..body.len() - 1] {
        classify::register_read(s, &r, &mut vp, &mut po, &mut reads);
    }
    let groups = |ss: Vec<Value>, po: &BTreeMap<String, usize>| {
        classify::split_by_part(
            ss.into_iter()
                .map(|s| {
                    let mut ns = vec![];
                    luavm::walk(&s, &mut ns);
                    let parts: BTreeSet<_> = ns
                        .iter()
                        .filter(|n| luavm::ty(n) == "AstExprLocal")
                        .filter_map(|n| po.get(&luavm::location(&n["local"])).copied())
                        .collect();
                    (s, parts)
                })
                .collect(),
        )
        .unwrap()
    };
    let loop_body = luavm::array(&body.last().unwrap()["body"]["body"]);
    assert_eq!(
        groups(loop_body.to_vec(), &po)
            .iter()
            .map(|g| g.0)
            .collect::<Vec<_>>(),
        vec![2]
    );
    let hoisted = classify::hoist_loop_reads(loop_body, &r, &mut vp, &mut po, &mut reads);
    assert_eq!(reads.keys().copied().collect::<Vec<_>>(), vec![0, 1, 2]);
    assert_eq!(reads[&1].len(), 2);
    let got = groups(hoisted.clone(), &po);
    assert_eq!(got.iter().map(|g| g.0).collect::<Vec<_>>(), vec![2, 1]);
    assert_eq!(
        got.iter().map(|g| g.1.len()).collect::<Vec<_>>(),
        vec![2, 1]
    );
    let mut nodes = vec![];
    for s in &got[0].1 {
        luavm::walk(s, &mut nodes);
    }
    assert!(nodes.iter().any(|n| luavm::ty(n) == "AstStatReturn"));
    assert_eq!(luavm::ty(&hoisted[2]), "AstStatBlock");
    let front = classify::hoist_loop_reads(&hoisted, &r, &mut vp, &mut po, &mut reads);
    assert_eq!(front, hoisted);
}
#[test]
fn numeric_closures_use_derived_proto_fields() {
    for (source, expected) in [
        (
            "local function h(M,K) local x=M[3];local y=M[7][x];K(1) end",
            "NEWCLOSURE",
        ),
        ("local function h(M,K) local x=M[3];K(x) end", "DUPCLOSURE"),
    ] {
        let root = parse(source);
        let f = &root["body"][0]["func"];
        let l = json!({"layout":"numeric","numeric_fields":{"num_upvalues":3,"children":7},"role_local":{"PROTO":f["args"][0],"KGET":f["args"][1]}});
        let c = classify::Classifier::new(&l, Default::default(), None);
        assert_eq!(c.structural(&f["body"]).as_deref(), Some(expected));
    }
}
#[test]
fn safe_fixture_mapping_and_complete_program_match_python_golden() {
    let golden: Value =
        serde_json::from_str(include_str!("fixtures/safe_vm_reference.json")).unwrap();
    let l = loader::analyze(include_bytes!("samples/safe_vm.lua")).unwrap();
    let mapping =
        classify::classify(&l, &golden["dump"], std::time::Duration::from_secs(60)).unwrap();
    assert_eq!(mapping, golden["mapping"]);
    let program = decode::decode_program(&golden["dump"], &mapping, &l).unwrap();
    assert_eq!(program, golden["program"]);
}
#[test]
fn classification_deadline_and_all_reference_specs_execute() {
    for name in refspec::specs() {
        for trial in 0..24 {
            let mut st =
                refspec::State::new(&name, 7919 * trial as u64 + 13, None, None, trial).unwrap();
            let result = st.run();
            assert!(
                result.is_ok() || matches!(result, Err(VmError::Lua(_))),
                "{name}: {result:?}"
            );
        }
    }
    let mut c = classify::Classifier::new(
        &json!({}),
        Default::default(),
        Some(std::time::Instant::now()),
    );
    assert_eq!(
        c.matches(
            &classify::Runner::Block(block(vec![])),
            "NOP",
            None,
            None,
            true
        )
        .unwrap_err()
        .code,
        "timeout"
    );
}

fn simple_loader(source: &str) -> Value {
    let root = parse(source);
    json!({"simple_fn":root["body"][0]["func"],"call_roles":{"SIMPLE":["REGS","KGET","IMP","ENV","UP","OP","A","B","C","RF","HF","AUX"]},"role_local":{},"roles":{},"funcs":{},"simple_table_items":[],"frame_body":[],"hot_chain":[],"ctrl_chain":[],"ded_decl":{},"junk_op":null})
}
#[test]
fn hf_selectors_and_fixed_call_handlers_are_classified_by_behavior() {
    let l=simple_loader("local function h(r,k,imp,e,u,op,a,b,c,rf,hf,aux) if hf%2==0 then r[a]=r[b] else r[a]=k(rf) end end");
    let m = classify::classify(
        &l,
        &json!({"protos":{},"simple":[44]}),
        std::time::Duration::from_secs(60),
    )
    .unwrap();
    assert_eq!(m["unknown"], json!([]));
    assert_eq!(m["simple"]["44"][0], "HF");
    for hf in 0..64 {
        assert_eq!(
            m["simple"]["44"][1][hf.to_string()],
            if hf % 2 == 0 { "MOVE" } else { "LOADK" }
        );
    }
    let l = simple_loader(
        "local function h(r,k,imp,e,u,op,a,b,c,rf,hf,aux) r[a]=r[a](r[a+1],r[a+2]) end",
    );
    let m = classify::classify(
        &l,
        &json!({"protos":{},"simple":[45]}),
        std::time::Duration::from_secs(30),
    )
    .unwrap();
    assert_eq!(m["simple"]["45"], "CALL_3_2");
    assert_eq!(
        classify::strip_fixed("CALL_3_2"),
        ("CALL".into(), json!({"B":3,"C":2}))
    );
}
#[test]
fn dedicated_fused_numeric_loop_is_classified_and_expanded() {
    let source = r#"local function h(rec,R,dec,nxt,top)
local p0=rec[11];local a,b,c,rf=dec(p0)
local p2=rec[13];local base=dec(p2)
R[a]=rf
while true do
 do local p1=rec[12];local x,y=dec(p1);R[x]=R[y] end
 R[base+2] += R[base+1]
 if not ((R[base+1]>0 and R[base+2]<=R[base]) or (R[base+1]<=0 and R[base+2]>=R[base])) then return nxt,top end
end end"#;
    let root = parse(source);
    let f = &root["body"][0]["func"];
    let mut l = layout();
    l["ded_decl"] = json!({"4":[[99,f]]});
    l["call_roles"] = json!({"D4":["REC","REGS","FUSED","NXT","TOP"]});
    let mut r = rec(
        0,
        0,
        0,
        0,
        0,
        1,
        json!([[4, 0, 0, 7, 0, 0], [6, 7, 0, 0, 0, 0], [0, 0, 0, 0, 0, 0]]),
    );
    r["sem"] = json!(99);
    r["ded"] = json!("4");
    let d = dump(vec![r], 0);
    let m = classify::classify(&l, &d, std::time::Duration::from_secs(30)).unwrap();
    assert_eq!(m["unknown"], json!([]));
    assert_eq!(
        m["ded"]["4:99"],
        json!({"loop":{"pre":[["LOADN",0,{},false]],"head":[],"body":[["MOVE",1,{},false]],"tail":[["FORNLOOP",2,{},true]]}})
    );
    let p = decode::decode_program(&d, &m, &l).unwrap();
    assert_eq!(p["protos"]["0"]["decoded"]["0"]["ins"][0][0], "LOADN");
    assert_eq!(
        p["protos"]["0"]["decoded"][(1u64 << 40).to_string()]["ctrl"][0],
        "FORNLOOP"
    );
}
#[test]
fn lua_host_primitives_numeric_format_and_tagged_constants() {
    let root = parse(
        r##"local t=table.pack(4,nil,6)
local x,y,z=unpack(t,1,t.n)
table.insert(t,2,5)
local s=table.concat({1,"a",3},":")
return x,y,z,t[2],s,select("#",x,y,z),select(-1,x,y,z),tonumber("4.5"),tonumber(true),tostring(1.234567890123456),getmetatable(setmetatable({},t))==t,bit32.extract(255,2,3),bit32.arshift(2147483648,31),bit32.lrotate(2147483648,1),string.sub("abcde",-3,-1),#string.char(0,255)"##,
    );
    let mut vm = Interp::new(1000);
    let mut env = Env::default();
    let Err(VmError::Return(v)) = vm.exec_block(&root, &mut env) else {
        panic!("no return")
    };
    assert_eq!(
        v.iter().map(V::string).collect::<Vec<_>>(),
        vec![
            "4",
            "nil",
            "6",
            "5",
            "1:a:3",
            "3",
            "6",
            "4.5",
            "nil",
            "1.2345678901235",
            "true",
            "7",
            "4294967295",
            "1",
            "cde",
            "2"
        ]
    );
    let mut d = dump(vec![rec(20, 0, 0, 0, 0, 1, json!([]))], 0);
    d["protos"]["0"]["consts"] = json!({"0":["n","nan"],"1":["n","inf"],"2":["n","-inf"],"3":["s",[0,128,255]],"4":["n",-0.0]});
    let p = decode::decode_program(&d, &mapping(), &layout()).unwrap();
    assert_eq!(p["protos"]["0"]["consts"], d["protos"]["0"]["consts"]);
    assert_eq!(
        bgdeobf::model::decode_number(&p["protos"]["0"]["consts"]["4"][1])
            .unwrap()
            .to_bits(),
        (-0.0f64).to_bits()
    );
}

#[test]
fn malformed_functions_are_not_noops_and_opaque_numbers_keep_precision() {
    let mut vm = Interp::new(100);
    let mut env = Env::default();
    assert!(matches!(
        vm.exec_block(&Value::Null, &mut env),
        Err(VmError::Unsupported(_))
    ));
    assert!(matches!(
        vm.call(V::function(Value::Null, env.cells.clone()), vec![]),
        Err(VmError::Unsupported(_))
    ));
    let a = V::Num(1.234567890123456);
    let b = V::Num(1.234567890123457);
    assert_ne!(a.key(), b.key());
    let a = luavm::arith("Add", refspec::opaque("x", &[]), a).unwrap();
    let b = luavm::arith("Add", refspec::opaque("x", &[]), b).unwrap();
    assert!(!luavm::lua_eq(&a, &b));
}

#[test]
fn discarded_trials_release_cycles_and_preserve_reachable_closures() {
    let root = V::table();
    let heap = luavm::TrialHeap::new(vec![root.clone()]);
    let weak = {
        let t = V::table();
        t.set(V::bytes(b"self"), t.clone()).unwrap();
        let V::Table(r) = t else { unreachable!() };
        std::rc::Rc::downgrade(&r)
    };
    let mut env = Env::default();
    let v = V::table();
    v.set(n(1), n(7)).unwrap();
    env.declare("x".into(), v);
    let f = V::function(
        json!({"type":"AstExprFunction","args":[],"location":"cycle-test","vararg":false,"body":block(vec![json!({"type":"AstStatReturn","list":[local("x")]})])}),
        env.cells.clone(),
    );
    env.declare("self".into(), f.clone());
    root.set(n(1), f.clone()).unwrap();
    drop(env);
    drop(heap);
    assert!(weak.upgrade().is_none());
    let mut vm = Interp::new(100);
    let result = vm.call(f, vec![]).unwrap();
    assert!(luavm::lua_eq(&result[0].get(&n(1)).unwrap(), &n(7)));
}

#[test]
fn native_ast_number_boundary_preserves_special_values() {
    let mut vm = Interp::new(100);
    let mut env = Env::default();
    for (encoded, expected) in [
        (json!("nan"), f64::NAN),
        (json!("inf"), f64::INFINITY),
        (json!("-inf"), f64::NEG_INFINITY),
        (json!(-0.0), -0.0),
    ] {
        let v = vm
            .eval(
                &json!({"type":"AstExprConstantNumber","value":encoded}),
                &mut env,
            )
            .unwrap();
        let V::Num(v) = v else { panic!("not numeric") };
        assert!(if expected.is_nan() {
            v.is_nan()
        } else {
            v.to_bits() == expected.to_bits()
        });
    }
}

#[test]
fn domain_restricted_calls_preserve_fixed_fields() {
    let source = r#"local function h(r,k,imp,e,u,op,a,b,c,rf,hf,aux)
    local values=table.pack(r[a](r[a+1],r[a+2]))
    if c==0 then for i=1,values.n do r[a+i-1]=values[i] end
    else for i=1,c-1 do r[a+i-1]=values[i] end end end"#;
    let mut l = simple_loader(source);
    l["perm"] = layout()["perm"].clone();
    l["fused_perm"] = layout()["fused_perm"].clone();
    let mut d = dump(vec![rec(45, 0, 3, 2, 0, 1, json!([]))], 0);
    d["simple"] = json!([45]);
    let m = classify::classify(&l, &d, std::time::Duration::from_secs(30)).unwrap();
    assert_eq!(m["unknown"], json!([]));
    assert_eq!(m["simple"]["45"], "CALL");
    let dom = classify::collect_domains(&l, &d).unwrap();
    let st = refspec::State::new("CALL", 13, Some(&dom["op:45"]), None, 0).unwrap();
    assert_eq!(st.field("B"), 3.0);
    assert_eq!(st.field("C"), 2.0);
}

#[test]
fn dedicated_capture_tailcall_fixed_operands_patterns_and_junk_decode() {
    let mut m = mapping();
    m["ded"] = json!({"1:9":[["CAPTURE",0,{},false]],"2:10":[["CALL",0,{"B":3,"C":2},false]],"6:267":[["TAILCALL",0,{},false]]});
    let mut cap = rec(0, 0, 0, 0, 0, 1, json!([[0, 0, 0, 7, 0, 0, 0, 0, 0, 0]]));
    cap["sem"] = json!(9);
    cap["ded"] = json!("1");
    let mut fixed = rec(0, 0, 0, 0, 0, 2, json!([[8, 9, 10, 11, 12, 13]]));
    fixed["sem"] = json!(10);
    fixed["ded"] = json!("2");
    let mut pat = rec(
        0,
        0,
        0,
        0,
        0,
        3,
        json!([[1, 0, 0, 5, 0, 0], [2, 1, 0, 0, 0, 0]]),
    );
    pat["pat"] = json!([20 ^ 7, 26 ^ 7]);
    let mut junk = rec(255, 0, 0, 0, 0, 7, json!([]));
    junk["mix"] = json!(3);
    let mut d = dump(vec![cap, fixed, pat, junk], 0);
    let mut tail = rec(0, 4, 0, 0, 0, 1, json!([[4, 3, 0, 0, 0, 0]]));
    tail["sem"] = json!(11);
    tail["ded"] = json!("6");
    d["protos"]["1"] = d["protos"]["0"].clone();
    d["protos"]["1"]["recs"] = json!({"0":tail});
    let mut l = layout();
    l["cap_perm"] = json!({"0":[1,3,4,9]});
    let p = decode::decode_program(&d, &m, &l).unwrap();
    let ds = &p["protos"]["0"]["decoded"];
    assert_eq!(
        ds["0"]["ins"][0],
        json!(["CAPTURE",{"A":0,"B":7,"C":0,"rf":0,"Hf":0,"aux":0}])
    );
    assert_eq!(
        ds["1"]["ins"][0],
        json!(["CALL",{"A":8,"B":3,"C":2,"rf":0,"Hf":0,"aux":0}])
    );
    assert_eq!(ds["2"]["kind"], "PAT");
    assert_eq!(ds["2"]["ins"][1][0], "MOVE");
    assert_eq!(ds["3"]["nxt"], 7 ^ 3);
    let tail = &p["protos"]["1"]["decoded"]["0"];
    assert_eq!(tail["kind"], "TAILCALL");
    assert_eq!(tail["ins"][0][0], "CALL");
    assert_eq!(tail["ins"][0][1]["C"], 0);
    assert_eq!(tail["ctrl"][0], "RETURN");
    let dom = classify::collect_domains(&l, &d).unwrap();
    assert_eq!(dom["ded:6:267:0"][0]["A"], 4.0);
}

#[test]
fn permuted_layouts_and_bad_selectors_do_not_use_fixture_positions() {
    let mut r = rec(26, 7, 2, 0, 0, 1, json!([]));
    r["T"].as_array_mut().unwrap().reverse();
    let d = dump(vec![r], 0);
    let mut l = layout();
    l["perm"] = json!({"0":[10,9,8,7,6,5,4,3,2,1]});
    let p = decode::decode_program(&d, &mapping(), &l).unwrap();
    assert_eq!(p["protos"]["0"]["decoded"]["0"]["ins"][0][1]["A"], 7);
    assert_eq!(p["protos"]["0"]["decoded"]["0"]["ins"][0][1]["B"], 2);
    let mut m = mapping();
    m["simple"]["26"] = json!(["HF",{"1":"MOVE"}]);
    assert!(decode::decode_program(&d, &m, &l)
        .unwrap_err()
        .message
        .contains("selector 0"));
}

#[test]
fn classifier_seed_preserves_latin1_table_keys_and_values() {
    let (mut l, _, loc) = seed_loader(TEMPLATE_HANDLER, &["REGS", "KGET", "A", "RF", "NXT", "TOP"]);
    l["simple_table_items"][0][1] = json!([["\u{ff}", {
        "type":"AstExprConstantString", "value":"\0\u{80}\u{ff}"
    }]]);
    let st = refspec::State::new("DUPTABLE", 13, None, None, 0).unwrap();
    let mut c = classify::Classifier::new(&l, Default::default(), None);
    let env = c.seed(&st).unwrap();
    let tab = env.get(&loc).unwrap();
    assert!(luavm::lua_eq(
        &tab.get(&V::bytes(&[255])).unwrap(),
        &V::bytes(&[0, 128, 255])
    ));
    assert!(tab.get(&V::bytes(&[195, 191])).unwrap().is_nil());
}

#[test]
fn bit32_wraps_finite_numbers_outside_signed_integer_range() {
    let mut vm = Interp::new(100);
    let bit32 = vm.global("bit32");
    for (name, a, expected) in [
        ("band", vec![V::Num(9223372036854775808.0)], 0),
        ("bor", vec![V::Num(18446744073709551616.0), n(7)], 7),
        ("bxor", vec![V::Num(-18446744073709551616.0), n(3)], 3),
        ("bnot", vec![V::Num(9223372036854775808.0)], 4294967295),
        ("rshift", vec![V::Num(9223372036854775808.0), n(1)], 0),
    ] {
        let f = bit32.get(&V::bytes(name.as_bytes())).unwrap();
        let got = vm.call(f, a).unwrap();
        assert!(luavm::lua_eq(&got[0], &n(expected)), "{name}: {got:?}");
    }
}

#[test]
fn extreme_host_ranges_report_errors_without_native_integer_overflow() {
    let mut vm = Interp::new(100);
    let table = vm.global("table");
    let bit32 = vm.global("bit32");
    let large = V::Num(9223372036854775808.0);
    let small = V::Num(-9223372036854775808.0);
    let extract = bit32.get(&V::bytes(b"extract")).unwrap();
    assert!(matches!(
        vm.call(extract, vec![n(0), large.clone(), n(1)]),
        Err(VmError::Lua(_))
    ));
    let unpack = table.get(&V::bytes(b"unpack")).unwrap();
    assert!(matches!(
        vm.call(unpack, vec![V::table(), small.clone(), large.clone()]),
        Err(VmError::Lua(_))
    ));
    let mover = table.get(&V::bytes(b"move")).unwrap();
    assert!(matches!(
        vm.call(mover, vec![V::table(), small, large.clone(), n(0)]),
        Err(VmError::StepLimit)
    ));
    let rotate = bit32.get(&V::bytes(b"lrotate")).unwrap();
    let values = vm.call(rotate, vec![n(1), large]).unwrap();
    assert!(luavm::lua_eq(&values[0], &n(1)));
}

#[test]
fn string_byte_default_end_uses_the_original_start() {
    let mut vm = Interp::new(100);
    let byte = vm.global("string").get(&V::bytes(b"byte")).unwrap();
    for start in [0, -1, -10] {
        assert!(vm
            .call(byte.clone(), vec![V::bytes(b"abc"), n(start)])
            .unwrap()
            .is_empty());
    }
    let got = vm.call(byte, vec![V::bytes(b"abc"), n(0), n(2)]).unwrap();
    assert_eq!(
        got.iter().map(V::string).collect::<Vec<_>>(),
        vec!["97", "98"]
    );
}

#[test]
fn unpack_extreme_bounds_return_an_error_instead_of_overflowing() {
    let mut vm = Interp::new(100);
    let unpack = vm.global("unpack");
    assert!(matches!(
        vm.call(
            unpack,
            vec![V::table(), V::Num(i64::MIN as f64), V::Num(i64::MAX as f64)]
        ),
        Err(VmError::Lua(_))
    ));
}

#[test]
fn front_loaded_loop_reads_and_unrelated_blocks_keep_reference_grouping() {
    use std::collections::{BTreeMap, BTreeSet};
    let root = parse(
        r#"local function handler(rec, regs, dec, nxt, top)
 local p0=rec[11];local a=dec(p0)
 local p1=rec[12];local x,y=dec(p1)
 local p2=rec[13];local b,c=dec(p2)
 while true do
  regs[b]=regs[b]+1
  if regs[b]>c then return nxt,top end
  do regs[y]=regs[x] end
 end
end"#,
    );
    let f = &root["body"][0]["func"];
    let body = luavm::array(&f["body"]["body"]);
    let r = luavm::location(&f["args"][0]);
    let (mut vp, mut po, mut reads) = (BTreeMap::new(), BTreeMap::new(), BTreeMap::new());
    for s in &body[..body.len() - 1] {
        classify::register_read(s, &r, &mut vp, &mut po, &mut reads);
    }
    let original = luavm::array(&body.last().unwrap()["body"]["body"]);
    let hoisted = classify::hoist_loop_reads(original, &r, &mut vp, &mut po, &mut reads);
    assert_eq!(reads.keys().copied().collect::<Vec<_>>(), vec![0, 1, 2]);
    assert_eq!(hoisted, original);
    assert_eq!(
        hoisted.iter().map(luavm::ty).collect::<Vec<_>>(),
        vec!["AstStatAssign", "AstStatIf", "AstStatBlock"]
    );
    let groups = classify::split_by_part(
        hoisted
            .into_iter()
            .map(|s| {
                let mut nodes = vec![];
                luavm::walk(&s, &mut nodes);
                let ps: BTreeSet<_> = nodes
                    .iter()
                    .filter(|n| luavm::ty(n) == "AstExprLocal")
                    .filter_map(|n| po.get(&luavm::location(&n["local"])).copied())
                    .collect();
                (s, ps)
            })
            .collect(),
    )
    .unwrap();
    assert_eq!(groups.iter().map(|g| g.0).collect::<Vec<_>>(), vec![2, 1]);
}

#[test]
fn zero_offsets_and_capture_chains_match_every_reference_field() {
    let d = dump(
        vec![
            rec(26, 7, 1, 0, 0, 1, json!([])),
            rec(25, 0, 1, 0, 0, decode::NONE, json!([])),
        ],
        0,
    );
    let out = decode::decode_program(&d, &mapping(), &layout()).unwrap();
    let a = &out["protos"]["0"]["decoded"]["0"]["ins"][0][1];
    assert_eq!((a["A"].clone(), a["B"].clone()), (json!(7), json!(1)));
    for derived_reader in [false, true] {
        let d = dump(
            vec![
                rec(
                    23,
                    4,
                    0,
                    0,
                    2,
                    1,
                    json!([
                        [0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                        [0, 0, 0, 3, 0, 0, 0, 0, 0, 0],
                        [0, 0, 1, 5, 0, 0, 0, 0, 0, 0],
                        [0, 0, 2, 1, 0, 0, 0, 0, 0, 0]
                    ]),
                ),
                rec(25, 4, 2, 0, 0, decode::NONE, json!([])),
            ],
            3,
        );
        let mut l = layout();
        if derived_reader {
            l["cap_perm"] = json!({"0":[1,3,4,9]});
        }
        let out = decode::decode_program(&d, &mapping(), &l).unwrap();
        let ds = &out["protos"]["0"]["decoded"];
        assert_eq!(ds.as_object().unwrap().len(), 5);
        assert_eq!(ds["0"]["ins"][0][0], "DUPCLOSURE");
        assert_eq!(ds["0"]["ins"][0][1]["A"], 1);
        assert_eq!(ds["0"]["nxt"], 1u64 << 40);
        let extras = ds["0"]["extra"].as_array().unwrap();
        assert_eq!(
            extras
                .iter()
                .map(|x| x[0].as_u64().unwrap())
                .collect::<Vec<_>>(),
            vec![1u64 << 40, 2u64 << 40, 3u64 << 40]
        );
        for (q, kind, b, nxt) in [(0, 0, 0, 2u64 << 40), (1, 1, 2, 3u64 << 40), (2, 2, 1, 1)] {
            let vd = &extras[q][1];
            assert_eq!(vd, &ds[extras[q][0].as_u64().unwrap().to_string()]);
            assert_eq!(vd["kind"], "CAPTURE");
            assert_eq!(vd["ins"][0][0], "CAPTURE");
            assert_eq!(vd["ins"][0][1]["A"], kind);
            assert_eq!(vd["ins"][0][1]["B"], b);
            assert_eq!(vd["nxt"], nxt);
            assert_eq!(vd["alt"], decode::NONE);
        }
    }
    let d = dump(
        vec![
            rec(23, 0, 0, 0, 2, 1, json!([])),
            rec(25, 0, 1, 0, 0, decode::NONE, json!([])),
        ],
        0,
    );
    let out = decode::decode_program(&d, &mapping(), &layout()).unwrap();
    let ds = &out["protos"]["0"]["decoded"];
    assert_eq!(ds.as_object().unwrap().len(), 2);
    assert_eq!(ds["0"]["nxt"], 1);
    assert!(ds["0"].get("extra").is_none());

    let d = dump(
        vec![
            rec(
                23,
                0,
                0,
                0,
                2,
                1,
                json!([
                    [0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                    [0, 0, 0, 7, 0, 0, 0, 0, 0, 0]
                ]),
            ),
            rec(25, 0, 1, 0, 0, decode::NONE, json!([])),
        ],
        0,
    );
    let mut l = layout();
    l["cap_perm"] = json!({"0":[1,3,4,9]});
    let out = decode::decode_program(&d, &mapping(), &l).unwrap();
    let ds = &out["protos"]["0"]["decoded"];
    let mut ids: Vec<u64> = ds
        .as_object()
        .unwrap()
        .keys()
        .map(|s| s.parse().unwrap())
        .collect();
    ids.sort_unstable();
    assert_eq!(ids, vec![0, 1, 1u64 << 40]);
    assert_eq!(ds[(1u64 << 40).to_string()]["kind"], "CAPTURE");
    assert_eq!(ds["0"]["extra"][0][1]["ins"][0][1]["A"], 0);
    assert_eq!(ds["0"]["extra"][0][1]["ins"][0][1]["B"], 7);

    let mut d = d;
    d["protos"]["0"]["recs"]["0"]["parts"][1] = json!([0, 0, 7, 0, 0, 0, 0, 0, 0, 0]);
    l["cap_perm"] = json!({"0":[1,4,3,9]});
    let out = decode::decode_program(&d, &mapping(), &l).unwrap();
    assert_eq!(
        out["protos"]["0"]["decoded"]["0"]["extra"][0][1]["ins"][0][1]["B"],
        7
    );
}

#[test]
fn fused_loop_edges_and_all_reference_body_shapes() {
    let cases = [
        (
            json!({"pre":[["LOADN",0,{},false]],"head":[["FORGLOOP",1,{},true]],"body":[["MOVE",2,{},false]],"tail":[]}),
            json!([[4, 0, 0, 1, 0, 0], [5, 0, 2, 0, 0, 0], [6, 7, 0, 0, 0, 0]]),
        ),
        (
            json!({"pre":[],"head":[["FORGLOOP",0,{},true]],"body":[],"tail":[]}),
            json!([[5, 0, 0, 0, 0, 0]]),
        ),
        (
            json!({"pre":[],"head":[],"body":[["MOVE",0,{},false],["MOVE",1,{},false]],"tail":[["FORNLOOP",2,{},true]]}),
            json!([[1, 2, 0, 0, 0, 0], [3, 4, 0, 0, 0, 0], [0, 0, 0, 0, 0, 0]]),
        ),
        (
            json!({"pre":[["LOADN",0,{},false]],"head":[],"body":[["MOVE",1,{},false]],"tail":[["FORNLOOP",2,{},true]]}),
            json!([[1, 0, 0, 2, 0, 0], [3, 4, 0, 0, 0, 0], [0, 0, 0, 0, 0, 0]]),
        ),
    ];
    for (case, (lp, parts)) in cases.into_iter().enumerate() {
        let mut r = rec(0, 0, 0, 0, 0, 1, parts);
        r["sem"] = json!(9);
        r["ded"] = json!("4");
        let mut m = mapping();
        m["ded"]["4:9"] = json!({"loop":lp});
        let d = dump(vec![r, rec(25, 0, 1, 0, 0, decode::NONE, json!([]))], 0);
        let out = decode::decode_program(&d, &m, &layout()).unwrap();
        let ds = &out["protos"]["0"]["decoded"];
        let entry = &ds["0"];
        let head = 1u64 << 40;
        match case {
            0 => {
                assert_eq!(entry["ins"].as_array().unwrap().len(), 1);
                assert_eq!(entry["extra"].as_array().unwrap().len(), 2);
                assert_eq!(entry["ins"][0][0], "LOADN");
                assert_eq!(entry["ctrl"][0], "JUMP");
                assert_eq!(entry["alt"], head);
                assert_eq!(entry["nxt"], 1);
                let h = &entry["extra"][0];
                let b = &entry["extra"][1];
                assert_eq!(h[0], head);
                assert_eq!(b[0], 2u64 << 40);
                assert_eq!(h[1]["ins"], json!([]));
                assert_eq!(h[1]["ctrl"][0], "FORGLOOP");
                assert_eq!(h[1]["ctrl"][1]["A"], 5);
                assert_eq!(h[1]["nxt"], 1);
                assert_eq!(h[1]["alt"], 2u64 << 40);
                assert_eq!(b[1]["ins"][0][0], "MOVE");
                assert_eq!(b[1]["ins"].as_array().unwrap().len(), 1);
                assert_eq!(b[1]["ctrl"][0], "JUMP");
                assert_eq!(b[1]["alt"], head);
            }
            1 => {
                assert_eq!(entry["alt"], head);
                assert_eq!(entry["extra"].as_array().unwrap().len(), 1);
                assert_eq!(entry["extra"][0][1]["alt"], head);
                assert_eq!(entry["extra"][0][1]["nxt"], 1);
            }
            2 => {
                assert_eq!(
                    entry["ins"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|i| i[0].as_str().unwrap())
                        .collect::<Vec<_>>(),
                    vec!["MOVE", "MOVE"]
                );
                assert_eq!(entry["ctrl"][0], "FORNLOOP");
                assert_eq!(entry["nxt"], 1);
                assert_eq!(entry["alt"], 0);
                assert!(entry.get("extra").is_none());
            }
            3 => {
                assert_eq!(entry["ins"].as_array().unwrap().len(), 1);
                assert_eq!(entry["extra"].as_array().unwrap().len(), 1);
                assert_eq!(entry["ins"][0][0], "LOADN");
                assert!(entry["ctrl"].is_null());
                assert_eq!(entry["nxt"], head);
                assert_eq!(entry["extra"][0][0], head);
                let lp = &entry["extra"][0][1];
                assert_eq!(lp["ins"][0][0], "MOVE");
                assert_eq!(lp["ins"].as_array().unwrap().len(), 1);
                assert_eq!(lp["ctrl"][0], "FORNLOOP");
                assert_eq!(lp["nxt"], 1);
                assert_eq!(lp["alt"], head);
            }
            _ => unreachable!(),
        }
    }
}

#[test]
fn offsets_fused_closure_and_loop_recompile_with_identical_bytecode() {
    let mut records = vec![
        rec(21, 3, 0, 0, 0, 1, json!([])),
        rec(22, 3, 1, 2, 0, 2, json!([])),
        rec(
            23,
            4,
            0,
            0,
            2,
            3,
            json!([
                [0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                [0, 0, 0, 3, 0, 0, 0, 0, 0, 0]
            ]),
        ),
        rec(20, 7, 0, 0, 1, 4, json!([])),
        rec(20, 5, 0, 0, 3, 5, json!([])),
        rec(20, 6, 0, 0, 1, 6, json!([])),
        rec(24, 5, 0, 0, 0, 7, json!([])),
        rec(
            0,
            0,
            0,
            0,
            0,
            8,
            json!([
                [8, 0, 0, 0, 0, (1u64 << 30) | (1 << 20)],
                [9, 7, 0, 0, 0, 0],
                [8, 2, 1, 0, 0, 0],
                [5, 0, 0, 0, 0, 0]
            ]),
        ),
        rec(25, 4, 2, 0, 0, decode::NONE, json!([])),
    ];
    records[0]["T"][7] = json!(1u64 << 30);
    records[6]["T"][9] = json!(8);
    records[7]["sem"] = json!(9);
    records[7]["ded"] = json!("4");
    let mut d = dump(records, 3);
    d["protos"]["0"]["consts"] =
        json!({"0":["s",[103]],"1":["s",[112,114,105,110,116]],"2":["n",1]});
    let child = dump(
        vec![
            rec(27, 5, 0, 0, 0, 1, json!([])),
            rec(25, 5, 2, 0, 0, decode::NONE, json!([])),
        ],
        5,
    );
    d["protos"]["1"] = child["protos"]["0"].clone();
    d["protos"]["1"]["nup"] = json!(1);
    let mut m = mapping();
    m["simple"]["21"] = json!("GETIMPORT");
    m["ded"]["4:9"] = json!({"loop":{"pre":[],"head":[],
        "body":[["GETIMPORT",0,{},false],["MOVE",1,{},false],["CALL",2,{},false]],"tail":[["FORNLOOP",3,{},true]]}});
    let program = decode::decode_program(&d, &m, &layout()).unwrap();
    let output = bgdeobf::render::render(&program, None, false).unwrap();
    let text = output["text"].as_str().unwrap();
    let compiled = bgdeobf::engine::compile_check(text).unwrap();
    assert_eq!(compiled["ok"], true, "{compiled}\n{text}");
    assert!(
        regex::Regex::new(r"(?m)^local (\w+) = g\(\)$")
            .unwrap()
            .is_match(text),
        "{text}"
    );
    let captures = regex::Regex::new(r"(?m)^for (\w+) = 1, 3 do\n\tprint\((\w+)\)\nend$").unwrap();
    let matched = captures
        .captures(text)
        .unwrap_or_else(|| panic!("missing numeric loop: {text}"));
    assert_eq!(&matched[1], &matched[2]);
    let report =
        bgdeobf::fidelity::compare(&program, compiled["bytecode"].as_str().unwrap()).unwrap();
    assert_eq!(report["differing"], json!([]), "{report}\n{text}");
    assert_eq!(report["identical"], 2, "{report}\n{text}");
}

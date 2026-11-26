use bgdeobf::{astutil, dump, loader, patch, static_image};
use serde_json::{json, Value};

#[test]
fn safe_fixture_matches_independent_python_layout_and_dump() {
    use std::{io::Write, process::Command, time::Duration};
    let reference: Value =
        serde_json::from_str(include_str!("fixtures/safe_vm_reference.json")).unwrap();
    let source: Vec<u8> = reference["source"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap() as u8)
        .collect();
    let l = loader::analyze(&source).unwrap();
    for (k, v) in reference["layout"].as_object().unwrap() {
        assert_eq!(&l[k], v, "layout field {k}");
    }
    assert_eq!(l["sem_index"], 2);
    for format in 0..4 {
        assert_eq!(l["cap_perm"][format.to_string()], json!([3, 4, 5, 6]));
    }
    let bytes = patch::build_dump_script(&l).unwrap();
    let mut file = tempfile::Builder::new().suffix(".lua").tempfile().unwrap();
    file.write_all(&bytes).unwrap();
    file.flush().unwrap();
    let mut cmd = Command::new(astutil::find_bin("luau").unwrap());
    cmd.arg(file.path());
    let out = bgdeobf::process::capture(
        cmd,
        &bgdeobf::process::Limits {
            timeout: Duration::from_secs(20),
            memory_mb: 256,
            cpu_seconds: 20,
            max_output_bytes: 1024 * 1024,
            new_group: true,
        },
    )
    .unwrap();
    assert_eq!(out.code, Some(0), "{}", astutil::latin1(&out.err));
    let d = dump::parse_dump(&astutil::latin1(&out.out)).unwrap();
    assert_eq!(d, reference["dump"]);
}

#[test]
fn simple_reader_uses_the_last_function_assignment_like_the_reference() {
    let reference: Value =
        serde_json::from_str(include_str!("fixtures/safe_vm_reference.json")).unwrap();
    let bytes: Vec<u8> = reference["source"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap() as u8)
        .collect();
    let source = astutil::latin1(&astutil::normalize_source(&bytes));
    let original = "simpleFields = function(t) return t[1], t[3], t[4], t[5], t[6], t[7], t[8] end";
    let changed = source.replace(
        original,
        &format!(
            "simpleFields = function(t) return t[8],t[7],t[6],t[5],t[4],t[3],t[1] end; {original}"
        ),
    );
    assert_ne!(changed, source);
    let l = loader::analyze(&astutil::unlatin1(&changed).unwrap()).unwrap();
    assert_eq!(l["simple_perm"], reference["layout"]["simple_perm"]);
}

#[test]
fn fused_reader_assignment_keeps_the_first_value_with_extra_results() {
    let reference: Value =
        serde_json::from_str(include_str!("fixtures/safe_vm_reference.json")).unwrap();
    let bytes: Vec<u8> = reference["source"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap() as u8)
        .collect();
    let source = astutil::latin1(&astutil::normalize_source(&bytes));
    let original = "fused = function(t) return t[3], t[4], t[5], t[6], t[7], t[8] end";
    let changed = source.replace(original, &format!("{original}, 0"));
    assert_ne!(changed, source);
    let l = loader::analyze(&astutil::unlatin1(&changed).unwrap()).unwrap();
    assert_eq!(l["fused_perm"], reference["layout"]["fused_perm"]);
}

fn statement(source: &str) -> Value {
    astutil::parse_ast(source.as_bytes()).unwrap()["body"][0].clone()
}

fn numeric_fixture() -> Vec<u8> {
    let reference: Value =
        serde_json::from_str(include_str!("fixtures/safe_vm_reference.json")).unwrap();
    let source: Vec<u8> = reference["source"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap() as u8)
        .collect();
    let mut s = String::from_utf8(astutil::normalize_source(&source)).unwrap();
    s=s.replace("return function(...)\n",&format!("return function(...)\n --{}\n local numParams = proto[1]\n local regShift = proto[24] or 0\n if proto[2] then local n = select('#', ...) end\n", "numeric structural padding ".repeat(650)));
    s = s.replace("local numParams = proto.numParams", "");
    s = s.replace("local kget = proto.constantReader", "");
    s = s.replace("imp = proto.importReader", "imp = proto.importReader\n if imp == nil then imp = function(aux) return kget(bit32.band(bit32.rshift(aux, 20), 1023)) end; proto[27] = imp end");
    s=s.replace("local recycle\n", "local recycle\n local native = proto[25]\n if native ~= nil then native() end\n local kget = proto[14]\n if kget == nil then kget = function(id) local cached = proto[15][id]; local value = proto[16][id]; return value end; proto[14] = kget end\n");
    s=s.replace("local reader = proto.instructionReader", "local reader = proto[11]\n if reader == nil then reader = function(id) local cached = proto[12][id]; local v = proto[13][id]; return {v[1],v[2],v[3],v[4],v[5],v[6],v[7],v[8],v[9],v[10]} end; proto[11] = reader end");
    s=s.replace("simpleFields = function(t) return t[1], t[3], t[4], t[5], t[6], t[7], t[8] end", "simpleFields = function(t) return t[1], t[3], t[4], t[5], t[6], t[7], t[8] end; proto[17] = simpleFields");
    s = s.replace(
        "fused = function(t) return t[3], t[4], t[5], t[6], t[7], t[8] end",
        "fused = function(t) return t[3], t[4], t[5], t[6], t[7], t[8] end; proto[18] = fused",
    );
    s = s.replace(
        "capture = function(t) return t[3], t[4], t[5], t[6] end",
        "capture = function(t) return t[3], t[4], t[5], t[6] end; proto[19] = capture",
    );
    s = s.replace(
        "if op == 20 then",
        "if op == 20 then\n for i = 0, child[3] - 1 do local nested = proto[26][i] end",
    );
    let permutation="op, ef, a, b, c, rf, hf, aux, nxt, alt = rec[1], rec[2], rec[3], rec[4], rec[5], rec[6], rec[7], rec[8], rec[9], rec[10]";
    let ladder = (0..4)
        .map(|i| {
            format!(
                "{} proto[23] == {} then {} ",
                if i == 0 { "if" } else { "elseif" },
                i,
                permutation
            )
        })
        .collect::<String>()
        + "end";
    s = s.replace(permutation, &ladder);
    for (from, to) in [
        ("proto.maxStack", "proto[4]"),
        ("proto.simpleFields", "proto[17]"),
        ("proto.importReader", "proto[27]"),
        ("proto.fusedFields", "proto[18]"),
        ("proto.captureFields", "proto[19]"),
        ("proto.entry", "proto[21]"),
        ("proto.fetchInstruction", "proto[20]"),
        ("proto.index", "proto[22]"),
    ] {
        s = s.replace(from, to);
    }
    s=s.replace("local proto = {", "local function decodeProto(bytes)\n local work = {[1]=0,[2]=false ~= true,[3]=0,[4]=4,[5]=0,[6]=0,[7]=0}\n work[28] = bit32.band(bit32.rshift(0, 1), 3)\n work[24] = bit32.band(0, 31)\n return work\n end\n local proto = {");
    s = s.replace(
        "index = 0, numParams = 0, isVararg = true, numUpvalues = 0,",
        "[22] = 0, [1] = 0, [2] = true, [3] = 0,",
    );
    s = s.replace(
        "maxStack = 4, entry = 1, dispatchFormat = 0, vmFamily = 0,",
        "[4] = 4, [21] = 1, [23] = 0, [28] = 0, [24] = 0,",
    );
    s = s.replace(
        "instructionCount = 7, children = {}, constantData = constants, recordData = records,",
        "instructionCount = 7, [26] = {}, [15] = {}, [16] = constants, [12] = {}, [13] = records,",
    );
    s = s
        .replace("constantReader = function", "[14] = function")
        .replace("instructionReader = function", "[11] = function")
        .replace("fetchInstruction = function", "[20] = function")
        .replace("importReader = function", "[27] = function");
    assert!(s.contains("numeric structural padding"));
    s.into_bytes()
}

fn masked_numeric_fixture() -> String {
    let mut source = astutil::latin1(&numeric_fixture());
    source = source.replace("[11] = function(id) return records[id] end,", "");
    source = source.replace(
        "[22] = 0,",
        "[22] = 0, [29] = 305419896, [30] = 3735928559, [31] = 4294967295,",
    );
    for name in ["rec", "t"] {
        for (index, mask) in [(1, 29), (3, 30), (4, 30), (5, 30), (8, 31)] {
            source = source.replace(
                &format!("{name}[{index}]"),
                &format!("bit32.bxor({name}[{index}], proto[{mask}])"),
            );
        }
    }
    source = source.replace(
        "return {v[1],v[2],v[3],v[4],v[5],v[6],v[7],v[8],v[9],v[10]}",
        "local function fields(v) return {bit32.bxor(v[1],proto[29]),v[2],bit32.bxor(v[3],proto[30]),bit32.bxor(v[4],proto[30]),bit32.bxor(v[5],proto[30]),v[6],v[7],bit32.bxor(v[8],proto[31]),v[9],v[10]} end; local function pack(v) local t=fields(v); for i=11,#v do t[i]=pack(v[i]) end; return t end; return pack(v)",
    );
    source
}

#[test]
fn numeric_masked_records_and_parts_produce_the_reference_dump_in_every_format() {
    let reference: Value =
        serde_json::from_str(include_str!("fixtures/safe_vm_reference.json")).unwrap();
    for format in 0..4 {
        let source = masked_numeric_fixture().replace("[23] = 0,", &format!("[23] = {format},"));
        let l = loader::analyze(source.as_bytes()).unwrap();
        assert_eq!(l["numeric_fields"]["record_reader"], 11);
        let script = patch::build_dump_script(&l).unwrap();
        let out = bgdeobf::sandbox::run_luau(&script, std::time::Duration::from_secs(20), 256, 1)
            .unwrap();
        let mut expected = reference["dump"].clone();
        expected["protos"]["0"]["df"] = json!(format);
        assert_eq!(
            dump::parse_dump(&astutil::latin1(&out)).unwrap(),
            expected,
            "format {format}"
        );
    }
}

#[test]
fn numeric_masked_sparse_part_fields_remain_nil() {
    let original = masked_numeric_fixture();
    let source = original.replace(
        "local t=fields(v); for i=11,#v do t[i]=pack(v[i]) end; return t",
        "local t=fields(v); for i=11,#v do t[i]=pack(v[i]) end; if v[11] ~= nil then t[11][3] = nil end; return t",
    );
    assert_ne!(
        source, original,
        "fixture must clear a masked slot in a fused part"
    );
    let layout = loader::analyze(source.as_bytes()).unwrap();
    let script = patch::build_dump_script(&layout).unwrap();
    let out =
        bgdeobf::sandbox::run_luau(&script, std::time::Duration::from_secs(20), 256, 1).unwrap();
    let mut expected: Value =
        serde_json::from_str(include_str!("fixtures/safe_vm_reference.json")).unwrap();
    expected["dump"]["protos"]["0"]["recs"]["1"]["parts"][0][2] = Value::Null;
    assert_eq!(
        dump::parse_dump(&astutil::latin1(&out)).unwrap(),
        expected["dump"]
    );
}

#[test]
fn numeric_masked_readers_reject_other_operations_and_inconsistent_masks() {
    let source = masked_numeric_fixture();
    for changed in [
        source.replace("bit32.bxor(t[3], proto[30])", "bit32.bor(t[3], proto[30])"),
        source.replace("bit32.bxor(t[3], proto[30])", "bit32.bxor(t[3], other[30])"),
        source.replace("bit32.bxor(t[3], proto[30])", "bit32.bxor(t[3], proto[29])"),
    ] {
        assert_ne!(changed, source);
        assert!(loader::analyze(changed.as_bytes()).is_err());
    }
}

#[test]
fn numeric_masked_reads_must_use_current_record() {
    let source = masked_numeric_fixture();
    for (from, to) in [
        (
            "bit32.bxor(rec[3], proto[30])",
            "bit32.bxor(other[3], proto[30])",
        ),
        (
            "bit32.bxor(t[3], proto[30])",
            "bit32.bxor(other[3], proto[30])",
        ),
    ] {
        let changed = source.replacen(from, to, 1);
        assert_ne!(changed, source);
        assert!(loader::analyze(changed.as_bytes()).is_err(), "{from}");
    }
}

#[test]
fn numeric_record_reader_rejects_multiple_lazy_assignments() {
    let source = astutil::latin1(&numeric_fixture());
    let original = "reader = function(id) local cached = proto[12][id]; local v = proto[13][id]; return {v[1],v[2],v[3],v[4],v[5],v[6],v[7],v[8],v[9],v[10]} end; proto[11] = reader end";
    let changed = source.replace(
        original,
        "reader = function(id) local cached = proto[12][id]; local v = proto[13][id]; return {v[1],v[2],v[3],v[4],v[5],v[6],v[7],v[8],v[9],v[10]} end; reader = function(id) local cached = proto[12][id]; local v = proto[33][id]; return {v[1],v[2],v[3],v[4],v[5],v[6],v[7],v[8],v[9],v[10]} end; proto[11] = reader end",
    );
    assert_ne!(changed, source);
    assert!(loader::analyze(&astutil::unlatin1(&changed).unwrap()).is_err());
}

#[test]
fn numeric_mapped_prototypes_are_initialized_before_native_handlers_are_disabled() {
    let source = masked_numeric_fixture()
        .replace(
            "local function make(proto, up)",
            "local function prepare(P) if P[32] then return end; P[32]=true; P[25]=function() error('native payload ran') end end\nlocal function make(proto, up) prepare(proto)",
        )
        .replace(
            "local protos = {[0] = proto}",
            "local function protoKey(i) return bit32.bxor(i * 3, 17) end\nlocal protos = {[protoKey(0)] = proto}",
        )
        .replace("protos[mainIndex]", "protos[protoKey(mainIndex)]");
    let source = source.replace(
        "if native ~= nil then native() end",
        "if native ~= nil then return function(...) return native(...) end end",
    );
    let l = loader::analyze(source.as_bytes()).unwrap();
    let script = patch::build_dump_script(&l).unwrap();
    let out =
        bgdeobf::sandbox::run_luau(&script, std::time::Duration::from_secs(20), 256, 1).unwrap();
    let reference: Value =
        serde_json::from_str(include_str!("fixtures/safe_vm_reference.json")).unwrap();
    assert_eq!(
        dump::parse_dump(&astutil::latin1(&out)).unwrap(),
        reference["dump"]
    );
}

#[test]
fn numeric_reader_initialised_by_local_function_without_cache_back_is_recognised() {
    let source = astutil::latin1(&numeric_fixture());
    let original = "if reader == nil then reader = function(id) local cached = proto[12][id]; local v = proto[13][id]; return {v[1],v[2],v[3],v[4],v[5],v[6],v[7],v[8],v[9],v[10]} end; proto[11] = reader end";
    let replacement = "if reader == nil then local function packRecord(v) return {v[1],v[2],v[3],v[4],v[5],v[6],v[7],v[8],v[9],v[10]} end; reader = function(id) local cached = proto[12][id]; local v = proto[13][id]; local result = packRecord(v); for i=11,#v do result[i]=v[i] end; return result end end";
    let changed = source.replace(original, replacement);
    assert_ne!(changed, source);
    let l = loader::analyze(&astutil::unlatin1(&changed).unwrap()).unwrap();
    assert_eq!(l["layout"], "numeric");
    assert_eq!(l["numeric_fields"]["record_reader"], 11);

    let changed = changed.replace("[11] = function(id) return records[id] end,", "");
    let l = loader::analyze(&astutil::unlatin1(&changed).unwrap()).unwrap();
    let script = patch::build_dump_script(&l).unwrap();
    let out =
        bgdeobf::sandbox::run_luau(&script, std::time::Duration::from_secs(20), 256, 1).unwrap();
    let reference: Value =
        serde_json::from_str(include_str!("fixtures/safe_vm_reference.json")).unwrap();
    assert_eq!(
        dump::parse_dump(&astutil::latin1(&out)).unwrap(),
        reference["dump"]
    );
}

#[test]
fn numeric_loader_derives_every_dump_field_and_produces_reference_dump() {
    use std::{io::Write, process::Command, time::Duration};
    let l = loader::analyze(&numeric_fixture()).unwrap();
    assert_eq!(l["layout"], "numeric");
    for role in ["READER", "KGET", "REGS", "SIMPLEFN", "CTRL", "MIX"] {
        assert!(l["role_local"].get(role).is_some(), "role {role}");
    }
    assert_eq!(l["ded_decl"].as_object().unwrap().len(), 6);
    assert!(!l["simple_tables"].as_array().unwrap().is_empty());
    for (key, want) in [
        ("num_params", 1),
        ("is_vararg", 2),
        ("num_upvalues", 3),
        ("max_stack", 4),
        ("record_reader", 11),
        ("record_cache", 12),
        ("record_data", 13),
        ("constant_reader", 14),
        ("constant_cache", 15),
        ("constant_data", 16),
        ("entry", 21),
        ("proto_id", 22),
        ("dispatch_format", 23),
        ("reg_shift", 24),
        ("aot_handler", 25),
        ("children", 26),
        ("vm_family", 28),
    ] {
        assert_eq!(l["numeric_fields"][key], json!(want), "numeric field {key}");
    }
    let script = patch::build_dump_script(&l).unwrap();
    assert!(astutil::latin1(&script).contains("__RL.NUMERIC="));
    assert!(astutil::latin1(&script).contains("[11]"));
    let mut file = tempfile::Builder::new().suffix(".lua").tempfile().unwrap();
    file.write_all(&script).unwrap();
    file.flush().unwrap();
    let mut cmd = Command::new(astutil::find_bin("luau").unwrap());
    cmd.arg(file.path());
    let out = bgdeobf::process::capture(
        cmd,
        &bgdeobf::process::Limits {
            timeout: Duration::from_secs(20),
            memory_mb: 256,
            cpu_seconds: 20,
            max_output_bytes: 1024 * 1024,
            new_group: true,
        },
    )
    .unwrap();
    assert_eq!(out.code, Some(0), "{}", astutil::latin1(&out.err));
    let reference: Value =
        serde_json::from_str(include_str!("fixtures/safe_vm_reference.json")).unwrap();
    assert_eq!(
        dump::parse_dump(&astutil::latin1(&out.out)).unwrap(),
        reference["dump"]
    );
}

fn check_authorized_numeric_artifact(sample: &str) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("results")
        .join(sample);
    let l = loader::analyze(&std::fs::read(path).expect("optional authorized artifact is absent"))
        .unwrap();
    assert_eq!(l["layout"], "numeric");
    for key in [
        "proto_id",
        "record_reader",
        "constant_reader",
        "dispatch_format",
        "num_params",
        "max_stack",
    ] {
        assert!(l["numeric_fields"].get(key).is_some(), "{sample}: {key}");
    }
    for role in ["READER", "KGET", "REGS", "SIMPLEFN", "CTRL", "MIX"] {
        assert!(l["role_local"].get(role).is_some(), "{sample}: {role}");
    }
    assert_eq!(l["ded_decl"].as_object().unwrap().len(), 6);
    assert!(!l["simple_tables"].as_array().unwrap().is_empty());
    let script = astutil::latin1(&patch::build_dump_script(&l).unwrap());
    assert!(script.contains("__RL.NUMERIC="));
    assert!(script.contains(&format!("[{}]", l["numeric_fields"]["record_reader"])));
}

#[test]
#[ignore = "requires optional local results/loveyou_fetched.lua; AST analysis only"]
fn authorized_numeric_loveyou_structure_and_patch() {
    check_authorized_numeric_artifact("loveyou_fetched.lua");
}

#[test]
#[ignore = "requires optional local results/QxU6VyGTVW_fetched.lua; AST analysis only"]
fn authorized_numeric_qxu6_structure_and_patch() {
    check_authorized_numeric_artifact("QxU6VyGTVW_fetched.lua");
}

#[test]
#[ignore = "requires optional local results/F2naYujWnI_fetched.lua; AST analysis only"]
fn authorized_numeric_f2na_structure_and_patch() {
    check_authorized_numeric_artifact("F2naYujWnI_fetched.lua");
}

#[test]
fn source_bytes_bom_newlines_and_locations() {
    let bytes: Vec<u8> = (0..=255).collect();
    assert_eq!(astutil::unlatin1(&astutil::latin1(&bytes)).unwrap(), bytes);
    assert!(astutil::unlatin1("€").is_err());
    assert_eq!(
        astutil::normalize_source(b"\xef\xbb\xbf\xef\xbb\xbf\xff\r\nx\ry\n"),
        b"\xff\nx\ny\n"
    );
    let src = astutil::Src::new("éx\nÿz").unwrap();
    assert_eq!(src.span_loc("0,1 - 1,1").unwrap(), (1, 4));
    assert_eq!(src.slice(&json!({"location":"0,1 - 1,1"})).unwrap(), "x\nÿ");
    assert!(src.span_loc("0,9 - 1,1").is_err());
    let bytes = b"local x='\xff\xc3\xa9'\nreturn x";
    let root = astutil::parse_ast(bytes).unwrap();
    let string = &root["body"][0]["values"][0];
    let source = astutil::Src::new(&astutil::latin1(bytes)).unwrap();
    assert_eq!(
        astutil::unlatin1(&source.slice(string).unwrap()).unwrap(),
        b"'\xff\xc3\xa9'"
    );
    assert_eq!(
        astutil::unlatin1(string["value"].as_str().unwrap()).unwrap(),
        vec![255, 195, 169]
    );
    for bad in ["invalid", "0,0 - 9,0", "1,2 - 0,0", "0,-1 - 0,1"] {
        assert!(source.span_loc(bad).is_err(), "{bad}");
    }
}

#[test]
fn ast_walk_retains_table_children_parent_links_and_local_identity() {
    let root = astutil::parse_ast(b"local x=7\nlocal t={[x]=(x),x}\nreturn t").unwrap();
    let nodes = astutil::walk(&root);
    assert!(nodes[0].1.is_none());
    let table = &root["body"][1]["values"][0];
    let children = astutil::kids(table);
    assert_eq!(children.len(), 2);
    assert_eq!(
        children.iter().map(|n| loader::ty(n)).collect::<Vec<_>>(),
        vec!["AstExprTableItem", "AstExprTableItem"]
    );
    let item_children = astutil::kids(children[0]);
    assert_eq!(
        item_children
            .iter()
            .map(|n| loader::ty(n))
            .collect::<Vec<_>>(),
        vec!["AstExprLocal", "AstExprGroup"]
    );
    assert_eq!(loader::ty(astutil::kids(children[1])[0]), "AstExprLocal");
    let declaration = astutil::lkey(&root["body"][0]["vars"][0]);
    for (node, _) in &nodes {
        if loader::ty(node) == "AstExprLocal" && node["local"]["name"] == "x" {
            assert_eq!(astutil::lkey(&node["local"]), declaration);
        }
    }
    let parent = nodes
        .iter()
        .find(|(node, _)| std::ptr::eq(*node, item_children[0]))
        .unwrap()
        .1
        .unwrap();
    assert!(std::ptr::eq(parent, children[0]));
    assert_eq!(
        loader::ty(astutil::unparen(item_children[1])),
        "AstExprLocal"
    );
}

#[test]
fn layouts_and_handler_trees() {
    for source in [
        "return T[9], T[4], T[6]",
        "return {T[9], T[4], T[6]}",
        "a,b,c=T[9],T[4],T[6]",
    ] {
        let got = loader::dispatch_perm(&statement(source), None, None).unwrap();
        for k in 0..4 {
            assert_eq!(got[k.to_string()], json!([9, 4, 6]));
        }
    }
    let got = loader::dispatch_perm(&statement("if M.dispatchFormat == 0 then return T[1],T[2] elseif M.dispatchFormat == 3 then return T[2],T[1] end"), None, None).unwrap();
    assert_eq!(got, json!({"0":[1,2],"3":[2,1]}));
    assert_eq!(loader::dispatch_perm(&statement("if M.dispatchFormat == 0 then return T[1],T[2],T[3] elseif M.dispatchFormat == 3 then return T[3],T[1],T[2] else error('BG:s',0) end"), None, None).unwrap(), json!({"0":[1,2,3],"3":[3,1,2]}));
    assert_eq!(loader::dispatch_perm(&statement("if M.dispatchFormat==1 then a,b=T[2],T[1] elseif M.dispatchFormat==0 then a,b=T[1],T[2] end"),None,None).unwrap(),json!({"1":[2,1],"0":[1,2]}));
    let mut independent =
        loader::dispatch_perm(&statement("return T[1],T[2]"), None, None).unwrap();
    independent["0"].as_array_mut().unwrap().push(json!(99));
    assert_eq!(independent["1"], json!([1, 2]));
    for (source, message) in [
        ("print(1)", "dispatchFormat body"),
        ("return T[1],5", "dispatchFormat index"),
        ("return T[x],T[2]", "dispatchFormat index"),
        (
            "if M.size==0 then return T[1] end",
            "dispatchFormat ladder expected",
        ),
    ] {
        let error = loader::dispatch_perm(&statement(source), None, None).unwrap_err();
        assert!(error.message.contains(message), "{source}: {error}");
    }
    for (source, expected) in [
        ("if op<=12 then if op<=4 then if op==4 then if operand==2 then output=10 else output=20 end else error('bad') end else if op==8 then output=30 else error('bad') end end else if op==18 then else error('bad') end end", vec![4,8,18]),
        ("if (4==op) then output=1 elseif (op==8) then output=2 end",vec![4,8]),
    ] {
        let root = astutil::parse_ast(format!("local op=0\n{source}").as_bytes()).unwrap();
        let got = loader::handler_chain(&root["body"][1], &root["body"][0]["vars"][0]).unwrap();
        let text=format!("local op=0\n{source}");
        let src=astutil::Src::new(&text).unwrap();
        let bodies=got.iter().map(|p|src.slice(&p[1]).unwrap().trim().to_owned()).collect::<Vec<_>>();
        assert_eq!(bodies,if expected.len()==3 {vec!["if operand==2 then output=10 else output=20 end","output=30",""]}else{vec!["output=1","output=2"]});
        assert_eq!(got.iter().map(|p|p[0].as_i64().unwrap()).collect::<Vec<_>>(), expected);
    }
    for source in [
        "if op<=4 then print('effect'); if op==4 then output=1 end else if op==8 then output=2 end end",
        "if op<=4 then if op==8 then output=1 end else if op==18 then output=2 end end",
        "if op<=4 then if op==4 then output=1 end end",
        "if op<=4 then if op==4 then output=1 end else if other==8 then output=2 end end",
        "if op==4 then output=1 elseif op==4 then output=2 end",
        "if op==4.5 then output=1 end",
        "if op+1<=4 then if op==4 then output=1 end else if op==8 then output=2 end end",
    ] {
        let root = astutil::parse_ast(format!("local op=0\n{source}").as_bytes()).unwrap();
        assert!(loader::handler_chain(&root["body"][1], &root["body"][0]["vars"][0]).is_err(),"{source}");
    }
    assert!(
        loader::analyze(b"local M={[17]=4}; local function f(...) return M[17] end; return f")
            .is_err()
    );
}

#[test]
fn deep_handler_ladders_are_not_limited_by_jsons_default_depth() {
    let mut source = String::from("local op=0\n");
    for op in 0..150 {
        source.push_str(&format!(
            "{} op=={} then local x={} ",
            if op == 0 { "if" } else { "elseif" },
            op,
            op
        ));
    }
    source.push_str("end");
    let root = astutil::parse_ast(source.as_bytes()).unwrap();
    let branches = loader::handler_chain(&root["body"][1], &root["body"][0]["vars"][0]).unwrap();
    assert_eq!(branches.len(), 150);
}

#[test]
fn dump_all_record_and_constant_kinds() {
    let text = "N 1 xorkey 7 main 0\nD 1 12\nD 2 \nD 3 \nD 4 \nD 5 \nD 6 \nP1 12 00ff\nP2 45 80\nS 4,5\nP 0 np=1 va=1 nup=0 ms=4 entry=3 df=2 vf=0 nrec=1 rs=2\nH 0 \nC 0 0 s ff00\nC 0 1 n 1.25\nC 0 2 z\nC 0 3 b 1\nC 0 4 i 1,2\nC 0 5 t 1:-3,4:9\nC 0 6 ? opaque\nR 0 3 sem=12 ded=1 pat=00ff raw=1,2,3,4,5,6,7,8,-,- parts=0,1,2,3,4,5,6,7,8,9 mix=4294967295\n";
    let d = dump::parse_dump(text).unwrap();
    assert_eq!(d["xorkey"], 7);
    assert_eq!(d["main"], 0);
    assert_eq!(
        d["ded"],
        json!({"1":[12],"2":[],"3":[],"4":[],"5":[],"6":[]})
    );
    assert_eq!(d["simple"], json!([4, 5]));
    assert_eq!(d["pat1"]["12"], json!([0, 255]));
    assert_eq!(d["pat2"]["45"], json!([128]));
    assert_eq!(d["protos"]["0"]["consts"]["0"], json!(["s", [255, 0]]));
    assert_eq!(d["protos"]["0"]["consts"]["1"], json!(["n", 1.25]));
    assert_eq!(d["protos"]["0"]["consts"]["2"], json!(["z", null]));
    assert_eq!(d["protos"]["0"]["consts"]["3"], json!(["b", true]));
    assert_eq!(d["protos"]["0"]["consts"]["4"], json!(["i", [1, 2]]));
    assert_eq!(d["protos"]["0"]["consts"]["6"], json!(["?", "opaque"]));
    assert_eq!(
        d["protos"]["0"]["consts"]["5"],
        json!(["t", [[1, -3], [4, 9]]])
    );
    assert_eq!(d["protos"]["0"]["recs"]["3"]["T"][8], Value::Null);
    assert_eq!(
        d["protos"]["0"]["recs"]["3"],
        json!({"sem":12,"ded":"1","pat":[0,255],"T":[1,2,3,4,5,6,7,8,null,null],"parts":[[0,1,2,3,4,5,6,7,8,9]],"mix":4294967295u32})
    );
    assert_eq!(
        dump::parse_dump(&text.replace(" mix=4294967295", "")).unwrap()["protos"]["0"]["recs"]["3"]
            ["mix"],
        Value::Null
    );
    for bad in [
        "",
        "N 1 xorkey 0 main 0\n",
        "N nonsense",
        "garbage",
        "N 1 xorkey 0 main 0\nP 0 np=0",
        "N 1 xorkey 0 main 0\nC 0 0 s ff",
    ] {
        assert!(dump::parse_dump(bad).is_err(), "{bad}");
    }
    for (from, to) in [
        ("nrec=1", "nrec=2"),
        ("pat=00ff", "pat=0ff"),
        ("raw=1,2,3,4,5,6,7,8,-,-", "raw=1,2"),
        ("C 0 3 b 1", "C 0 3 b 9"),
        ("main 0", "main 4"),
    ] {
        assert!(dump::parse_dump(&text.replace(from, to)).is_err(), "{to}");
    }
}

#[test]
fn dump_preserves_nonfinite_and_negative_zero_numbers() {
    let source="N 1 xorkey 0 main 0\nP 0 np=0 va=0 nup=0 ms=1 entry=0 df=0 vf=0 nrec=0\nH 0 \nC 0 0 n nan\nC 0 1 n inf\nC 0 2 n -inf\nC 0 3 n -0\nC 0 4 s 696e66\n";
    let d = dump::parse_dump(source).unwrap();
    assert_eq!(d["protos"]["0"]["consts"]["0"], json!(["n", "nan"]));
    assert_eq!(d["protos"]["0"]["consts"]["1"], json!(["n", "inf"]));
    assert_eq!(d["protos"]["0"]["consts"]["2"], json!(["n", "-inf"]));
    assert!(d["protos"]["0"]["consts"]["3"][1]
        .as_f64()
        .unwrap()
        .is_sign_negative());
    assert_eq!(
        d["protos"]["0"]["consts"]["4"],
        json!(["s", [105, 110, 102]])
    );
}

#[test]
fn ast_errors_are_structured_and_do_not_disclose_temp_paths() {
    let error = astutil::parse_ast(b"local =").unwrap_err();
    assert_eq!(error.code, "syntax");
    assert!(!error.message.contains("AppData"), "{}", error.message);
    assert!(!error.message.contains("/tmp/"), "{}", error.message);
}

#[test]
fn bundled_luau_tools_work() {
    let tools = astutil::check_tools();
    assert_eq!(tools.len(), 3);
    assert!(tools.iter().all(|(_, ok, _)| *ok), "{tools:?}");
}

#[test]
fn dump_accepts_six_field_fused_parts_used_by_decoder_regressions() {
    let text = "N 1 xorkey 7 main 0\nP 0 np=0 va=0 nup=0 ms=16 entry=0 df=0 vf=0 nrec=1 rs=3\nH 0 \nR 0 0 sem=10 ded=4 pat=- raw=0,0,0,0,0,0,0,0,1,4294967295 parts=4,0,0,1,0,0;5,0,2,0,0,0;6,7,0,0,0,0\n";
    let d = dump::parse_dump(text).unwrap();
    let p = &d["protos"]["0"];
    assert_eq!(p["rs"], 3);
    assert_eq!(
        p["recs"]["0"]["parts"],
        json!([[4, 0, 0, 1, 0, 0], [5, 0, 2, 0, 0, 0], [6, 7, 0, 0, 0, 0]])
    );
    assert_eq!(p["recs"]["0"]["mix"], Value::Null);
    let without_shift = dump::parse_dump(&text.replace(" rs=3", "")).unwrap();
    assert_eq!(without_shift["protos"]["0"]["rs"], 0);
}

#[test]
fn dump_rejects_duplicate_truncated_and_invalid_cross_references() {
    let text = "N 1 xorkey 7 main 0\nP 0 np=0 va=0 nup=0 ms=4 entry=0 df=0 vf=0 nrec=1\nH 0 \nC 0 0 z\nR 0 0 sem=0 ded=- pat=- raw=0,0,0,0,0,0,0,0,-,- parts= mix=-\n";
    assert!(dump::parse_dump(text).is_ok());
    let record = text.lines().last().unwrap();
    for bad in [
        text.replace("N 1", "N 2"),
        text.replace("N 1", "N -1"),
        text.replace(
            "N 1 xorkey 7 main 0",
            "N 1 xorkey 7 main 0\nN 1 xorkey 7 main 0",
        ),
        text.replace("H 0 \n", ""),
        text.replace("H 0 \n", "H 0 1\n"),
        text.replace("H 0 \n", "H 0 \nH 0 \n"),
        text.replace("C 0 0 z", "C 0 0 z\nC 0 0 z"),
        text.replace("C 0 0 z", "C 1 0 z"),
        text.replace("C 0 0 z", "C 0 0 z unexpected"),
        text.replace("C 0 0 z", "C 0 0 s xyz"),
        text.replace("C 0 0 z", "C 0 0 n nope"),
        text.replace("C 0 0 z", "C 0 0 t 1:2:3"),
        text.replace("C 0 0 z", "C 0 0 i 1,,2"),
        text.replace("va=0", "va=2"),
        text.replace("ms=4", "ms=-1"),
        text.replace("ms=4", "ms=4 ms=5"),
        text.replace("ms=4", "ms=4 extra=1"),
        text.replace("P 0 np=0 va=0 nup=0 ms=4 entry=0 df=0 vf=0 nrec=1", "P 0 np=0 va=0 nup=0 ms=4 entry=0 df=0 vf=0 nrec=1\nP 0 np=0 va=0 nup=0 ms=4 entry=0 df=0 vf=0 nrec=1"),
        text.replace("sem=0", "sem=-1"),
        text.replace("sem=0", "sem=0 sem=1"),
        text.replace("sem=0", "sem=9223372036854775808"),
        text.replace("ded=-", "ded=7"),
        text.replace("parts=", "parts=0,0,0,0,0"),
        text.replace("mix=-", "mix=- extra=1"),
        format!("{text}{record}\n"),
        format!("{text}D 0 \n"),
        format!("{text}D 1 \nD 1 \n"),
        format!("{text}S \nS \n"),
        format!("{text}P1 0 ff\nP1 0 ff\n"),
    ] {
        let error = dump::parse_dump(&bad).unwrap_err();
        assert_eq!(error.code, "decode", "{bad}");
    }
    assert_eq!(
        dump::parse_dump(&text.replace('\n', "\r\n")).unwrap(),
        dump::parse_dump(text).unwrap()
    );
}

#[test]
fn patch_preserves_non_utf8_and_multibyte_source_spans() {
    let reference: Value =
        serde_json::from_str(include_str!("fixtures/safe_vm_reference.json")).unwrap();
    let mut input = b"\xef\xbb\xbf\xef\xbb\xbf-- \xff\xc3\xa9\r\n".to_vec();
    input.extend(
        reference["source"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_u64().unwrap() as u8),
    );
    let loader = loader::analyze(&input).unwrap();
    let script = patch::build_dump_script(&loader).unwrap();
    assert!(script.windows(7).any(|s| s == b"-- \xff\xc3\xa9\n"));
    assert!(!script.windows(3).any(|s| s == b"\xef\xbb\xbf"));
    assert!(!loader["text"].as_str().unwrap().contains('\r'));
    astutil::parse_ast(&script).unwrap();
}

#[test]
fn template_keeps_native_handlers_and_refuses_unknown_constants() {
    use std::{io::Write, process::Command, time::Duration};
    let stand_in = r#"
local up
local function proto(aot)
 return {numParams=0,isVararg=false,numUpvalues=0,maxStack=1,entry=0,dispatchFormat=0,vmFamily=0,
 instructionCount=0,children={},constantData={},recordData={},aotHandler=aot}
end
__RL.PROTOS={[0]=proto(nil),[1]=proto(function() return up[0] end)}
__RL.MAINIDX=0;__RL.XORKEY=0
__RL.MK=function(P,u) return function(...)
 if P.aotHandler~=nil then return P.aotHandler(...) end
 if (...)==__DUMP then return end;error("the program ran") end end
for ti=1,6 do __RL["D"..ti]={} end
__RL.P1,__RL.P2,__RL.SIMPLESET={},{},{}
__DUMPER()
print("handler kept: "..tostring(__RL.PROTOS[1].aotHandler~=nil))
"#;
    for unknown in [false, true] {
        let stand_in = if unknown {
            stand_in.replace("constantData={},recordData={}","constantData={[0]=function()end},constantReader=function()return function()end end,recordData={}")
        } else {
            stand_in.to_owned()
        };
        let script = patch::TEMPLATE
            .replace("%PERM%", "{}")
            .replace("%SEMIDX%", "1")
            + "\n"
            + &stand_in;
        let mut file = tempfile::Builder::new().suffix(".lua").tempfile().unwrap();
        file.write_all(script.as_bytes()).unwrap();
        file.flush().unwrap();
        let mut cmd = Command::new(astutil::find_bin("luau").unwrap());
        cmd.arg(file.path());
        let out = bgdeobf::process::capture(
            cmd,
            &bgdeobf::process::Limits {
                timeout: Duration::from_secs(20),
                memory_mb: 256,
                cpu_seconds: 20,
                max_output_bytes: 1024 * 1024,
                new_group: true,
            },
        )
        .unwrap();
        if unknown {
            assert_ne!(out.code, Some(0));
            assert!(astutil::latin1(&out.err).contains("unsupported constant type"));
            assert!(out.out.is_empty());
        } else {
            assert_eq!(out.code, Some(0), "{}", astutil::latin1(&out.err));
            let output = astutil::latin1(&out.out);
            assert!(output.contains("\nP 0 "));
            assert!(output.contains("\nP 1 "));
            assert!(output.contains("handler kept: true"));
        }
    }
}

#[test]
fn patch_tail_shapes() {
    for (source,want) in [
        ("local module=make()\nlocal function run(...) return module(1, 2)(...) end\nreturn run(...)","__DUMPER(module(1, 2))"),
        ("local run=make()\nreturn run(...)","__DUMPER()"),
        ("local module=make()\nlocal host={}\nlocal env=getfenv()\nreturn module(host, env)(...)","__DUMPER(module(host, env))"),
        ("local module=make()\nlocal a,b=1,2\nreturn module( a , b )(...)","__DUMPER(module( a , b ))"),
        ("local results={make()}\nreturn results[1](...)","__DUMPER()"),
        ("local module=make()\nlocal host={}\nlocal env=getfenv()\nlocal results={module(host,env)}\nreturn results[1](...)","__DUMPER()"),
    ] {
        let l=loader::index_source(source.as_bytes()).unwrap();
        let (last, replacement) = patch::tail_replacement(&l).unwrap();
        assert!(astutil::Src::new(l["text"].as_str().unwrap()).unwrap().slice(&last).unwrap().starts_with("return"));
        assert_eq!(replacement,want);
    }
    for (source, message) in [
        ("local x=1\nreturn 5", "tail not recognised"),
        ("local x=1\nprint(x)", "tail not recognised"),
        ("return print(1)", "tail not recognised"),
        ("local t={}\nreturn t.run()(...)", "tail not recognised"),
        ("local run=make()\nreturn run(...),2", "tail not recognised"),
        (
            "local function run(...) print(1) end\nreturn run(...)",
            "runner not recognised",
        ),
        (
            "local function t() end\nreturn t[1](...)",
            "runner not recognised",
        ),
        (
            "local i=1\nlocal t={}\nreturn t[i](...)",
            "tail not recognised",
        ),
        ("return results[1](...)", "tail not recognised"),
    ] {
        let error =
            patch::tail_replacement(&loader::index_source(source.as_bytes()).unwrap()).unwrap_err();
        assert!(error.message.contains(message), "{source}: {error}");
    }
}

fn page(data: &[u8], size: usize) -> Value {
    let alphabet: Vec<char> = (33u8..118).map(char::from).collect();
    let mut encoded = String::new();
    for chunk in data.chunks(4) {
        let mut block = [0; 4];
        block[..chunk.len()].copy_from_slice(chunk);
        let mut n = u32::from_be_bytes(block) as u64;
        let mut digits = [0; 5];
        for i in (0..5).rev() {
            digits[i] = (n % 85) as usize;
            n /= 85;
        }
        for i in digits {
            encoded.push(alphabet[i]);
        }
    }
    json!({"1":encoded,"2":data.len(),"3":size,"4":8192,"5":0,"6":0,"7":0})
}

#[test]
fn static_transport_vectors_and_fail_closed() {
    let alphabet: String = (33u8..118).map(char::from).collect();
    let decode = |p: &Value, limit| static_image::decode_page(p, &alphabet, &[[13, 17, 5]], limit);
    assert_eq!(
        decode(&page(b"\x02ABC\x80\x02\x00", 6), 1024).unwrap(),
        b"ABCABC"
    );
    assert_eq!(
        decode(&page(b"\x00A\x82\x00\x00", 6), 1024).unwrap(),
        b"AAAAAA"
    );
    for (mode, data) in [(0, b"\x03@CB"), (1, b"\x03BCD"), (2, b"\x01@AB")] {
        let mut p = page(data, 3);
        p["4"] = json!(8192 | mode << 11);
        p["7"] = json!(1);
        assert_eq!(decode(&p, 1024).unwrap(), b"ABC");
    }
    for p in [
        page(b"\x02AB", 3),
        page(b"\x80\x00\x00", 3),
        page(b"\x02ABC", 2),
    ] {
        assert!(decode(&p, 1024).is_err());
    }
    assert!(decode(&page(b"\x02ABC", 3), 2).is_err());
    let mut p = page(b"\x02ABC", 3);
    let encoded = p["1"].as_str().unwrap().to_owned();
    p["1"] = json!({"1":&encoded[2..],"2":&encoded[..2]});
    p["8"] = json!({"1":2,"2":1});
    assert_eq!(decode(&p, 1024).unwrap(), b"ABC");
    for bad in [json!("~!!!!"), json!("!!!!")] {
        let mut p = page(b"\x02ABC", 3);
        p["1"] = bad;
        assert!(decode(&p, 1024).is_err());
    }
    let mut p = page(b"\x02ABC", 3);
    p["4"] = json!(8192 + 7 * 256);
    assert!(decode(&p, 1024).is_err());
    let mut r = static_image::Reader::new(vec![0x78, 0x56, 0x34, 0x12, 0xac, 2]);
    assert_eq!(r.uint(4).unwrap(), 0x12345678);
    assert_eq!(r.varint().unwrap(), 300);
    r.finish().unwrap();
    assert!(r.uint(1).is_err());
    for bad in [vec![128], vec![255, 255, 255, 255, 16], vec![128; 5]] {
        assert!(static_image::Reader::new(bad).varint().is_err());
    }
    for source in [
        "return arbitraryCode()",
        "return {1,math.random()}",
        "return {[1]=2,[1]=3}",
    ] {
        assert!(static_image::literal(&statement(source)["list"][0]).is_err());
    }
    assert!(!static_image::matches_source(b"return 1\n"));
    assert_eq!(static_image::record_index(0x12345698, 2, 0x12345678, 5), 7);
    assert_eq!(static_image::record_index(7, 0, 0x12345678, 5), 7);
    assert_eq!(static_image::record_index(0x1234567f, 1, 0x12345678, 5), 7);
    assert_eq!(
        static_image::field_value(0b10110101, 1, (0b1010, 7, 3)),
        223
    );
    assert_eq!(
        static_image::field_value(0x2468, 2, (0x1234, 0x56, 0)),
        13862
    );
    for (family, encoded) in [(0, 192), (1, 22), (2, 118)] {
        assert_eq!(
            static_image::opcode_value(encoded, family, (5, 37, 22, 3)),
            7
        );
    }
    assert_eq!(static_image::mix_value([0, 0, 0, 0, 0, 0]), 2392564452);
    assert_eq!(static_image::mix_value([0, 0, 0, 1, 0, 0]), 2392564453);
    assert_eq!(
        static_image::mix_value([0, 5, 5, -1335373922, 0, 2012159304]),
        4184079462
    );
    let (mut a, mut b) = (48393u64, 44979u64);
    for byte in b"ABC" {
        a = (a + *byte as u64 * 21) % 65413;
        b = (b + a) % 65413;
    }
    assert_eq!(
        static_image::checksum(b"prefixABC", 6),
        ((b * 65536 + a) ^ 2828804675) as u32
    );
}

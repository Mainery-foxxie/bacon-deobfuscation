//! Native ports of the source-cleanup, dispatch, shared-decoder, naming and fidelity regressions.
use bgdeobf::{astutil, clean, engine, fidelity, sandbox, source_cleanup};
use serde_json::{json, Value};
use std::time::Duration;

const INLINE: &str = r#"(function()
    local input = buffer.fromstring("\001\001")
    local output = buffer.create(2)
    for offset = 0, 1 do
        buffer.writeu8(output, offset, bit32.bxor(buffer.readu8(input, offset), 73, offset % 256))
    end
    return buffer.tostring(output)
end)()"#;
const SHARED: &str = r#"local readLength = buffer and buffer.len
local newBuffer = buffer and buffer.create
local readByte = buffer and buffer.readu8
local writeByte = buffer and buffer.writeu8
local toString = buffer and buffer.tostring
local xorByte = bit32 and bit32.bxor
local cache = setmetatable({}, {__mode = "v"})
local function decode(input, key, id)
    if id and cache[id] then return cache[id] end
    local size = readLength(input)
    local output = newBuffer(size)
    for offset = 0, size - 1 do
        writeByte(output, offset, xorByte(readByte(input, offset), key, offset % 256))
    end
    local text = toString(output)
    if id then cache[id] = text end
    return text
end
"#;
const SCAFFOLD: &str = r#"local function trap()
    local t = {}
    t[t] = t
    return setmetatable(t, {__index=t, __tostring=function() return "decoy" end})
end
local function decoys() return "one", "two", "three", "four" end
local function install()
    local decoy = trap()
    local target = getgenv()
    local names = {"getconstants", "getprotos", "getupvalues", "getconstant", "getproto", "getupvalue"}
    for _, name in ipairs(names) do
        local original = target[name]
        target[name] = function(fn)
            if fn == install or fn == decoys then return decoy end
            return original(fn)
        end
    end
end
install()
"#;
const ENVIRONMENT: &str = r#"local function verifyEnvironment()
    if type(buffer) ~= "table" or type(bit32) ~= "table" then return false end
    local data = buffer.create(4)
    buffer.writeu8(data, 0, 186)
    if buffer.readu8(data, 0) ~= 186 then return false end
    if debug and debug.info and debug.info(buffer.readu8, "s") ~= "[C]" then return false end
    if string and string.dump and pcall(string.dump, bit32.bxor) then return false end
    return true
end
if not verifyEnvironment() then error("environment verification failed", 0) end
"#;
const PRIMITIVE: &str = r#"local function native(fn)
    if type(fn) ~= "function" then return false end
    if debug and debug.info and debug.info(fn, "s") ~= "[C]" then return false end
    if string and string.dump and pcall(string.dump, fn) then return false end
    return true
end
if buffer and bit32 then
    local length = buffer.len
    if not (native(length) and native(bit32.bxor)) then error("primitive integrity failed", 0) end
end
"#;
const TIMING: &str = r#"local function timing()
    local started = os.clock()
    local sum = 0
    for index = 1, 150 do sum = (sum + index) % 256 end
    if os.clock() - started > 0.5 then error("thread suspension detected", 0) end
    return true
end
timing()
"#;
const FORWARD: &str = r#"local function verifyRemote()
    local ok, mt = pcall(getrawmetatable, game)
    if ok and mt and mt.__namecall and debug.info(mt.__namecall, "s") ~= "[C]" then return false end
    return true
end
local function send(remote, ...)
    if not verifyRemote() then return nil end
    if remote and remote.FireServer then return remote:FireServer(...) end
    return nil
end
local function invoke(remote, ...)
    if not verifyRemote() then return nil end
    if remote and remote.InvokeServer then return remote:InvokeServer(...) end
    return nil
end
"#;
const CHAIN: &str = "local function run(n)\n    local total = 0\n    local step = 100\n    while step do\n        if step == 100 then\n            total = total + n\n            step = 200\n        elseif step == 200 then\n            total = total * 2\n            step = 300\n        elseif step == 300 then\n            total = total - 1\n            step = nil\n        end\n    end\n    return total\nend\nprint(run(3))\n";
const CLOCKS: [(&str, bool); 7] = [
    ("bit32.band(math.floor(os.clock() * 100), 0) == 0", true),
    ("math.abs(math.floor(os.clock() * 10) - 999999) < 0", false),
    (
        "bit32.bor(math.floor(os.clock() * 50), 4294967295) == 4294967295",
        true,
    ),
    ("math.floor(math.abs(os.clock()) * 10) % 1 == 0", true),
    ("math.floor(os.clock()) * 2 % 2 == 0", true),
    ("math.abs(os.clock()) + 1 > 0", true),
    (
        "bit32.bxor(1337, 7331) + math.floor(os.clock() * 50) < 0",
        false,
    ),
];
fn compiled(text: &str) {
    let result = engine::compile_check(text).unwrap();
    assert_eq!(result["ok"], true, "{result}\n{text}");
}
fn run(text: &str) -> Vec<u8> {
    sandbox::run_luau(text.as_bytes(), Duration::from_secs(10), 512, 8).unwrap()
}
fn static_clean(text: &str) -> (String, Value) {
    let result = source_cleanup::clean_helpers_source(text, true).unwrap();
    compiled(&result.0);
    result
}
fn dynamic_clean(text: &str) -> (String, Value) {
    let result = clean::clean_source(text, Duration::from_secs(3), 512, false).unwrap();
    compiled(&result.0);
    result
}
fn no_stats(stats: &Value) -> bool {
    stats
        .as_object()
        .unwrap()
        .values()
        .all(|v| v.as_u64() == Some(0))
}
fn call(raw: &[u8], id: i64) -> String {
    let cipher: String = raw
        .iter()
        .enumerate()
        .map(|(i, b)| format!("\\{:03}", b ^ 73 ^ (i as u8)))
        .collect();
    format!("decode(buffer.fromstring(\"{cipher}\"), 73, {id})")
}

#[test]
fn shared_and_inline_decoders_preserve_bytes_and_index_wrap() {
    for raw in [
        b"HI".to_vec(),
        vec![255, 0, 254],
        "🎁 gift".as_bytes().to_vec(),
        vec![b'x'; 260],
        vec![],
    ] {
        let src = format!("{SHARED}return {}, {}\n", call(&raw, 1), call(&raw, 1));
        let (out, stats) = static_clean(&src);
        assert_eq!(stats["strings"], 2);
        assert!(!out.contains("function decode"));
        assert_eq!(
            run(&format!("local s=(function()\n{out}\nend)()\nprint(s)\n")),
            [raw.clone(), b"\n".to_vec()].concat()
        );
        let cipher: String = raw
            .iter()
            .enumerate()
            .map(|(i, b)| format!("\\{:03}", b ^ 73 ^ i as u8))
            .collect();
        let expression = INLINE
            .replace("\\001\\001", &cipher)
            .replace("create(2)", &format!("create({})", raw.len()))
            .replace("0, 1 do", &format!("0, {} do", raw.len() as i64 - 1));
        let source = format!("return {expression}\n");
        let (out, counts) = static_clean(&source);
        assert_eq!(
            counts["strings"], 1,
            "inline bytes={raw:?}; source={source}"
        );
        assert_eq!(
            run(&format!("local s=(function()\n{out}\nend)()\nprint(s)\n")),
            [raw, b"\n".to_vec()].concat()
        );
    }
}

#[test]
fn decoded_strings_remain_valid_method_receivers() {
    for source in [
        format!("print({INLINE}:lower())\n"),
        format!("{SHARED}print({}:lower())\n", call(b"HI", 1)),
    ] {
        let (out, stats) = static_clean(&source);
        assert_eq!(stats["strings"], 1);
        assert_eq!(run(&out), b"hi\n");
        assert_eq!(run(&out), run(&source));
    }
}

#[test]
fn mutable_escaped_conflicting_and_nonmatching_decoders_stay() {
    for extra in [
        "readByte = function() return 0 end\n",
        "cache[1] = 'external'\n",
        "local escaped = decode\n",
        "print(cache)\n",
        "local input=buffer.fromstring('x')\nlocal unused=decode(input,73,2)\n",
    ] {
        let (out, stats) = static_clean(&format!("{SHARED}{extra}return {}\n", call(b"Frame", 1)));
        assert_eq!(stats["strings"], 0, "{extra}");
        assert!(out.contains("function decode("));
    }
    let (out, stats) = static_clean(&format!(
        "{SHARED}return {}, {}\n",
        call(b"first", 1),
        call(b"second", 1)
    ));
    assert_eq!(stats["strings"], 0);
    assert_eq!(
        run(&format!(
            "local a,b=(function()\n{out}\nend)()\nprint(a,b)\n"
        )),
        b"first\tfirst\n"
    );
    for expr in [
        INLINE.replace("local output =", "print('effect')\nlocal output ="),
        INLINE.replace("create(2)", "create(3)"),
        INLINE.replace("0, 1 do", "0, 2 do"),
        INLINE.replace("offset % 256", "offset % 255"),
        INLINE.replace("return buffer.tostring(output)", "return output"),
        INLINE.replace("buffer.fromstring", "buffer:fromstring"),
        INLINE.replace("end)()", "end)(unexpected())"),
        INLINE.replace(
            "return buffer.tostring(output)",
            "return buffer.tostring(output), 'extra'",
        ),
    ] {
        let src = format!("return {expr}\n");
        let (out, stats) = static_clean(&src);
        assert_eq!(out, src);
        assert_eq!(stats["strings"], 0);
    }
}

#[test]
fn dispatchers_preserve_order_scope_returns_and_nested_breaks() {
    let (out, stats) = static_clean(CHAIN);
    assert_eq!(stats["states"], 1);
    assert!(!out.contains("while step"));
    assert_eq!(run(&out), run(CHAIN));
    assert_eq!(run(&out), b"5\n");
    let source="value='global'\nlocal out=''\nlocal step=1\nwhile step do\n if step==1 then\n  local value='local'\n  out=value\n  step=2\n else\n  if step==2 then\n   out=out..value\n   step=nil\n  end\n end\nend\nprint(out)\n";
    let (out, stats) = static_clean(source);
    assert_eq!(stats["states"], 1);
    assert_eq!(run(&out), b"localglobal\n");
    let (again, counts) = static_clean(&out);
    assert_eq!(again, out);
    assert!(no_stats(&counts));
}

#[test]
fn dispatchers_reject_dynamic_observed_cyclic_conditional_or_escaping_state() {
    for source in [
        CHAIN.replace("step = 200", "step = n"),
        CHAIN.replace("return total", "print(step)\n    return total"),
        CHAIN.replace("step = nil", "step = 100"),
        CHAIN.replace(
            "total = total + n",
            "if n then break end\n            total = total + n",
        ),
        CHAIN.replace("step = 200", "if n then step = 200 else step = 300 end"),
    ] {
        let (out, stats) = static_clean(&source);
        assert_eq!(out, source);
        assert_eq!(stats["states"], 0);
    }
}

#[test]
fn final_state_continue_preserves_shared_tail() {
    let src="local function f(flag)\n local step=1\n while step do\n  if step==1 then\n   if flag then\n    step=nil\n    continue\n   end\n   print('tail')\n   step=nil\n  end\n end\n print('after')\nend\nf(true)\nf(false)\n";
    let (out, stats) = static_clean(src);
    assert_eq!(stats["states"], 1);
    assert_eq!(run(&out), b"after\ntail\nafter\n");
    for replacement in [
        "step=2\n    continue",
        "step=nextStep\n    continue",
        "step=nil\n    print(step)\n    continue",
    ] {
        let source = src.replace("step=nil\n    continue", replacement);
        let (out, counts) = static_clean(&source);
        assert_eq!(out, source);
        assert_eq!(counts["states"], 0);
    }
}

#[test]
fn all_clock_templates_choose_only_the_proven_arm_and_end_returns() {
    for (condition, yes) in CLOCKS {
        let src=format!("local function f()\n if {condition} then\n  print('yes')\n  return 'selected'\n else\n  print('no')\n  return 'other'\n end\n print('unreachable')\nend\nprint(f())\n");
        let (out, stats) = static_clean(&src);
        assert_eq!(stats["checks"], 1);
        assert!(!out.contains("unreachable"));
        assert_eq!(
            run(&out),
            if yes {
                b"yes\nselected\n".as_slice()
            } else {
                b"no\nother\n".as_slice()
            }
        );
    }
    for src in [
        "if bit32.band(applicationEvent(),0)==0 then print('keep') end\n".to_string(),
        format!(
            "os.clock=replacement\nif {} then return true end\n",
            CLOCKS[0].0
        ),
        format!(
            "local math=applicationMath\nif {} then return true end\n",
            CLOCKS[0].0
        ),
        "if os.clock()>5 then late() end\n".into(),
    ] {
        let (out, stats) = static_clean(&src);
        assert_eq!(out, src);
        assert_eq!(stats["checks"], 0);
    }
}

#[test]
fn static_literals_preserve_precedence_integrity_checks_and_multiline_bytes() {
    for (src, want) in [
        ("return bit32.bxor(1,2)..'x'\n", "return (3)..'x'\n"),
        ("return x - (bit32.band(7,3)-10)\n", "return x - ((-7))\n"),
        ("return bit32.bxor(1,2):rep(2)\n", "return (3):rep(2)\n"),
    ] {
        let (out, counts) = static_clean(src);
        assert_eq!(out, want);
        assert_eq!(counts["numbers"], 1);
    }
    for src in [
        "return bit32.bxor(runtimeValue,1)\n",
        "return bit32.bxor(1.5,2)\n",
        "return bit32.bxor(-1,2)\n",
        "return bit32.bnot(1,2)\n",
        "if bit32.bxor(12345,67890)~=80139 then error('integrity',0) end\n",
        "bit32.bxor=replacement\nreturn bit32.bxor(1,2)\n",
    ] {
        let (out, stats) = static_clean(src);
        assert_eq!(out, src);
        assert_eq!(stats["numbers"], 0);
    }
    let src = format!(
        "if {} then\n local text=[[first\n   literal spaces\n   last]]\n print(text)\nend\n",
        CLOCKS[0].0
    );
    let (out, stats) = static_clean(&src);
    assert_eq!(src, out);
    assert_eq!(stats["checks"], 0);
}

#[test]
fn names_use_bindings_stable_gui_labels_json_roles_and_collisions() {
    let src="local value=player.Character\nlocal flag=configuration.AutoDive or false\nlocal waitForChild2=storage:WaitForChild(('Core'),15)\nreturn value,flag,waitForChild2\n";
    let (out, stats) = static_clean(src);
    assert!(
        out.contains("local character=")
            && out.contains("local autoDive=")
            && out.contains("local core=")
    );
    assert_eq!(stats["names"], 3);
    let src="local Players='existing'\nlocal value=lookup('Players')\nlocal value2=lookup('Players')\nprint(Players,Players2,value.Name,value2.Name)\n";
    let (out, stats) = static_clean(src);
    assert!(out.contains("local Players3=") && out.contains("local Players4="));
    assert_eq!(stats["names"], 2);
    let src="local frame=Instance.new('Frame')\nframe.Name='KeyPanel'\nlocal textButton=Instance.new('TextButton')\ntextButton.Text='Get Key'\nlocal function read(frame) return frame,textButton.Text end\nreturn frame,read\n";
    let (out, stats) = static_clean(src);
    assert!(
        out.contains("local keyPanel=")
            && out.contains("local getKeyButton=")
            && out.contains("return frame,getKeyButton.Text")
    );
    assert_eq!(stats["names"], 2);
    for role in ["settings", "config"] {
        let src=format!("local text='game_{role}.json'\nlocal items={{AutoDive=false}}\nlocal clone=table.clone(items)\nwritefile(text,HttpService:JSONEncode(clone))\nreturn items,clone\n");
        let (out, stats) = static_clean(&src);
        assert!(
            out.contains(&format!("local {role}File="))
                && out.contains(&format!("local {role}=table.clone(default"))
        );
        assert_eq!(stats["names"], 3);
    }
}

#[test]
fn names_preserve_mutation_ambiguity_user_names_types_and_verifiers() {
    for src in ["local myName=lookup('Players')\nreturn myName\n","local value=lookup(dynamicName)\nreturn value\n","local require2=require(folder:WaitForChild('Require'))\nreturn require2\n","local require2=require(folder.Shop)\ntype Result=require2.Result\nreturn require2\n","local frame=Instance.new('Frame')\nframe.Name='First'\nframe.Name='Second'\nreturn frame\n","local frame=Instance.new('Frame')\nframe.Name='KeyPanel'\nframe[property]='other'\nreturn frame\n","local value=lookup('Players')\nlocal function verify() print(debug.info(print,'s'),'[C]'); return value end\nreturn value,verify\n"] {
        let (out,stats)=static_clean(src);
        assert_eq!(out,src);
        assert_eq!(stats.get("names").and_then(Value::as_u64).unwrap_or(0),0);
    }
}

#[test]
fn protection_removal_requires_verified_scaffold_and_keeps_application_validation() {
    let src=format!("{SCAFFOLD}{ENVIRONMENT}{PRIMITIVE}{TIMING}local key='valid'\nif key~='valid' then error('application key') end\nlocal data=buffer.create(1)\nif buffer.readu8(data,0)~=0 then error('application checksum failed',0) end\nprint('app')\n");
    let (unchanged, stats) = source_cleanup::strip_protection_source(&src, false).unwrap();
    assert_eq!(unchanged, src);
    assert!(no_stats(&stats));
    let (out, stats) = source_cleanup::strip_protection_source(&src, true).unwrap();
    compiled(&out);
    for pattern in [
        "getconstants",
        "verifyEnvironment",
        "primitive integrity",
        "thread suspension",
    ] {
        assert!(!out.contains(pattern), "{pattern}\n{out}");
    }
    assert!(out.contains("application key"));
    assert!(out.contains("application checksum failed"));
    assert!(stats["checks"].as_u64().unwrap() >= 3);
    assert!(stats["statements"].as_u64().unwrap() >= 4);
    assert_eq!(run(&out), b"app\n");
    let (again, stats) = source_cleanup::strip_protection_source(&out, true).unwrap();
    assert_eq!(again, out);
    assert!(no_stats(&stats));
    assert_eq!(
        source_cleanup::strip_protection_source(ENVIRONMENT, true)
            .unwrap()
            .0,
        ENVIRONMENT
    );
    for verified in [false, true] {
        let out = clean::clean_source(&src, Duration::from_secs(3), 512, verified)
            .unwrap()
            .0;
        assert!(
            out.contains("install()")
                && out.contains("verifyEnvironment()")
                && out.contains("timing()")
        );
    }
}

#[test]
fn protection_preserves_side_effects_in_arguments_callbacks_writes_and_inspector_names() {
    for source in [
        SCAFFOLD.replace("local t = {}", "print('application event')\nlocal t = {}"),
        SCAFFOLD.replace(
            "local target = getgenv()",
            "local target = getgenv()\ntarget.applicationCounter=1",
        ),
        SCAFFOLD.replace(
            "\"getconstants\",",
            "\"applicationCounter\",\"getconstants\",",
        ),
        SCAFFOLD.replace("target[name] = function(fn)", "target[_] = function(fn)"),
    ] {
        let (out, _) = source_cleanup::strip_protection_source(&source, true).unwrap();
        assert!(out.contains("install()"));
    }
    for verifier in [
        ENVIRONMENT.replace(
            "local data = buffer.create(4)",
            "debug.applicationCounter=1\nlocal data = buffer.create(4)",
        ),
        ENVIRONMENT.replace(
            "local data = buffer.create(4)",
            "pcall(debug.applicationEvent)\nlocal data = buffer.create(4)",
        ),
        ENVIRONMENT.replace(
            "buffer.writeu8(data, 0, 186)",
            "buffer.writeu8(buffer.applicationData,0,186)",
        ),
    ] {
        let (out, _) =
            source_cleanup::strip_protection_source(&format!("{SCAFFOLD}{verifier}"), true)
                .unwrap();
        assert!(out.contains("verifyEnvironment()"));
    }
    let source = format!(
        "{SCAFFOLD}{}",
        PRIMITIVE.replace(
            "error(\"primitive integrity failed\", 0)",
            "error(\"primitive integrity failed\", auditApplicationEvent())"
        )
    );
    let (out, _) = source_cleanup::strip_protection_source(&source, true).unwrap();
    assert!(out.contains("auditApplicationEvent()"));
    let setup="getgenv=function() return {} end\ndebug={info=function() return 'Lua' end}\nlocal events=0\nauditApplicationEvent=function() events+=1; return 0 end\n";
    let wrapped =
        |s: &str| format!("{setup}local ok=pcall(function()\n{s}\nend)\nprint(ok,events)\n");
    assert_eq!(run(&wrapped(&source)), run(&wrapped(&out)));
    assert_eq!(run(&wrapped(&out)), b"false\t1\n");
}

#[test]
fn remote_protection_removal_retains_varargs_nil_checks_and_application_denials() {
    let source=format!("{SCAFFOLD}{FORWARD}local r={{FireServer=function(self,...) print('fire',select('#',...),...); return 'sent' end,InvokeServer=function(self,...) return 'invoked',select('#',...) end}}\nprint(send(r,'a',nil,'b'))\nprint(invoke(r,'a',nil))\nprint(send(nil),invoke(nil))\n");
    let (out, stats) = source_cleanup::strip_protection_source(&source, true).unwrap();
    assert!(!out.contains("verifyRemote"));
    assert_eq!(stats["checks"], 2);
    compiled(&out);
    assert_eq!(
        run(&out),
        b"fire\t3\ta\tnil\tb\nsent\ninvoked\t2\nnil\tnil\n"
    );
    let src = format!(
        "{SCAFFOLD}{}",
        FORWARD.replace(
            "then return nil end",
            "then do print('denied audit'); return nil end end"
        )
    );
    let (out, _) = source_cleanup::strip_protection_source(&src, true).unwrap();
    assert!(out.contains("denied audit") && out.contains("verifyRemote()"));
}

#[test]
fn dynamic_cleanup_preserves_utf8_byte_offsets_precedence_and_effects() {
    for (src, want) in [
        (
            "print(\"é € 🎁\",bit32.bxor(5,3),\"中\",bit32.bxor(6,3))\n",
            "print(\"é € 🎁\",6,\"中\",5)\n",
        ),
        (
            "local n=#(\"é\"..tostring(bit32.bxor(1,1)))\n",
            "local n=3\n",
        ),
        ("return x-(bit32.bxor(1,2)-10)\n", "return x-(-7)\n"),
        ("return (bit32.bxor(1,2)-10)^2\n", "return 49\n"),
    ] {
        assert_eq!(dynamic_clean(src).0, want);
    }
    let src = "local v=(function()\n print('hi')\n return bit32.bxor(1,2)\nend)()\n";
    let (out, stats) = dynamic_clean(src);
    assert_eq!(out, src.replace("bit32.bxor(1,2)", "3"));
    assert_eq!(stats["numbers"], 1);
    assert_eq!(stats["values"], 0);
    let (out, stats) = dynamic_clean(&format!("print({INLINE})\n"));
    assert_eq!(out, "print(\"HI\")\n");
    assert_eq!(stats["strings"], 1);
    let (again, stats) = dynamic_clean(&out);
    assert_eq!(again, out);
    assert!(no_stats(&stats));
}

#[test]
fn fidelity_reports_complete_matches_and_incomplete_function_counts() {
    let p = json!({"main":0,"protos":{"0":{"entry":0,"np":0,"nup":0,"kids":{},"consts":{},"decoded":{"0":{"kind":"RETURN","ins":[],"ctrl":["RETURN",{"A":0,"B":1}],"nxt":4294967295u64,"alt":4294967295u64}}}}});
    let report = fidelity::compare(&p, "Function 0 (??):\nRETURN R0 0\n").unwrap();
    assert_eq!(report["functions_orig"], 1);
    assert_eq!(report["identical"], 1);
    assert_eq!(report["match"]["0"], 0);
    let report = fidelity::compare(&p, "").unwrap();
    assert_eq!(report["functions_new"], 0);
    assert_eq!(report["differing"], json!([0]));
    assert!(report["match"]["0"].is_null());
    assert!(fidelity::compare(&p, "Function 0 (??):\nnot bytecode\n").is_err());
    let extra = fidelity::compare(
        &p,
        "Function 0 (??):\nRETURN R0 0\nFunction 1 (??):\nRETURN R0 0\n",
    )
    .unwrap();
    assert_eq!(extra["functions_new"], 2);
    assert_ne!(extra["functions_orig"], extra["functions_new"]);
}

#[test]
fn fidelity_matches_independent_full_safe_vm_reference_report() {
    let golden: Value =
        serde_json::from_str(include_str!("fixtures/safe_vm_reference.json")).unwrap();
    let report =
        fidelity::compare(&golden["program"], golden["bytecode"].as_str().unwrap()).unwrap();
    assert_eq!(report, golden["fidelity"]);
    assert_eq!(report["reordered"], 1);
    assert_eq!(report["identical"], 0);
}

#[test]
fn invalid_source_is_an_error_and_syntax_is_not_equivalence() {
    assert!(source_cleanup::clean_helpers_source("local = broken", true).is_err());
    let check = engine::compile_check("local = broken").unwrap();
    assert_ne!(check["ok"], true);
    assert!(astutil::parse_ast(b"return 1").is_ok());
}

#[test]
fn dynamic_values_cover_original_decryptor_strings_methods_and_signed_contexts() {
    let decrypt = r#"(function()
 local fromstring=buffer.fromstring("\015?5=/1/(s&>p\002\017\022t,+$-")
 local create=buffer.create(20)
 for i=0,19 do
  buffer.writeu8(create,i,bit32.bxor(buffer.readu8(fromstring,i),91,i%256))
 end
 return buffer.tostring(create)
end)()"#;
    for (src, want) in [
        (
            format!("print({decrypt})\n"),
            "print(\"Teleport to UGC game\")\n".to_owned(),
        ),
        (
            format!("loadstring(game:HttpGet((({decrypt}))))()\n"),
            "loadstring(game:HttpGet(\"Teleport to UGC game\"))()\n".into(),
        ),
        (
            format!("local s=({decrypt}):rep(2)\n"),
            "local s=\"Teleport to UGC gameTeleport to UGC game\"\n".into(),
        ),
        (
            format!("local n=({decrypt}):len()+x\nlocal m=({decrypt}):upper()..x\n"),
            "local n=20+x\nlocal m=\"TELEPORT TO UGC GAME\"..x\n".into(),
        ),
        (
            format!("print(({decrypt}):sub(1,3)..x)\nlocal t=x:find(({decrypt}))\n"),
            "print(\"Tel\"..x)\nlocal t=x:find(\"Teleport to UGC game\")\n".into(),
        ),
    ] {
        assert_eq!(dynamic_clean(&src).0, want);
    }
    for (src,want) in [
        ("local s=bit32.bxor(bit32.bor(1732885655,1732885947),bit32.band(1732885655,1732885947))\n","local s=300\n"),
        ("local a=2^bit32.bxor(4294967295,1)\nlocal b=x-bit32.bnot(5)+bit32.bxor(0,0)\n","local a=2^4294967294\nlocal b=x-4294967290+0\n"),
        ("local a=x-(bit32.bxor(1,2)-10)\n","local a=x-(-7)\n"),
        ("local b=x^(bit32.bxor(1,2)-10)\n","local b=x^(-7)\n"),
        ("local c=y..(bit32.bxor(1,2)-10)\n","local c=y..(-7)\n"),
        ("local d=(bit32.bxor(1,2)-10):rep(2)\n","local d=(-7):rep(2)\n"),
        ("local e=x*(bit32.bxor(1,2)-10)\n","local e=x*(-7)\n"),
        ("local f=x- -bit32.bxor(3,1)\n","local f=x- (-2)\n"),
        ("local g=(bit32.bxor(1,2)-10)^2\n","local g=49\n"),
        ("local h=-(bit32.bxor(1,2)-10)\n","local h=7\n"),
        ("local i={(bit32.bxor(1,2)-10),x}\n","local i={-7,x}\n"),
        ("foo(bit32.bxor(1,2)-10)\n","foo(-7)\n"),
        ("local j=-bit32.bxor(3,1)..\"x\"\n","local j=\"-2x\"\n"),
        ("print(x .. bit32.bxor(1,2) .. y)\nprint(x..bit32.bxor(1,2)..y)\n","print(x .. (3) .. y)\nprint(x..(3)..y)\n"),
        ("print(\"a\"..bit32.bxor(1,2)..\"b\")\n","print(\"a3b\")\n"),
        ("local s=buffer.tostring(buffer.fromstring(\"\\240\\159\\142\\129 hi\"))\n","local s=\"🎁 hi\"\n"),
        ("local s=buffer.tostring(buffer.fromstring(\"\\255\\254 ok\"))\n","local s=\"\\255\\254 ok\"\n"),
    ] { let (out,_)=dynamic_clean(src); assert_eq!(out,want,"input: {src}"); }
}

#[test]
fn dynamic_chains_cover_middle_arms_unwrapping_shadowing_exits_and_idempotence() {
    let yes = CLOCKS[2].0;
    let no = CLOCKS[6].0;
    for (src,want) in [
        (format!("if {yes} then\n\tprint(1)\n\tprint(2)\nelseif {no} then\n\terror('dead',0)\nend\nprint(3)\n"),"print(1)\nprint(2)\nprint(3)\n".to_owned()),
        (format!("if {yes} then\n\tx=1\nelse\n\tlocal items={{}}\n\titems[1]='UNREACHABLE'\nend\n"),"x=1\n".into()),
        (format!("before()\nif {no} then\n\terror('dead',0)\nend\nafter()\n"),"before()\nafter()\n".into()),
        (format!("if {no} then\n\terror('dead',0)\nelse\n\tgo()\nend\n"),"go()\n".into()),
        (format!("if a then\n\tone()\nelseif {no} then\n\ttwo()\nelseif b then\n\tthree()\nelse\n\tfour()\nend\n"),"if a then\n\tone()\nelseif b then\n\tthree()\nelse\n\tfour()\nend\n".into()),
        (format!("if a then\n\tone()\nelseif {yes} then\n\ttwo()\nelse\n\tfour()\nend\n"),"if a then\n\tone()\nelse\n\ttwo()\nend\n".into()),
        (format!("local x=1\nif {yes} then\n\tlocal x=2\n\tprint(x)\nend\nprint(x)\n"),"local x=1\ndo\n\tlocal x=2\n\tprint(x)\nend\nprint(x)\n".into()),
        (format!("local x=1\nif {yes} then\n\tlocal y=2\n\tprint(y)\nend\nprint(x)\n"),"local x=1\nlocal y=2\nprint(y)\nprint(x)\n".into()),
        (format!("if {yes} then\n\tif {yes} then\n\t\tx=1\n\telse\n\t\terror('dead')\n\tend\nend\n"),"x=1\n".into()),
    ] {let (out,_)=dynamic_clean(&src);assert_eq!(out,want);let (again,stats)=dynamic_clean(&out);assert_eq!(out,again);assert!(no_stats(&stats));}
    let src = format!(
        "local function f()\n\tif {yes} then\n\t\treturn 1\n\tend\n\treturn nil\nend\nprint(f())\n"
    );
    let (out, _) = dynamic_clean(&src);
    assert!(out.contains("\treturn 1\nend"));
    assert!(!out.contains("return nil"));
    let ordinary="local a=math.pi/2\nlocal b=tostring(5)\nlocal c=string.format('%d',5)\nif math.huge>1 then\n go()\nend\nlocal d=math.random(1,9)\nlocal e=game:GetService('Players')\nprint(bit32.band(flags,1))\n";
    let (out, stats) = dynamic_clean(ordinary);
    assert_eq!(out, ordinary);
    assert!(no_stats(&stats));
    let infinite="local v=(function()\n while true do\n  local x=bit32.bxor(1,2)\n end\n return 1\nend)()\nprint(1)\n";
    let (out, _) = dynamic_clean(infinite);
    assert_eq!(out, infinite);
}

#[test]
fn declarations_merge_only_when_no_scope_or_read_hazard_exists() {
    let src = "local function f()\n local x\n local y\n x,y=1,2\n return x,y\nend\nprint(f())\n";
    let (out, _) = dynamic_clean(src);
    assert!(out.contains("local x,y=1,2"));
    assert_eq!(run(&out), run(src));
    for src in [
        "local x\nprint(x)\nx=1\n",
        "local x\nx=x or 1\n",
        "repeat\n local x\n x=1\nuntil x\n",
    ] {
        assert_eq!(dynamic_clean(src).0, src);
    }
}

#[test]
fn unavailable_compiler_depth_limit_never_counts_as_success() {
    let source = format!("return {}1{}", "(".repeat(4096), ")".repeat(4096));
    let report = engine::compile_check(&source).unwrap();
    assert!(report["ok"].is_null(), "{report}");
    assert!(report["bytecode"].is_null());
    assert!(report["message"].as_str().unwrap().contains("unavailable"));
}

#[test]
fn shared_decoders_using_direct_global_paths_reject_modified_primitive_namespaces() {
    let mut shared = SHARED.to_owned();
    for (alias, path) in [
        ("readLength(", "buffer.len("),
        ("newBuffer(", "buffer.create("),
        ("readByte(", "buffer.readu8("),
        ("writeByte(", "buffer.writeu8("),
        ("toString(", "buffer.tostring("),
        ("xorByte(", "bit32.bxor("),
    ] {
        shared = shared.replace(alias, path);
    }
    let shared = shared[shared.find("local cache").unwrap()..].to_owned();
    let source = format!(
        "buffer.readu8=function() return 0 end\n{shared}return {}\n",
        call(b"Frame", 1)
    );
    for clean in [static_clean as fn(&str) -> (String, Value), dynamic_clean] {
        let (out, counts) = clean(&source);
        assert_eq!(out, source);
        assert_eq!(counts["strings"], 0);
    }
}

#[test]
fn dynamic_cleanup_folds_a_decoder_written_as_a_state_machine() {
    let src = r#"local name = (function()
 local fromstring
 local n
 local create
 local n2 = 5520093
 while n2 do
  if n2 == 7671419 then
   for i = 0, n - 1 do
    buffer.writeu8(create, i, bit32.bxor(buffer.readu8(fromstring, i), 230, i % 256))
   end
   n2 = 7376995
  else
   if n2 == 7376995 then return buffer.tostring(create) end
   if n2 == 5520093 then
    fromstring = buffer.fromstring("\181\132\150\140\146\151\167\180\167")
    n2 = 6825499
   elseif n2 == 6825499 then
    n = 9
    n2 = 251176
   elseif n2 == 251176 then
    create = buffer.create(n)
    n2 = 7671419
   end
  end
 end
end)()
"#;
    let (out, counts) = dynamic_clean(src);
    assert!(out.starts_with("local name = \"") && !out.contains("while"));
    assert_eq!(out.matches('\n').count(), 1);
    assert_eq!(counts["strings"], 1);
}

#[test]
fn dispatch_early_returns_sibling_arms_and_nested_breaks_are_preserved() {
    let src="local function f(flag)\n local out='a'\n local state=1\n while state do\n  if state==1 then\n   out=out..'b'\n   state=2\n  elseif state==2 then\n   if flag then return out..'!' end\n   state=3\n  elseif state==3 then\n   out=out..'c'\n   state=nil\n  end\n end\n return out\nend\nprint(f(true),f(false))\n";
    let (out, stats) = static_clean(src);
    assert_eq!(stats["states"], 1);
    assert_eq!(run(&out), b"ab!\tabc\n");
    let src="local function copy(argument)\n local items=nil\n local step=8466868\n while step do\n  if step==405020 then return items end\n  if step==7472822 then\n   for key,value in pairs(argument) do items[key]=value end\n   step=405020\n  elseif step==8466868 then\n   items={}\n   step=7472822\n  end\n end\nend\nprint(copy({a=1}).a)\n";
    let (out, stats) = static_clean(src);
    assert_eq!(stats["states"], 1);
    assert_eq!(run(&out), b"1\n");
    let src="local step=1\nwhile step do\n if step==1 then\n  for i=1,3 do\n   if i==2 then break end\n   print(i)\n  end\n  step=nil\n end\nend\n";
    let (out, stats) = static_clean(src);
    assert_eq!(stats["states"], 1);
    assert_eq!(run(&out), b"1\n");
}

#[test]
fn static_state_numbers_and_literal_defaults_are_native_exact_integer_folds() {
    let src="local state=bit32.bxor(100,101)\nwhile state do\n if state==bit32.bor(4,5)-bit32.band(4,5) then\n  print('one')\n  state=bit32.bxor(bit32.bor(8,10),bit32.band(8,10))\n elseif state==bit32.bxor(16,18) then\n  print('two')\n  state=nil\n end\nend\n";
    let (out, counts) = static_clean(src);
    assert!(!out.contains("while state") && !out.contains("bit32."));
    assert_eq!(counts["states"], 1);
    assert_eq!(counts["numbers"], 4);
    assert_eq!(run(&out), b"one\ntwo\n");
    let src="local value13=bit32.bor(1847412693,1847412733)-bit32.band(1847412693,1847412733)\nlocal result={KeysNeed=settings.KEYS_PER_ITEM or bit32.bxor(745913778,745913789)}\nreturn value13,result\n";
    let (out, counts) = static_clean(src);
    assert_eq!(out,"local value13=40\nlocal result={KeysNeed=settings.KEYS_PER_ITEM or 15}\nreturn value13,result\n");
    assert_eq!(counts["numbers"], 2);
    for settings in ["{}", "{KEYS_PER_ITEM=7}"] {
        let wrap = |s: &str| {
            format!("local settings={settings}\nlocal n,t=(function()\n{s}\nend)()\nprint(n,t.KeysNeed)\n")
        };
        assert_eq!(run(&wrap(src)), run(&wrap(&out)));
    }
}

#[test]
fn static_checks_keep_real_branches_scope_cleanup_disconnects_and_multiline_literals() {
    let src=format!("local value='outer'\nlocal function f(flag)\n if flag then\n  return 'early'\n elseif {} then\n  local value='inner'\n  print(value)\n else\n  print('never')\n end\n return value\nend\nprint(f(true),f(false))\n",CLOCKS[0].0);
    let (out, counts) = static_clean(&src);
    assert!(out.contains("if flag then") && !out.contains("elseif ") && !out.contains("'never'"));
    assert_eq!(counts["checks"], 1);
    assert_eq!(run(&out), b"inner\nearly\touter\n");
    let src=format!("local function f(flag)\n if flag then\n  return 'early'\n elseif {} then\n  return 'never'\n elseif not flag then\n  return 'fallback'\n end\n return 'after'\nend\nprint(f(true),f(false))\n",CLOCKS[1].0);
    let (out, counts) = static_clean(&src);
    assert!(out.contains("elseif not flag then"));
    assert_eq!(counts["checks"], 1);
    assert_eq!(run(&out), b"early\tfallback\n");
    let src=format!("local state={{cleanup=function() print('cleanup') end}}\nif type(state.cleanup)=='function' then\n if {} then\n  pcall(state.cleanup)\n else\n  print('TRAP_DECOY_JUNK')\n end\n print('wait')\nend\nlocal connections={{{{Disconnect=function() print('disconnect') end}}}}\nfor _,value in ipairs(connections) do\n pcall(function()\n  if {} then\n   value:Disconnect()\n   return\n  end\n  if {} then\n   return\n  end\n end)\nend\ntable.clear(connections)\nprint(#connections)\n",CLOCKS[2].0,CLOCKS[3].0,CLOCKS[1].0);
    let (out, counts) = static_clean(&src);
    assert!(!out.contains("os.clock()") && !out.contains("TRAP_DECOY_JUNK"));
    assert!(
        out.contains("pcall(state.cleanup)")
            && out.contains("value:Disconnect()")
            && out.contains("table.clear(connections)")
    );
    assert!(counts["checks"].as_u64().unwrap() >= 2);
    assert_eq!(run(&out), b"cleanup\nwait\ndisconnect\n0\n");
    let src="local state=1\nwhile state do\n if state==1 then\n  local text=[[first\n   literal spaces\n   last]]\n  print(text)\n  state=nil\n end\nend\n";
    let (out, counts) = static_clean(src);
    assert_eq!(out, src);
    assert_eq!(counts["states"], 0);
    assert_eq!(run(&out), run(src));
}

#[test]
fn modified_primitives_decoder_aliases_and_verifiers_remain_opaque() {
    for src in [
        format!(
            "buffer.readu8=function() return 0 end\n{SHARED}return {}\n",
            call(b"Frame", 1)
        ),
        format!(
            "{SHARED}return {}\n",
            call(b"Frame", 1).replace("buffer.fromstring", "buffer:fromstring")
        ),
        format!("{SHARED}return {}\n", call(b"Frame", 1)).replace(
            "local size = readLength(input)",
            "print('side effect')\nlocal size = readLength(input)",
        ),
        format!("buffer.readu8=replacement\nreturn {INLINE}\n"),
    ] {
        let (out, counts) = static_clean(&src);
        assert_eq!(out, src);
        assert_eq!(counts["strings"], 0);
    }
    let verifier="local function validate()\n if type(bit32)~='table' then return false end\n if bit32.bxor(12345,67890)~=80139 then return false end\n if debug and debug.info and debug.info(bit32.bxor,'s')~='[C]' then return false end\n return true\nend\n";
    let (out, stats) = static_clean(&format!("{verifier}return bit32.bxor(8,9),validate\n"));
    assert_eq!(out, format!("{verifier}return 1,validate\n"));
    assert_eq!(stats["numbers"], 1);
}

#[test]
fn unstable_gui_labels_are_never_name_evidence() {
    for (variable, class, property, label, write) in [
        (
            "textButton",
            "TextButton",
            "Text",
            "Get Key",
            "textButton[\"Text\"]=message",
        ),
        (
            "textButton",
            "TextButton",
            "Text",
            "Get Key",
            "textButton.Text..=suffix",
        ),
        (
            "frame",
            "Frame",
            "Name",
            "KeyPanel",
            "frame[\"Name\"]=message",
        ),
        ("frame", "Frame", "Name", "KeyPanel", "frame.Name..=suffix"),
        (
            "textButton",
            "TextButton",
            "Text",
            "Get Key",
            "textButton[property]=message",
        ),
        (
            "textButton",
            "TextButton",
            "Text",
            "Get Key",
            "function textButton.Text() return 'Changed' end",
        ),
    ] {
        let src=format!("local {variable}=Instance.new('{class}')\n{variable}.{property}='{label}'\n{write}\nreturn {variable}\n");
        let (out, counts) = static_clean(&src);
        assert_eq!(out, src);
        assert_eq!(counts.get("names").and_then(Value::as_u64).unwrap_or(0), 0);
    }
}

#[test]
fn delivery_keeps_installer_effects_combined_predicates_and_outer_callbacks() {
    for effect in [
        "target.applicationCounter=1",
        "target['applicationCounter']=1",
        "target.applicationCounter+=1",
        "function target.applicationCounter() return 1 end",
    ] {
        let src = SCAFFOLD.replace(
            "local target = getgenv()",
            &format!("local target = getgenv()\n{effect}"),
        );
        let (out, _) = source_cleanup::strip_protection_source(&src, true).unwrap();
        let wrap = |s: &str| {
            format!("local exported={{applicationCounter=0}}\ngetgenv=function() return exported end\n{s}\nprint(type(exported.applicationCounter)=='function' and exported.applicationCounter() or exported.applicationCounter)\n")
        };
        assert_eq!(run(&wrap(&src)), b"1\n");
        assert_eq!(run(&wrap(&out)), b"1\n");
    }
    let src = format!(
        "{SCAFFOLD}local key='wrong'\n{}",
        PRIMITIVE.replace(
            "native(length) and native(bit32.bxor)",
            "native(length) and native(bit32.bxor) and key=='valid'"
        )
    );
    let (out, _) = source_cleanup::strip_protection_source(&src, true).unwrap();
    let wrap = |s: &str| {
        format!(
            "getgenv=function() return {{}} end\nlocal ok=pcall(function()\n{s}\nend)\nprint(ok)\n"
        )
    };
    assert_eq!(run(&wrap(&src)), b"false\n");
    assert_eq!(run(&wrap(&out)), b"false\n");
    let src = format!(
        "{SCAFFOLD}{}",
        PRIMITIVE.replace(
            "if buffer and bit32 then",
            "if applicationPermission() then"
        )
    );
    let (out, _) = source_cleanup::strip_protection_source(&src, true).unwrap();
    let wrap = |s: &str| {
        format!("getgenv=function() return {{}} end\nlocal calls=0\napplicationPermission=function() calls+=1; return true end\n{s}\nprint(calls)\n")
    };
    assert_eq!(run(&wrap(&src)), b"1\n");
    assert_eq!(run(&wrap(&out)), b"1\n");
}

fn proto(entry: u64, decoded: Value) -> Value {
    json!({"entry":entry,"np":0,"nup":0,"kids":[],"consts":{},"decoded":decoded})
}
fn record(ins: Value, ctrl: Value, nxt: Value, alt: Value) -> Value {
    json!({"kind":"SIMPLE","ins":ins,"ctrl":ctrl,"nxt":nxt,"alt":alt})
}

#[test]
fn fidelity_dead_definitions_branch_orientation_and_virtual_ids_are_preserved() {
    let p = proto(
        9007199254740993,
        json!({"9007199254740993":record(json!([["LOADN",{"A":0,"rf":1}],["LOADN",{"A":0,"rf":2}]]),json!(["RETURN",{"A":0,"B":2}]),Value::Null,Value::Null)}),
    );
    let report = fidelity::compare(
        &json!({"main":0,"protos":{"0":p}}),
        "Function 0 (??):\nLOADN R0 2\nRETURN R0 1\n",
    )
    .unwrap();
    assert_eq!(report["dead_loads"], 1);
    assert_eq!(report["identical"], 1);
    let p = proto(
        0,
        json!({
            "0":record(json!([["LOADB",{"A":0,"B":1}]]),json!(["JUMPIF",{"A":0}]),json!(1),json!(2)),
            "1":record(json!([]),json!(["RETURN",{"A":0,"B":1}]),Value::Null,Value::Null),
            "2":record(json!([["LOADN",{"A":0,"rf":7}]]),json!(["RETURN",{"A":0,"B":2}]),Value::Null,Value::Null)
        }),
    );
    let report = fidelity::compare(
        &json!({"main":0,"protos":{"0":p}}),
        "Function 0 (??):\nLOADB R0 1\nJUMPIFNOT R0 L0\nLOADN R0 7\nRETURN R0 1\nL0: RETURN R0 0\n",
    )
    .unwrap();
    assert_eq!(report["identical"], 1, "{report}");
}

#[test]
fn fidelity_byref_captures_keep_later_register_definitions_alive() {
    let mut parent = proto(
        0,
        json!({
            "0":record(json!([["LOADN",{"A":0,"rf":1}]]),Value::Null,json!(1),Value::Null),
            "1":record(json!([["NEWCLOSURE",{"A":1,"rf":0}]]),Value::Null,json!(2),Value::Null),
            "2":{"kind":"CAPTURE","ins":[["CAPTURE",{"A":1,"B":0}]],"ctrl":null,"nxt":3,"alt":null},
            "3":record(json!([["LOADN",{"A":0,"rf":2}]]),json!(["RETURN",{"A":1,"B":2}]),Value::Null,Value::Null)
        }),
    );
    parent["kids"] = json!([1]);
    let mut child = proto(
        0,
        json!({"0":record(json!([["GETUPVAL",{"A":0,"B":0}]]),json!(["RETURN",{"A":0,"B":2}]),Value::Null,Value::Null)}),
    );
    child["nup"] = json!(1);
    let program = json!({"main":0,"protos":{"0":parent,"1":child}});
    let compiled="Function 0 (??):\nLOADN R0 1\nNEWCLOSURE R1 P0\nCAPTURE REF R0\nLOADN R0 2\nRETURN R1 1\nFunction 1 (??):\nGETUPVAL R0 0\nRETURN R0 1\n";
    let report = fidelity::compare(&program, compiled).unwrap();
    assert_eq!(report["dead_loads"], 0);
    assert_eq!(report["identical"], 2, "{report}");
    let missing =
        fidelity::compare(&program, compiled.split("Function 1").next().unwrap()).unwrap();
    assert_eq!(missing["functions_orig"], 2);
    assert_eq!(missing["functions_new"], 1);
    assert!(!missing["differing"].as_array().unwrap().is_empty());
}

#[test]
fn fidelity_original_constant_and_token_assertions() {
    assert_eq!(fidelity::sstr(b"plain text"), "plain text");
    assert_eq!(fidelity::sstr(b"a\x01\x1fb\n\t"), "a\\x01\\x1Fb\\x0A\\x09");
    assert_eq!(fidelity::sstr(&[127, 200]), "\u{7f}È");
    assert_eq!(fidelity::sstr(&[1; 10]), "\\x01".repeat(6));
    assert_eq!(fidelity::sstr(&[b'x'; 40]), "x".repeat(24));
    for raw in [
        b"a\x01\x1fb\n\t end of a long string that goes past twenty four chars".as_slice(),
        b"short\x02",
        b"\x01\x02\x03\x04\x05\x06\x07\x08\x0b\x0c",
        b"tab\there",
        &[b'x'; 60],
    ] {
        let literal: String = raw
            .iter()
            .map(|b| {
                if *b < 32 {
                    format!("\\{b}")
                } else {
                    (*b as char).to_string()
                }
            })
            .collect();
        let result = engine::compile_check(&format!("local s=\"{literal}\"\nprint(s)\n")).unwrap();
        assert_eq!(result["ok"], true, "{result}");
        let shown: Vec<_> = result["bytecode"]
            .as_str()
            .unwrap()
            .lines()
            .filter(|l| l.contains("LOADK"))
            .filter_map(fidelity::ann)
            .filter(|s| s.starts_with('\''))
            .collect();
        assert_eq!(shown.len(), 1);
        assert_eq!(fidelity::unq(Some(&shown[0])), fidelity::sstr(raw));
    }
    let tokens = |s: &[&str]| s.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let moved = tokens(&[
        "CALL:-1:1",
        "JEQ",
        "K:N0",
        "IMP:type",
        "CALL:1:1",
        "OP:-",
        "K:N1",
        "FORNPREP",
    ]);
    let after = tokens(&[
        "CALL:-1:1",
        "JEQ",
        "IMP:type",
        "CALL:1:1",
        "OP:-",
        "K:N0",
        "K:N1",
        "FORNPREP",
    ]);
    assert!(fidelity::only_constant_loads_differ(&moved, &after));
    assert!(fidelity::only_constant_loads_differ(&after, &moved));
    for b in [tokens(&["K:N1", "LN", "RET:0"]), tokens(&["K:N1", "RET:0"])] {
        assert!(fidelity::only_constant_loads_differ(
            &tokens(&["LN", "K:N1", "RET:0"]),
            &b
        ));
    }
    let base = tokens(&["K:N0", "IMP:type", "CALL:1:1", "RET:0"]);
    for other in [
        base.clone(),
        tokens(&["IMP:type", "K:N1", "CALL:1:1", "RET:0"]),
        tokens(&["K:N0", "K:N0", "IMP:type", "CALL:1:1", "RET:0"]),
        tokens(&["IMP:type", "CALL:1:1", "RET:0"]),
        tokens(&["K:N0", "IMP:typeof", "CALL:1:1", "RET:0"]),
        tokens(&["IMP:type", "K:N0", "CALL:1:1", "JMP", "RET:0"]),
    ] {
        assert!(!fidelity::only_constant_loads_differ(&base, &other));
    }
    assert!(!fidelity::only_constant_loads_differ(
        &tokens(&[
            "IMP:print",
            "K:Sa",
            "CALL:1:0",
            "IMP:print",
            "K:Sb",
            "CALL:1:0",
            "RET:0"
        ]),
        &tokens(&[
            "IMP:print",
            "K:Sb",
            "CALL:1:0",
            "IMP:print",
            "K:Sa",
            "CALL:1:0",
            "RET:0"
        ])
    ));
    assert!(!fidelity::only_constant_loads_differ(
        &tokens(&["K:Sa", "IMP:print", "CALL:1:0", "RET:0"]),
        &tokens(&["IMP:print", "K:Sa", "CALL:1:0", "RET:0"])
    ));
}

#[test]
fn shared_decoder_aliases_effects_and_builtins_cover_reference_assertions() {
    let source = format!("{SHARED}return {}\n", call(b"Frame", 1))
        .replace("decode", "qW7")
        .replace("cache", "savedBytes");
    for clean in [static_clean as fn(&str) -> (String, Value), dynamic_clean] {
        let (out, stats) = clean(&source);
        assert_eq!(out.trim(), "return \"Frame\"");
        assert_eq!(stats["strings"], 1);
        let mixed = format!("{SHARED}return {}, {INLINE}\n", call(b"Frame", 1));
        let (out, stats) = clean(&mixed);
        assert_eq!(out.trim(), "return \"Frame\", \"HI\"");
        assert_eq!(stats["strings"], 2);
        for source in [
            format!(
                "{}return {}\n",
                SHARED.replace(
                    "local size = readLength(input)",
                    "print('side effect')\nlocal size = readLength(input)"
                ),
                call(b"Frame", 1)
            ),
            format!(
                "{SHARED}return {}\n",
                call(b"Frame", 1).replace("buffer.fromstring", "buffer:fromstring")
            ),
            format!(
                "buffer.readu8=function() return 0 end\n{SHARED}return {}\n",
                call(b"Frame", 1)
            ),
        ] {
            let (out, stats) = clean(&source);
            assert_eq!(stats["strings"], 0, "{source}\n{out}");
            assert!(out.contains("function decode("));
        }
    }
    for source in [
        "return buffer.fromstring('HI')\n".to_owned(),
        format!("buffer.readu8=replacement\nreturn {INLINE}\n"),
    ] {
        assert_eq!(static_clean(&source).0, source);
    }
}

#[test]
fn dispatch_early_return_siblings_scrambled_states_and_nested_loop_breaks() {
    let cases = [
        ("local function f(flag)\n local state=1\n local out='a'\n while state do\n  if state==1 then\n   out=out..'b'\n   state=2\n  elseif state==2 then\n   if flag then return out..'!' end\n   state=3\n  elseif state==3 then\n   out=out..'c'\n   state=nil\n  end\n end\n return out\nend\nprint(f(true),f(false))\n", "ab!\tabc\n"),
        ("local function copy(argument)\n local items=nil\n local step=8466868\n while step do\n  if step==405020 then\n   return items\n  end\n  if step==7472822 then\n   for key,value in pairs(argument) do\n    items[key]=value\n   end\n   step=405020\n  elseif step==8466868 then\n   items={}\n   step=7472822\n  end\n end\nend\nprint(copy({a=1}).a)\n", "1\n"),
        ("local step=1\nwhile step do\n if step==1 then\n  for i=1,3 do\n   if i==2 then break end\n   print(i)\n  end\n  step=nil\n end\nend\n", "1\n"),
        ("local function f(a,b,c)\n local ready,flag4,flag5,value=false,false,false,nil\n local state=9268136\n while state do\n  if state==9268136 then\n   print('first')\n   if a then print('condition kept') end\n   state=5914102\n  elseif state==4610615 then\n   flag5=b\n   state=3620163\n  elseif state==4555853 then\n   ready=true\n   state=nil\n  elseif state==5914102 then\n   flag4=a\n   state=4610615\n  elseif state==3620163 then\n   value=c\n   state=4555853\n  end\n end\n return ready,flag4,flag5,value\nend\nprint(f(true,false,'last'))\n", "first\ncondition kept\ntrue\ttrue\tfalse\tlast\n"),
    ];
    for (source, expected) in cases {
        let (out, stats) = static_clean(source);
        assert_eq!(stats["states"], 1, "{out}");
        assert_eq!(run(&out), expected.as_bytes());
        assert_eq!(run(&out), run(source));
        let (again, counts) = static_clean(&out);
        assert_eq!(again, out);
        assert!(no_stats(&counts));
    }
    for source in [
        "local i=0\nwhile i<3 do i=i+1 end\nprint(i)\n",
        "local step=1\nwhile step do\n step=nil\nend\nprint(step)\n",
    ] {
        let (out, counts) = static_clean(source);
        assert_eq!(out, source);
        assert_eq!(counts["states"], 0);
    }
}

#[test]
fn xor_masked_dispatcher_flattens_only_proven_state_transitions() {
    let source = "local state=bit32.bxor(1,3)\nlocal out={}\nwhile state do\n if bit32.bxor(state,3)==1 then\n  table.insert(out,'one')\n  state=bit32.bxor(state,2)\n elseif bit32.bxor(state,3)==3 then\n  table.insert(out,'two')\n  state=nil\n end\nend\nprint(table.concat(out,','))\n";
    let (out, stats) = static_clean(source);
    assert_eq!(stats["states"], 1, "{out}");
    assert!(
        !out.contains("while state") && !out.contains("bit32."),
        "{out}"
    );
    assert_eq!(run(source), b"one,two\n");
    assert_eq!(run(&out), b"one,two\n");
    for source in [
        source.replace("state=bit32.bxor(state,2)", "state=0"),
        source
            .replace("bit32.bxor(state,3)==1", "1==bit32.bxor(1,state,2)")
            .replace("bit32.bxor(state,3)==3", "3==bit32.bxor(2,state,1)"),
        format!(
            "local xor=bit32.bxor\n{}",
            source.replace("bit32.bxor", "xor")
        ),
        source
            .replace("local state=bit32.bxor(1,3)", "local state=2")
            .replace("bit32.bxor(state,3)==1", "bit32.bxor(state,0)==2")
            .replace("bit32.bxor(state,3)==3", "bit32.bxor(state,0)==0"),
    ] {
        let (out, stats) = static_clean(&source);
        assert_eq!(stats["states"], 1, "{out}");
        assert_eq!(run(&source), b"one,two\n");
        assert_eq!(run(&out), b"one,two\n");
        assert_eq!(static_clean(&out).0, out);
    }
}

#[test]
fn xor_dispatcher_ignores_only_proven_unreachable_decoy_arms() {
    for decoy in [
        "state=bit32.bxor(state,3)\nstate=nil",
        "state=unknownTransition()",
        "print(state)\nstate=nil",
        "break",
    ] {
        let source = format!("local state=2\nwhile state do\n if bit32.bxor(state,3)==1 then\n  print('first')\n  state=bit32.bxor(state,2)\n elseif bit32.bxor(state,3)==7 then\n  {decoy}\n elseif bit32.bxor(state,3)==3 then\n  print('last')\n  state=nil\n end\nend\n");
        assert_eq!(run(&source), b"first\nlast\n");
        let (out, stats) = static_clean(&source);
        assert_eq!(stats["states"], 1, "{out}");
        assert!(!out.contains("while state"), "{out}");
        assert_eq!(run(&out), b"first\nlast\n");
        assert_eq!(static_clean(&out).0, out);
    }
}

#[test]
fn xor_dispatcher_resolves_only_immutable_local_constants() {
    let source = "local mask=3\nlocal seed=2\nlocal initial=seed\nlocal state=initial\nwhile state do\n if bit32.bxor(state,mask)==1 then\n  print('first')\n  local delta=2\n  state=bit32.bxor(state,delta)\n elseif bit32.bxor(state,mask)==3 then\n  print('last')\n  state=nil\n end\nend\n";
    assert_eq!(run(source), b"first\nlast\n");
    let (out, stats) = static_clean(source);
    assert_eq!(stats["states"], 1, "{out}");
    assert_eq!(run(&out), b"first\nlast\n");
    assert_eq!(static_clean(&out).0, out);
    for (declaration, mutation) in [
        ("local mask=3\n", "mask+=0\n"),
        ("local seed=2\n", "seed+=0\n"),
        ("local delta=2\n", "delta+=0\n"),
    ] {
        let source = source.replace(declaration, &format!("{declaration}{mutation}"));
        let (out, stats) = static_clean(&source);
        assert_eq!(stats["states"], 0, "{out}");
        assert_eq!(run(&out), b"first\nlast\n");
    }
}

#[test]
fn xor_dispatcher_defers_repeated_expression_aliases() {
    let source = "local a0=1\nlocal a1=bit32.bxor(a0,a0,a0,a0,a0,a0,a0,a0)\nlocal a2=bit32.bxor(a1,a1,a1,a1,a1,a1,a1,a1)\nlocal a3=bit32.bxor(a2,a2,a2,a2,a2,a2,a2,a2)\nlocal state=a3\nwhile state do\n if state==0 then\n  print('done')\n  state=nil\n end\nend\n";
    let root = astutil::parse_ast(source.as_bytes()).unwrap();
    let (out, states) = bgdeobf::unflatten::flatten_dispatchers(source, &root).unwrap();
    assert_eq!(states, 0, "{out}");
    assert_eq!(out, source);
    let (out, stats) = static_clean(source);
    assert_eq!(stats["states"], 1, "{out}");
    assert_eq!(run(source), b"done\n");
    assert_eq!(run(&out), b"done\n");
}

#[test]
fn xor_dispatcher_rejects_ambiguous_or_observable_state() {
    let source = "local state=2\nlocal out={}\nwhile state do\n if bit32.bxor(state,3)==1 then\n  table.insert(out,'one')\n  state=bit32.bxor(state,2)\n elseif bit32.bxor(state,3)==3 then\n  table.insert(out,'two')\n  state=nil\n end\nend\nprint(table.concat(out,','))\n";
    for source in [
        source.replacen("bxor(state,3)==3", "bxor(state,4)==3", 1),
        source.replace("bxor(state,2)", "bxor(state,unknown)"),
        source.replace("bxor(state,3)", "bxor(state,unknown)"),
        format!("{source}print(state)\n"),
        source.replace("table.insert(out,'one')", "table.insert(out,state)"),
        source.replace("bxor(state,3)==3", "bxor(state,3)==1"),
        source.replace("state=nil", "state=2"),
        source.replace("state=nil", "state=unknown"),
        format!("local bit32=applicationBits\n{source}"),
        format!("bit32.bxor=replacement\n{source}"),
    ] {
        let (out, stats) = static_clean(&source);
        assert_eq!(stats["states"], 0, "{source}\n{out}");
        assert!(out.contains("while state"), "{out}");
    }
}

#[test]
fn xor_dispatcher_keeps_impossible_unsigned_cases() {
    for state in [-1_i64, 4294967296] {
        let source = format!("local state={state}\nwhile state do\n if bit32.bxor(state,0)=={state} then\n  print('unreachable')\n  state=nil\n end\nend\n");
        let (out, stats) = static_clean(&source);
        assert_eq!(stats["states"], 0, "{out}");
        assert!(out.contains("while state"), "{out}");
    }
}

#[test]
fn xor_dispatcher_keeps_mixed_zero_mask_domains() {
    let source = "local state=-1\nwhile state do\n if bit32.bxor(state,0)==4294967295 then\n  print('xor')\n  state=nil\n elseif state==-1 then\n  print('direct')\n  state=nil\n end\nend\n";
    assert_eq!(run(source), b"xor\n");
    let root = astutil::parse_ast(source.as_bytes()).unwrap();
    let (direct, states) = bgdeobf::unflatten::flatten_dispatchers(source, &root).unwrap();
    assert_eq!(states, 0, "{direct}");
    let (out, stats) = static_clean(source);
    assert_eq!(stats["states"], 0, "{out}");
    assert_eq!(run(&out), b"xor\n");
}

#[test]
fn xor_dispatcher_keeps_sibling_guard_errors() {
    for (mask, first, second) in [(3, 1, 3), (0, 2, 0)] {
        let guards = format!("if bit32.bxor(state,{mask})=={first} then\n print('first')\n state=nil\nend\nif bit32.bxor(state,{mask})=={second} then\n state=nil\nend\n");
        for guards in [
            guards.clone(),
            format!("if bit32.bxor(state,{mask})==7 then\n state=nil\nelse\n{guards}end\n"),
        ] {
            let source = format!("local ok=pcall(function()\n local state=2\n while state do\n{guards} end\nend)\nprint(ok)\n");
            assert_eq!(run(&source), b"first\nfalse\n");
            let (out, stats) = static_clean(&source);
            assert_eq!(stats["states"], 0, "{out}");
            assert_eq!(run(&out), b"first\nfalse\n");
        }
    }
}

#[test]
fn xor_dispatcher_terminal_siblings_preserve_returns() {
    for (mask, first, finish) in [(3, 1, 3), (0, 2, 0)] {
        let guards = format!("if bit32.bxor(state,{mask})=={finish} then\n print('finish')\n return 42,'ok'\nend\nif bit32.bxor(state,{mask})=={first} then\n print('first')\n if flag then return 'early',false end\n state=bit32.bxor(state,2)\nelseif bit32.bxor(state,{mask})==7 then\n state=bit32.bxor(state,3)\n state=nil\nend\n");
        for guards in [
            guards.clone(),
            format!("if bit32.bxor(state,{mask})==9 then\n state=nil\nelse\n{guards}end\n"),
        ] {
            let source = format!("local function f(flag)\n local state=2\n while state do\n{guards} end\n print('unreachable')\nend\nprint(f(true))\nprint(f(false))\n");
            let expected = b"first\nearly\tfalse\nfirst\nfinish\n42\tok\n";
            assert_eq!(run(&source), expected);
            let root = astutil::parse_ast(source.as_bytes()).unwrap();
            let (direct, states) = bgdeobf::unflatten::flatten_dispatchers(&source, &root).unwrap();
            assert_eq!(states, 1, "{direct}");
            assert_eq!(run(&direct), expected);
            let (out, stats) = static_clean(&source);
            assert_eq!(stats["states"], 1, "{out}");
            assert!(!out.contains("while state"), "{out}");
            assert_eq!(run(&out), expected);
            assert_eq!(static_clean(&out).0, out);
        }
    }
}

#[test]
fn xor_dispatcher_keeps_conditional_returns_and_outer_sibling_errors() {
    for guards in [
        "if bit32.bxor(state,3)==1 then\n if flag then return end\n print('first')\n state=nil\nend\nif bit32.bxor(state,3)==3 then\n state=nil\nend\n",
        "if bit32.bxor(state,3)==9 then\n return\nelse\n if bit32.bxor(state,3)==1 then\n  print('first')\n  state=nil\n end\nend\nif bit32.bxor(state,3)==3 then\n return\nend\n",
    ] {
        let source = format!("local flag=false\nlocal ok=pcall(function()\n local state=2\n while state do\n{guards} end\nend)\nprint(ok)\n");
        assert_eq!(run(&source), b"first\nfalse\n");
        let root = astutil::parse_ast(source.as_bytes()).unwrap();
        let (direct, states) = bgdeobf::unflatten::flatten_dispatchers(&source, &root).unwrap();
        assert_eq!(states, 0, "{direct}");
        assert_eq!(run(&direct), b"first\nfalse\n");
        let (out, stats) = static_clean(&source);
        assert_eq!(stats["states"], 0, "{out}");
        assert_eq!(run(&out), b"first\nfalse\n");
    }
}

#[test]
fn xor_dispatcher_terminal_returns_keep_valid_scope() {
    for condition in ["bit32.bxor(state,3)==1", "state==2"] {
        let source = format!("local function f()\n local state=2\n while state do\n  if {condition} then\n   return 42\n  end\n end\n print('unreachable')\nend\nprint(f())\n");
        assert_eq!(run(&source), b"42\n");
        let (out, stats) = static_clean(&source);
        assert_eq!(stats["states"], 1, "{out}");
        assert_eq!(run(&out), b"42\n");
        assert_eq!(static_clean(&out).0, out);
    }
}

#[test]
fn static_state_keys_numbers_and_unknown_semantics_cover_reference_assertions() {
    let source="local function f()\n local state=bit32.bxor(100,101)\n while state do\n  if state==bit32.bor(4,5)-bit32.band(4,5) then\n   print('one')\n   state=bit32.bxor(bit32.bor(8,10),bit32.band(8,10))\n  elseif state==bit32.bxor(16,18) then\n   print('two')\n   state=nil\n  end\n end\nend\nf()\n";
    let (out, stats) = static_clean(source);
    assert!(!out.contains("while state") && !out.contains("bit32."));
    assert_eq!(stats["states"], 1);
    assert_eq!(stats["numbers"], 4);
    assert_eq!(run(&out), b"one\ntwo\n");
    assert_eq!(run(&out), run(source));
    let source="local value13=bit32.bor(1847412693,1847412733)-bit32.band(1847412693,1847412733)\nlocal result={KeysNeed=settings.KEYS_PER_ITEM or bit32.bxor(745913778,745913789)}\nreturn value13,result\n";
    let (out, stats) = static_clean(source);
    assert_eq!(out,"local value13=40\nlocal result={KeysNeed=settings.KEYS_PER_ITEM or 15}\nreturn value13,result\n");
    assert_eq!(stats["numbers"], 2);
    for (settings, expected) in [
        ("{}", b"40\t15\n".as_slice()),
        ("{KEYS_PER_ITEM=7}", b"40\t7\n"),
    ] {
        for text in [source, &out] {
            assert_eq!(run(&format!("local settings={settings}\nlocal n,t=(function()\n{text}\nend)()\nprint(n,t.KeysNeed)\n")),expected);
        }
    }
    for source in ["local state=applicationState()\nwhile state do if state==1 then state=nil end end\n", "local state=bit32.bxor(unknown,1)\nwhile state do if state==1 then state=nil end end\n", "bit32.bxor=replacement\nlocal state=bit32.bxor(8,9)\nwhile state do if state==1 then state=nil end end\n", "return bit32.band(applicationEvent(),0)\n", "local bit32=applicationBits\nreturn bit32.bxor(1,2)\n"] {
        let (out,stats)=static_clean(source);assert_eq!(out,source);assert_eq!(stats["numbers"],0);assert_eq!(stats["states"],0);
    }
    for (source, want) in [
        ("return 'x'..bit32.bxor(1,2)\n", "return 'x'..(3)\n"),
        ("return 2^(bit32.band(7,3)-10)\n", "return 2^((-7))\n"),
        ("return bit32.bxor(1,2)()\n", "return (3)()\n"),
    ] {
        let (out, stats) = static_clean(source);
        assert_eq!(out, want);
        assert_eq!(stats["numbers"], 1);
    }
    let verifier="local function validate()\n if type(bit32)~='table' then return false end\n if bit32.bxor(12345,67890)~=80139 then return false end\n if debug and debug.info and debug.info(bit32.bxor,'s')~='[C]' then return false end\n return true\nend\n";
    let (out, stats) = static_clean(&format!("{verifier}return bit32.bxor(8,9),validate\n"));
    assert_eq!(out, format!("{verifier}return 1,validate\n"));
    assert_eq!(stats["numbers"], 1);
}

#[test]
fn private_predicate_helpers_preserve_arguments_and_native_errors() {
    for body in [
        "return bit32.bor(argument,13)-bit32.band(argument,13)==bit32.bxor(argument,13)",
        "local masked=bit32.band(argument,65535)\nreturn masked*(masked+1)%2==0",
        "local masked=bit32.band(argument,65535)\nreturn (masked*masked-masked)%2==0",
        "return bit32.band(bit32.bor(argument,6),6)==6",
        "return bit32.bxor(argument,bit32.bnot(argument))==4294967295",
    ] {
        let source = format!("local function run()\n local function predicate(argument)\n {body}\n end\n if predicate(math.floor(os.clock()*100)+17) then\n  print('selected')\n  return 42,false\n else\n  error('TRAP_DECOY_JUNK',0)\n end\nend\nprint(run())\n");
        let (out, stats) = static_clean(&source);
        assert_eq!(stats["checks"], 1, "{out}");
        assert!(!out.contains("local function predicate"), "{out}");
        assert!(!out.contains("TRAP_DECOY_JUNK"), "{out}");
        assert_eq!(run(&source), b"selected\n42\tfalse\n");
        assert_eq!(run(&out), b"selected\n42\tfalse\n");
        let setup = "local calls=0\nlocal env=getfenv()\nenv.os={clock=function() calls+=1;print('clock');return 0.5 end}\n";
        for text in [&source, &out] {
            assert_eq!(
                run(&format!("{setup}{text}\nprint(calls)\n")),
                b"clock\nselected\n42\tfalse\n1\n"
            );
            assert_eq!(run(&format!("local env=getfenv()\nenv.os={{clock=function() print('clock');error('clock failure',0) end}}\nlocal ok=pcall(function()\n{text}\nend)\nprint(ok)\n")), b"clock\nfalse\n");
            assert_eq!(run(&format!("local env=getfenv()\nenv.bit32={{}}\nlocal ok=pcall(function()\n{text}\nend)\nprint(ok)\n")), b"false\n");
        }
        assert_eq!(static_clean(&out).0, out);
    }
}

#[test]
fn private_predicate_helpers_keep_unproven_bodies_inputs_and_references() {
    let declaration =
        "local function predicate(argument)\n return bit32.band(bit32.bor(argument,6),6)==6\nend\n";
    let source = format!("{declaration}if predicate(math.floor(os.clock()*100)+71) then\n print('selected')\nelse\n print('other')\nend\n");
    for source in [
        source.replace("return bit32.band", "print('audit')\n return bit32.band"),
        source.replace("bor(argument,6),6)==6", "bor(argument,6),6)==7"),
        source.replace("math.floor(os.clock()*100)+71", "applicationValue()"),
        source.replace("math.floor(os.clock()*100)+71", "nil"),
        source.replace("math.floor(os.clock()*100)+71", "'1'"),
        format!("{source}local exported=predicate\nprint(exported)\n"),
        source.replace(
            "if predicate",
            "predicate=function() return false end\nif predicate",
        ),
        format!("bit32.band=replacement\n{source}"),
        format!("local bit32=applicationBits\n{source}"),
        format!("local os=applicationClock\n{source}"),
        format!("local math=applicationMath\n{source}"),
    ] {
        let (out, stats) = static_clean(&source);
        assert_eq!(stats["checks"], 0, "{out}");
        assert!(out.contains("predicate"), "{out}");
    }
    let source = source.replace("math.floor(os.clock()*100)+71", "applicationValue()");
    let setup = "local function applicationValue() print('argument');return 1 end\n";
    let (out, _) = static_clean(&source);
    for text in [&source, &out] {
        assert_eq!(run(&format!("{setup}{text}")), b"argument\nselected\n");
    }
}

#[test]
fn private_predicate_helpers_preserve_elseif_scope_and_temporary_names() {
    let source = "local predicateValue0='outside'\nlocal masked='outer'\nlocal function f(flag)\n local function predicate(argument)\n  local masked=bit32.band(argument,65535)\n  return masked*(masked+1)%2==0\n end\n if flag then\n  print('early')\n elseif predicate(math.floor(os.clock()*100)+17) then\n  local masked='inner'\n  print(masked,predicateValue0)\n  return masked\n else\n  error('decoy',0)\n end\n return masked\nend\nprint(f(true),f(false))\n";
    let (out, stats) = static_clean(source);
    assert_eq!(stats["checks"], 1, "{out}");
    assert!(!out.contains("local function predicate"), "{out}");
    assert_eq!(run(source), b"early\ninner\toutside\nouter\tinner\n");
    assert_eq!(run(&out), b"early\ninner\toutside\nouter\tinner\n");
    let setup =
        "local calls=0\ngetfenv().os={clock=function() calls+=1;print('clock');return 0.5 end}\n";
    for text in [source, &out] {
        assert_eq!(
            run(&format!("{setup}{text}\nprint(calls)\n")),
            b"early\nclock\ninner\toutside\nouter\tinner\n1\n"
        );
    }
    assert_eq!(static_clean(&out).0, out);
}

#[test]
fn private_predicate_helpers_preserve_empty_elseif_effects() {
    let source = "local function predicate(argument)\n return bit32.band(bit32.bor(argument,6),6)==6\nend\nif applicationFlag then\n print('early')\nelseif predicate(math.floor(os.clock()*100)+17) then\nelse\n error('decoy',0)\nend\nprint('after')\n";
    let (out, stats) = static_clean(source);
    assert_eq!(stats["checks"], 1, "{out}");
    let setup = "getfenv().os={clock=function() print('clock');return 0.5 end}\n";
    for text in [source, &out] {
        assert_eq!(run(&format!("{setup}{text}")), b"clock\nafter\n");
        assert_eq!(
            run(&format!("applicationFlag=true\n{setup}{text}")),
            b"early\nafter\n"
        );
        assert_eq!(
            run(&format!(
                "getfenv().bit32={{}}\nlocal ok=pcall(function()\n{text}\nend)\nprint(ok)\n"
            )),
            b"false\n"
        );
    }
    assert_eq!(static_clean(&out).0, out);
}

#[test]
fn private_predicate_helpers_accept_grouped_parameter_references() {
    let source = "local function predicate(argument)\n return bit32.band(bit32.bor(((argument)),6),6)==6\nend\nif predicate(math.floor(os.clock()*100)+17) then\n print('selected')\nelse\n error('decoy',0)\nend\n";
    assert_eq!(run(source), b"selected\n");
    let (out, stats) = static_clean(source);
    assert_eq!(stats["checks"], 1, "{out}");
    assert_eq!(run(&out), b"selected\n");
    let setup = "getfenv().os={clock=function() print('clock');return 0.5 end}\n";
    for text in [source, &out] {
        assert_eq!(run(&format!("{setup}{text}")), b"clock\nselected\n");
    }
    assert_eq!(static_clean(&out).0, out);
}

#[test]
fn private_predicate_helpers_keep_definition_bindings() {
    let mut observations = vec![];
    for (declarations, body, shadow) in [
        (
            "",
            "return bit32.band(bit32.bor(argument,6),6)==6",
            "local bit32={bor=function() return 0 end,band=function() print('shadow');return 0 end}",
        ),
        (
            "local nativeXor=bit32.bxor\n",
            "return nativeXor(argument,bit32.bnot(argument))==4294967295",
            "local nativeXor=function() print('shadow');return 0 end",
        ),
    ] {
        let source = format!("{declarations}local function predicate(argument)\n {body}\nend\ndo\n {shadow}\n if predicate(math.floor(os.clock()*100)+17) then\n  print('selected')\n else\n  print('other')\n end\nend\n");
        assert_eq!(run(&source), b"selected\n");
        let (out, stats) = static_clean(&source);
        observations.push((run(&out), stats["checks"].as_u64().unwrap()));
    }
    assert_eq!(observations, vec![(b"selected\n".to_vec(), 0); 2]);
}

#[test]
fn static_clock_chains_keep_live_branches_callbacks_and_literal_whitespace() {
    let source=format!("local value='outer'\nlocal function f(flag)\n if flag then\n  return 'early'\n elseif {} then\n  local value='inner'\n  print(value)\n else\n  print('never')\n end\n return value\nend\nprint(f(true),f(false))\n",CLOCKS[0].0);
    let (out, stats) = static_clean(&source);
    assert_eq!(stats["checks"], 1);
    assert!(!out.contains("elseif") && !out.contains("'never'"));
    assert_eq!(run(&out), b"inner\nearly\touter\n");
    let source=format!("local function f(flag)\n if flag then\n  return 'early'\n elseif {} then\n  return 'never'\n elseif not flag then\n  return 'fallback'\n end\n return 'after'\nend\nprint(f(true),f(false))\n",CLOCKS[1].0);
    let (out, stats) = static_clean(&source);
    assert_eq!(stats["checks"], 1);
    assert!(out.contains("elseif not flag") && !out.contains("999999"));
    assert_eq!(run(&out), b"early\tfallback\n");
    let source=format!("local state={{cleanup=function() print('cleanup') end}}\nif type(state.cleanup)=='function' then\n if {} then\n  pcall(state.cleanup)\n else\n  print('TRAP_DECOY_JUNK')\n end\n print('wait')\nend\nlocal connections={{{{Disconnect=function() print('disconnect') end}}}}\nfor _,value in ipairs(connections) do\n pcall(function()\n  if {} then\n   value:Disconnect()\n   return\n  end\n  if {} then\n   return\n  end\n end)\nend\ntable.clear(connections)\nprint(#connections)\n",CLOCKS[2].0,CLOCKS[3].0,CLOCKS[1].0);
    let (out, stats) = static_clean(&source);
    assert!(!out.contains("os.clock()") && !out.contains("TRAP_DECOY_JUNK"));
    assert!(stats["checks"].as_u64().unwrap() >= 2);
    assert_eq!(run(&out), b"cleanup\nwait\ndisconnect\n0\n");
    for source in [
        "if bit32.band(applicationEvent(),0)==0 then print('keep') end\n",
        "if bit32.band(math.floor(os.clock()*100),mask)==0 then print('keep') end\n",
        "if os.clock()-started>0.5 then error('timing guard') end\n",
        "if bit32.bor(applicationEvent(),4294967295)==4294967295 then print('keep') end\n",
        "if math.abs(os.clock()-started)<0.5 then print('real time') end\n",
        "math.abs=replacement\nif math.abs(os.clock())+1>0 then print('keep') end\n",
        "local os=appClock\nif math.floor(os.clock())*2%2==0 then print('keep') end\n",
    ] {
        let (out, stats) = static_clean(source);
        assert_eq!(out, source);
        assert_eq!(stats["checks"], 0);
    }
    for dispatch in [false, true] {
        let body="        local text=[[first\n        literal spaces\n        last]]\n        print(text)\n";
        let source = if dispatch {
            format!("local state=1\nwhile state do\n    if state==1 then\n{body}        state=nil\n    end\nend\n")
        } else {
            format!("if {} then\n{body}end\n", CLOCKS[5].0)
        };
        let (out, stats) = static_clean(&source);
        assert_eq!(out, source);
        assert_eq!(stats["checks"], 0);
        assert_eq!(stats["states"], 0);
        assert_eq!(run(&out), run(&source));
    }
}

#[test]
fn protection_delivery_keeps_all_external_effects_and_unknown_predicates() {
    let source = format!(
        "{SCAFFOLD}local key='wrong'\n{}",
        PRIMITIVE.replace(
            "native(length) and native(bit32.bxor)",
            "native(length) and native(bit32.bxor) and key=='valid'"
        )
    );
    let (out, _) = source_cleanup::strip_protection_source(&source, true).unwrap();
    for text in [&source, &out] {
        assert_eq!(run(&format!("getgenv=function() return {{}} end\nlocal ok=pcall(function()\n{text}\nend)\nprint(ok)\n")),b"false\n");
    }
    let source = format!(
        "{SCAFFOLD}{}",
        PRIMITIVE.replace(
            "if buffer and bit32 then",
            "if applicationPermission() then"
        )
    );
    let (out, _) = source_cleanup::strip_protection_source(&source, true).unwrap();
    for text in [&source, &out] {
        assert_eq!(run(&format!("getgenv=function() return {{}} end\nlocal calls=0\napplicationPermission=function() calls+=1;return true end\n{text}\nprint(calls)\n")),b"1\n");
    }
    for effect in [
        "target.applicationCounter=1",
        "target['applicationCounter']=1",
        "target.applicationCounter+=1",
        "function target.applicationCounter() return 1 end",
    ] {
        let source = SCAFFOLD.replace(
            "local target = getgenv()",
            &format!("local target = getgenv()\n{effect}"),
        );
        let (out, _) = source_cleanup::strip_protection_source(&source, true).unwrap();
        for text in [&source, &out] {
            assert_eq!(run(&format!("local exported={{applicationCounter=0}}\ngetgenv=function() return exported end\n{text}\nprint(type(exported.applicationCounter)=='function' and exported.applicationCounter() or exported.applicationCounter)\n")),b"1\n");
        }
    }
    for (source,setup,observe,expected) in [
        (SCAFFOLD.replace("\"getconstants\",","\"applicationCounter\",\"getconstants\","),"local original=function() return 'application' end\nlocal exported={applicationCounter=original}\ngetgenv=function() return exported end\n","print(exported.applicationCounter==original)\n","false\n"),
        (SCAFFOLD.replace("target[name] = function(fn)","target[_] = function(fn)"),"local exported={}\ngetgenv=function() return exported end\n","print(type(exported[1]))\n","function\n"),
        (format!("{SCAFFOLD}{}",FORWARD.replace("then return nil end","then do print('denied audit');return nil end end")),"getgenv=function() return {} end\ngetrawmetatable=function() return {__namecall=function() end} end\n","print(send(nil))\n","denied audit\nnil\n"),
    ] {let (out,_)=source_cleanup::strip_protection_source(&source,true).unwrap();for text in [&source,&out] {assert_eq!(run(&format!("{setup}{text}{observe}")),expected.as_bytes());}}
    let environment=ENVIRONMENT.replace("local data = buffer.create(4)","if bit32.bxor(12345,67890)~=80139 or bit32.band(65535,255)~=255 then return false end\nlocal data = buffer.create(4)");
    let (out,stats)=source_cleanup::strip_protection_source(&format!("{SCAFFOLD}{environment}if applicationKey~='valid' then error('application key failed',0) end\n"),true).unwrap();
    assert!(!out.contains("verifyEnvironment"));
    assert!(out.contains("application key failed"));
    assert_eq!(stats["checks"], 1);
    compiled(&out);
}

#[test]
fn protection_cleanup_preserves_function_definition_writes() {
    let verifier = ENVIRONMENT.replace(
        "local data = buffer.create(4)",
        "function table.applicationHook() return true end\nlocal data = buffer.create(4)",
    );
    let source = format!("{SCAFFOLD}{verifier}");
    let (out, _) = source_cleanup::strip_protection_source(&source, true).unwrap();
    assert!(out.contains("function table.applicationHook"), "{out}");
    assert!(out.contains("verifyEnvironment"), "{out}");
    let setup =
        "local env=getfenv(); env.table=table.clone(table); env.getgenv=function() return {} end\n";
    for text in [&source, &out] {
        assert_eq!(
            run(&format!(
                "{setup}{text}\nprint(type(table.applicationHook))"
            )),
            b"function\n"
        );
    }
}

#[test]
fn dynamic_cleanup_retains_runtime_protection_and_remote_behavior() {
    let setup = "getgenv=function() return {} end\n";
    for verified in [false, true] {
        let (out, _) =
            clean::clean_source(SCAFFOLD, Duration::from_secs(3), 512, verified).unwrap();
        assert_eq!(run(&format!("local exported={{}}\ngetgenv=function() return exported end\n{out}print(type(exported.getconstants))\n")),b"function\n");
    }
    for source in ["local function inspect(fn) return debug.getconstants(fn) end\nreturn inspect\n".to_owned(),format!("{ENVIRONMENT}print('app')\n"),"local function send(remote,...) if not applicationPermission() then return nil end return remote:FireServer(...) end\nreturn send\n".into()] {assert_eq!(dynamic_clean(&source).0,source);}
    let source = format!(
        "local notify=print\n{}",
        SCAFFOLD.replace(
            "local decoy = trap()",
            "notify('application event')\nlocal decoy = trap()"
        )
    );
    let (out, _) = dynamic_clean(&source);
    assert!(out.contains("notify('application event')") && out.contains("install()"));
    let app="local remote={FireServer=function(self,...) print('fire',select('#',...),...);return 'sent' end,InvokeServer=function(self,...) return 'invoked',select('#',...) end}\nprint(send(remote,'a',nil,'b'))\nprint(invoke(remote,'a',nil))\nprint(send(nil),invoke(nil))\n";
    let (out, _) = dynamic_clean(&format!("{SCAFFOLD}{FORWARD}{app}"));
    assert!(out.contains("verifyRemote"));
    assert_eq!(
        run(&format!("{setup}{out}")),
        b"fire\t3\ta\tnil\tb\nsent\ninvoked\t2\nnil\tnil\n"
    );
    assert_eq!(
        run(&format!(
            "{setup}getrawmetatable=function() return {{__namecall=function() end}} end\n{out}"
        )),
        b"nil\nnil\nnil\tnil\n"
    );
}

#[test]
fn source_naming_reference_bindings_runtime_and_all_mutations() {
    let source="local value=lookup(('Players'))\nlocal require2=require(folder:WaitForChild('ShopClient'))\nlocal function use() return value.Name,require2.ready end\nlocal function shadow(value) return value,'value require2',{value='property'} end\nlocal a,b,properties=shadow('argument')\nprint(use(),a,b,properties.value)\n";
    let (out, stats) = static_clean(source);
    assert_eq!(stats["names"], 2);
    assert!(
        out.contains("local Players=")
            && out.contains("local shopClient=")
            && out.contains("return Players.Name,shopClient.ready")
            && out.contains("shadow(value)")
    );
    assert!(out.contains("'value require2',{value='property'}"));
    let setup="local lookup=function() return {Name='players'} end\nlocal folder={WaitForChild=function(_,name) return name end}\nlocal require=function() return {ready=true} end\n";
    assert_eq!(
        run(&format!("{setup}{out}")),
        run(&format!("{setup}{source}"))
    );
    let (again, counts) = static_clean(&out);
    assert_eq!(again, out);
    assert!(no_stats(&counts));
    for source in ["local value=application()\nreturn value\n", "local require2=custom(folder:WaitForChild('Net'))\nreturn require2\n", "local value=lookup('NotAService')\nreturn value\n", "local require2=require(folder:WaitForChild('not a valid identifier'))\nreturn require2\n", "local function native() local value=lookup('Players');print(debug.info(value,'s'),'[C]');return value end\nreturn native\n", "local value=lookup('Players')\nfunction value() end\nprint(debug.info(value,'n'))\n", "local keyPanel=Instance.new('Frame')\nkeyPanel.Name='OtherName'\nreturn keyPanel\n", "local textLabel=Instance.new('TextLabel')\ntextLabel.Text='Ready'\ntextLabel.Text=message\nreturn textLabel\n", "local frame=Instance.new('Frame')\nframe.Name='Frame'\nreturn frame\n"] {let (out,stats)=static_clean(source);assert_eq!(out,source);assert_eq!(stats.get("names").and_then(Value::as_u64).unwrap_or(0),0);}
    for (variable, class, property, label, write) in [
        (
            "textButton",
            "TextButton",
            "Text",
            "Get Key",
            "textButton['Text']=message",
        ),
        (
            "textButton",
            "TextButton",
            "Text",
            "Get Key",
            "textButton.Text..=suffix",
        ),
        (
            "frame",
            "Frame",
            "Name",
            "KeyPanel",
            "frame['Name']=message",
        ),
        ("frame", "Frame", "Name", "KeyPanel", "frame.Name..=suffix"),
        (
            "textButton",
            "TextButton",
            "Text",
            "Get Key",
            "textButton[property]=message",
        ),
        (
            "textButton",
            "TextButton",
            "Text",
            "Get Key",
            "function textButton.Text() return 'Changed' end",
        ),
    ] {
        let source=format!("local {variable}=Instance.new('{class}')\n{variable}.{property}='{label}'\n{write}\nreturn {variable}\n");
        let (out, stats) = static_clean(&source);
        assert_eq!(out, source);
        assert_eq!(stats.get("names").and_then(Value::as_u64).unwrap_or(0), 0);
    }
    let source="local frame=Instance.new('Frame')\nframe.Name='KeyPanel'\nlocal keyPanel='existing'\nprint(frame.Name,keyPanel,keyPanel2,({frame='literal'}).frame)\n";
    let (out, _) = static_clean(source);
    assert!(out.contains("local keyPanel3=") && out.contains("({frame='literal'}).frame"));
    let setup = "local Instance={new=function() return {} end}\n";
    assert_eq!(
        run(&format!("{setup}{out}")),
        run(&format!("{setup}{source}"))
    );
    let (unnamed, counts) = source_cleanup::clean_helpers_source(source, false).unwrap();
    assert_eq!(unnamed, source);
    assert_eq!(counts.get("names").and_then(Value::as_u64).unwrap_or(0), 0);
}

#[test]
fn dynamic_flattened_value_nil_results_and_application_functions() {
    let flattened = r#"(function()
 local fromstring
 local n
 local create
 local n2=5520093
 while n2 do
  if n2==7671419 then
   for i=0,n-1 do
    buffer.writeu8(create,i,bit32.bxor(buffer.readu8(fromstring,i),230,i%256))
   end
   n2=7376995
  else
   if n2==7376995 then
    return buffer.tostring(create)
   end
   if n2==5520093 then
    fromstring=buffer.fromstring("\181\132\150\140\146\151\167\180\167")
    n2=6825499
   elseif n2==6825499 then
    n=9
    n2=251176
   elseif n2==251176 then
    create=buffer.create(n)
    n2=7671419
   end
  end
 end
end)()"#;
    let (out, stats) = dynamic_clean(&format!("local name={flattened}\n"));
    assert!(out.starts_with("local name=\"") && out.lines().count() == 1 && !out.contains("while"));
    assert_eq!(stats["strings"], 1);
    let source="local v=(function()\n print('hi')\n return bit32.bxor(1,2)\nend)()\nlocal w=(function()\n got=1\n return bit32.bxor(1,2)\nend)()\n";
    let (out, stats) = dynamic_clean(source);
    assert_eq!(out, source.replace("bit32.bxor(1,2)", "3"));
    assert_eq!(stats["numbers"], 2);
    assert_eq!(stats["values"], 0);
    assert_eq!(stats["strings"], 0);
    let source="local function f()\n if math.abs(os.clock())+1>0 then\n  print('real')\n else\n  print('junk')\n end\n local x=(function()\n  return nil\n end)()\n return x\nend\nf()\n";
    let (out, stats) = dynamic_clean(source);
    assert_eq!(
        out,
        "local function f()\n print('real')\n local x=nil\n return x\nend\nf()\n"
    );
    assert_eq!(
        stats,
        json!({"strings":0,"numbers":0,"values":1,"checks":1,"arms":1,"statements":0,"states":0})
    );
    assert!(!out.contains("junk"));
    assert_eq!(stats["checks"], 1);
    assert_eq!(stats["values"], 1);
    assert_eq!(run(&out), b"real\n");
    let ordinary="local applicationConstant=17\nlocal function unusedApplicationFunction() return applicationConstant end\nreturn 2\n";
    assert_eq!(dynamic_clean(ordinary).0, ordinary);
    assert_eq!(static_clean(ordinary).0, ordinary);
    let source =
        "if os.clock()>5 then\n late()\nend\nif math.floor(os.clock())%2==0 then\n even()\nend\n";
    let (out, stats) = dynamic_clean(source);
    assert_eq!(out, source);
    assert_eq!(stats["checks"], 0);
}

#[test]
fn tidy_and_plain_statement_all_reference_assertions() {
    for (source, want) in [
        ("a()\n\n\n\nb()\n", "a()\nb()\n"),
        ("if x then\n\n\ta()\n\nend\n", "if x then\n\ta()\nend\n"),
        (
            "local function f()\n\n\ta()\nend\n",
            "local function f()\n\ta()\nend\n",
        ),
        (
            "foo(function() return 1 end)\n\nbar()\n",
            "foo(function() return 1 end)\nbar()\n",
        ),
    ] {
        assert_eq!(clean::tidy(source), want);
    }
    for source in ["a()\n\nif x then\n\tb()\nend\n\nc()\n", "x=[[a\n\n\nb]]\n"] {
        assert_eq!(clean::tidy(source), source);
    }
    assert!(clean::plain_statement("\tfoo(1,\"a)\")"));
    for source in [
        "\tif x then",
        "\tfoo(function()",
        "\tlocal t={",
        "\treturn 1",
    ] {
        assert!(!clean::plain_statement(source));
    }
}

#[test]
fn naming_reference_outputs_keep_runtime_results_and_json_roles() {
    let source="local value=player.Character\nlocal flag=configuration.AutoDive or false\nlocal waitForChild2=storage:WaitForChild(('Core'),15)\nprint(value.Name,flag,waitForChild2.Name)\n";
    let (out, stats) = static_clean(source);
    assert_eq!(stats["names"], 3);
    let setup="local player={Character={Name='avatar'}}\nlocal configuration={}\nlocal storage={WaitForChild=function(_,name) return {Name=name} end}\n";
    for text in [source, &out] {
        assert_eq!(run(&format!("{setup}{text}")), b"avatar\tfalse\tCore\n");
    }
    let source="local frame=Instance.new('Frame')\nframe.Name='KeyPanel'\nlocal textButton=Instance.new('TextButton')\ntextButton.Text='Get Key'\nlocal function read()\n local frame='shadowed'\n return frame,textButton.Text\nend\nprint(frame.Name,read())\n";
    let (out, stats) = static_clean(source);
    assert_eq!(stats["names"], 2);
    assert!(out.contains("local frame='shadowed'"));
    let setup = "local Instance={new=function(class) return {ClassName=class} end}\n";
    for text in [source, &out] {
        assert_eq!(
            run(&format!("{setup}{text}")),
            b"KeyPanel\tshadowed\tGet Key\n"
        );
    }
    let (again, stats) = static_clean(&out);
    assert_eq!(again, out);
    assert!(no_stats(&stats));
    for role in ["settings", "config"] {
        let source=format!("local text='game_{role}.json'\nlocal items={{AutoDive=false}}\nlocal clone=table.clone(items)\nwritefile(text,HttpService:JSONEncode(clone))\nprint(items.AutoDive,clone.AutoDive)\n");
        let (out, stats) = static_clean(&source);
        assert_eq!(stats["names"], 3);
        let defaults = if role == "settings" {
            "defaultSettings"
        } else {
            "defaultConfig"
        };
        assert!(out.contains(&format!("local {defaults}={{AutoDive=false}}")));
        assert!(out.contains(&format!("local {role}=table.clone({defaults})")));
        assert!(out.contains(&format!(
            "writefile({role}File,HttpService:JSONEncode({role}))"
        )));
        let setup="local writefile=function(path,data) print(path,data) end\nlocal HttpService={JSONEncode=function(_,data) return tostring(data.AutoDive) end}\n";
        assert_eq!(
            run(&format!("{setup}{out}")),
            run(&format!("{setup}{source}"))
        );
    }
    let source="local Players='existing'\nlocal value=lookup('Players')\nlocal value2=lookup('Players')\nprint(Players,Players2,value.Name,value2.Name)\n";
    let (out, stats) = static_clean(source);
    assert_eq!(stats["names"], 2);
    assert!(out.contains("local Players3=") && out.contains("local Players4="));
    let setup = "local lookup=function() return {Name='player'} end\n";
    assert_eq!(
        run(&format!("{setup}{out}")),
        run(&format!("{setup}{source}"))
    );
}

#[test]
fn dynamic_protection_preserves_each_reference_guard_effect() {
    for verifier in [
        ENVIRONMENT.replace(
            "local data = buffer.create(4)",
            "debug.applicationCounter=1\nlocal data = buffer.create(4)",
        ),
        ENVIRONMENT.replace(
            "local data = buffer.create(4)",
            "pcall(debug.applicationEvent)\nlocal data = buffer.create(4)",
        ),
        ENVIRONMENT.replace(
            "buffer.writeu8(data, 0, 186)",
            "buffer.writeu8(buffer.applicationData,0,186)",
        ),
    ] {
        let source = format!("{SCAFFOLD}{verifier}");
        let (out, _) = dynamic_clean(&source);
        assert_eq!(out, source);
    }
    let source=format!("{SCAFFOLD}{ENVIRONMENT}{PRIMITIVE}{TIMING}local data=buffer.create(1)\nif buffer.readu8(data,0)~=0 then error('checksum failed',0) end\nprint('app')\n");
    let (out, _) = dynamic_clean(&source);
    for check in [
        "checksum failed",
        "verifyEnvironment()",
        "native(length)",
        "timing()",
    ] {
        assert!(out.contains(check), "{check}\n{out}");
    }
    let source = SCAFFOLD.replace("local t = {}", "print('application event')\nlocal t = {}");
    let (out, _) = dynamic_clean(&source);
    assert!(out.contains("print('application event')") && out.contains("install()"));
}

#[test]
fn mixed_static_passes_and_dynamic_string_counts_are_idempotent() {
    let source = CHAIN.replace(
        "total = total + n",
        &format!(
            "if {} then\n                total = total + n\n            end",
            CLOCKS[0].0
        ),
    );
    let (out, stats) = static_clean(&source);
    assert_eq!(stats["states"], 1);
    assert_eq!(stats["checks"], 1);
    assert_eq!(run(&out), run(&source));
    let (again, counts) = static_clean(&out);
    assert_eq!(again, out);
    assert!(no_stats(&counts));
    let (out, stats) = dynamic_clean(&format!(
        "if {} then\n print({INLINE})\nelseif {} then\n error('unreachable',0)\nend\n",
        CLOCKS[2].0, CLOCKS[6].0
    ));
    assert_eq!(out, "print(\"HI\")\n");
    assert_eq!(stats["strings"], 1);
    assert_eq!(stats["checks"], 1);
    assert_eq!(stats["arms"], 1);
    let (again, counts) = dynamic_clean(&out);
    assert_eq!(again, out);
    assert!(no_stats(&counts));
    let (out, stats) = dynamic_clean("local gift=\"🎁 Auto Gift\"\nprint(gift,bit32.bxor(5,3))\n");
    assert_eq!(out, "local gift=\"🎁 Auto Gift\"\nprint(gift,6)\n");
    assert_eq!(stats["numbers"], 1);
    let (out,stats)=dynamic_clean("local s=buffer.tostring(buffer.fromstring(\"\\240\\159\\142\\129 hi\"))\nprint(s,bit32.bxor(5,3))\n");
    assert_eq!(out, "local s=\"🎁 hi\"\nprint(s,6)\n");
    assert_eq!(stats["strings"], 1);
    assert_eq!(stats["numbers"], 1);
    assert_eq!(dynamic_clean("foo(bit32.bxor(3,4))\n").0, "foo(7)\n");
}

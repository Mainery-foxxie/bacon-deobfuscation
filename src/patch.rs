//! Insert dump hooks at AST byte spans without rewriting source or identifiers.
use crate::{
    astutil::{lkey, unlatin1, unparen, Src},
    error::{Error, Result},
    loader::{array, const_num, loc_of, ty},
};
use serde_json::Value;
pub const ROLE_ALIASES: [&str; 12] = [
    "MK",
    "D1",
    "D2",
    "D3",
    "D4",
    "D5",
    "D6",
    "P1",
    "P2",
    "XORKEY",
    "SIMPLESET",
    "MIX",
];
pub const TEMPLATE: &str = include_str!("lua/dumper.lua.tpl");
fn err(s: impl Into<String>) -> Error {
    Error::new("unsupported", s)
}
pub fn tail_replacement(loader: &Value) -> Result<(Value, String)> {
    let source = Src::new(
        loader["text"]
            .as_str()
            .ok_or_else(|| err("loader text missing"))?,
    )?;
    let last = array(&loader["root"]["body"])
        .last()
        .ok_or_else(|| err("loader tail not recognised"))?;
    if ty(last) != "AstStatReturn" || array(&last["list"]).len() != 1 {
        return Err(err("loader tail not recognised"));
    }
    let call = unparen(&last["list"][0]);
    if ty(call) != "AstExprCall" {
        return Err(err("loader tail not recognised"));
    }
    let outer = unparen(&call["func"]);
    if ty(outer) == "AstExprCall" && loc_of(&outer["func"]).is_some() {
        return Ok((last.clone(), format!("__DUMPER({})", source.slice(outer)?)));
    }
    if ty(outer) == "AstExprIndexExpr"
        && loc_of(&outer["expr"]).is_some()
        && const_num(&outer["index"]).is_some()
    {
        let local = loc_of(&outer["expr"]).unwrap();
        let decl = &loader["local_decl"][lkey(local)];
        if ty(&decl[0]) == "AstStatLocal" {
            return Ok((last.clone(), "__DUMPER()".into()));
        }
        return Err(err("loader runner not recognised"));
    }
    let local = loc_of(&call["func"]).ok_or_else(|| err("loader tail not recognised"))?;
    let key = lkey(local);
    let f = &loader["funcs"][key];
    let decl = &loader["local_decl"][key];
    if !f.is_null() {
        let st = array(&f["body"]["body"]);
        if st.len() == 1 && ty(&st[0]) == "AstStatReturn" && !array(&st[0]["list"]).is_empty() {
            let inner = unparen(&st[0]["list"][0]);
            if ty(inner) == "AstExprCall" && ty(unparen(&inner["func"])) == "AstExprCall" {
                return Ok((
                    last.clone(),
                    format!("__DUMPER({})", source.slice(unparen(&inner["func"]))?),
                ));
            }
        }
    } else if ty(&decl[0]) == "AstStatLocal" {
        return Ok((last.clone(), "__DUMPER()".into()));
    }
    Err(err("loader runner not recognised"))
}
pub fn build_dump_script(loader: &Value) -> Result<Vec<u8>> {
    let text = loader["text"]
        .as_str()
        .ok_or_else(|| err("loader text missing"))?;
    let source = Src::new(text)?;
    let mut edits = vec![];
    let local = &loader["role_local"]["PROF"];
    let prof = array(&loader["frame_body"])
        .iter()
        .rfind(|s| {
            ty(s) == "AstStatLocal" && array(&s["vars"]).iter().any(|v| lkey(v) == lkey(local))
        })
        .ok_or_else(|| err("profile statement not found"))?;
    let readers = if loader["layout"] == "numeric" {
        let reader = loader["role_local"]["READER"]["name"]
            .as_str()
            .ok_or_else(|| err("record reader name missing"))?;
        let kget = loader["role_local"]["KGET"]["name"]
            .as_str()
            .ok_or_else(|| err("constant reader name missing"))?;
        format!(" {reader},{kget}")
    } else {
        String::new()
    };
    edits.push((
        source.span(prof)?.0,
        format!("if __DUMP~=nil and (...)==__DUMP then return{readers} end "),
    ));
    for role in ROLE_ALIASES {
        let local = &loader["role_local"][role];
        let decl = &loader["local_decl"][lkey(local)];
        if decl.is_null() {
            return Err(err(format!("declaration of {role} not found")));
        }
        let name = local["name"]
            .as_str()
            .ok_or_else(|| err("local name missing"))?;
        edits.push((source.span(&decl[0])?.1, format!(" __RL.{role}={name};")));
    }
    for (role, alias) in [("PROTOMAP", "PROTO_MAP"), ("PROTOINIT", "PROTO_INIT")] {
        let local = &loader["role_local"][role];
        if local.is_null() {
            continue;
        }
        let decl = &loader["local_decl"][lkey(local)];
        let name = local["name"]
            .as_str()
            .ok_or_else(|| err(format!("{role} name missing")))?;
        edits.push((source.span(&decl[0])?.1, format!(" __RL.{alias}={name};")));
    }
    let protos = loader["role_local"]["PROTOS"]["name"]
        .as_str()
        .ok_or_else(|| err("PROTOS name missing"))?;
    let main = loader["role_local"]["MAINIDX"]["name"]
        .as_str()
        .ok_or_else(|| err("MAINIDX name missing"))?;
    edits.push((
        source.span(&loader["main_stmt"])?.1,
        format!(" __RL.PROTOS={protos};__RL.MAINIDX={main};"),
    ));
    let (last, new) = tail_replacement(loader)?;
    let (a, b) = source.span(&last)?;
    let mut out = source.bytes.clone();
    out.splice(a..b, unlatin1(&new)?);
    edits.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
    for (at, text) in edits {
        if at > a {
            return Err(err("edit after tail"));
        }
        out.splice(at..at, unlatin1(&text)?);
    }
    let perms = loader["perm"]
        .as_object()
        .ok_or_else(|| err("dispatch permutations missing"))?;
    let mut sorted = vec![];
    for (k, v) in perms {
        sorted.push((
            k.parse::<i64>().map_err(|_| err("bad permutation key"))?,
            array(v)
                .iter()
                .map(|v| {
                    v.as_i64()
                        .map(|n| n.to_string())
                        .ok_or_else(|| err("bad permutation index"))
                })
                .collect::<Result<Vec<_>>>()?
                .join(","),
        ));
    }
    sorted.sort_by_key(|(k, _)| *k);
    let perm = format!(
        "{{{}}}",
        sorted
            .into_iter()
            .map(|(k, v)| format!("[{k}]={{{v}}}"))
            .collect::<Vec<_>>()
            .join(",")
    );
    let sem = loader["sem_index"]
        .as_i64()
        .ok_or_else(|| err("semantic index missing"))?;
    let mut head = TEMPLATE
        .replace("%PERM%", &perm)
        .replace("%SEMIDX%", &sem.to_string());
    head.push('\n');
    if loader["layout"] == "numeric" {
        let mut fields = vec![];
        for key in [
            "num_params",
            "is_vararg",
            "num_upvalues",
            "max_stack",
            "entry",
            "dispatch_format",
            "vm_family",
            "reg_shift",
            "record_data",
            "constant_data",
            "children",
            "aot_handler",
            "record_reader",
            "constant_reader",
        ] {
            let n = loader["numeric_fields"][key]
                .as_i64()
                .ok_or_else(|| err(format!("numeric dump fields missing: {key}")))?;
            fields.push(format!("{key}={n}"));
        }
        let mut masks = vec![];
        if let Some(formats) = loader["record_masks"].as_object() {
            for (format, values) in formats {
                let format = format
                    .parse::<u8>()
                    .ok()
                    .filter(|v| *v < 4)
                    .ok_or_else(|| err("invalid record mask format"))?;
                let mut items = vec![];
                for (index, field) in values
                    .as_object()
                    .ok_or_else(|| err("invalid record masks"))?
                {
                    let index = index
                        .parse::<u8>()
                        .ok()
                        .filter(|v| (1..=10).contains(v))
                        .ok_or_else(|| err("invalid record mask index"))?;
                    let field = field
                        .as_i64()
                        .ok_or_else(|| err("invalid record mask field"))?;
                    items.push(format!("[{index}]={field}"));
                }
                masks.push(format!("[{format}]={{{}}}", items.join(",")));
            }
        }
        fields.push(format!("masks={{{}}}", masks.join(",")));
        head.push_str(&format!("__RL.NUMERIC={{{}}}\n", fields.join(",")));
    }
    let mut result = unlatin1(&head)?;
    result.extend(out);
    Ok(result)
}

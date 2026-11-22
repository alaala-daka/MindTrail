use serde_json::{json,Map,Value};

pub struct NormalizedOperation { pub session:String, pub operation_id:String, pub kind:&'static str, pub payload:Value }
struct Definition { name:&'static str, kind:&'static str, required:&'static [&'static str], optional:&'static [&'static str] }
const DEFINITIONS:[Definition;7]=[
    Definition{name:"trace_task_start",kind:"task.start",required:&["title","goal","project"],optional:&[]},
    Definition{name:"trace_intent",kind:"capability.intent",required:&["id","title"],optional:&["summary"]},
    Definition{name:"trace_done",kind:"capability.done",required:&["id"],optional:&["summary"]},
    Definition{name:"trace_api",kind:"api.expose",required:&["id","signature","kind"],optional:&["capability","summary"]},
    Definition{name:"trace_file",kind:"file.change",required:&["id","path","action"],optional:&["capability","purpose","add","del"]},
    Definition{name:"trace_call",kind:"call.edge",required:&["from","to"],optional:&["kind"]},
    Definition{name:"trace_task_done",kind:"task.done",required:&[],optional:&["summary"]},
];
fn field_schema(tool:&str, field:&str)->Value {
    match (tool,field) {
        ("trace_api","kind")=>json!({"type":"string","enum":["http","cli","fn"]}),
        ("trace_file","action")=>json!({"type":"string","enum":["create","modify","delete"]}),
        ("trace_call","kind")=>json!({"type":"string","enum":["calls","imports"]}),
        ("trace_file","add"|"del")=>json!({"type":"integer","minimum":0,"maximum":crate::protocol::MAX_SAFE_INTEGER}),
        _=>json!({"type":"string"}),
    }
}
pub fn schemas()->Vec<(&'static str,Value)> {
    DEFINITIONS.iter().map(|d| {
        let mut properties=Map::new();
        for field in ["session","operation_id","quote","context"].into_iter().chain(d.required.iter().copied()).chain(d.optional.iter().copied()) {properties.insert(field.to_owned(),field_schema(d.name,field));}
        let mut required=vec!["session","operation_id"]; required.extend_from_slice(d.required);
        (d.name,json!({"type":"object","properties":properties,"required":required,"additionalProperties":false}))
    }).collect()
}
pub fn normalize_tool(name:&str,arguments:&Value)->Result<NormalizedOperation,&'static str> {
    let d=DEFINITIONS.iter().find(|d|d.name==name).ok_or("unknown_tool")?;
    let object=arguments.as_object().ok_or("invalid_arguments")?;
    for (key,value) in object {
        if !["session","operation_id","quote","context"].contains(&key.as_str()) && !d.required.contains(&key.as_str()) && !d.optional.contains(&key.as_str()) {return Err("invalid_arguments")}
        if matches!((name,key.as_str()),("trace_file","add"|"del")) {
            if !value.as_u64().is_some_and(|n|n<=crate::protocol::MAX_SAFE_INTEGER) {return Err("invalid_arguments")}
        } else if !value.is_string() {return Err("invalid_arguments")}
        let s=value.as_str().unwrap_or("");
        if matches!((name,key.as_str()),("trace_api","kind")) && !["http","cli","fn"].contains(&s) || matches!((name,key.as_str()),("trace_file","action")) && !["create","modify","delete"].contains(&s) || matches!((name,key.as_str()),("trace_call","kind")) && !["calls","imports"].contains(&s) {return Err("invalid_arguments")}
    }
    for field in ["session","operation_id"].into_iter().chain(d.required.iter().copied()) {if !object.get(field).and_then(Value::as_str).is_some_and(|s|!s.trim().is_empty()) {return Err("invalid_arguments")}}
    let mut payload=Map::new();
    for (key,value) in object {
        match key.as_str() {"session"|"operation_id"|"add"|"del"=>{},"capability"=>{payload.insert("capabilityId".into(),value.clone());},_=>{payload.insert(key.clone(),value.clone());}}
    }
    if name=="trace_file" {payload.insert("stats".into(),json!({"add":object.get("add").cloned().unwrap_or(json!(0)),"del":object.get("del").cloned().unwrap_or(json!(0))}));}
    if name=="trace_call" && !payload.contains_key("kind") {payload.insert("kind".into(),json!("calls"));}
    Ok(NormalizedOperation{session:object["session"].as_str().unwrap().into(),operation_id:object["operation_id"].as_str().unwrap().into(),kind:d.kind,payload:Value::Object(payload)})
}

#[cfg(test)] mod tests {
    use super::*;
    #[test] fn all_schemas_and_normalization_cover_seven_tools() {
        let cases=[("trace_task_start",json!({"title":"T","goal":"G","project":"P"}),"task.start"),("trace_intent",json!({"id":"i","title":"T"}),"capability.intent"),("trace_done",json!({"id":"i"}),"capability.done"),("trace_api",json!({"id":"a","signature":" f() ","kind":"fn","capability":"i"}),"api.expose"),("trace_file",json!({"id":"f","path":"../资料","action":"modify"}),"file.change"),("trace_call",json!({"from":"a","to":"f"}),"call.edge"),("trace_task_done",json!({}),"task.done")];
        assert_eq!(schemas().len(),7);
        for (name,mut args,kind) in cases {args["session"]=json!(" s ");args["operation_id"]=json!("op");let value=normalize_tool(name,&args).unwrap();assert_eq!(value.kind,kind);assert_eq!(value.session," s ");assert_eq!(value.operation_id,"op");assert_eq!(schemas().iter().find(|(n,_)|*n==name).unwrap().1["additionalProperties"],false);}
        let mut file=json!({"session":"s","operation_id":"o","id":"f","path":"x","action":"create"});let first=normalize_tool("trace_file",&file).unwrap();file["add"]=json!(0);file["del"]=json!(0);assert_eq!(first.payload,normalize_tool("trace_file",&file).unwrap().payload);file["quote"]=json!("");assert_ne!(first.payload,normalize_tool("trace_file",&file).unwrap().payload);
    }
    #[test] fn rejects_bad_shape_values_and_unknown_properties() {
        let base=json!({"session":"s","operation_id":"o","id":"f","path":"x","action":"create"});
        for (k,v) in [("session",json!(" ")),("id",Value::Null),("action",json!("other")),("add",json!(-1)),("add",json!(9007199254740992u64)),("extra",json!("x"))] {let mut bad=base.clone();bad[k]=v;assert!(normalize_tool("trace_file",&bad).is_err());}
        assert!(normalize_tool("unknown",&base).is_err());
    }
    #[test] fn schemas_match_runtime_required_types_and_enums() {
        for definition in &DEFINITIONS {
            let mut args=json!({"session":"s","operation_id":"op"});
            for field in definition.required {args[*field]=match (definition.name,*field) {("trace_api","kind")=>json!("fn"),("trace_file","action")=>json!("create"),_=>json!("value")};}
            let schema=schemas().into_iter().find(|(name,_)|*name==definition.name).unwrap().1;
            assert!(normalize_tool(definition.name,&args).is_ok());
            for field in ["session","operation_id"].into_iter().chain(definition.required.iter().copied()) {
                assert!(schema["required"].as_array().unwrap().iter().any(|v|v==field));
                let mut missing=args.clone();missing.as_object_mut().unwrap().remove(field);assert!(normalize_tool(definition.name,&missing).is_err());
                let mut blank=args.clone();blank[field]=json!(" \t");assert!(normalize_tool(definition.name,&blank).is_err());
            }
            for field in ["quote","context"].into_iter().chain(definition.optional.iter().copied()) {
                assert!(schema["properties"].get(field).is_some());let mut null=args.clone();null[field]=Value::Null;assert!(normalize_tool(definition.name,&null).is_err());
            }
        }
        for (tool,key,base,valid,invalid) in [
            ("trace_api","kind",json!({"session":"s","operation_id":"o","id":"a","signature":"x"}),vec!["http","cli","fn"],"calls"),
            ("trace_file","action",json!({"session":"s","operation_id":"o","id":"f","path":"p"}),vec!["create","modify","delete"],"read"),
            ("trace_call","kind",json!({"session":"s","operation_id":"o","from":"a","to":"b"}),vec!["calls","imports"],"fn")
        ] {let schema=schemas().into_iter().find(|(name,_)|*name==tool).unwrap().1;for item in valid {let mut args=base.clone();args[key]=json!(item);assert!(normalize_tool(tool,&args).is_ok());assert!(schema["properties"][key]["enum"].as_array().unwrap().iter().any(|v|v==item));}let mut args=base;args[key]=json!(invalid);assert!(normalize_tool(tool,&args).is_err());}
    }
}

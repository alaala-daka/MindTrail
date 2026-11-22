use serde_json::Value;
use std::collections::HashSet;
use super::{is_valid_timestamp, raw};
pub const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtocolError { pub code: &'static str, pub message: String, pub index: Option<usize>, pub last_seq: Option<u64>, pub expected_seq: Option<u64>, pub offending_seq: Option<u64>, pub(crate) status: u16 }
impl ProtocolError { pub fn status(&self)->u16 { self.status } }
fn error(message: impl Into<String>, index: Option<usize>, status:u16)->ProtocolError { ProtocolError { code: if status==413 {"body_too_large"} else {"invalid_request"}, message:message.into(), index, last_seq:None, expected_seq:None, offending_seq:None, status } }
#[derive(Debug, Clone, PartialEq)]
pub struct ExplicitEvent { value: Value, seq:u64 }
#[derive(Debug, Clone, PartialEq)]
pub struct AppendEvent { value: Value, operation_id:String }
#[derive(Debug, Clone, PartialEq)]
pub struct ExplicitBatch { items: Vec<ExplicitEvent> }
#[derive(Debug, Clone, PartialEq)]
pub struct AppendBatch { items: Vec<AppendEvent> }
impl ExplicitBatch { pub fn items(&self)->&[ExplicitEvent] { &self.items } }
impl AppendBatch { pub fn items(&self)->&[AppendEvent] { &self.items } }
macro_rules! accessors { ($t:ty) => { impl $t { pub fn session(&self)->&str { self.value["session"].as_str().unwrap() } pub fn ts(&self)->&str { self.value["ts"].as_str().unwrap() } pub fn kind(&self)->&str { self.value["type"].as_str().unwrap() } pub fn payload(&self)->&Value { &self.value["payload"] } pub fn value(&self)->&Value { &self.value } } }; }
accessors!(ExplicitEvent); accessors!(AppendEvent);
impl ExplicitEvent { pub fn seq(&self)->u64 { self.seq } }
impl AppendEvent { pub fn operation_id(&self)->&str { &self.operation_id } pub fn with_seq(&self, seq:u64)->Option<Value> { if seq==0 || seq>MAX_SAFE_INTEGER {return None} let mut v=self.value.clone(); let m=v.as_object_mut().unwrap(); m.remove("operation_id"); m.insert("seq".into(),seq.into()); Some(v) } }
fn integer(v: &Value) -> Option<u64> {
    let text = v.as_number()?.to_string();
    let (mantissa, exponent_text) = match text.find(['e', 'E']) {
        Some(i) => (&text[..i], &text[i + 1..]),
        None => (text.as_str(), "0"),
    };
    let unsigned = mantissa.strip_prefix('-').unwrap_or(mantissa);
    let fraction = unsigned.split_once('.').map_or(0, |(_, f)| f.len());
    let digits: String = unsigned.chars().filter(|c| *c != '.').collect();
    let digits = digits.trim_start_matches('0');
    if digits.is_empty() { return Some(0); }
    if mantissa.starts_with('-') { return None; }
    let exponent = exponent_text.parse::<i32>().ok()?;
    let scale = i64::from(exponent) - fraction as i64;
    let whole = if scale < 0 {
        let remove = usize::try_from(-scale).ok()?;
        if remove > digits.len() || !digits[digits.len()-remove..].bytes().all(|c| c == b'0') { return None; }
        digits[..digits.len()-remove].to_owned()
    } else {
        let append = usize::try_from(scale).ok()?;
        if digits.len().checked_add(append)? > 16 { return None; }
        format!("{}{}", digits, "0".repeat(append))
    };
    let n = if whole.is_empty() { 0 } else { whole.parse::<u64>().ok()? };
    (n <= MAX_SAFE_INTEGER).then_some(n)
}
fn validate(mut v:Value, append:bool, i:usize)->Result<Value,ProtocolError> {
 let bad=||error("invalid event",Some(i),400);
 let m=v.as_object_mut().ok_or_else(bad)?;
 let key=if append {"operation_id"} else {"seq"};
 if m.len()!=6 || !["v","session",key,"ts","type","payload"].iter().all(|k|m.contains_key(*k)) {return Err(bad())}
 if integer(&m["v"])!=Some(1) {return Err(bad())} m.insert("v".into(),1.into());
 for k in ["session",if append {"operation_id"} else {"session"}] { if !m[k].as_str().is_some_and(|s|!s.trim().is_empty()) {return Err(bad())} }
 if !append { let n=integer(&m["seq"]).filter(|n|*n>0).ok_or_else(bad)?; m.insert("seq".into(),n.into()); }
 if !m["ts"].as_str().is_some_and(is_valid_timestamp) {return Err(bad())}
 let kind=m["type"].as_str().ok_or_else(bad)?.to_owned();
 let (required,optional):(&[&str],&[&str])=match kind.as_str() {"task.start"=>(&["title","goal","project"],&[]),"capability.intent"=>(&["id","title"],&["summary"]),"capability.done"=>(&["id"],&["summary"]),"api.expose"=>(&["id","kind","signature"],&["capabilityId","summary"]),"file.change"=>(&["id","path","action","stats"],&["capabilityId","purpose"]),"call.edge"=>(&["from","to","kind"],&[]),"task.done"=>(&[],&["summary"]),_=>return Err(bad())};
 let p=m.get_mut("payload").and_then(Value::as_object_mut).ok_or_else(bad)?;
 if !required.iter().all(|k|p.contains_key(*k)) || p.keys().any(|k| !required.contains(&k.as_str()) && !optional.contains(&k.as_str()) && k!="quote" && k!="context") {return Err(bad())}
 for (k,val) in p.iter_mut() { if k=="stats" { let stats=val.as_object_mut().ok_or_else(bad)?; if stats.len()!=2 || !stats.contains_key("add") || !stats.contains_key("del") {return Err(bad())} for n in stats.values_mut() { *n=integer(n).ok_or_else(bad)?.into(); } } else { let s=val.as_str().ok_or_else(bad)?; if required.contains(&k.as_str()) && s.trim().is_empty() {return Err(bad())} if k=="capabilityId" && s.trim().is_empty() {return Err(bad())} } }
 let enum_ok=match kind.as_str() { "api.expose"=>["http","cli","fn"].contains(&p["kind"].as_str().unwrap()), "file.change"=>["create","modify","delete"].contains(&p["action"].as_str().unwrap()), "call.edge"=>["calls","imports"].contains(&p["kind"].as_str().unwrap()), _=>true }; if !enum_ok {return Err(bad())} Ok(v)
}
fn parse(bytes:&[u8],append:bool)->Result<Vec<Value>,ProtocolError> { let raw=raw::parse_batch(bytes).map_err(|e|error(e.message,e.index,e.status))?; let mut result=Vec::new(); let mut session=String::new(); let mut seen=HashSet::new(); let mut previous=0; for (i,v) in raw.into_iter().enumerate() { let v=validate(v,append,i)?; let s=v["session"].as_str().unwrap(); if i==0 {session=s.to_owned()} else if s!=session {return Err(error("mixed sessions",Some(i),400))} if append {if !seen.insert(v["operation_id"].as_str().unwrap().to_owned()) {return Err(error("duplicate operation_id",Some(i),400))}} else {let n=v["seq"].as_u64().unwrap(); if n<=previous {return Err(error("sequence must increase",Some(i),400))} previous=n;} result.push(v); } Ok(result) }
pub fn decode_explicit(bytes:&[u8])->Result<ExplicitBatch,ProtocolError> { Ok(ExplicitBatch { items:parse(bytes,false)?.into_iter().map(|value|ExplicitEvent {seq:value["seq"].as_u64().unwrap(),value}).collect() }) }
pub fn decode_append(bytes:&[u8])->Result<AppendBatch,ProtocolError> { Ok(AppendBatch { items:parse(bytes,true)?.into_iter().map(|value|AppendEvent {operation_id:value["operation_id"].as_str().unwrap().to_owned(),value}).collect() }) }



/// Validate one persisted event without applying the HTTP request body limit.
pub fn decode_stored_event(bytes: &[u8]) -> Result<ExplicitEvent, ProtocolError> {
    let value=raw::parse_stored_event(bytes).map_err(|e| error(e.message,None,400))?;
    let value=validate(value,false,0)?;
    Ok(ExplicitEvent { seq:value["seq"].as_u64().unwrap(), value })
}
pub fn decode_stored_operation(bytes: &[u8]) -> Result<AppendEvent, ProtocolError> {
    let value=raw::parse_stored_event(bytes).map_err(|e| error(e.message,None,400))?;
    let value=validate(value,true,0)?;
    Ok(AppendEvent { operation_id:value["operation_id"].as_str().unwrap().to_owned(), value })
}

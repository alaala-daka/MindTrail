use super::*;
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteMode { Explicit, Append }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStatus { Running, Done }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind { Capability, Api, File }
#[derive(Debug, Clone, PartialEq)]
pub struct SessionMetadata { pub session:String, pub title:String, pub project:String, pub status:SessionStatus, pub last_seq:u64, pub mode:WriteMode }
/// Transaction-local view for the batch session. Lookups return complete decoded and normalized values. The caller must commit the successful plan atomically within the same transaction; discard it on any error.
pub trait SessionSnapshot {
 fn metadata(&self)->&SessionMetadata;
 fn event(&self, seq:u64)->Result<Option<Value>, ProtocolError>;
 fn operation(&self, id:&str)->Result<Option<(Value,u64)>, ProtocolError>;
 fn node_kind(&self, id:&str)->Result<Option<NodeKind>, ProtocolError>;
}
#[derive(Debug, Clone, PartialEq)]
pub struct ExplicitReceipt { pub accepted:usize, pub duplicates:usize, pub last_seq:u64 }
#[derive(Debug, Clone, PartialEq)]
pub struct AppendResult { pub operation_id:String, pub seq:u64 }
#[derive(Debug, Clone, PartialEq)]
pub struct AppendReceipt { pub accepted:usize, pub duplicates:usize, pub last_seq:u64, pub results:Vec<AppendResult> }
#[derive(Debug, Clone, PartialEq)]
pub struct OperationMapping { pub operation_id:String, pub input:Value, pub seq:u64 }
#[derive(Debug, Clone, PartialEq)]
pub struct WritePlan<R> { pub receipt:R, pub events:Vec<Value>, pub operations:Vec<OperationMapping>, pub node_updates:HashMap<String,NodeKind>, pub metadata:SessionMetadata }
fn fail(code:&'static str,i:usize,last:u64)->ProtocolError { ProtocolError { code, message:code.into(), index:Some(i), last_seq:Some(last), expected_seq:None, offending_seq:None, status:409 } }
pub fn temporary_storage_error(message:impl Into<String>)->ProtocolError { ProtocolError {code:"temporary_storage",message:message.into(),index:None,last_seq:None,expected_seq:None,offending_seq:None,status:503} }
fn node(s:Option<&dyn SessionSnapshot>, nodes:&HashMap<String,NodeKind>, id:&str, i:usize)->Result<Option<NodeKind>,ProtocolError> { if let Some(k)=nodes.get(id) {Ok(Some(*k))} else if let Some(s)=s {s.node_kind(id).map_err(|mut e| { e.index=Some(i); e })} else {Ok(None)} }
fn apply(v:&Value, meta:&mut Option<SessionMetadata>, mode:WriteMode, nodes:&mut HashMap<String,NodeKind>, s:Option<&dyn SessionSnapshot>, i:usize,last:u64)->Result<(),ProtocolError> {
 let k=v["type"].as_str().unwrap(); let p=&v["payload"];
 if meta.is_none() && k!="task.start" {return Err(fail("task_not_started",i,last))}
 if let Some(m)=meta.as_ref() {if m.status==SessionStatus::Done {return Err(fail("task_closed",i,last))} if k=="task.start" {return Err(fail("task_already_started",i,last))}}
 if k=="task.start" {*meta=Some(SessionMetadata {session:v["session"].as_str().unwrap().into(),title:p["title"].as_str().unwrap().into(),project:p["project"].as_str().unwrap().into(),status:SessionStatus::Running,last_seq:0,mode});}
 if k=="task.done" {meta.as_mut().unwrap().status=SessionStatus::Done;}
 let nk=match k {"capability.intent"=>Some(NodeKind::Capability),"api.expose"=>Some(NodeKind::Api),"file.change"=>Some(NodeKind::File),_=>None};
 if let Some(nk)=nk {let id=p["id"].as_str().unwrap(); if id=="root" || id=="unassigned" || node(s,nodes,id,i)?.is_some_and(|old|old!=nk) {return Err(fail("node_id_conflict",i,last))} nodes.insert(id.into(),nk);}
 if k=="capability.done" && node(s,nodes,p["id"].as_str().unwrap(),i)?!=Some(NodeKind::Capability) {return Err(fail("invalid_reference",i,last))}
 if k=="call.edge" {for key in ["from","to"] {if !matches!(node(s,nodes,p[key].as_str().unwrap(),i)?,Some(NodeKind::Api|NodeKind::File)) {return Err(fail("invalid_reference",i,last))}}}
 Ok(())
}
pub fn plan_explicit(batch:&ExplicitBatch,s:Option<&dyn SessionSnapshot>)->Result<WritePlan<ExplicitReceipt>,ProtocolError> {
 if s.is_some_and(|s|s.metadata().session!=batch.items()[0].session()) {return Err(ProtocolError {code:"storage_invariant",message:"snapshot session mismatch".into(),index:None,last_seq:None,expected_seq:None,offending_seq:None,status:500})} let last=s.map_or(0,|s|s.metadata().last_seq); if s.is_some_and(|s|s.metadata().mode!=WriteMode::Explicit) {return Err(fail("write_mode_conflict",0,last))} let mut meta=s.map(|s|s.metadata().clone()); let mut cursor=last; let mut events=Vec::new(); let mut nodes=HashMap::new(); let mut dup=0;
 for (i,e) in batch.items().iter().enumerate() {if let Some(old)=if let Some(s)=s {s.event(e.seq()).map_err(|mut err| { err.index=Some(i); err })?} else {None} {if old!=*e.value() {return Err(fail("seq_conflict",i,last))} dup+=1; continue} if e.seq()!=cursor+1 {let mut err=fail("seq_gap",i,last); err.expected_seq=Some(cursor+1); err.offending_seq=Some(e.seq()); return Err(err)} apply(e.value(),&mut meta,WriteMode::Explicit,&mut nodes,s,i,last)?; cursor=e.seq(); meta.as_mut().unwrap().last_seq=cursor; events.push(e.value().clone()); }
 Ok(WritePlan {receipt:ExplicitReceipt {accepted:events.len(),duplicates:dup,last_seq:cursor},events,operations:vec![],node_updates:nodes,metadata:meta.unwrap()})
}
pub fn plan_append(batch:&AppendBatch,s:Option<&dyn SessionSnapshot>)->Result<WritePlan<AppendReceipt>,ProtocolError> {
 if s.is_some_and(|s|s.metadata().session!=batch.items()[0].session()) {return Err(ProtocolError {code:"storage_invariant",message:"snapshot session mismatch".into(),index:None,last_seq:None,expected_seq:None,offending_seq:None,status:500})} let last=s.map_or(0,|s|s.metadata().last_seq); if s.is_some_and(|s|s.metadata().mode!=WriteMode::Append) {return Err(fail("write_mode_conflict",0,last))} let mut meta=s.map(|s|s.metadata().clone()); let mut cursor=last; let mut events=Vec::new(); let mut nodes=HashMap::new(); let mut operations=Vec::new(); let mut results=Vec::new(); let mut dup=0;
 for (i,e) in batch.items().iter().enumerate() {if let Some((old,seq))=if let Some(s)=s {s.operation(e.operation_id()).map_err(|mut err| { err.index=Some(i); err })?} else {None} {if old!=*e.value() {return Err(fail("operation_conflict",i,last))} dup+=1; results.push(AppendResult {operation_id:e.operation_id().into(),seq}); continue} if meta.as_ref().is_some_and(|m|m.status==SessionStatus::Done) {return Err(fail("task_closed",i,last))} if cursor>=MAX_SAFE_INTEGER {return Err(fail("seq_exhausted",i,last))} let seq=cursor+1; let v=e.with_seq(seq).unwrap(); apply(&v,&mut meta,WriteMode::Append,&mut nodes,s,i,last)?; cursor=seq; meta.as_mut().unwrap().last_seq=cursor; events.push(v); operations.push(OperationMapping {operation_id:e.operation_id().into(),input:e.value().clone(),seq}); results.push(AppendResult {operation_id:e.operation_id().into(),seq}); }
 Ok(WritePlan {receipt:AppendReceipt {accepted:events.len(),duplicates:dup,last_seq:cursor,results},events,operations,node_updates:nodes,metadata:meta.unwrap()})
}


pub fn not_found_error() -> ProtocolError { ProtocolError { code:"not_found", message:"session not found".into(), index:None, last_seq:None, expected_seq:None, offending_seq:None, status:404 } }
pub fn invalid_query_error() -> ProtocolError { ProtocolError { code:"invalid_request", message:"invalid page limit".into(), index:None, last_seq:None, expected_seq:None, offending_seq:None, status:400 } }

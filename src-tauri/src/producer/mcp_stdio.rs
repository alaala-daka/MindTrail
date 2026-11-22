use std::{collections::HashMap,io::{self,Write},sync::{Arc,Mutex,mpsc},time::Instant};
use rmcp::{RoleServer,model::{ClientJsonRpcMessage,ClientNotification,ServerJsonRpcMessage},transport::Transport};
use super::{framing::{read_frame,FrameError},limits::{Limits,Permit}};

enum OutputMessage {Frame(Vec<u8>),Response{key:String,frame:Vec<u8>},Drain(mpsc::Sender<()>)}
type Output=mpsc::SyncSender<OutputMessage>;
pub struct DrainHandle(Output);
impl DrainHandle {
    pub fn drain(&self,budget:std::time::Duration)->Result<(), &'static str> {
        let deadline=Instant::now()+budget;let (done,ack)=mpsc::channel();let mut message=OutputMessage::Drain(done);
        loop {match self.0.try_send(message) {Ok(())=>break,Err(mpsc::TrySendError::Full(value))=>message=value,Err(mpsc::TrySendError::Disconnected(_))=>return Err("output_unavailable")};if Instant::now()>=deadline{return Err("output_unavailable")}std::thread::sleep(std::time::Duration::from_millis(5));}
        ack.recv_timeout(deadline.saturating_duration_since(Instant::now())).map_err(|_|"output_unavailable")
    }
}
fn frame(value:&serde_json::Value)->io::Result<Vec<u8>> {let mut frame=serde_json::to_vec(value).map_err(io::Error::other)?;frame.push(b'\n');Ok(frame)}
fn emit(output:&Output,value:&serde_json::Value)->io::Result<()> {
    output.try_send(OutputMessage::Frame(frame(value)?)).map_err(|_|io::Error::other("output unavailable"))
}
pub struct ActiveRequest {pub _permit:Permit,pub external:serde_json::Value,pub completed:bool,pub cancelled:bool,pub writing:bool}
#[derive(Default)]
pub struct Ledger {pub active:HashMap<String,ActiveRequest>,pub current:HashMap<String,String>,next:u64}
pub type Active=Arc<Mutex<Ledger>>;
pub fn finish_cancelled(ledger:&mut Ledger,key:&str) {
    if let Some(request)=ledger.active.remove(key) {let external=request.external.to_string();if ledger.current.get(&external).is_some_and(|v|v==key) {ledger.current.remove(&external);}}
}
pub struct StrictTransport {input:tokio::sync::mpsc::Receiver<serde_json::Value>,output:Output,limits:Limits,prefix:String,pub active:Active}
impl StrictTransport {
    pub fn drain_handle(&self)->DrainHandle {DrainHandle(self.output.clone())}
    pub fn start()->Result<Self,&'static str> {
        let mut entropy=[0u8;16];getrandom::fill(&mut entropy).map_err(|_|"mcp_unavailable")?;let prefix=format!("mindtrail-{}-",entropy.iter().map(|v|format!("{v:02x}")).collect::<String>());
        let active=Arc::new(Mutex::new(Ledger::default()));let writer_active=active.clone();let (out,frames)=mpsc::sync_channel::<OutputMessage>(64);
        std::thread::spawn(move || {let stdout=io::stdout();let mut writer=stdout.lock();#[cfg(feature="producer-test-fixture")] let mut gate=std::env::var_os("MINDTRAIL_TEST_OUTPUT_GATE").map(std::path::PathBuf::from);while let Ok(message)=frames.recv(){let (frame,key)=match message {OutputMessage::Drain(ack)=>{let _=ack.send(());continue},OutputMessage::Frame(frame)=>(frame,None),OutputMessage::Response{key,frame}=>{
            #[cfg(feature="producer-test-fixture")] if gate.as_ref().is_some_and(|_|serde_json::from_slice::<serde_json::Value>(&frame).ok().is_some_and(|v|v["id"]==100)) {let path=gate.take().unwrap();let _=std::fs::write(path.join("ready"),b"ready");let deadline=Instant::now()+std::time::Duration::from_secs(10);while !path.join("release").exists() && Instant::now()<deadline {std::thread::sleep(std::time::Duration::from_millis(5));}}
            let mut ledger=writer_active.lock().unwrap_or_else(|e|e.into_inner());if let Some(request)=ledger.active.get_mut(&key) {request.writing=true;(frame,Some(key))} else {continue}}};let result=writer.write_all(&frame).and_then(|_|writer.flush());if let Some(key)=key {finish_cancelled(&mut writer_active.lock().unwrap_or_else(|e|e.into_inner()),&key);}if result.is_err(){break}}});
        let (send,input)=tokio::sync::mpsc::channel(16);let writer=out.clone();
        std::thread::spawn(move || {let stdin=io::stdin();let mut reader=io::BufReader::new(stdin.lock());loop {match read_frame(&mut reader) {
            Ok(Some(value))=>if send.blocking_send(value).is_err(){break},Ok(None)=>break,
            Err(FrameError::Invalid)=>if emit(&writer,&serde_json::json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"Parse error"}})).is_err(){break},
            Err(FrameError::TooLarge)=>{let _=emit(&writer,&serde_json::json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"Frame too large"}}));break},Err(FrameError::Io)=>break,
        }}});
        Ok(Self{input,output:out,limits:Limits::new(Instant::now()),prefix,active})
    }
    fn reject(&mut self,value:&serde_json::Value)->bool {if emit(&self.output,value).is_err(){self.input.close();false}else{true}}
}
fn request_id(value:&serde_json::Value)->Option<serde_json::Value> {value.get("id").filter(|id|id.is_string()||id.as_i64().is_some()||id.as_u64().is_some()).cloned()}
impl Transport<RoleServer> for StrictTransport {
    type Error=io::Error;
    fn send(&mut self,item:ServerJsonRpcMessage)->impl std::future::Future<Output=Result<(),Self::Error>> + Send + 'static {
        let mut value=serde_json::to_value(item).map_err(io::Error::other);let mut internal=None;
        if let Ok(value)=&mut value {if let Some(id)=request_id(value) {
            if let Some(key)=id.as_str().filter(|s|s.starts_with(&self.prefix)) {
                let mut ledger=self.active.lock().unwrap_or_else(|e|e.into_inner());
                if let Some(request)=ledger.active.get(key) {value["id"]=request.external.clone();internal=Some(key.to_owned());} else {return std::future::ready(Ok(()));}
            }
        }}
        let result=value.and_then(|v|if let Some(key)=internal {self.output.try_send(OutputMessage::Response{key,frame:frame(&v)?}).map_err(|_|io::Error::other("output unavailable"))} else {emit(&self.output,&v)});if result.is_err(){self.input.close()};
        std::future::ready(result)
    }
    async fn receive(&mut self)->Option<ClientJsonRpcMessage> {
        while let Some(mut value)=self.input.recv().await {
            let id=request_id(&value);
            if let Some(object)=value.as_object() {
                let bad_envelope=object.get("method").is_some() && (object.get("jsonrpc").and_then(|v|v.as_str())!=Some("2.0") || object.keys().any(|k|!matches!(k.as_str(),"jsonrpc"|"id"|"method"|"params")));
                let bad_call=object.get("method").and_then(|v|v.as_str())==Some("tools/call") && object.get("params").and_then(|v|v.as_object()).is_none_or(|p|p.keys().any(|k|!matches!(k.as_str(),"name"|"arguments"|"_meta")));
                let bad_cancel=object.get("method").and_then(|v|v.as_str())==Some("notifications/cancelled") && (object.contains_key("id") || object.get("params").and_then(|v|v.as_object()).is_none_or(|p|p.keys().any(|k|!matches!(k.as_str(),"requestId"|"reason"|"_meta")) || p.get("requestId").is_none_or(|v|!v.is_string() && v.as_i64().is_none() && v.as_u64().is_none()) || p.get("reason").is_some_and(|v|!v.is_string()) || p.get("_meta").is_some_and(|v|!v.is_object())));
                if bad_envelope || bad_call || bad_cancel {if object.contains_key("id") && !self.reject(&serde_json::json!({"jsonrpc":"2.0","id":id,"error":{"code":-32600,"message":"Invalid request"}})){return None}continue}
            }
            let message:ClientJsonRpcMessage=match serde_json::from_value(value.clone()) {Ok(v)=>v,Err(_)=>{if !self.reject(&serde_json::json!({"jsonrpc":"2.0","id":id,"error":{"code":-32600,"message":"Invalid request"}})){return None}continue}};
            let valid_cancel=matches!(&message,ClientJsonRpcMessage::Notification(n) if matches!(&n.notification,ClientNotification::CancelledNotification(_)));
            if valid_cancel {
                if let Some(external)=value.get("params").and_then(|p|p.get("requestId")) {let mut ledger=self.active.lock().unwrap_or_else(|e|e.into_inner());if let Some(key)=ledger.current.get(&external.to_string()).cloned() {if let Some(request)=ledger.active.get_mut(&key) {request.cancelled=true;let completed=request.completed && !request.writing;value["params"]["requestId"]=serde_json::json!(key);if completed {finish_cancelled(&mut ledger,&key);}}}}
                if let Ok(remapped)=serde_json::from_value(value) {return Some(remapped)}
                continue
            }
            if value.get("method").is_some() {if let Some(id)=&id {if self.active.lock().unwrap_or_else(|e|e.into_inner()).current.contains_key(&id.to_string()) {if !self.reject(&serde_json::json!({"jsonrpc":"2.0","id":id,"error":{"code":-32600,"message":"Duplicate request ID"}})){return None}continue}}}
            if value.get("method").and_then(|v|v.as_str())==Some("tools/call") {
                if let Some(id)=id {match self.limits.try_acquire(Instant::now()) {Some(permit)=>{
                    let mut ledger=self.active.lock().unwrap_or_else(|e|e.into_inner());let key=format!("{}{}",self.prefix,ledger.next);ledger.next=ledger.next.wrapping_add(1);ledger.current.insert(id.to_string(),key.clone());ledger.active.insert(key.clone(),ActiveRequest{_permit:permit,external:id,completed:false,cancelled:false,writing:false});value["id"]=serde_json::json!(key);drop(ledger);if let Ok(remapped)=serde_json::from_value(value) {return Some(remapped)}return None
                },None=>{let data=serde_json::json!({"error":{"code":"rate_limited","message":"Too many tool calls."}});if !self.reject(&serde_json::json!({"jsonrpc":"2.0","id":id,"result":{"isError":true,"content":[{"type":"text","text":data.to_string()}],"structuredContent":data}})){return None}continue}}}
            }
            return Some(message)
        }
        None
    }
    async fn close(&mut self)->Result<(),Self::Error> {Ok(())}
}
#[cfg(test)] mod tests {
    use super::*;
    #[test] fn late_old_completion_does_not_release_reused_external_id() {
        let limits=Limits::new(Instant::now());let mut ledger=Ledger::default();let id=serde_json::json!(77);
        ledger.current.insert(id.to_string(),"new".into());
        for key in ["old","new"] {ledger.active.insert(key.into(),ActiveRequest{_permit:limits.try_acquire(Instant::now()).unwrap(),external:id.clone(),completed:true,cancelled:false,writing:false});}
        finish_cancelled(&mut ledger,"old");assert_eq!(ledger.current.get("77").map(String::as_str),Some("new"));assert!(ledger.active.contains_key("new"));
    }
    #[test] fn drain_waits_for_prior_output_and_times_out_when_writer_stalls() {
        let (tx,rx)=mpsc::sync_channel(2);tx.send(OutputMessage::Frame(b"prior\n".to_vec())).unwrap();let handle=DrainHandle(tx);let (finished,wait)=mpsc::channel();
        let thread=std::thread::spawn(move || {finished.send(handle.drain(std::time::Duration::from_secs(1))).unwrap();});
        assert!(matches!(rx.recv().unwrap(),OutputMessage::Frame(_)));let barrier=match rx.recv().unwrap(){OutputMessage::Drain(ack)=>ack,_=>panic!("expected drain barrier")};
        assert!(wait.recv_timeout(std::time::Duration::from_millis(30)).is_err());barrier.send(()).unwrap();assert!(wait.recv_timeout(std::time::Duration::from_secs(1)).unwrap().is_ok());thread.join().unwrap();
        let (tx,_held)=mpsc::sync_channel(1);let handle=DrainHandle(tx);assert_eq!(handle.drain(std::time::Duration::from_millis(30)),Err("output_unavailable"));
    }
}

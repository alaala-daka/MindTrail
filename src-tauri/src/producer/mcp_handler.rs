use std::{sync::mpsc,time::{Duration,Instant}};
use rmcp::{RoleServer,ServerHandler,model::{CallToolRequestParams,CallToolResponse,CallToolResult,ErrorCode,ErrorData,Implementation,ListToolsResult,PaginatedRequestParams,ProtocolVersion,ServerCapabilities,ServerConfig,Tool},service::RequestContext};
use serde_json::{json,Value};
use super::{tools,worker::{Command},queue::Entry};

pub struct Handler {pub commands:mpsc::SyncSender<Command>,pub active:super::mcp_stdio::Active}
struct Release {active:super::mcp_stdio::Active,key:String}
impl Drop for Release {fn drop(&mut self){let mut active=self.active.lock().unwrap_or_else(|e|e.into_inner());if let Some(request)=active.active.get_mut(&self.key) {request.completed=true;if request.cancelled && !request.writing {super::mcp_stdio::finish_cancelled(&mut active,&self.key);}}}}
fn failure(code:&str)->CallToolResult {CallToolResult::structured_error(json!({"error":{"code":code,"message":"The operation could not be completed."}}))}
fn send(commands:&mpsc::SyncSender<Command>,command:Command)->Result<(),&'static str> {commands.try_send(command).map_err(|e|match e {mpsc::TrySendError::Full(_)=>"busy",mpsc::TrySendError::Disconnected(_)=>"queue_unavailable"})}
async fn answer<T>(receiver:mpsc::Receiver<T>,deadline:Instant,context:&RequestContext<RoleServer>)->Result<T,&'static str> {
    loop {if context.ct.is_cancelled() {return Err("cancelled")} match receiver.try_recv() {Ok(value)=>return Ok(value),Err(mpsc::TryRecvError::Disconnected)=>return Err("queue_unavailable"),Err(mpsc::TryRecvError::Empty)=>{}} if Instant::now()>=deadline {return Err("timeout")} tokio::time::sleep(Duration::from_millis(5)).await;}
}
fn render(entry:Entry,session:&str,operation_id:&str)->CallToolResult {
    match entry.status.as_str() {
        "pending"=>CallToolResult::structured(json!({"status":"queued","session":session,"operation_id":operation_id})),
        "committed"=>match entry.outcome.as_ref().and_then(|s|serde_json::from_str::<Value>(s).ok()).and_then(|v|v["results"][0]["seq"].as_u64()) {Some(seq)=>CallToolResult::structured(json!({"status":"committed","session":session,"operation_id":operation_id,"seq":seq})),None=>failure("queue_corrupt")},
        "rejected"=>match entry.outcome.as_ref().and_then(|s|serde_json::from_str::<Value>(s).ok()).and_then(|v|v.as_object().cloned()) {Some(mut detail)=>{detail.insert("message".into(),json!("The gateway rejected this operation."));CallToolResult::structured_error(json!({"error":detail}))},None=>failure("queue_corrupt")},
        _=>failure("queue_corrupt")
    }
}
impl ServerHandler for Handler {
    fn supported_protocol_versions(&self)->std::borrow::Cow<'static,[ProtocolVersion]> {std::borrow::Cow::Owned(vec![ProtocolVersion::V_2025_11_25])}
    fn get_info(&self)->ServerConfig {
        let capabilities=ServerCapabilities::builder().enable_tools().build();
        ServerConfig::new(capabilities).with_protocol_version(ProtocolVersion::V_2025_11_25).with_server_info(Implementation::new("mindtrail-producer","0.1.0"))
    }
    async fn list_tools(&self,_request:Option<PaginatedRequestParams>,_context:RequestContext<RoleServer>)->Result<ListToolsResult,ErrorData> {
        let list=tools::schemas().into_iter().map(|(name,schema)|Tool::new(name,"Record a MindTrail event",schema.as_object().unwrap().clone())).collect();
        let mut result=ListToolsResult::default();result.tools=list;Ok(result)
    }
    async fn call_tool(&self,request:CallToolRequestParams,context:RequestContext<RoleServer>)->Result<CallToolResponse,ErrorData> {
        let _release=Release{active:self.active.clone(),key:serde_json::to_value(&context.id).ok().and_then(|v|v.as_str().map(str::to_owned)).unwrap_or_default()};
        let args=Value::Object(request.arguments.unwrap_or_default());
        let operation=match tools::normalize_tool(&request.name,&args) {Ok(v)=>v,Err("unknown_tool")=>return Err(ErrorData::new(ErrorCode::METHOD_NOT_FOUND,"Unknown tool",None)),Err(e)=>return Ok(failure(e).into())};
        let deadline=Instant::now()+Duration::from_secs(2);let (reply,admitted)=mpsc::channel();
        if let Err(e)=send(&self.commands,Command::AdmitMcp{session:operation.session.clone(),operation_id:operation.operation_id.clone(),kind:operation.kind.into(),payload:operation.payload,reply}) {return Ok(failure(e).into())}
        let id=match answer(admitted,deadline,&context).await {Ok(Ok(id))=>id,Ok(Err(e))|Err(e)=>return Ok(failure(e).into())};
        if context.ct.is_cancelled() {return Err(ErrorData::invalid_params("Cancelled",None))}
        let watch_deadline=deadline-Duration::from_millis(50);
        let (reply,watch)=mpsc::channel();if let Err(e)=send(&self.commands,Command::Watch{id,deadline:watch_deadline,reply}) {return Ok(failure(e).into())}
        let entry=match answer(watch,watch_deadline,&context).await {Ok(Ok(Some(entry)))=>Some(entry),Ok(Ok(None))=>None,Ok(Err(e))=>return Ok(failure(e).into()),Err("timeout"|"queue_unavailable")=>None,Err(e)=>return Ok(failure(e).into())};
        if let Some(entry)=entry {return Ok(render(entry,&operation.session,&operation.operation_id).into())}
        let (reply,get)=mpsc::channel();if let Err(e)=send(&self.commands,Command::Get{id,reply}) {return Ok(failure(e).into())}
        match answer(get,deadline,&context).await {Ok(Ok(Some(entry)))=>Ok(render(entry,&operation.session,&operation.operation_id).into()),Ok(Err(e))|Err(e)=>Ok(failure(e).into()),_=>Ok(failure("queue_corrupt").into())}
    }
}

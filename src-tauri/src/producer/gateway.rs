use std::{net::{Ipv4Addr,SocketAddrV4},path::Path,time::Duration};
use http_body_util::{BodyExt, Full};
use hyper::{body::Bytes, http::{Request,header}, client::conn::http1};
use hyper_util::rt::TokioIo;
use super::queue::{Entry,QueueMode,valid_outcome};

#[derive(Clone, PartialEq, Eq)]
pub struct EndpointSnapshot { port:u16, token:String, store_id:String }
impl EndpointSnapshot {
    pub fn read(root:&Path) -> Result<Self,&'static str> {
        let bytes=crate::gateway::read_producer_discovery(root).map_err(|e| if e.kind()==std::io::ErrorKind::NotFound {"discovery_missing"} else {"discovery_unavailable"})?;
        Self::parse(&bytes)
    }
    fn parse(bytes:&[u8]) -> Result<Self,&'static str> {
        let value=crate::protocol::raw::parse_strict_value(bytes,4096).map_err(|_|"discovery_invalid")?;
        let object=value.as_object().ok_or("discovery_invalid")?;
        if object.len()!=4 || !["port","token","version","store_id"].iter().all(|key|object.contains_key(*key)) { return Err("discovery_invalid"); }
        if object["version"].as_u64()!=Some(1) { return Err("discovery_incompatible"); }
        let port=object["port"].as_u64().filter(|p|(1..=65535).contains(p)).ok_or("discovery_invalid")? as u16;
        let valid_hex=|v:&serde_json::Value| v.as_str().filter(|s|s.len()==64 && s.bytes().all(|b|b.is_ascii_digit()||(b'a'..=b'f').contains(&b))).map(str::to_owned);
        Ok(Self {port,token:valid_hex(&object["token"]).ok_or("discovery_invalid")?,store_id:valid_hex(&object["store_id"]).ok_or("discovery_invalid")?})
    }
    pub fn store_id(&self)->&str { &self.store_id }
}
pub enum Attempt { Committed(serde_json::Value), Rejected(serde_json::Value), Unauthorized, Retry }
pub struct GatewayClient { budget:Duration }
fn valid_rejection(mode:QueueMode, entry:&Entry, status:u16, value:&serde_json::Value)->bool {
    if !valid_outcome(mode,entry,"rejected",value) { return false; }
    let Some(code)=value.get("code").and_then(|v|v.as_str()) else {return false};
    let count=match mode {QueueMode::Mcp=>match crate::protocol::decode_append(&entry.request) {Ok(b)=>b.items().len(),Err(_)=>return false},QueueMode::Explicit=>match crate::protocol::decode_explicit(&entry.request) {Ok(b)=>b.items().len(),Err(_)=>return false}};
    if let Some(index)=value.get("index") {if !index.as_u64().is_some_and(|v|v < count as u64) {return false}}
    match status {
        400=>code=="invalid_request",
        413=>code=="payload_too_large" && value.as_object().is_some_and(|o|o.len()==1),
        409=>{
            let (Some(index),Some(last))=(value.get("index").and_then(|v|v.as_u64()),value.get("last_seq").and_then(|v|v.as_u64())) else {return false};
            if matches!(code,"invalid_request"|"payload_too_large") || (code=="operation_conflict" && mode!=QueueMode::Mcp) || (matches!(code,"seq_conflict"|"seq_gap") && mode!=QueueMode::Explicit) || (code=="seq_exhausted" && mode!=QueueMode::Mcp) {return false}
            if code=="seq_gap" {
                let (Some(expected),Some(offending))=(value.get("expected_seq").and_then(|v|v.as_u64()),value.get("offending_seq").and_then(|v|v.as_u64())) else {return false};
                let Ok(batch)=crate::protocol::decode_explicit(&entry.request) else {return false};
                expected>last && offending>expected && batch.items()[index as usize].seq()==offending
            } else {value.get("expected_seq").is_none() && value.get("offending_seq").is_none()}
        },
        _=>false,
    }
}
impl GatewayClient {
    pub fn new(budget:Duration)->Self {Self {budget}}
    async fn request(&self, endpoint:&EndpointSnapshot, method:hyper::Method, path:&'static str, body:Vec<u8>) -> Result<(u16,Vec<u8>), &'static str> {
        tokio::time::timeout(self.budget,async {
            let addr=SocketAddrV4::new(Ipv4Addr::LOCALHOST,endpoint.port);
            let stream=tokio::net::TcpStream::connect(addr).await.map_err(|_|"gateway_unavailable")?;
            let (mut sender,connection)=http1::handshake(TokioIo::new(stream)).await.map_err(|_|"gateway_unavailable")?;
            let task=tokio::spawn(async move { let _=connection.await; });
            struct Abort(tokio::task::JoinHandle<()>); impl Drop for Abort {fn drop(&mut self){self.0.abort()}}
            let _task=Abort(task);
            let request=Request::builder().method(method).uri(path).header(header::HOST,addr.to_string()).header(header::AUTHORIZATION,format!("Bearer {}",endpoint.token)).header(header::CONTENT_TYPE,"application/json").body(Full::new(Bytes::from(body))).map_err(|_|"gateway_unavailable")?;
            let response=sender.send_request(request).await.map_err(|_|"gateway_unavailable")?;
            let status=response.status().as_u16(); let mut body=response.into_body(); let mut bytes=Vec::new();
            while let Some(frame)=body.frame().await { let frame=frame.map_err(|_|"gateway_unavailable")?; if let Some(data)=frame.data_ref() { if bytes.len().saturating_add(data.len())>65536 {return Err("gateway_unavailable");} bytes.extend_from_slice(data); } }
            Ok((status,bytes))
        }).await.map_err(|_|"gateway_unavailable")?
    }
    pub async fn health(&self, endpoint:&EndpointSnapshot)->Result<(), &'static str> {
        let (status,bytes)=self.request(endpoint,hyper::Method::GET,"/v1/health",Vec::new()).await?;
        if status==401 {return Err("gateway_unauthorized")}; if status!=200 {return Err("gateway_unavailable")};
        let value=crate::protocol::raw::parse_strict_value(&bytes,65536).map_err(|_|"gateway_unavailable")?;
        let obj=value.as_object().ok_or("gateway_unavailable")?;
        if obj.len()!=2 || obj.get("version").and_then(|v|v.as_u64())!=Some(1) || obj.get("store_id").and_then(|v|v.as_str())!=Some(endpoint.store_id()) {return Err("gateway_mismatch")}; Ok(())
    }
    pub async fn submit(&self, endpoint:&EndpointSnapshot, mode:QueueMode, entry:&Entry)->Attempt {
        let path=match mode {QueueMode::Mcp=>"/v1/append",QueueMode::Explicit=>"/v1/events"};
        let Ok((status,bytes))=self.request(endpoint,hyper::Method::POST,path,entry.request.clone()).await else {return Attempt::Retry};
        if status==401 {return Attempt::Unauthorized};
        let Ok(value)=crate::protocol::raw::parse_strict_value(&bytes,65536) else {return Attempt::Retry};
        if status==200 && valid_outcome(mode,entry,"committed",&value) {return Attempt::Committed(value)}
        if matches!(status,400|409|413) {
            if let Some(error)=value.as_object().filter(|o|o.len()==1).and_then(|o|o.get("error")).and_then(|v|v.as_object()) {
                if !error.get("message").and_then(|v|v.as_str()).is_some_and(|s|!s.is_empty()) {return Attempt::Retry}
                let mut safe=error.clone(); safe.remove("message"); let normalized=serde_json::Value::Object(safe);
                if valid_rejection(mode,entry,status,&normalized) {return Attempt::Rejected(normalized)}
            }
        }
        Attempt::Retry
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read,Write};
    fn endpoint(port:u16) -> EndpointSnapshot { EndpointSnapshot {port,token:"a".repeat(64),store_id:"b".repeat(64)} }
    fn serve(responses:Vec<(u16,String)>) -> (u16,std::thread::JoinHandle<Vec<String>>) {
        let listener=std::net::TcpListener::bind("127.0.0.1:0").unwrap(); let port=listener.local_addr().unwrap().port();
        let worker=std::thread::spawn(move || { let mut paths=Vec::new(); for (status,body) in responses { let (mut s,_)=listener.accept().unwrap(); s.set_read_timeout(Some(Duration::from_secs(5))).unwrap(); let mut bytes=Vec::new(); let mut one=[0u8;1]; while !bytes.ends_with(b"\r\n\r\n") { s.read_exact(&mut one).unwrap(); bytes.push(one[0]); } let head=String::from_utf8(bytes).unwrap(); paths.push(head.lines().next().unwrap().to_string()); let length=head.lines().find_map(|l|l.to_ascii_lowercase().strip_prefix("content-length: ").map(str::to_owned)).and_then(|v|v.parse::<usize>().ok()).unwrap_or(0); let mut body_bytes=vec![0;length]; s.read_exact(&mut body_bytes).unwrap(); write!(s,"HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap(); } paths }); (port,worker)
    }
    #[test] fn discovery_rejects_duplicates_unknown_and_unsafe_values() {
        let good=format!(r#"{{"port":123,"version":1,"token":"{}","store_id":"{}"}}"#,"a".repeat(64),"b".repeat(64));
        assert!(EndpointSnapshot::parse(good.as_bytes()).is_ok());
        for bad in [good.replace("\"port\":123","\"port\":0"),good.replace("\"port\":123","\"port\":123,\"port\":124"),good.replace("\"version\":1","\"version\":2"),good.replace("\"port\":123","\"port\":123,\"extra\":0")] {assert!(EndpointSnapshot::parse(bad.as_bytes()).is_err())}
    }
    #[test] fn health_and_receipt_are_validated_over_loopback() {
        let health=format!(r#"{{"version":1,"store_id":"{}"}}"#,"b".repeat(64));
        let good=r#"{"accepted":1,"duplicates":0,"last_seq":1,"results":[{"operation_id":"op","seq":1}]}"#.to_owned();
        let bad=r#"{"accepted":1,"duplicates":0,"last_seq":1,"results":[{"operation_id":"wrong","seq":1}]}"#.to_owned();
        let (port,server)=serve(vec![(200,health),(200,good),(200,bad),(401,"{}".into())]); let ep=endpoint(port); let client=GatewayClient::new(Duration::from_secs(3));
        let entry=Entry{id:1,session:"s".into(),status:"pending".into(),outcome:None,request:br#"[{"v":1,"session":"s","operation_id":"op","ts":"2024-01-01T00:00:00Z","type":"task.start","payload":{"title":"T","goal":"G","project":"P"}}]"#.to_vec()};
        let rt=tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap(); rt.block_on(async { assert!(client.health(&ep).await.is_ok()); assert!(matches!(client.submit(&ep,QueueMode::Mcp,&entry).await,Attempt::Committed(_))); assert!(matches!(client.submit(&ep,QueueMode::Mcp,&entry).await,Attempt::Retry)); assert!(matches!(client.submit(&ep,QueueMode::Mcp,&entry).await,Attempt::Unauthorized)); });
        assert_eq!(server.join().unwrap(),vec!["GET /v1/health HTTP/1.1","POST /v1/append HTTP/1.1","POST /v1/append HTTP/1.1","POST /v1/append HTTP/1.1"]);
    }
    #[test] fn malformed_responses_and_http_errors_do_not_forge_confirmation() {
        let bad_id=r#"{"accepted":1,"duplicates":0,"last_seq":1,"results":[{"operation_id":"op","seq":2}]}"#.to_owned();
        let duplicate=r#"{"accepted":1,"accepted":1,"duplicates":0,"last_seq":1,"results":[]}"#.to_owned();
        let rejected=r#"{"error":{"code":"task_closed","message":"closed","index":0,"last_seq":1}}"#.to_owned();
        let malformed=r#"{"error":{"code":"task_closed","message":"closed"}}"#.to_owned();
        let bad_index=r#"{"error":{"code":"task_closed","message":"closed","index":1,"last_seq":1}}"#.to_owned();
        let wrong_mode=r#"{"error":{"code":"seq_conflict","message":"conflict","index":0,"last_seq":1}}"#.to_owned();
        let (port,server)=serve(vec![(200,bad_id),(200,duplicate),(200,"x".repeat(65537)),(503,"{}".into()),(302,"{}".into()),(409,bad_index),(409,wrong_mode),(409,rejected),(409,malformed)]);
        let entry=Entry{id:1,session:"s".into(),status:"pending".into(),outcome:None,request:br#"[{"v":1,"session":"s","operation_id":"op","ts":"2024-01-01T00:00:00Z","type":"task.start","payload":{"title":"T","goal":"G","project":"P"}}]"#.to_vec()};
        let ep=endpoint(port); let client=GatewayClient::new(Duration::from_secs(3)); let rt=tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async { for _ in 0..7 { assert!(matches!(client.submit(&ep,QueueMode::Mcp,&entry).await,Attempt::Retry)); } assert!(matches!(client.submit(&ep,QueueMode::Mcp,&entry).await,Attempt::Rejected(_))); assert!(matches!(client.submit(&ep,QueueMode::Mcp,&entry).await,Attempt::Retry)); });
        assert_eq!(server.join().unwrap().len(),9);
    }
    #[test] fn health_requires_matching_identity_and_exact_shape() {
        let mismatch=format!(r#"{{"version":1,"store_id":"{}"}}"#,"c".repeat(64));
        let extra=format!(r#"{{"version":1,"store_id":"{}","extra":true}}"#,"b".repeat(64));
        let (port,server)=serve(vec![(200,mismatch),(200,extra)]); let ep=endpoint(port); let rt=tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async { let client=GatewayClient::new(Duration::from_secs(3)); assert_eq!(client.health(&ep).await,Err("gateway_mismatch")); assert_eq!(client.health(&ep).await,Err("gateway_mismatch")); }); server.join().unwrap();
    }
    #[test] fn stalled_or_truncated_responses_remain_retryable() {
        let entry=Entry{id:1,session:"s".into(),status:"pending".into(),outcome:None,request:br#"[{"v":1,"session":"s","operation_id":"op","ts":"2024-01-01T00:00:00Z","type":"task.start","payload":{"title":"T","goal":"G","project":"P"}}]"#.to_vec()};
        let listener=std::net::TcpListener::bind("127.0.0.1:0").unwrap(); let port=listener.local_addr().unwrap().port(); let (release,wait)=std::sync::mpsc::channel::<()>();
        let thread=std::thread::spawn(move || {let (mut first,_)=listener.accept().unwrap(); let _=wait.recv_timeout(Duration::from_secs(5)); drop(first); let (mut second,_)=listener.accept().unwrap(); second.set_read_timeout(Some(Duration::from_secs(3))).unwrap(); let mut buf=[0u8;4096]; let _=second.read(&mut buf); write!(second,"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\n{{}}").unwrap(); });
        let rt=tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap(); let ep=endpoint(port);
        rt.block_on(async {let short=GatewayClient::new(Duration::from_millis(200)); assert!(matches!(short.submit(&ep,QueueMode::Mcp,&entry).await,Attempt::Retry)); release.send(()).unwrap(); let normal=GatewayClient::new(Duration::from_secs(3)); assert!(matches!(normal.submit(&ep,QueueMode::Mcp,&entry).await,Attempt::Retry));}); thread.join().unwrap();
    }
    #[test] fn gap_error_must_match_the_original_batch_and_transaction_watermark() {
        let entry=Entry{id:1,session:"s".into(),status:"pending".into(),outcome:None,request:br#"[{"v":1,"session":"s","seq":3,"ts":"2024-01-01T00:00:00Z","type":"task.start","payload":{"title":"T","goal":"G","project":"P"}}]"#.to_vec()};
        let error=serde_json::json!({"code":"seq_gap","index":0,"last_seq":1,"expected_seq":2,"offending_seq":3});
        assert!(valid_rejection(QueueMode::Explicit,&entry,409,&error));
        let mut changed=error.clone(); changed["index"]=1.into(); assert!(!valid_rejection(QueueMode::Explicit,&entry,409,&changed));
        let mut changed=error.clone(); changed["expected_seq"]=1.into(); assert!(!valid_rejection(QueueMode::Explicit,&entry,409,&changed));
        let mut changed=error.clone(); changed["offending_seq"]=4.into(); assert!(!valid_rejection(QueueMode::Explicit,&entry,409,&changed));
    }
}

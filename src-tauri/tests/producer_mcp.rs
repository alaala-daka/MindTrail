#![cfg(all(windows, feature="producer-test-fixture"))]
use rmcp::{ServiceExt,model::{CallToolRequestParams,ClientConfig,ClientRequest,PingRequest,ProtocolVersion,ServerResult},transport::TokioChildProcess};

#[test]
fn official_client_initializes_lists_and_queues() {
    let mut random=[0u8;16];getrandom::fill(&mut random).unwrap();let suffix:String=random.iter().map(|v|format!("{v:02x}")).collect();
    let fixture=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailMcpClient-{suffix}"));std::fs::create_dir(&fixture).unwrap();
    let mut command=tokio::process::Command::new(env!("CARGO_BIN_EXE_mindtrail-producer"));command.args(["mcp","--queue","client"]).env("MINDTRAIL_TEST_ROOT",fixture.join("state"));
    let rt=tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(async {
        let transport=TokioChildProcess::new(command).unwrap();let config=ClientConfig::default().with_protocol_version(ProtocolVersion::V_2025_11_25);
        let client=tokio::time::timeout(std::time::Duration::from_secs(10),config.serve(transport)).await.unwrap().unwrap();
        let info=client.peer_info().unwrap();assert_eq!(info.protocol_version,ProtocolVersion::V_2025_11_25);assert!(info.capabilities.tools.is_some());
        assert!(matches!(client.send_request(ClientRequest::PingRequest(PingRequest::default())).await.unwrap(),ServerResult::EmptyResult(_)));
        let tools=client.list_tools(None).await.unwrap();assert_eq!(tools.tools.len(),7);
        let arguments=serde_json::json!({"session":"s","operation_id":"op","title":"标题","goal":"G","project":"P"}).as_object().unwrap().clone();
        let result=client.call_tool(CallToolRequestParams::new("trace_task_start").with_arguments(arguments)).await.unwrap();
        assert_eq!(result.structured_content.unwrap()["status"],"queued");assert_eq!(result.is_error,Some(false));
        client.cancel().await.unwrap();
    });
}

#[test]
fn seven_events_commit_and_old_receipt_survives_restart() {
    let mut random=[0u8;16];getrandom::fill(&mut random).unwrap();let suffix:String=random.iter().map(|v|format!("{v:02x}")).collect();
    let fixture=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailMcpReal-{suffix}"));std::fs::create_dir(&fixture).unwrap();let root=fixture.join("state");
    let gateway=mindtrail_lib::gateway::lifecycle::Gateway::start(&root).unwrap();
    let rt=tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(async {
        let start=|| {let mut cmd=tokio::process::Command::new(env!("CARGO_BIN_EXE_mindtrail-producer"));cmd.args(["mcp","--queue","real"]).env("MINDTRAIL_TEST_ROOT",&root);TokioChildProcess::new(cmd).unwrap()};
        let config=||ClientConfig::default().with_protocol_version(ProtocolVersion::V_2025_11_25);
        let client=config().serve(start()).await.unwrap();
        let events=[("trace_task_start",serde_json::json!({"title":" T ","goal":"G","project":"../project","quote":"quote","context":"context"})),("trace_intent",serde_json::json!({"id":"cap","title":"Capability"})),("trace_api",serde_json::json!({"id":"api","kind":"fn","signature":"fn run()","capability":"cap"})),("trace_file",serde_json::json!({"id":"file","path":"../文件","action":"modify","capability":"cap"})),("trace_call",serde_json::json!({"from":"api","to":"file"})),("trace_done",serde_json::json!({"id":"cap"})),("trace_task_done",serde_json::json!({"summary":"done"}))];
        for (index,(name,mut args)) in events.into_iter().enumerate() {args["session"]=serde_json::json!("real-session");args["operation_id"]=serde_json::json!(format!("op-{index}"));let result=client.call_tool(CallToolRequestParams::new(name).with_arguments(args.as_object().unwrap().clone())).await.unwrap();let data=result.structured_content.unwrap();assert_eq!(data["status"],"committed","{data}");assert_eq!(data["seq"],(index+1) as u64);}
        client.cancel().await.unwrap();
        let client=config().serve(start()).await.unwrap();let mut args=serde_json::json!({"session":"real-session","operation_id":"op-0","title":" T ","goal":"G","project":"../project","quote":"quote","context":"context"});let result=client.call_tool(CallToolRequestParams::new("trace_task_start").with_arguments(args.as_object().unwrap().clone())).await.unwrap();assert_eq!(result.structured_content.unwrap()["seq"],1);args["title"]=serde_json::json!("changed");let conflict=client.call_tool(CallToolRequestParams::new("trace_task_start").with_arguments(args.as_object().unwrap().clone())).await.unwrap();assert_eq!(conflict.is_error,Some(true));assert_eq!(conflict.structured_content.unwrap()["error"]["code"],"operation_conflict");client.cancel().await.unwrap();
    });
    let discovery:serde_json::Value=serde_json::from_slice(&std::fs::read(root.join("endpoint.json")).unwrap()).unwrap();let port=discovery["port"].as_u64().unwrap();let token=discovery["token"].as_str().unwrap();
    use std::io::{Read,Write};let mut stream=std::net::TcpStream::connect(("127.0.0.1",port as u16)).unwrap();stream.set_read_timeout(Some(std::time::Duration::from_secs(3))).unwrap();write!(stream,"GET /v1/sessions/real-session/events HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n").unwrap();let mut response=String::new();stream.read_to_string(&mut response).unwrap();let (header,body)=response.split_once("\r\n\r\n").unwrap();assert!(header.starts_with("HTTP/1.1 200"));let value:serde_json::Value=serde_json::from_str(body).unwrap();assert_eq!(value["events"].as_array().unwrap().len(),7);assert_eq!(value["events"][0]["payload"]["quote"],"quote");assert_eq!(value["events"][0]["payload"]["context"],"context");assert_eq!(value["events"][3]["payload"]["path"],"../文件");
    gateway.shutdown().unwrap();
}

#[test]
fn offline_restart_preserves_timestamp_and_recovers_without_new_calls() {
    let mut random=[0u8;16];getrandom::fill(&mut random).unwrap();let suffix:String=random.iter().map(|v|format!("{v:02x}")).collect();
    let fixture=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailMcpOffline-{suffix}"));std::fs::create_dir(&fixture).unwrap();let root=fixture.join("state");
    let rt=tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let start=|| {let mut cmd=tokio::process::Command::new(env!("CARGO_BIN_EXE_mindtrail-producer"));cmd.args(["mcp","--queue","offline"]).env("MINDTRAIL_TEST_ROOT",&root);TokioChildProcess::new(cmd).unwrap()};
    let config=||ClientConfig::default().with_protocol_version(ProtocolVersion::V_2025_11_25);
    rt.block_on(async {let client=config().serve(start()).await.unwrap();let args=serde_json::json!({"session":"offline-session","operation_id":"first","title":"T","goal":"G","project":"P"});let result=client.call_tool(CallToolRequestParams::new("trace_task_start").with_arguments(args.as_object().unwrap().clone())).await.unwrap();assert_eq!(result.structured_content.unwrap()["status"],"queued");client.cancel().await.unwrap();});
    let db=root.join("producers/mcp/offline/queue.sqlite3");let read_request=|| {let db=rusqlite::Connection::open_with_flags(&db,rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();db.query_row("SELECT request FROM entries WHERE operation_id='first'",[],|r|r.get::<_,Vec<u8>>(0)).unwrap()};
    let original=read_request();let event:serde_json::Value=serde_json::from_slice(&original).unwrap();let timestamp=event[0]["ts"].as_str().unwrap().to_owned();
    let client=rt.block_on(async {config().serve(start()).await.unwrap()});assert_eq!(read_request(),original);
    let gateway=mindtrail_lib::gateway::lifecycle::Gateway::start(&root).unwrap();
    let discovery:serde_json::Value=serde_json::from_slice(&std::fs::read(root.join("endpoint.json")).unwrap()).unwrap();let port=discovery["port"].as_u64().unwrap() as u16;let token=discovery["token"].as_str().unwrap();
    let deadline=std::time::Instant::now()+std::time::Duration::from_secs(10);let received=loop {use std::io::{Read,Write};let mut stream=std::net::TcpStream::connect(("127.0.0.1",port)).unwrap();stream.set_read_timeout(Some(std::time::Duration::from_secs(2))).unwrap();write!(stream,"GET /v1/sessions/offline-session/events HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n").unwrap();let mut response=String::new();stream.read_to_string(&mut response).unwrap();let (header,body)=response.split_once("\r\n\r\n").unwrap();if header.starts_with("HTTP/1.1 200") {break serde_json::from_str::<serde_json::Value>(body).unwrap()} assert!(std::time::Instant::now()<deadline,"automatic recovery timed out");std::thread::sleep(std::time::Duration::from_millis(100));};
    assert_eq!(received["events"].as_array().unwrap().len(),1);assert_eq!(received["events"][0]["ts"],timestamp);assert_eq!(read_request(),original);
    rt.block_on(async {client.cancel().await.unwrap();});gateway.shutdown().unwrap();
}

#[test]
fn cancelled_durable_operations_recover_to_real_gateway() {
    let python=std::env::var_os("MINDTRAIL_TEST_PYTHON").expect("set MINDTRAIL_TEST_PYTHON to an isolated-test Python executable");
    let test_script=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/producer_mcp_concurrency.py");
    let output=std::process::Command::new(python).arg(test_script).arg(env!("CARGO_BIN_EXE_mindtrail-producer")).arg(env!("CARGO_MANIFEST_DIR")).output().unwrap();assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));
    let message=String::from_utf8(output.stdout).unwrap();let fixture=std::path::PathBuf::from(message.trim().split_once("fixture: ").unwrap().1);let root=fixture.join("state");
    let db=root.join("producers/mcp/concurrent/queue.sqlite3");let read_counts=|| {let db=rusqlite::Connection::open_with_flags(&db,rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();db.query_row("SELECT count(*),sum(status='committed') FROM entries",[],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?))).unwrap()};assert_eq!(read_counts(),(17,0));
    let gateway=mindtrail_lib::gateway::lifecycle::Gateway::start(&root).unwrap();let rt=tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();let mut command=tokio::process::Command::new(env!("CARGO_BIN_EXE_mindtrail-producer"));command.args(["mcp","--queue","concurrent"]).env("MINDTRAIL_TEST_ROOT",&root);
    let client=rt.block_on(async {ClientConfig::default().with_protocol_version(ProtocolVersion::V_2025_11_25).serve(TokioChildProcess::new(command).unwrap()).await.unwrap()});
    let deadline=std::time::Instant::now()+std::time::Duration::from_secs(20);while read_counts()!=(17,17) {assert!(std::time::Instant::now()<deadline,"cancelled operations did not all commit");std::thread::sleep(std::time::Duration::from_millis(100));}
    let discovery:serde_json::Value=serde_json::from_slice(&std::fs::read(root.join("endpoint.json")).unwrap()).unwrap();let port=discovery["port"].as_u64().unwrap() as u16;let token=discovery["token"].as_str().unwrap();
    for id in 100..116 {use std::io::{Read,Write};let mut stream=std::net::TcpStream::connect(("127.0.0.1",port)).unwrap();stream.set_read_timeout(Some(std::time::Duration::from_secs(2))).unwrap();write!(stream,"GET /v1/sessions/s{id}/events HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n").unwrap();let mut response=String::new();stream.read_to_string(&mut response).unwrap();let (header,body)=response.split_once("\r\n\r\n").unwrap();assert!(header.starts_with("HTTP/1.1 200"));let value:serde_json::Value=serde_json::from_str(body).unwrap();assert_eq!(value["events"].as_array().unwrap().len(),1);assert_eq!(value["events"][0]["session"],format!("s{id}"));}
    rt.block_on(async {client.cancel().await.unwrap();});gateway.shutdown().unwrap();
}

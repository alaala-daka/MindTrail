#![cfg(all(windows,feature="producer-test-fixture"))]
use rmcp::{ServiceExt,model::{CallToolRequestParams,ClientConfig,ProtocolVersion},transport::TokioChildProcess};
use serde_json::{json,Value};
#[test] fn isolated_desktop_writer() {
    let Ok(root)=std::env::var("MINDTRAIL_DESKTOP_ROOT") else{return};let Ok(control)=std::env::var("MINDTRAIL_DESKTOP_CONTROL") else{return};let dir=std::path::PathBuf::from(control);
    let mut cmd=tokio::process::Command::new(env!("CARGO_BIN_EXE_mindtrail-producer"));cmd.args(["mcp","--queue","desktop"]).env("MINDTRAIL_TEST_ROOT",root);
    let rt=tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();rt.block_on(async {
        let client=ClientConfig::default().with_protocol_version(ProtocolVersion::V_2025_11_25).serve(TokioChildProcess::new(cmd).unwrap()).await.unwrap();
        let facts=[("trace_task_start",json!({"title":"MCP桌面任务","goal":"验证恢复","project":"isolated"})),("trace_intent",json!({"id":"cap","title":"能力"})),("trace_api",json!({"id":"api","capability":"cap","kind":"http","signature":"GET /mcp","quote":"中文证据","context":"隔离上下文"})),("trace_file",json!({"id":"file","capability":"cap","path":"src/mcp.ts","action":"create"})),("trace_call",json!({"from":"api","to":"file"})),("trace_done",json!({"id":"cap"})),("trace_task_done",json!({"summary":"完成"}))];
        for (index,(name,mut args)) in facts.into_iter().enumerate(){args["session"]=json!("mcp-desktop");args["operation_id"]=json!(format!("desktop-op-{index}"));let result=client.call_tool(CallToolRequestParams::new(name).with_arguments(args.as_object().unwrap().clone())).await.unwrap();let data:Value=result.structured_content.unwrap();assert_eq!(data["status"],"queued");}
        std::fs::write(dir.join("producer-ready"),b"ready").unwrap();let until=std::time::Instant::now()+std::time::Duration::from_secs(180);
        while !dir.join("producer-second-start").is_file() && !dir.join("producer-stop").is_file() && std::time::Instant::now()<until {tokio::time::sleep(std::time::Duration::from_millis(100)).await;}
        if dir.join("producer-stop").is_file() {client.cancel().await.unwrap();return}
        let start=json!({"session":"mcp-second","operation_id":"second-start","title":"离线第二任务","goal":"验证运行状态","project":"isolated"});let result=client.call_tool(CallToolRequestParams::new("trace_task_start").with_arguments(start.as_object().unwrap().clone())).await.unwrap().structured_content.unwrap();assert_eq!(result["status"],"queued");std::fs::write(dir.join("producer-second-ready"),b"ready").unwrap();
        while !dir.join("producer-second-finish").is_file() && !dir.join("producer-stop").is_file() && std::time::Instant::now()<until {tokio::time::sleep(std::time::Duration::from_millis(100)).await;}
        if dir.join("producer-stop").is_file() {client.cancel().await.unwrap();return}
        let done=json!({"session":"mcp-second","operation_id":"second-done","summary":"完成"});let result=client.call_tool(CallToolRequestParams::new("trace_task_done").with_arguments(done.as_object().unwrap().clone())).await.unwrap().structured_content.unwrap();assert_eq!(result["status"],"committed");std::fs::write(dir.join("producer-second-done"),b"done").unwrap();
        while !dir.join("producer-stop").is_file() && std::time::Instant::now()<until {tokio::time::sleep(std::time::Duration::from_millis(100)).await;}client.cancel().await.unwrap();
    });
}

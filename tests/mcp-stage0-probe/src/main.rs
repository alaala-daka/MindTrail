#[path = "../../../src-tauri/src/protocol/raw.rs"] mod raw;
use rmcp::{RoleServer, ServerHandler, ServiceExt, model::{ClientJsonRpcMessage, ServerJsonRpcMessage, ProtocolVersion}, transport::Transport};
use tokio::sync::mpsc;
struct Server;
impl ServerHandler for Server {}
fn valid_frame(frame: &[u8]) -> bool { frame.len() <= 2 * 1024 * 1024 && raw::parse_stored_event(frame).is_ok() }
struct Strict { input: mpsc::Receiver<Vec<u8>>, output: mpsc::Sender<ServerJsonRpcMessage>, rejected: std::sync::Arc<std::sync::atomic::AtomicUsize> }
impl Transport<RoleServer> for Strict {
    type Error = std::io::Error;
    fn send(&mut self, item: ServerJsonRpcMessage) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send + 'static {
        let output = self.output.clone(); async move { output.send(item).await.map_err(|_| std::io::Error::other("closed")) }
    }
    async fn receive(&mut self) -> Option<ClientJsonRpcMessage> {
        while let Some(frame) = self.input.recv().await {
            if !valid_frame(&frame) { self.rejected.fetch_add(1, std::sync::atomic::Ordering::SeqCst); continue; }
            if let Ok(value) = serde_json::from_slice(&frame) { return Some(value); }
        }
        None
    }
    async fn close(&mut self) -> Result<(), Self::Error> { Ok(()) }
}
#[tokio::main(flavor="current_thread")]
async fn main() {
    assert_eq!(serde_json::to_string(&ProtocolVersion::V_2025_11_25).unwrap(), "\"2025-11-25\"");
    let input = r#"{"jsonrpc":"2.0","id":9007199254740991,"method":"tools/call","params":{"name":"test","arguments":{"number":9007199254740991}}}"#;
    let msg: ClientJsonRpcMessage = serde_json::from_str(input).unwrap();
    let value = serde_json::to_value(msg).unwrap(); assert_eq!(value["id"].as_u64(), Some(9007199254740991)); assert_eq!(value["params"]["arguments"]["number"].as_u64(), Some(9007199254740991));
    let (tx, rx) = mpsc::channel(8); let (out, mut results) = mpsc::channel(8);
    let rejected = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)); let observed = rejected.clone();
    let server = tokio::spawn(async move { Server.serve(Strict { input: rx, output: out, rejected }).await.unwrap().waiting().await.unwrap(); });
    let duplicate = br#"{"jsonrpc":"2.0","id":1,"\u0069d":2,"method":"ping"}"#.to_vec();
    let invalid_utf8 = b"{\"jsonrpc\":\"2.0\",\"bad\":\"\xff\"}".to_vec();
    let mut boundary = vec![b'"']; boundary.extend(vec![b'a'; 2 * 1024 * 1024 - 2]); boundary.push(b'"');
    assert_eq!(boundary.len(), 2 * 1024 * 1024); assert!(valid_frame(&boundary));
    let mut oversized = boundary.clone(); oversized.insert(1, b'a');
    assert!(serde_json::from_slice::<serde_json::Value>(&oversized).is_ok()); assert!(!valid_frame(&oversized));
    assert!(!valid_frame(&duplicate)); assert!(!valid_frame(&invalid_utf8));
    tx.send(duplicate).await.unwrap(); tx.send(invalid_utf8).await.unwrap(); tx.send(oversized).await.unwrap();
    tx.send(br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"probe","version":"1"}}}"#.to_vec()).await.unwrap();
    let first = tokio::time::timeout(std::time::Duration::from_secs(5), results.recv()).await.unwrap().unwrap();
    let first = serde_json::to_value(first).unwrap(); assert_eq!(first["id"],1); assert_eq!(first["result"]["protocolVersion"], "2025-11-25");
    tx.send(br#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#.to_vec()).await.unwrap();
    tx.send(br#"{"jsonrpc":"2.0","id":3,"method":"ping"}"#.to_vec()).await.unwrap();
    let ping = tokio::time::timeout(std::time::Duration::from_secs(5), results.recv()).await.unwrap().unwrap(); let ping = serde_json::to_value(ping).unwrap(); assert_eq!(ping["id"],3); assert_eq!(ping["result"], serde_json::json!({})); assert!(ping.get("error").is_none());
    assert_eq!(observed.load(std::sync::atomic::Ordering::SeqCst), 3);
    drop(tx); server.await.unwrap(); println!("PASS duplicate rejected before SDK, initialize and ping, exact integer");
}

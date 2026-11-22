//! Process-interruption coverage. This module and its checkpoints exist only in test builds.
use std::{io::{BufRead, BufReader, Read, Write}, net::TcpStream, process::{Command, Stdio}, sync::mpsc, time::Duration};
use super::store::Store;

const BODY: &str = r#"[{"v":1,"session":"recovery","operation_id":"one","ts":"2024-01-01T00:00:00Z","type":"task.start","payload":{"title":"T","goal":"G","project":"P"}},{"v":1,"session":"recovery","operation_id":"two","ts":"2024-01-01T00:00:00Z","type":"capability.intent","payload":{"id":"c","title":"C"}},{"v":1,"session":"recovery","operation_id":"three","ts":"2024-01-01T00:00:00Z","type":"task.done","payload":{}}]"#;


pub(super) fn checkpoint(phase: &str) {
    if std::env::var("MINDTRAIL_RECOVERY_PHASE").ok().as_deref() == Some(phase) {
        println!("RECOVERY_CHECKPOINT {phase}"); std::io::stdout().flush().unwrap();
        let mut line = String::new(); let _ = std::io::stdin().read_line(&mut line);
    }
}

#[cfg(windows)]
#[test]
fn crash_recovery() {
    if let Ok(path) = std::env::var("MINDTRAIL_RECOVERY_DIR") {
        let _gateway = super::lifecycle::Gateway::start(std::path::Path::new(&path)).unwrap();
        println!("RECOVERY_READY"); std::io::stdout().flush().unwrap();
        loop { std::thread::park(); }
    }
    let mut previous_ids = Vec::new();
    for phase in ["before_commit", "after_commit", "after_response"] {
        let mut random = [0u8; 16]; getrandom::fill(&mut random).unwrap();
        let suffix: String = random.iter().map(|b| format!("{b:02x}")).collect();
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailRecovery-{suffix}")); std::fs::create_dir(&dir).unwrap();
        let state_dir = dir.join("state"); let path = state_dir.join("mindtrail.sqlite3");
        let mut child = Command::new(std::env::current_exe().unwrap()).args(["--exact", "gateway::recovery::crash_recovery", "--nocapture"]).env("MINDTRAIL_RECOVERY_DIR", &state_dir).env("MINDTRAIL_RECOVERY_PHASE", phase).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit()).spawn().unwrap();
        struct ChildGuard(std::process::Child);
        impl Drop for ChildGuard { fn drop(&mut self) { if self.0.try_wait().ok().flatten().is_none() { let _ = self.0.kill(); let _ = self.0.wait(); } } }
        let mut child = ChildGuard(child);
        let stdout = child.0.stdout.take().unwrap(); let (lines_tx, lines_rx) = mpsc::channel();
        std::thread::spawn(move || for line in BufReader::new(stdout).lines() { if let Ok(line) = line { let _ = lines_tx.send(line); } });
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        let read_line = |needle: &str| loop { let remaining = deadline.saturating_duration_since(std::time::Instant::now()); let line = lines_rx.recv_timeout(remaining).expect("child checkpoint timed out"); if let Some(index) = line.find(needle) { break line[index..].to_owned(); } };
        read_line("RECOVERY_READY");
        let endpoint: serde_json::Value = serde_json::from_slice(&std::fs::read(state_dir.join("endpoint.json")).unwrap()).unwrap();
        let id = endpoint["store_id"].as_str().unwrap().to_owned();
        assert!(!previous_ids.contains(&id), "independent fixtures must have different identities"); previous_ids.push(id.clone());
        let old_token = endpoint["token"].as_str().unwrap().to_owned();
        let addr = format!("127.0.0.1:{}", endpoint["port"].as_u64().unwrap());
        let request_token = old_token.clone();
        let (response_tx, response_rx) = mpsc::channel();
        let requester = std::thread::spawn(move || {
            let result = (|| -> std::io::Result<String> { let mut stream = TcpStream::connect(&addr)?; stream.set_read_timeout(Some(Duration::from_secs(15)))?; write!(stream, "POST /v1/append HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer {request_token}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{BODY}", BODY.len())?; let mut response=String::new(); stream.read_to_string(&mut response)?; Ok(response) })();
            let _ = response_tx.send(result);
        });
        if phase == "after_response" { assert!(response_rx.recv_timeout(Duration::from_secs(15)).unwrap().unwrap().starts_with("HTTP/1.1 200")); }
        else { assert!(read_line("RECOVERY_CHECKPOINT ").contains(phase)); }
        child.0.kill().unwrap(); child.0.wait().unwrap(); requester.join().unwrap();
        let gateway = super::lifecycle::Gateway::start(&state_dir).unwrap();
        let restarted: serde_json::Value = serde_json::from_slice(&std::fs::read(state_dir.join("endpoint.json")).unwrap()).unwrap();
        assert_eq!(restarted["store_id"], id); assert_ne!(restarted["token"], old_token);
        let new_addr = format!("127.0.0.1:{}", restarted["port"].as_u64().unwrap());
        let health = |token: &str| { let mut stream = TcpStream::connect(&new_addr).unwrap(); stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap(); write!(stream,"GET /v1/health HTTP/1.1\r\nHost: {new_addr}\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n").unwrap(); let mut response=String::new(); stream.read_to_string(&mut response).unwrap(); response };
        assert!(health(&old_token).starts_with("HTTP/1.1 401")); assert!(health(restarted["token"].as_str().unwrap()).starts_with("HTTP/1.1 200"));
        let store = Store::open(&path).unwrap(); assert_eq!(store.store_id(), id);
        let (wal, synchronous) = super::store::test_writer_pragmas(&path); assert_eq!(wal.to_lowercase(), "wal"); assert_eq!(synchronous, 2);
        let input: Vec<serde_json::Value> = serde_json::from_str(BODY).unwrap();
        let expected: Vec<serde_json::Value> = input.iter().enumerate().map(|(i, item)| { let mut event=item.clone(); let object=event.as_object_mut().unwrap(); object.remove("operation_id"); object.insert("seq".into(), serde_json::json!(i+1)); event }).collect();
        if phase == "before_commit" { let error=store.events("recovery", 0, 200).unwrap_err(); assert_eq!(error.code, "not_found"); assert_eq!(error.status,404); }
        else { let page = store.events("recovery", 0, 200).unwrap(); assert_eq!(page.last_seq,3); assert_eq!(page.status,"done"); assert_eq!(page.events,expected); }
        let mut stream = TcpStream::connect(&new_addr).unwrap(); stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let token = restarted["token"].as_str().unwrap();
        write!(stream, "POST /v1/append HTTP/1.1\r\nHost: {new_addr}\r\nAuthorization: Bearer {token}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{BODY}", BODY.len()).unwrap();
        let mut response=String::new(); stream.read_to_string(&mut response).unwrap(); assert!(response.starts_with("HTTP/1.1 200"));
        let receipt: serde_json::Value = serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(receipt["last_seq"],3); assert_eq!(receipt["accepted"], if phase == "before_commit" {3} else {0}); assert_eq!(receipt["duplicates"], if phase == "before_commit" {0} else {3});
        assert_eq!(receipt["results"], serde_json::json!([{"operation_id":"one","seq":1},{"operation_id":"two","seq":2},{"operation_id":"three","seq":3}]));
        let page = store.events("recovery", 0, 200).unwrap(); assert_eq!(page.last_seq,3); assert_eq!(page.status,"done"); assert_eq!(page.events,expected);
        let connection = rusqlite::Connection::open(&path).unwrap();
        let mut stmt = connection.prepare("SELECT operation_id, seq FROM operations WHERE session='recovery' ORDER BY seq").unwrap();
        let operations = stmt.query_map([], |row| Ok((row.get::<_,String>(0)?, row.get::<_,u64>(1)?))).unwrap().collect::<Result<Vec<_>,_>>().unwrap();
        assert_eq!(operations, vec![("one".into(),1),("two".into(),2),("three".into(),3)]);
        let owners: i64 = connection.query_row("SELECT COUNT(*) FROM event_owners WHERE session='recovery'",[],|row|row.get(0)).unwrap(); assert_eq!(owners,3);
        drop(stmt); drop(connection); drop(store);
        gateway.shutdown().unwrap();
        eprintln!("recovery {phase} passed; isolated fixture retained: {}", dir.display());
    }
}

use std::{io::{Read, Write}, net::{TcpListener, TcpStream}, sync::Arc, time::Duration};
use mindtrail_lib::gateway::{http::{router, HttpState}, store::Store};
fn wait_until(predicate: impl Fn() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !predicate() { assert!(std::time::Instant::now() < deadline, "notification did not arrive"); std::thread::yield_now(); }
}

#[test]
fn loopback_guards_health_and_limits_body() {
    let mut random = [0u8; 16]; getrandom::fill(&mut random).unwrap();
    let dir = std::env::temp_dir().join(format!("mindtrail-http-{random:?}"));
    std::fs::create_dir(&dir).unwrap();
    let store = Arc::new(Store::open(&dir.join("store.sqlite")).unwrap());
    let id = store.store_id().to_owned();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap(); listener.set_nonblocking(true).unwrap();
    let state = HttpState::new(store, addr, "01234567890123456789012345678901", |_| {});
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let server = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async { axum::serve(tokio::net::TcpListener::from_std(listener).unwrap(), router(state)).with_graceful_shutdown(async { let _ = stop_rx.await; }).await.unwrap(); });
    });
    let send_method = |method: &str, headers: &str, body: &[u8]| {
        let mut s = TcpStream::connect(addr).unwrap(); s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        write!(s, "{method} /v1/health HTTP/1.1\r\n{headers}Connection: close\r\n\r\n").unwrap(); s.write_all(body).unwrap();
        let mut out = String::new(); s.read_to_string(&mut out).unwrap(); out
    };
    let send = |headers: &str, body: &[u8]| send_method("GET", headers, body);
    let auth = "Authorization: Bearer 01234567890123456789012345678901\r\n";
    let mut socket = TcpStream::connect(addr).unwrap();
    write!(socket, "GET /v1/events HTTP/1.1\r\nHost: {addr}\r\n{auth}Connection: close\r\n\r\n").unwrap();
    let mut wrong_method = String::new(); socket.read_to_string(&mut wrong_method).unwrap();
    assert!(wrong_method.starts_with("HTTP/1.1 405"));
    assert!(wrong_method.to_lowercase().contains("allow: post"));
    assert_eq!(serde_json::from_str::<serde_json::Value>(wrong_method.split("\r\n\r\n").nth(1).unwrap()).unwrap()["error"]["code"], "method_not_allowed");
    let valid = send(&format!("Host: {addr}\r\n{auth}"), &[]);
    assert!(valid.starts_with("HTTP/1.1 200"));
    let json: serde_json::Value = serde_json::from_str(valid.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(json, serde_json::json!({"version":1,"store_id":id}));
    assert!(valid.to_lowercase().contains("cache-control: no-store"));
    let unauthorized = send("Host: evil\r\nOrigin: null\r\nAuthorization: Bearer 11234567890123456789012345678901\r\n", &[]);
    assert!(unauthorized.starts_with("HTTP/1.1 401"));
    assert!(unauthorized.to_lowercase().contains("cache-control: no-store"));
    assert!(!unauthorized.to_lowercase().contains("access-control-allow-origin"));
    let method = send_method("POST", &format!("Host: {addr}\r\n{auth}"), &[]);
    assert!(method.starts_with("HTTP/1.1 405"));
    assert!(method.to_lowercase().contains("allow: get, head"));
    assert!(method.to_lowercase().contains("cache-control: no-store"));
    let json: serde_json::Value = serde_json::from_str(method.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(json["error"]["code"], "method_not_allowed");
    assert!(send(&format!("Host: {addr}\r\nHost: {addr}\r\n{auth}"), &[]).starts_with("HTTP/1.1 400"));
    assert!(send(&format!("Host: {addr}\r\n"), &[]).starts_with("HTTP/1.1 401"));
    assert!(send(&format!("Host: {addr}\r\n{auth}{auth}"), &[]).starts_with("HTTP/1.1 401"));
    assert!(send(&format!("Host: evil\r\nX-Forwarded-Host: {addr}\r\n{auth}"), &[]).starts_with("HTTP/1.1 400"));
    assert!(send(&format!("Host: {addr}\r\nOrigin: null\r\n{auth}"), &[]).starts_with("HTTP/1.1 400"));
    let mut chunk = format!("{:x}\r\n", 1024 * 1024 + 1).into_bytes(); chunk.extend(vec![b'a'; 1024 * 1024 + 1]); chunk.extend(b"\r\n0\r\n\r\n");
    assert!(send(&format!("Host: {addr}\r\n{auth}Transfer-Encoding: chunked\r\n"), &chunk).starts_with("HTTP/1.1 413"));
    stop_tx.send(()).unwrap(); server.join().unwrap(); std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn write_routes_commit_retry_and_concurrent_append() {
    use std::sync::Mutex;
    let mut random = [0u8; 16]; getrandom::fill(&mut random).unwrap();
    let dir = std::env::temp_dir().join(format!("mindtrail-http-write-{random:?}"));
    std::fs::create_dir(&dir).unwrap();
    let store = Arc::new(Store::open(&dir.join("store.sqlite")).unwrap());
    let notices = Arc::new(Mutex::new(Vec::<String>::new()));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap(); listener.set_nonblocking(true).unwrap();
    let notify = notices.clone();
    let state = HttpState::new(store.clone(), addr, "01234567890123456789012345678901", move |s| notify.lock().unwrap().push(s.to_owned()));
    let permits = state.permits();
    let held = permits.clone().try_acquire_many_owned(8).unwrap();
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let server = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async { axum::serve(tokio::net::TcpListener::from_std(listener).unwrap(), router(state)).with_graceful_shutdown(async { let _ = stop_rx.await; }).await.unwrap(); });
    });
    let request = move |path: &str, body: &str| -> (u16, serde_json::Value) {
        let mut s = TcpStream::connect(addr).unwrap(); s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        write!(s, "POST {path} HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer 01234567890123456789012345678901\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        let mut out = String::new(); s.read_to_string(&mut out).unwrap();
        (out[9..12].parse().unwrap(), serde_json::from_str(out.split("\r\n\r\n").nth(1).unwrap()).unwrap())
    };
    let read = move |path: &str| -> (u16, serde_json::Value) {
        let mut s = TcpStream::connect(addr).unwrap(); s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        write!(s, "GET {path} HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer 01234567890123456789012345678901\r\nConnection: close\r\n\r\n").unwrap();
        let mut out = String::new(); s.read_to_string(&mut out).unwrap();
        (out[9..12].parse().unwrap(), serde_json::from_str(out.split("\r\n\r\n").nth(1).unwrap()).unwrap())
    };
    let explicit = r#"[{"v":1,"session":"explicit","seq":1,"ts":"2024-01-01T00:00:00Z","type":"task.start","payload":{"title":"T","goal":"G","project":"P"}}]"#;
    let append = r#"[{"v":1,"session":"append","operation_id":"start","ts":"2024-01-01T00:00:00Z","type":"task.start","payload":{"title":"T","goal":"G","project":"P"}}]"#;
    for (path, body) in [("/v1/events", explicit), ("/v1/append", append)] {
        let (status, body) = request(path, body);
        assert_eq!(status, 503); assert_eq!(body["error"]["code"], "temporary_storage");
    }
    assert!(notices.lock().unwrap().is_empty());
    assert!(store.sessions(None, 200).unwrap().is_empty());
    assert_eq!(read("/v1/sessions").0, 503);
    assert_eq!(read("/v1/sessions/missing/events").0, 503);
    drop(held);
    for (path, body) in [("/v1/events", explicit), ("/v1/append", append)] {
        let (status, first) = request(path, body); assert_eq!(status, 200); assert_eq!(first["accepted"], 1);
        let (status, retry) = request(path, body); assert_eq!(status, 200); assert_eq!(retry["duplicates"], 1); assert_eq!(retry["accepted"], 0);
        if path == "/v1/append" { assert_eq!(first["results"], retry["results"]); }
    }
    wait_until(|| notices.lock().unwrap().len() == 2);
    assert_eq!(*notices.lock().unwrap(), vec!["explicit", "append"]);
    let (status, err) = request("/v1/events", r#"[{"v":1,"v":1}]"#); assert_eq!(status, 400); assert_eq!(err["error"]["index"], 0);
    let oversized = format!("[{}]", vec!["{}"; 201].join(",")); assert_eq!(request("/v1/events", &oversized).0, 400);
    let gap = r#"[{"v":1,"session":"explicit","seq":3,"ts":"2024-01-01T00:00:00Z","type":"task.done","payload":{}}]"#;
    let (status, err) = request("/v1/events", gap); assert_eq!(status, 409); assert_eq!(err["error"]["expected_seq"], 2);
    let mut threads = vec![];
    for i in 0..4 {
        threads.push(std::thread::spawn(move || {
            let body = format!(r#"[{{"v":1,"session":"append","operation_id":"op{i}","ts":"2024-01-01T00:00:00Z","type":"capability.intent","payload":{{"id":"c{i}","title":"T"}}}}]"#);
            request("/v1/append", &body)
        }));
    }
    let mut seqs: Vec<_> = threads.into_iter().map(|t| { let (status, body) = t.join().unwrap(); assert_eq!(status, 200); body["results"][0]["seq"].as_u64().unwrap() }).collect();
    seqs.sort(); assert_eq!(seqs, vec![2,3,4,5]); wait_until(|| notices.lock().unwrap().len() == 6);
    let fact = r#"[{"v":1,"session":"append","operation_id":"race-fact","ts":"2024-01-01T00:00:00Z","type":"capability.intent","payload":{"id":"race","title":"T"}}]"#;
    let done = r#"[{"v":1,"session":"append","operation_id":"race-done","ts":"2024-01-01T00:00:00Z","type":"task.done","payload":{}}]"#;
    let barrier = Arc::new(std::sync::Barrier::new(3));
    let fact_barrier = barrier.clone();
    let fact_thread = std::thread::spawn(move || { fact_barrier.wait(); request("/v1/append", fact) });
    let done_barrier = barrier.clone();
    let done_thread = std::thread::spawn(move || { done_barrier.wait(); request("/v1/append", done) });
    barrier.wait();
    let (fact_status, fact_receipt) = fact_thread.join().unwrap();
    let (done_status, done_receipt) = done_thread.join().unwrap();
    assert_eq!(done_status, 200);
    let expected_count = if fact_status == 200 {
        assert_eq!(fact_receipt["results"][0]["seq"], 6);
        assert_eq!(done_receipt["results"][0]["seq"], 7);
        let (status, retry) = request("/v1/append", fact);
        assert_eq!(status, 200); assert_eq!(retry["results"], fact_receipt["results"]); assert_eq!(retry["accepted"], 0);
        7
    } else {
        assert_eq!(fact_status, 409); assert_eq!(fact_receipt["error"]["code"], "task_closed");
        assert_eq!(fact_receipt["error"]["index"], 0); assert_eq!(fact_receipt["error"]["last_seq"], 6);
        assert_eq!(done_receipt["results"][0]["seq"], 6);
        6
    };
    let (status, retry) = request("/v1/append", done);
    assert_eq!(status, 200); assert_eq!(retry["results"], done_receipt["results"]); assert_eq!(retry["accepted"], 0);
    let page = store.events("append", 0, 200).unwrap();
    assert_eq!(page.events.len(), expected_count); assert_eq!(page.last_seq, expected_count as u64);
    assert_eq!(page.events.last().unwrap()["type"], "task.done");
    if expected_count == 7 { assert_eq!(page.events[5]["payload"]["id"], "race"); }
    wait_until(|| notices.lock().unwrap().len() == expected_count + 1);
    for session in ["A/B", "A%2FB", "中文", "case", "Case"] {
        let body = serde_json::json!([{"v":1,"session":session,"seq":1,"ts":"2024-01-01T00:00:00Z","type":"task.start","payload":{"title":"T","goal":"G","project":"P"}}]).to_string();
        assert_eq!(request("/v1/events", &body).0, 200);
    }
    for (path, session) in [("A%2FB", "A/B"), ("A%252FB", "A%2FB"), ("%E4%B8%AD%E6%96%87", "中文"), ("case", "case"), ("Case", "Case")] {
        let (status, body) = read(&format!("/v1/sessions/{path}/events"));
        assert_eq!(status, 200); assert_eq!(body["events"][0]["session"], session);
    }
    let (_, first) = read("/v1/sessions?limit=1");
    assert_eq!(first["sessions"].as_array().unwrap().len(), 1);
    let (_, next) = read("/v1/sessions?after_session=A%252FB&limit=1");
    assert_eq!(next["sessions"][0]["session"], "A/B");
    let (_, page) = read("/v1/sessions/append/events?since_seq=1&limit=2");
    assert_eq!(page["events"].as_array().unwrap().len(), 2); assert_eq!(page["events"][0]["seq"], 2);
    let (_, page2) = read("/v1/sessions/append/events?since_seq=3&limit=2");
    assert_eq!(page2["events"][0]["seq"], 4);
    let (status, missing) = read("/v1/sessions/missing/events");
    assert_eq!(status, 404); assert_eq!(missing["error"]["code"], "not_found");
    let mut raw = TcpStream::connect(addr).unwrap();
    write!(raw, "GET /v1/sessions/missing/events HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer 01234567890123456789012345678901\r\nConnection: close\r\n\r\n").unwrap();
    let mut response = String::new(); raw.read_to_string(&mut response).unwrap();
    assert!(response.to_lowercase().contains("cache-control: no-store"));
    let mut raw = TcpStream::connect(addr).unwrap();
    write!(raw, "GET /v1/sessions/A/B/events HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer 01234567890123456789012345678901\r\nConnection: close\r\n\r\n").unwrap();
    let mut response = String::new(); raw.read_to_string(&mut response).unwrap(); assert!(response.starts_with("HTTP/1.1 404"));
    let (status, terminal) = read("/v1/sessions/append/events?since_seq=9007199254740991");
    assert_eq!(status, 200); assert_eq!(terminal["events"], serde_json::json!([]));
    assert_eq!(terminal["last_seq"], expected_count); assert_eq!(terminal["status"], "done");
    assert_eq!(read("/v1/sessions/append/events?since_seq=18446744073709551616").0, 400);
    assert_eq!(request("/v1/sessions", "").0, 405);
    assert_eq!(request("/v1/sessions/append/events", "").0, 405);
    for path in ["/v1/sessions?limit=0", "/v1/sessions?limit=201", "/v1/sessions?limit=1&%6cimit=2", "/v1/sessions?after_session=%ZZ", "/v1/sessions?after_session=%FF", "/v1/sessions/append/events?since_seq=9007199254740992", "/v1/sessions/append/events?since_seq=-1", "/v1/sessions/%ZZ/events", "/v1/sessions/%FF/events"] {
        assert_eq!(read(path).0, 400, "{path}");
    }
    stop_tx.send(()).unwrap(); server.join().unwrap(); std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn body_budget_timeout_and_auth_priority() {
    let mut random = [0u8; 16]; getrandom::fill(&mut random).unwrap();
    let dir = std::env::temp_dir().join(format!("mindtrail-http-budget-{random:?}")); std::fs::create_dir(&dir).unwrap();
    let store = Arc::new(Store::open(&dir.join("store.sqlite")).unwrap());
    let listener = TcpListener::bind("127.0.0.1:0").unwrap(); let addr = listener.local_addr().unwrap(); listener.set_nonblocking(true).unwrap();
    let mut state = HttpState::new(store, addr, "01234567890123456789012345678901", |_| {});
    state.body_permits = Arc::new(tokio::sync::Semaphore::new(1)); state.body_timeout = Duration::from_secs(2);
    let budget = state.body_permits.clone(); let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let server = std::thread::spawn(move || { let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap(); rt.block_on(async { axum::serve(tokio::net::TcpListener::from_std(listener).unwrap(), router(state)).with_graceful_shutdown(async { let _ = stop_rx.await; }).await.unwrap(); }); });
    let mut slow = TcpStream::connect(addr).unwrap(); slow.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write!(slow, "POST /v1/events HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer 01234567890123456789012345678901\r\nContent-Length: 100\r\nConnection: close\r\n\r\n[").unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(1);
    while budget.available_permits() != 0 { assert!(std::time::Instant::now() < deadline); std::thread::yield_now(); }
    for (auth, expected) in [("Bearer 01234567890123456789012345678901", "503"), ("Bearer wrong", "401")] {
        let mut s = TcpStream::connect(addr).unwrap(); s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        write!(s, "GET /v1/health HTTP/1.1\r\nHost: {addr}\r\nAuthorization: {auth}\r\nConnection: close\r\n\r\n").unwrap();
        let mut response = String::new(); s.read_to_string(&mut response).unwrap(); assert_eq!(&response[9..12], expected); assert!(response.to_lowercase().contains("cache-control: no-store"));
    }
    let mut response = String::new(); slow.read_to_string(&mut response).unwrap(); assert!(response.starts_with("HTTP/1.1 408"));
    assert!(response.to_lowercase().contains("cache-control: no-store"));
    stop_tx.send(()).unwrap(); server.join().unwrap(); std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn sqlite_lock_retry_and_panicking_notification() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let mut random = [0u8; 16]; getrandom::fill(&mut random).unwrap();
    let dir = std::env::temp_dir().join(format!("mindtrail-http-lock-{random:?}")); std::fs::create_dir(&dir).unwrap();
    let path = dir.join("store.sqlite"); let store = Arc::new(Store::open(&path).unwrap());
    let listener = TcpListener::bind("127.0.0.1:0").unwrap(); let addr = listener.local_addr().unwrap(); listener.set_nonblocking(true).unwrap();
    let calls = Arc::new(AtomicUsize::new(0)); let notify = calls.clone();
    let state = HttpState::new(store.clone(), addr, "01234567890123456789012345678901", move |_| { notify.fetch_add(1, Ordering::SeqCst); panic!("notification failure"); });
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let server = std::thread::spawn(move || { let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap(); rt.block_on(async { axum::serve(tokio::net::TcpListener::from_std(listener).unwrap(), router(state)).with_graceful_shutdown(async { let _ = stop_rx.await; }).await.unwrap(); }); });
    let request = |route: &str, body: &str| {
        let mut s = TcpStream::connect(addr).unwrap(); s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        write!(s, "POST {route} HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer 01234567890123456789012345678901\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        let mut out = String::new(); s.read_to_string(&mut out).unwrap(); (out[9..12].parse::<u16>().unwrap(), serde_json::from_str::<serde_json::Value>(out.split("\r\n\r\n").nth(1).unwrap()).unwrap())
    };
    let explicit = r#"[{"v":1,"session":"locked","seq":1,"ts":"2024-01-01T00:00:00Z","type":"task.start","payload":{"title":"T","goal":"G","project":"P"}}]"#;
    let conn = rusqlite::Connection::open(&path).unwrap(); conn.execute_batch("BEGIN IMMEDIATE").unwrap();
    let (status, body) = request("/v1/events", explicit); assert_eq!(status, 503); assert_eq!(body["error"]["code"], "temporary_storage"); assert_eq!(calls.load(Ordering::SeqCst), 0); assert!(store.sessions(None, 200).unwrap().is_empty());
    conn.execute_batch("ROLLBACK").unwrap(); drop(conn);
    let (status, body) = request("/v1/events", explicit); assert_eq!(status, 200); assert_eq!(body["accepted"], 1); wait_until(|| calls.load(Ordering::SeqCst) == 1); assert_eq!(store.events("locked", 0, 200).unwrap().last_seq, 1);
    let (status, body) = request("/v1/events", explicit); assert_eq!(status, 200); assert_eq!(body["duplicates"], 1); assert_eq!(calls.load(Ordering::SeqCst), 1);
    let append = r#"[{"v":1,"session":"append-panic","operation_id":"start","ts":"2024-01-01T00:00:00Z","type":"task.start","payload":{"title":"T","goal":"G","project":"P"}}]"#;
    let (status, body) = request("/v1/append", append); assert_eq!(status, 200); assert_eq!(body["accepted"], 1); wait_until(|| calls.load(Ordering::SeqCst) == 2);
    stop_tx.send(()).unwrap(); server.join().unwrap(); std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn blocked_notifications_do_not_block_requests_or_shutdown() {
    assert!(cfg!(panic = "unwind"), "release builds must unwind callback panics");
    let release = include_str!("../Cargo.toml").split("[profile.release]").nth(1).unwrap().split('[').next().unwrap();
    assert!(release.lines().any(|line| line.trim() == "panic = \"unwind\""));
    let mut random = [0u8; 16]; getrandom::fill(&mut random).unwrap();
    let dir = std::env::temp_dir().join(format!("mindtrail-blocked-notify-{random:?}")); std::fs::create_dir(&dir).unwrap();
    let store = Arc::new(Store::open(&dir.join("store.sqlite")).unwrap());
    let listener = TcpListener::bind("127.0.0.1:0").unwrap(); let addr = listener.local_addr().unwrap(); listener.set_nonblocking(true).unwrap();
    let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(1); let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let release = std::sync::Mutex::new(release_rx);
    let state = HttpState::new(store, addr, "01234567890123456789012345678901", move |_| { let _ = entered_tx.try_send(()); let _ = release.lock().unwrap().recv(); });
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let server = std::thread::spawn(move || { let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap(); rt.block_on(async { axum::serve(tokio::net::TcpListener::from_std(listener).unwrap(), router(state)).with_graceful_shutdown(async { let _ = stop_rx.await; }).await.unwrap(); }); });
    let request = |method: &str, path: &str, body: &str| {
        let mut s = TcpStream::connect(addr).unwrap(); s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        write!(s, "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer 01234567890123456789012345678901\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        let mut out = String::new(); s.read_to_string(&mut out).unwrap(); out[9..12].parse::<u16>().unwrap()
    };
    for i in 0..67 {
        let body = serde_json::json!([{"v":1,"session":format!("s{i}"),"seq":1,"ts":"2024-01-01T00:00:00Z","type":"task.start","payload":{"title":"T","goal":"G","project":"P"}}]).to_string();
        assert_eq!(request("POST", "/v1/events", &body), 200);
        if i == 0 { entered_rx.recv_timeout(Duration::from_secs(5)).unwrap(); }
    }
    assert_eq!(request("GET", "/v1/health", ""), 200);
    assert_eq!(request("GET", "/v1/sessions?limit=1", ""), 200);
    stop_tx.send(()).unwrap(); server.join().unwrap();
    drop(release_tx); std::fs::remove_dir_all(dir).unwrap();
}

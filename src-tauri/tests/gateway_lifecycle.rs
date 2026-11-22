#![cfg(windows)]

use mindtrail_lib::gateway::lifecycle::Gateway;
use std::{fs, io::{self, Read, Write}, net::{Ipv4Addr, SocketAddr, TcpStream}, os::windows::{fs::{MetadataExt, OpenOptionsExt}, io::AsRawHandle}, path::{Path, PathBuf}, process::{Child, Command, Stdio}, time::{Duration, Instant}};
use windows_sys::Win32::{Foundation::HANDLE, Storage::FileSystem::{FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_ID_INFO, FILE_READ_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, FileIdInfo, GetFileInformationByHandleEx}};

const ROLE: &str = "MINDTRAIL_LIFECYCLE_ROLE";
const ROOT: &str = "MINDTRAIL_LIFECYCLE_ROOT";
const MARKER: &str = "MINDTRAIL_LIFECYCLE_MARKER";
const PROOF: &str = "MINDTRAIL_LIFECYCLE_PROOF";
const TIMEOUT: Duration = Duration::from_secs(15);

fn fixture_guard(root: &Path) -> io::Result<(fs::File, (u64, [u8; 16]))> {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"));
    let name = root.file_name().and_then(|v| v.to_str()).ok_or_else(|| io::Error::other("invalid fixture name"))?;
    let suffix = name.strip_prefix("MindTrailLifecycle-").ok_or_else(|| io::Error::other("invalid fixture name"))?;
    if suffix.len() != 32 || !suffix.bytes().all(|b| b.is_ascii_hexdigit()) || root != workspace.join(name) {
        return Err(io::Error::other("fixture must be a direct, randomly named workspace child"));
    }
    let metadata = fs::symlink_metadata(root)?;
    if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(io::Error::other("fixture root is not a plain directory"));
    }
    let canonical = fs::canonicalize(root)?;
    if canonical.parent() != Some(fs::canonicalize(workspace)?.as_path()) {
        return Err(io::Error::other("fixture root escaped workspace"));
    }
    let handle = fs::OpenOptions::new().access_mode(FILE_READ_ATTRIBUTES).share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT).open(root)?;
    let mut id = FILE_ID_INFO::default();
    if unsafe { GetFileInformationByHandleEx(handle.as_raw_handle() as HANDLE, FileIdInfo, (&mut id as *mut FILE_ID_INFO).cast(), std::mem::size_of_val(&id) as u32) } == 0 { return Err(io::Error::last_os_error()); }
    Ok((handle, (id.VolumeSerialNumber, id.FileId.Identifier)))
}
fn fixed_marker(value: &std::ffi::OsStr) -> io::Result<&'static str> {
    match value.to_str() {
        Some("ready-first") => Ok("ready-first"),
        Some("ready-second") => Ok("ready-second"),
        _ => Err(io::Error::other("invalid fixture marker")),
    }
}
fn assert_withdrawn(path: &Path) {
    match fs::metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {},
        Err(error) => panic!("endpoint query failed: {error}"),
        Ok(_) => panic!("endpoint was not withdrawn"),
    }
}
// The helper is this test binary itself, so no test-only control is shipped in
// the application. The production entry exposes only ordinary start/shutdown.
#[test]
fn lifecycle_child() {
    let Ok(role) = std::env::var(ROLE) else { return; };
    let root = PathBuf::from(std::env::var_os(ROOT).expect("fixture root"));
    let (_root_guard, _identity) = fixture_guard(&root).expect("invalid fixture root");
    let proof = std::env::var(PROOF).expect("fixture proof");
    let proof_path = root.join(".fixture-proof");
    let mut proof_file = fs::OpenOptions::new().read(true).share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT).open(&proof_path).expect("fixture proof missing");
    let proof_metadata = proof_file.metadata().expect("fixture proof metadata");
    assert!(proof_metadata.is_file() && proof_metadata.len() == 32 && proof_metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0, "invalid fixture proof");
    assert!(proof.len() == 32 && proof.bytes().all(|byte| byte.is_ascii_hexdigit()));
    let mut proof_bytes = [0u8; 32]; proof_file.read_exact(&mut proof_bytes).unwrap();
    assert!(proof_bytes == proof.as_bytes(), "fixture proof mismatch");
    let marker_arg = std::env::var_os(MARKER).expect("fixture marker");
    let marker = if role == "contend" || role == "broken-store" {
        assert!(marker_arg == "unused", "invalid fixture marker"); None
    } else {
        assert!(role == "serve" || role == "drop", "invalid fixture role");
        Some(fixed_marker(&marker_arg).expect("invalid fixture marker"))
    };
    let state = root.join("state");
    if role == "contend" || role == "broken-store" {
        let error = match Gateway::start(&state) { Ok(_) => panic!("second process acquired the state directory"), Err(error) => error };
        let stage = if role == "contend" { "Gateway lock failed:" } else { "Gateway store failed:" };
        assert!(error.starts_with(stage), "unexpected startup stage: {error}");
        if role == "broken-store" {
            fs::OpenOptions::new().write(true).create_new(true).open(root.join("broken-returned")).unwrap().write_all(b"done").unwrap();
            let mut line = String::new();
            let _ = std::io::stdin().read_line(&mut line);
        }
        return;
    }
    let gateway = Gateway::start(&state).expect("start fixture gateway");
    fs::OpenOptions::new().write(true).create_new(true).open(root.join(marker.unwrap())).unwrap().write_all(b"ready").unwrap();
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
    if role == "drop" {
        drop(gateway);
        fs::OpenOptions::new().write(true).create_new(true).open(root.join("drop-complete")).unwrap().write_all(b"done").unwrap();
        line.clear();
        let _ = std::io::stdin().read_line(&mut line);
    } else {
        gateway.shutdown().expect("stop fixture gateway");
    }
}

struct Fixture { root: PathBuf, proof: String, guard: Option<fs::File>, identity: (u64, [u8; 16]), children: Vec<Child> }
impl Fixture {
    fn new() -> Self {
        let mut random = [0u8; 16]; getrandom::fill(&mut random).unwrap();
        let suffix: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailLifecycle-{suffix}"));
        fs::create_dir(&root).unwrap();
        let (guard, identity) = fixture_guard(&root).unwrap();
        let mut proof_bytes = [0u8; 16]; getrandom::fill(&mut proof_bytes).unwrap();
        let proof: String = proof_bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        fs::OpenOptions::new().write(true).create_new(true).open(root.join(".fixture-proof")).unwrap().write_all(proof.as_bytes()).unwrap();
        Self { root, proof, guard: Some(guard), identity, children: Vec::new() }
    }
    fn spawn(&mut self, role: &str, marker: &str) -> usize {
        let child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "lifecycle_child", "--nocapture"])
            .env(ROLE, role).env(ROOT, &self.root).env(MARKER, marker).env(PROOF, &self.proof)
            .stdin(Stdio::piped()).spawn().unwrap();
        self.children.push(child); self.children.len() - 1
    }
    fn wait_ready(&mut self, child: usize, marker: &str) {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if self.root.join(marker).is_file() { return; }
            if let Some(status) = self.children[child].try_wait().unwrap() { panic!("gateway fixture exited before readiness: {status}"); }
            assert!(Instant::now() < deadline, "gateway fixture readiness timed out");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    fn wait_exit(&mut self, child: usize) {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if let Some(status) = self.children[child].try_wait().unwrap() { assert!(status.success(), "fixture exited: {status}"); return; }
            assert!(Instant::now() < deadline, "gateway fixture exit timed out");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    fn send(&mut self, child: usize) {
        self.children[child].stdin.as_mut().unwrap().write_all(b"stop\n").unwrap();
    }
    fn stop(&mut self, child: usize) {
        self.send(child);
        self.children[child].stdin.take();
        self.wait_exit(child);
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let mut all_exited = true;
        for child in &mut self.children {
            if child.try_wait().ok().flatten().is_none() {
                let _ = child.kill();
                let deadline = Instant::now() + Duration::from_secs(5);
                while child.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(10));
                }
                if child.try_wait().ok().flatten().is_none() { all_exited = false; }
            }
        }
        if !all_exited { eprintln!("lifecycle fixture retained: child exit was not confirmed"); return; }
        match fixture_guard(&self.root) {
            Ok((_guard, identity)) if identity == self.identity => {},
            _ => { eprintln!("lifecycle fixture retained: root identity changed"); return; }
        }
        // The retained handle identifies the directory, but recursive deletion
        // by name after releasing it would permit a replacement race. Keep this
        // nonempty fixture until identity-bound recursive cleanup is available.
        eprintln!("lifecycle fixture retained (identity-bound recursive cleanup unavailable): {}", self.root.display());
    }
}

struct Endpoint { address: SocketAddr, token: String, store_id: String }
fn discover(root: &Path) -> Endpoint {
    let data: serde_json::Value = serde_json::from_slice(&fs::read(root.join("state/endpoint.json")).unwrap()).unwrap();
    assert_eq!(data["version"].as_u64(), Some(1));
    let port = u16::try_from(data["port"].as_u64().unwrap()).unwrap();
    let token = data["token"].as_str().unwrap().to_owned();
    let store_id = data["store_id"].as_str().unwrap().to_owned();
    assert_eq!(token.len(), 64); assert_eq!(store_id.len(), 64);
    Endpoint { address: SocketAddr::from((Ipv4Addr::LOCALHOST, port)), token, store_id }
}
fn request(endpoint: &Endpoint, method: &str, path: &str, token: &str, body: &str) -> (u16, serde_json::Value) {
    let mut stream = TcpStream::connect_timeout(&endpoint.address, Duration::from_secs(5)).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    stream.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
    write!(stream, "{method} {path} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {token}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", endpoint.address, body.len()).unwrap();
    let mut response = Vec::new(); stream.read_to_end(&mut response).unwrap();
    let response = String::from_utf8(response).unwrap();
    let (header, body) = response.split_once("\r\n\r\n").unwrap();
    let status = header.lines().next().unwrap().split_whitespace().nth(1).unwrap().parse().unwrap();
    (status, serde_json::from_str(body).unwrap())
}

#[test]
fn fixture_paths_reject_escape_and_marker_replacement() {
    for marker in ["../outside", "C:\\outside", "ready-first/../outside", "", "ready-third"] {
        assert!(fixed_marker(std::ffi::OsStr::new(marker)).is_err());
    }
    assert!(fixture_guard(Path::new("C:\\outside\\MindTrailLifecycle-00000000000000000000000000000000")).is_err());
    let fixture = Fixture::new();
    let marker = fixture.root.join("ready-first");
    fs::write(&marker, b"sentinel").unwrap();
    assert!(fs::OpenOptions::new().write(true).create_new(true).open(&marker).is_err());
    assert_eq!(fs::read(marker).unwrap(), b"sentinel");
}
#[test]
fn drop_withdraws_and_releases_lock_before_process_exit() {
    let mut fixture = Fixture::new();
    let child = fixture.spawn("drop", "ready-first"); fixture.wait_ready(child, "ready-first");
    fixture.send(child);
    fixture.wait_ready(child, "drop-complete");
    assert!(fixture.children[child].try_wait().unwrap().is_none(), "fixture process exited before lock verification");
    assert_withdrawn(&fixture.root.join("state/endpoint.json"));
    let reopened = Gateway::start(&fixture.root.join("state")).expect("lock should be free after Drop returns");
    reopened.shutdown().unwrap();
    assert!(fixture.children[child].try_wait().unwrap().is_none());
    fixture.stop(child);
    assert_withdrawn(&fixture.root.join("state/endpoint.json"));
}
#[test]
fn startup_failure_after_lock_releases_directory() {
    let mut fixture = Fixture::new();
    let first = fixture.spawn("serve", "ready-first"); fixture.wait_ready(first, "ready-first");
    fixture.stop(first);
    assert_withdrawn(&fixture.root.join("state/endpoint.json"));
    let database = fixture.root.join("state/mindtrail.sqlite3");
    let original = fs::read(&database).unwrap();
    fs::write(&database, b"deliberately invalid test database").unwrap();
    let broken = fixture.spawn("broken-store", "unused"); fixture.wait_ready(broken, "broken-returned");
    assert!(fixture.children[broken].try_wait().unwrap().is_none(), "failed-start process exited before lock verification");
    assert_withdrawn(&fixture.root.join("state/endpoint.json"));
    fs::write(&database, original).unwrap();
    let second = fixture.spawn("serve", "ready-second"); fixture.wait_ready(second, "ready-second");
    assert!(fixture.children[broken].try_wait().unwrap().is_none(), "failed-start process exited during lock verification");
    fixture.stop(second);
    fixture.stop(broken);
    assert_withdrawn(&fixture.root.join("state/endpoint.json"));
}
#[test]
fn isolated_lifecycle_authentication_exclusivity_and_restart() {
    let mut fixture = Fixture::new();
    let first = fixture.spawn("serve", "ready-first"); fixture.wait_ready(first, "ready-first");
    let initial = discover(&fixture.root);
    let (status, health) = request(&initial, "GET", "/v1/health", &initial.token, "");
    assert_eq!(status, 200); assert_eq!(health["store_id"].as_str(), Some(initial.store_id.as_str()));
    assert_eq!(request(&initial, "GET", "/v1/health", &"0".repeat(64), "").0, 401);
    let event = r#"[{"v":1,"session":"lifecycle","seq":1,"ts":"2024-01-01T00:00:00Z","type":"task.start","payload":{"title":"T","goal":"G","project":"P"}}]"#;
    let (status, written) = request(&initial, "POST", "/v1/events", &initial.token, event);
    assert_eq!(status, 200); assert_eq!(written["accepted"], 1);
    let (status, events) = request(&initial, "GET", "/v1/sessions/lifecycle/events", &initial.token, "");
    assert_eq!(status, 200); assert_eq!(events["events"].as_array().unwrap().len(), 1);
    let contender = fixture.spawn("contend", "unused"); fixture.wait_exit(contender);
    fixture.stop(first);
    assert_withdrawn(&fixture.root.join("state/endpoint.json"));
    let second = fixture.spawn("serve", "ready-second"); fixture.wait_ready(second, "ready-second");
    let restarted = discover(&fixture.root);
    assert_eq!(restarted.store_id, initial.store_id);
    assert!(restarted.token != initial.token, "restart reused the discovery token");
    assert_eq!(request(&restarted, "GET", "/v1/health", &initial.token, "").0, 401);
    let (status, events) = request(&restarted, "GET", "/v1/sessions/lifecycle/events", &restarted.token, "");
    assert_eq!(status, 200); assert_eq!(events["events"].as_array().unwrap().len(), 1);
    fixture.stop(second);
    assert_withdrawn(&fixture.root.join("state/endpoint.json"));
}








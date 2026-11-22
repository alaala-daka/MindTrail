use std::{net::SocketAddr, sync::Arc};
use axum::{body::{Body, Bytes, to_bytes}, extract::{OriginalUri, Request, State}, http::{header, HeaderValue, StatusCode, Uri}, middleware::{self, Next}, response::{IntoResponse, Response}, routing::{get, post}, Json, Router};
use serde_json::json;
use subtle::ConstantTimeEq;
use tokio::sync::Semaphore;
use super::store::{Store, SessionSummary, EventPage};
use super::access::StoreAccess;
use crate::protocol::{decode_append, decode_explicit, ProtocolError};

#[derive(Clone)]
pub struct HttpState {
    access: StoreAccess,
    admission: Arc<std::sync::atomic::AtomicBool>,
    store_id: String,
    pub address: SocketAddr,
    pub token: Arc<str>,
    notify: std::sync::mpsc::SyncSender<String>,
    pub body_permits: Arc<Semaphore>,
    pub body_timeout: std::time::Duration,
}
impl HttpState {
    pub(super) fn admission(&self) -> Arc<std::sync::atomic::AtomicBool> { self.admission.clone() }
    /// Returns the shared storage work limiter for inspection or controlled tests.
    pub fn permits(&self) -> Arc<Semaphore> { self.access.permits() }
    pub fn new(store: Arc<Store>, address: SocketAddr, token: impl Into<Arc<str>>, notify: impl Fn(&str) + Send + Sync + 'static) -> Self {
        Self::new_with_spawner(store, address, token.into(), notify, |worker| {
            std::thread::Builder::new().name("mindtrail-notify".into()).spawn(worker).map(|_| ())
        })
    }
    fn new_with_spawner(store: Arc<Store>, address: SocketAddr, token: Arc<str>, notify: impl Fn(&str) + Send + Sync + 'static, spawn: impl FnOnce(Box<dyn FnOnce() + Send>) -> std::io::Result<()>) -> Self {
        let store_id = store.store_id().to_owned();
        Self::with_access_and_spawner(StoreAccess::new(store), store_id, address, token, notify, spawn)
    }
    pub(super) fn with_access(access: StoreAccess, store_id: String, address: SocketAddr, token: impl Into<Arc<str>>, notify: impl Fn(&str) + Send + Sync + 'static) -> Self {
        Self::with_access_and_spawner(access, store_id, address, token.into(), notify, |worker| std::thread::Builder::new().name("mindtrail-notify".into()).spawn(worker).map(|_| ()))
    }
    fn with_access_and_spawner(access: StoreAccess, store_id: String, address: SocketAddr, token: Arc<str>, notify: impl Fn(&str) + Send + Sync + 'static, spawn: impl FnOnce(Box<dyn FnOnce() + Send>) -> std::io::Result<()>) -> Self {
        let (sender, receiver) = std::sync::mpsc::sync_channel::<String>(64);
        // Detached worker: a stuck callback cannot block shutdown or accumulate workers.
        // If thread creation fails, dropping the worker disconnects the channel and
        // best-effort notifications are discarded without affecting commits.
        let _ = spawn(Box::new(move || {
            while let Ok(session) = receiver.recv() {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| notify(&session)));
            }
        }));
        Self { access, admission: Arc::new(std::sync::atomic::AtomicBool::new(true)), store_id, address, token, notify: sender, body_permits: Arc::new(Semaphore::new(8)), body_timeout: std::time::Duration::from_secs(5) }
    }
}
#[derive(Clone)]
pub struct ReadService { access: StoreAccess }
impl ReadService {
    pub(super) fn from_access(access: StoreAccess) -> Self { Self { access } }
    pub fn new(state: &HttpState) -> Self { Self { access: state.access.clone() } }
    pub async fn sessions(&self, after: Option<String>, limit: u16) -> Result<Vec<SessionSummary>, ProtocolError> {
        self.access.run(move |store| store.sessions(after.as_deref(), limit)).await
    }
    pub async fn events(&self, session: String, since: u64, limit: u16) -> Result<EventPage, ProtocolError> {
        self.access.run(move |store| store.events(&session, since, limit)).await
    }
    pub(crate) async fn semantic_prefix(&self, session: String, prefix: u64) -> Result<(String, Vec<serde_json::Value>), ProtocolError> {
        self.access.run(move |store| Ok((store.store_id().to_owned(), store.semantic_prefix(&session, prefix)?))).await
    }
}
fn error(status: StatusCode, code: &'static str, message: &'static str) -> Response {
    (status, Json(json!({"error": {"code": code, "message": message}}))).into_response()
}
async fn guard(State(state): State<HttpState>, request: Request, next: Next) -> Response {
    if !state.admission.load(std::sync::atomic::Ordering::Acquire) {
        return error(StatusCode::SERVICE_UNAVAILABLE, "temporary_storage", "Storage temporarily unavailable");
    }
    let headers = request.headers();
    let mut auth = headers.get_all(header::AUTHORIZATION).iter();
    let authorized = match (auth.next(), auth.next()) {
        (Some(value), None) => value.to_str().ok().and_then(|s| s.strip_prefix("Bearer ")).is_some_and(|s| {
            s.len() == state.token.len() && bool::from(s.as_bytes().ct_eq(state.token.as_bytes()))
        }),
        _ => false,
    };
    let mut hosts = headers.get_all(header::HOST).iter();
    let valid_host = matches!((hosts.next(), hosts.next()), (Some(host), None) if host.as_bytes() == state.address.to_string().as_bytes());
    let mut response = if !authorized {
        error(StatusCode::UNAUTHORIZED, "unauthorized", "Unauthorized")
    } else if !valid_host || headers.contains_key(header::ORIGIN) {
        error(StatusCode::BAD_REQUEST, "invalid_request", "Invalid request")
    } else {
        let permit = state.body_permits.clone().try_acquire_owned();
        if let Ok(_permit) = permit {
        let (parts, body) = request.into_parts();
        match tokio::time::timeout(state.body_timeout, to_bytes(body, 1024 * 1024)).await {
            Ok(Ok(bytes)) => next.run(Request::from_parts(parts, Body::from(bytes))).await,
            Ok(Err(err)) => body_error(&err),
            Err(_) => error(StatusCode::REQUEST_TIMEOUT, "request_timeout", "Request body timed out"),
        }
        } else { error(StatusCode::SERVICE_UNAVAILABLE, "busy", "Server busy") }
    };
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}
fn body_error(err: &axum::Error) -> Response {
    use std::error::Error;
    let mut cause: Option<&(dyn Error + 'static)> = Some(err);
    while let Some(current) = cause {
        if current.is::<http_body_util::LengthLimitError>() {
            return error(StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large", "Request body too large");
        }
        cause = current.source();
    }
    error(StatusCode::BAD_REQUEST, "invalid_request", "Invalid request body")
}
async fn method_not_allowed() -> Response {
    let mut response = error(StatusCode::METHOD_NOT_ALLOWED, "method_not_allowed", "Method not allowed");
    response.headers_mut().insert(header::ALLOW, HeaderValue::from_static("GET, HEAD"));
    response
}
async fn post_method_not_allowed() -> Response {
    let mut response = error(StatusCode::METHOD_NOT_ALLOWED, "method_not_allowed", "Method not allowed");
    response.headers_mut().insert(header::ALLOW, HeaderValue::from_static("POST"));
    response
}
fn protocol_error(err: ProtocolError) -> Response {
    let status = StatusCode::from_u16(err.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let message = match err.status() { 400 => "Invalid request", 413 => "Request body too large", 503 => "Storage temporarily unavailable", _ => err.code };
    let mut details = json!({"code": err.code, "message": message});
    let object = details.as_object_mut().unwrap();
    for (key, value) in [("index", err.index.map(|v| v as u64)), ("last_seq", err.last_seq), ("expected_seq", err.expected_seq), ("offending_seq", err.offending_seq)] {
        if let Some(value) = value { object.insert(key.into(), value.into()); }
    }
    (status, Json(json!({"error": details}))).into_response()
}
async fn write(State(state): State<HttpState>, body: Bytes, append: bool) -> Response {
    enum Batch { Explicit(crate::protocol::ExplicitBatch), Append(crate::protocol::AppendBatch) }
    let batch = if append { decode_append(&body).map(Batch::Append) } else { decode_explicit(&body).map(Batch::Explicit) };
    let batch = match batch { Ok(batch) => batch, Err(err) => return protocol_error(err) };
    let notify = state.notify.clone();
    let result = state.access.run(move |store| {
        match batch {
            Batch::Explicit(batch) => {
                let session = batch.items()[0].session();
                let receipt = store.write_explicit(&batch)?;
                if receipt.accepted > 0 { let _ = notify.try_send(session.to_owned()); }
                Ok(json!({"accepted":receipt.accepted,"duplicates":receipt.duplicates,"last_seq":receipt.last_seq}))
            }
            Batch::Append(batch) => {
                let session = batch.items()[0].session();
                let receipt = store.write_append(&batch)?;
                if receipt.accepted > 0 { let _ = notify.try_send(session.to_owned()); }
                let results: Vec<_> = receipt.results.into_iter().map(|r| json!({"operation_id":r.operation_id,"seq":r.seq})).collect();
                Ok(json!({"accepted":receipt.accepted,"duplicates":receipt.duplicates,"last_seq":receipt.last_seq,"results":results}))
            }
        }
    }).await;
    match result { Ok(receipt) => Json(receipt).into_response(), Err(err) => protocol_error(err) }
}
async fn explicit(State(state): State<HttpState>, body: Bytes) -> Response { write(State(state), body, false).await }
async fn append(State(state): State<HttpState>, body: Bytes) -> Response { write(State(state), body, true).await }
fn decode_component(input: &str, query: bool) -> Option<String> {
    let bytes = input.as_bytes(); let mut out = Vec::with_capacity(bytes.len()); let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                if i + 2 >= bytes.len() { return None; }
                let hex = |c: u8| -> Option<u8> { match c { b'0'..=b'9' => Some(c-b'0'), b'a'..=b'f' => Some(c-b'a'+10), b'A'..=b'F' => Some(c-b'A'+10), _ => None } };
                out.push(hex(bytes[i+1])? * 16 + hex(bytes[i+2])?); i += 3;
            }
            b'+' if query => { out.push(b' '); i += 1; }
            b => { out.push(b); i += 1; }
        }
    }
    String::from_utf8(out).ok()
}
fn parameters(uri: &Uri, allowed: &[&str]) -> Option<std::collections::HashMap<String, String>> {
    let mut values = std::collections::HashMap::new();
    if let Some(query) = uri.query() {
        if !query.is_empty() { for pair in query.split('&') {
            let (key, value) = pair.split_once('=')?;
            let key = decode_component(key, true)?; let value = decode_component(value, true)?;
            if !allowed.contains(&key.as_str()) || values.insert(key, value).is_some() { return None; }
        }}
    }
    Some(values)
}
fn number(value: Option<&String>, default: u64, max: u64, min: u64) -> Option<u64> {
    match value { None => Some(default), Some(s) if !s.is_empty() && s.bytes().all(|c| c.is_ascii_digit()) => s.parse::<u64>().ok().filter(|n| *n >= min && *n <= max), _ => None }
}
fn invalid_query() -> Response { error(StatusCode::BAD_REQUEST, "invalid_request", "Invalid request") }
async fn sessions(State(state): State<HttpState>, OriginalUri(uri): OriginalUri) -> Response {
    let Some(params) = parameters(&uri, &["after_session", "limit"]) else { return invalid_query(); };
    let Some(limit) = number(params.get("limit"), 200, 200, 1) else { return invalid_query(); };
    match ReadService::new(&state).sessions(params.get("after_session").cloned(), limit as u16).await {
        Ok(sessions) => Json(json!({"sessions": sessions})).into_response(), Err(err) => protocol_error(err)
    }
}
async fn session_events(State(state): State<HttpState>, OriginalUri(uri): OriginalUri) -> Response {
    let Some(segment) = uri.path().strip_prefix("/v1/sessions/").and_then(|p| p.strip_suffix("/events")) else { return invalid_query(); };
    if segment.is_empty() || segment.contains('/') { return invalid_query(); }
    let Some(session) = decode_component(segment, false) else { return invalid_query(); };
    let Some(params) = parameters(&uri, &["since_seq", "limit"]) else { return invalid_query(); };
    let Some(limit) = number(params.get("limit"), 200, 200, 1) else { return invalid_query(); };
    let Some(since) = number(params.get("since_seq"), 0, crate::protocol::MAX_SAFE_INTEGER, 0) else { return invalid_query(); };
    match ReadService::new(&state).events(session, since, limit as u16).await {
        Ok(page) => Json(page).into_response(), Err(err) => protocol_error(err)
    }
}
async fn health(State(state): State<HttpState>) -> impl IntoResponse {
    Json(json!({"version": 1, "store_id": state.store_id}))
}
pub fn router(state: HttpState) -> Router {
    Router::new().route("/v1/health", get(health).fallback(method_not_allowed))
        .route("/v1/events", post(explicit).fallback(post_method_not_allowed))
        .route("/v1/append", post(append).fallback(post_method_not_allowed))
        .route("/v1/sessions", get(sessions).fallback(method_not_allowed))
        .route("/v1/sessions/{session}/events", get(session_events).fallback(method_not_allowed))
        .layer(middleware::from_fn_with_state(state.clone(), guard)).with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn states_using_same_access_share_limit() {
        let mut random = [0u8; 16]; getrandom::fill(&mut random).unwrap();
        let path = std::env::temp_dir().join(format!("mindtrail-shared-limit-{random:?}.sqlite"));
        let store = Arc::new(Store::open(&path).unwrap());
        let id = store.store_id().to_owned();
        let access = StoreAccess::new(store);
        let address = "127.0.0.1:1".parse().unwrap();
        let first = HttpState::with_access(access.clone(), id.clone(), address, "token", |_| {});
        let second = HttpState::with_access(access.clone(), id, address, "token", |_| {});
        assert!(Arc::ptr_eq(&first.permits(), &second.permits()));
        let held = first.permits().try_acquire_many_owned(8).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let err = runtime.block_on(ReadService::new(&second).sessions(None, 1)).unwrap_err();
        assert_eq!(err.status(), 503);
        drop(held);
        assert!(runtime.block_on(ReadService::new(&second).sessions(None, 1)).is_ok());
        drop(runtime); drop(first); drop(second); access.close(); drop(access);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn body_transport_error_is_bad_request() {
        let err = axum::Error::new(std::io::Error::other("broken stream"));
        assert_eq!(body_error(&err).status(), StatusCode::BAD_REQUEST);
    }
    #[test]
    fn failed_notification_thread_preserves_writes() {
        let mut random = [0u8; 16]; getrandom::fill(&mut random).unwrap();
        let dir = std::env::temp_dir().join(format!("mindtrail-notify-failure-{random:?}"));
        std::fs::create_dir(&dir).unwrap();
        let store = Arc::new(Store::open(&dir.join("store.sqlite")).unwrap());
        let state = HttpState::new_with_spawner(store.clone(), "127.0.0.1:1".parse().unwrap(), Arc::from("token"), |_| panic!("must not run"), |_worker| Err(std::io::Error::other("injected spawn failure")));
        assert!(matches!(state.notify.try_send("test".into()), Err(std::sync::mpsc::TrySendError::Disconnected(_))));
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let body = Bytes::from_static(br#"[{"v":1,"session":"s","seq":1,"ts":"2024-01-01T00:00:00Z","type":"task.start","payload":{"title":"T","goal":"G","project":"P"}}]"#);
        let response = runtime.block_on(write(State(state), body, false));
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(store.events("s", 0, 200).unwrap().last_seq, 1);
        drop(runtime); drop(store); std::fs::remove_dir_all(dir).unwrap();
    }
}

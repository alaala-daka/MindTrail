//! Private supervisor foundation; HTTP and endpoint publication are wired separately.
use std::{path::{Path, PathBuf}, sync::mpsc};
use tokio::sync::oneshot;
use super::{access::StoreAccess, windows_security::StateDirectory};

fn generate_token() -> std::io::Result<String> {
    generate_token_with(|bytes| getrandom::fill(bytes).map_err(|_| ()))
}

fn generate_token_with(fill: impl FnOnce(&mut [u8]) -> Result<(), ()>) -> std::io::Result<String> {
    let mut bytes = [0u8; 32];
    fill(&mut bytes).map_err(|_| std::io::Error::new(std::io::ErrorKind::Other, "failed to generate gateway token"))?;
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut token = String::with_capacity(64);
    for byte in bytes {
        token.push(HEX[(byte >> 4) as usize] as char);
        token.push(HEX[(byte & 15) as usize] as char);
    }
    Ok(token)
}
const GATEWAY_PORTS: std::ops::RangeInclusive<u16> = 7391..=7490;

fn bind_listener(ports: std::ops::RangeInclusive<u16>) -> std::io::Result<std::net::TcpListener> {
    bind_listener_with(ports, |address| std::net::TcpListener::bind(address))
}

fn bind_listener_with<T>(ports: std::ops::RangeInclusive<u16>, mut bind: impl FnMut(std::net::SocketAddrV4) -> std::io::Result<T>) -> std::io::Result<T> {
    for port in ports {
        match bind(std::net::SocketAddrV4::new(std::net::Ipv4Addr::LOCALHOST, port)) {
            Ok(listener) => return Ok(listener),
            Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => continue,
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::new(std::io::ErrorKind::AddrInUse, "no available gateway port"))
}

#[cfg(test)]
struct ObservedSupervisorListener { inner: tokio::net::TcpListener, accepted: Option<mpsc::Sender<()>> }
#[cfg(test)]
impl axum::serve::Listener for ObservedSupervisorListener {
    type Io = tokio::net::TcpStream;
    type Addr = std::net::SocketAddr;
    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        let connection = axum::serve::Listener::accept(&mut self.inner).await;
        if let Some(sender) = &self.accepted { let _ = sender.send(()); }
        connection
    }
    fn local_addr(&self) -> std::io::Result<Self::Addr> { self.inner.local_addr() }
}
#[derive(Debug)]
enum ServeError { Io(std::io::Error), DrainTimeout }

struct HttpDrain(std::pin::Pin<Box<dyn std::future::Future<Output = std::io::Result<()>> + Send>>);
impl HttpDrain {
    async fn finish(self, grace: std::time::Duration) -> Result<(), ServeError> {
        tokio::time::timeout(grace, self.0).await.map_err(|_| ServeError::DrainTimeout)?.map_err(ServeError::Io)
    }
}
async fn begin_shutdown<L>(listener: L, router: axum::Router, access: StoreAccess, admission: std::sync::Arc<std::sync::atomic::AtomicBool>, stop: oneshot::Receiver<()>) -> Result<Option<HttpDrain>, ServeError>
where L: axum::serve::Listener, L::Addr: std::fmt::Debug {
    begin_shutdown_with_hook(listener, router, access, admission, stop, || {}).await
}
async fn begin_shutdown_with_hook<L>(listener: L, router: axum::Router, access: StoreAccess, admission: std::sync::Arc<std::sync::atomic::AtomicBool>, mut stop: oneshot::Receiver<()>, stopping: impl FnOnce()) -> Result<Option<HttpDrain>, ServeError>
where L: axum::serve::Listener, L::Addr: std::fmt::Debug {
    use std::{future::{Future, IntoFuture}, task::Poll};
    let (graceful_tx, graceful_rx) = oneshot::channel::<()>();
    let mut serve: std::pin::Pin<Box<dyn Future<Output = std::io::Result<()>> + Send>> = Box::pin(axum::serve(listener, router).with_graceful_shutdown(async { let _ = graceful_rx.await; }).into_future());
    let ended = std::future::poll_fn(|cx| {
        if std::pin::Pin::new(&mut stop).poll(cx).is_ready() { return Poll::Ready(None); }
        match serve.as_mut().poll(cx) { Poll::Ready(result) => Poll::Ready(Some(result)), Poll::Pending => Poll::Pending }
    }).await;
    stopping();
    admission.store(false, std::sync::atomic::Ordering::Release);
    access.close();
    if let Some(result) = ended { return result.map(|_| None).map_err(ServeError::Io); }
    let _ = graceful_tx.send(());
    let result = std::future::poll_fn(|cx| Poll::Ready(serve.as_mut().poll(cx))).await;
    match result { Poll::Ready(result) => result.map(|_| None).map_err(ServeError::Io), Poll::Pending => Ok(Some(HttpDrain(serve))) }
}

#[derive(Debug)]
pub(super) struct StartupError { pub stage: &'static str, pub message: String }
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum GatewayStatus { Ready { store_id: String }, Unavailable { message: String } }
struct Completion { result: std::sync::Mutex<Option<Result<(), String>>>, changed: std::sync::Condvar, notify: tokio::sync::Notify }
impl Completion {
    fn publish(&self, result: Result<(), String>) {
        *self.result.lock().unwrap_or_else(|e| e.into_inner()) = Some(result);
        self.changed.notify_all(); self.notify.notify_waiters();
    }
    fn wait_blocking(&self) -> Result<(), String> {
        self.wait_blocking_with(|| {})
    }
    fn wait_blocking_with(&self, mut waiting: impl FnMut()) -> Result<(), String> {
        let mut result = self.result.lock().unwrap_or_else(|e| e.into_inner());
        while result.is_none() { waiting(); result = self.changed.wait(result).unwrap_or_else(|e| e.into_inner()); }
        result.clone().unwrap()
    }
    async fn wait(&self) -> Result<(), String> {
        loop {
            let notified = self.notify.notified(); tokio::pin!(notified); notified.as_mut().enable();
            if let Some(result) = self.result.lock().unwrap_or_else(|e| e.into_inner()).clone() { return result; }
            notified.await;
        }
    }
}
struct Control { stop: Option<oneshot::Sender<u8>>, done: std::sync::Arc<Completion> }
impl Control { fn request_shutdown(&mut self) { if let Some(stop) = self.stop.take() { let _ = stop.send(1); } } }
impl Drop for Control { fn drop(&mut self) { self.request_shutdown(); } }
struct Ready { access: StoreAccess, store_id: String, address: std::net::SocketAddr, token: String }
pub(super) struct PendingStart { ready: Option<oneshot::Receiver<Result<Ready, StartupError>>>, control: Option<Control>, status: std::sync::Arc<std::sync::Mutex<GatewayStatus>> }
impl PendingStart {
    pub(super) fn request_shutdown(&mut self) { if let Some(control) = &mut self.control { control.request_shutdown(); } }
    pub(super) fn wait_completion_blocking(&self) -> Result<(), String> { self.control.as_ref().expect("control").done.wait_blocking() }
    pub(super) async fn wait_completion(&self) -> Result<(), String> { self.control.as_ref().expect("control").done.wait().await }
    async fn ready(&mut self) -> Result<Ready, StartupError> { self.ready.as_mut().expect("ready").await.map_err(|_| StartupError { stage: "supervisor", message: "supervisor stopped during startup".into() })? }
    pub(super) async fn wait_ready(&mut self) -> Result<Supervisor, StartupError> { let ready = self.ready().await?; Ok(self.finish(ready)) }
    pub(super) fn wait_ready_blocking(&mut self) -> Result<Supervisor, StartupError> {
        let ready = self.ready.take().expect("ready").blocking_recv().map_err(|_| StartupError { stage: "supervisor", message: "supervisor stopped during startup".into() })??;
        Ok(self.finish(ready))
    }
    fn finish(&mut self, ready: Ready) -> Supervisor {
        { let mut current = self.status.lock().unwrap_or_else(|e| e.into_inner());
          if matches!(&*current, GatewayStatus::Unavailable { message } if message == "gateway starting") { *current = GatewayStatus::Ready { store_id: ready.store_id.clone() }; } }
        Supervisor { control: self.control.take().expect("control"), ready, status: self.status.clone() }
    }
}
pub(super) type SessionNotifier = std::sync::Arc<dyn Fn(&str) + Send + Sync + 'static>;
pub(super) struct Supervisor { control: Control, ready: Ready, status: std::sync::Arc<std::sync::Mutex<GatewayStatus>> }
impl Supervisor {
    pub(super) fn begin_start(path: &Path) -> Result<PendingStart, StartupError> { Self::begin_start_with_notify(path, std::sync::Arc::new(|_| {})) }
    pub(super) fn begin_start_with_notify(path: &Path, notify: SessionNotifier) -> Result<PendingStart, StartupError> { Self::begin_start_inner(path, notify, #[cfg(test)] || {}, #[cfg(test)] None, #[cfg(test)] None) }
    pub(super) async fn start(path: &Path) -> Result<Self, StartupError> {
        let mut pending = Self::begin_start(path)?; let ready = pending.ready().await?; Ok(pending.finish(ready))
    }
    #[cfg(test)]
    async fn start_inner(path: &Path, before_ready: impl FnOnce() + Send + 'static, thread_tx: Option<mpsc::Sender<std::thread::JoinHandle<()>>>, after_ready: Option<(mpsc::Sender<StoreAccess>, oneshot::Receiver<()>)>, accepted: Option<mpsc::Sender<()>>) -> Result<Self, StartupError> {
        let mut pending = Self::begin_start_inner(path, std::sync::Arc::new(|_| {}), before_ready, thread_tx, accepted)?;
        let ready = pending.ready().await?;
        if let Some((entered, release)) = after_ready { let _ = entered.send(ready.access.clone()); let _ = release.await; }
        Ok(pending.finish(ready))
    }
    fn begin_start_inner(path: &Path, notify: SessionNotifier, #[cfg(test)] before_ready: impl FnOnce() + Send + 'static, #[cfg(test)] thread_tx: Option<mpsc::Sender<std::thread::JoinHandle<()>>>, #[cfg(test)] accepted: Option<mpsc::Sender<()>>) -> Result<PendingStart, StartupError> {
        let path = path.to_owned(); let (stop, stopped) = oneshot::channel(); let (ready_tx, ready) = oneshot::channel();
        let done = std::sync::Arc::new(Completion { result: std::sync::Mutex::new(None), changed: std::sync::Condvar::new(), notify: tokio::sync::Notify::new() });
        let thread_done = done.clone();
        let status = std::sync::Arc::new(std::sync::Mutex::new(GatewayStatus::Unavailable { message: "gateway starting".into() }));
        let thread_status = status.clone();
        let thread = std::thread::Builder::new().name("mindtrail-supervisor".into()).spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| supervise_with_hook(path, stopped, ready_tx, Some(thread_status.clone()), notify, #[cfg(test)] before_ready, #[cfg(test)] |listener| tokio::net::TcpListener::from_std(listener), #[cfg(test)] accepted)));
            let result = match result { Ok(Ok(result)) => result, Ok(Err((sender, error))) => { let completion = format!("{}: {}", error.stage, error.message); let _ = sender.send(Err(error)); Err(completion) }, Err(_) => Err("gateway supervisor stopped unexpectedly".to_owned()) };
            *thread_status.lock().unwrap_or_else(|e| e.into_inner()) = GatewayStatus::Unavailable { message: result.as_ref().err().cloned().unwrap_or_else(|| "gateway stopped".into()) };
            thread_done.publish(result);
        }).map_err(|e| StartupError { stage: "thread", message: e.to_string() })?;
        #[cfg(test)] if let Some(thread_tx) = thread_tx { let _ = thread_tx.send(thread); }
        #[cfg(not(test))] drop(thread);
        Ok(PendingStart { ready: Some(ready), control: Some(Control { stop: Some(stop), done }), status })
    }
    pub(super) fn access(&self) -> StoreAccess { self.ready.access.clone() }
    pub(super) fn reader(&self) -> super::http::ReadService { super::http::ReadService::from_access(self.ready.access.clone()) }
    pub(super) fn status(&self) -> GatewayStatus { self.status.lock().unwrap_or_else(|e| e.into_inner()).clone() }
    pub(super) fn store_id(&self) -> &str { &self.ready.store_id }
    pub(super) fn request_shutdown(&mut self) { self.control.request_shutdown(); }
    #[cfg(test)] pub(super) fn request_shutdown_injected(&mut self, signal: u8) { if let Some(stop) = self.control.stop.take() { let _ = stop.send(signal); } }
    pub(super) async fn wait_completion(&self) -> Result<(), String> { self.control.done.wait().await }
    pub(super) fn wait_completion_blocking(&self) -> Result<(), String> { self.control.done.wait_blocking() }
    pub(super) async fn shutdown(mut self) -> Result<(), String> { self.request_shutdown(); self.wait_completion().await }
}

fn supervise_with_hook(path: PathBuf, stopped: oneshot::Receiver<u8>, sender: oneshot::Sender<Result<Ready, StartupError>>, status: Option<std::sync::Arc<std::sync::Mutex<GatewayStatus>>>, notify: SessionNotifier, #[cfg(test)] before_ready: impl FnOnce(), #[cfg(test)] register: impl FnOnce(std::net::TcpListener) -> std::io::Result<tokio::net::TcpListener>, #[cfg(test)] accepted: Option<mpsc::Sender<()>>) -> Result<Result<(), String>, (oneshot::Sender<Result<Ready, StartupError>>, StartupError)> {
    let state = StateDirectory::acquire(&path).map_err(|e| ("directory", e.to_string()));
    let state = match state { Ok(v) => v, Err((stage, message)) => return Err((sender, StartupError { stage, message })) };
    let lock = match state.lock() { Ok(v) => v, Err(e) => return Err((sender, StartupError { stage: "lock", message: format!("{e:?}") })) };
    if let Err(error) = lock.preflight_endpoint() { return Err((sender, StartupError { stage: "endpoint preflight", message: error.to_string() })); }
    let mut store = match lock.open_store() { Ok(v) => v, Err(e) => return Err((sender, StartupError { stage: "store", message: format!("{e:?}") })) };
    let store_id = store.store_id().to_owned();
    let guard = store.access_guard();
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(v) => v, Err(e) => return Err((sender, StartupError { stage: "runtime", message: e.to_string() }))
    };
    let token = match generate_token() { Ok(v) => v, Err(e) => return Err((sender, StartupError { stage: "token", message: e.to_string() })) };
    #[cfg(test)] let ports = if std::env::var_os("MINDTRAIL_UI_FIXTURE").is_some() { GATEWAY_PORTS } else { 0..=0 };
    #[cfg(not(test))] let ports = GATEWAY_PORTS;
    let listener = match bind_listener(ports) { Ok(v) => v, Err(e) => return Err((sender, StartupError { stage: "listener", message: e.to_string() })) };
    let address = match listener.local_addr() { Ok(v) => v, Err(e) => return Err((sender, StartupError { stage: "listener", message: e.to_string() })) };
    if let Err(e) = listener.set_nonblocking(true) { return Err((sender, StartupError { stage: "listener", message: e.to_string() })); }
    let listener = {
        let _entered = runtime.enter();
        #[cfg(test)] let result = register(listener);
        #[cfg(not(test))] let result = tokio::net::TcpListener::from_std(listener);
        match result { Ok(v) => v, Err(e) => return Err((sender, StartupError { stage: "listener", message: e.to_string() })) }
    };
    #[cfg(test)] let listener = ObservedSupervisorListener { inner: listener, accepted };
    let access = guard.access();
    let http = super::http::HttpState::with_access(access.clone(), store_id.clone(), address, token.clone(), move |session| notify(session));
    let admission = http.admission();
    let router = super::http::router(http);
    let endpoint = match super::discovery::Endpoint::new(address.port(), token.clone(), store_id.clone()) {
        Ok(value) => value, Err(error) => return Err((sender, StartupError { stage: "endpoint", message: error.to_string() }))
    };
    let mut publication = match lock.publish_endpoint(&endpoint) {
        Ok(value) => value, Err(error) => return Err((sender, StartupError { stage: "publication", message: error.to_string() }))
    };
    #[cfg(test)] before_ready();
    let outcome = if sender.send(Ok(Ready { access: access.clone(), store_id, address, token })).is_ok() {
        let (first, signal) = runtime.block_on(async {
            let (stop_tx, stop_rx) = oneshot::channel();
            let signal = tokio::spawn(async move { let signal = stopped.await; let _ = stop_tx.send(()); signal });
            (begin_shutdown_with_hook(listener, router, access, admission, stop_rx, || {
                if let Some(status) = &status { *status.lock().unwrap_or_else(|e| e.into_inner()) = GatewayStatus::Unavailable { message: "gateway stopping".into() }; }
            }).await, signal)
        });
        let withdrawal = publication.withdraw().map_err(|error| format!("endpoint withdrawal failed: {error}"));
        let result = runtime.block_on(async {
            let result = match first {
                Ok(Some(drain)) => drain.finish(std::time::Duration::from_secs(2)).await,
                Ok(None) => Ok(()),
                Err(error) => Err(error),
            };
            if signal.is_finished() {
                let signal = signal.await;
                #[cfg(test)] {
                    if matches!(signal, Ok(Ok(2))) { panic!("injected supervisor panic"); }
                    if matches!(signal, Ok(Ok(3))) { return HttpDrain(Box::pin(std::future::pending())).finish(std::time::Duration::from_millis(10)).await; }
                    if matches!(signal, Ok(Ok(4))) { return Err(ServeError::Io(std::io::Error::other("injected serve failure"))); }
                }
                #[cfg(not(test))] let _ = signal;
            } else { signal.abort(); }
            result
        });
        runtime.shutdown_timeout(std::time::Duration::from_secs(1));
        let service = result.map_err(|error| match error { ServeError::Io(error) => format!("HTTP service failed: {error}"), ServeError::DrainTimeout => "HTTP drain timed out".to_owned() });
        match (service, withdrawal) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
            (Err(service), Err(withdrawal)) => Err(format!("{service}; {withdrawal}")),
        }
    } else {
        drop(listener);
        let withdrawal = publication.withdraw().map_err(|error| format!("endpoint withdrawal failed: {error}"));
        runtime.shutdown_timeout(std::time::Duration::from_secs(1));
        withdrawal
    };
    drop(publication);
    drop(guard);
    drop(store); drop(lock); drop(state);
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::mpsc, time::{Duration, Instant}};
    #[test]
    fn completion_can_be_retried_and_shared() {
        let completion = std::sync::Arc::new(Completion { result: std::sync::Mutex::new(None), changed: std::sync::Condvar::new(), notify: tokio::sync::Notify::new() });
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        runtime.block_on(async { assert!(tokio::time::timeout(Duration::from_millis(20), completion.wait()).await.is_err()); });
        let other = completion.clone();
        let (waiting_tx, waiting_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        let waiter = std::thread::spawn(move || { let result = other.wait_blocking_with(|| { let _ = waiting_tx.send(()); }); let _ = finished_tx.send(result); });
        let blocking_registered = waiting_rx.recv_timeout(Duration::from_secs(5)).is_ok();
        let (registered, results) = runtime.block_on(async {
            use std::{future::{Future, poll_fn}, task::Poll};
            let a = completion.wait(); let b = completion.wait();
            tokio::pin!(a); tokio::pin!(b);
            let registered = poll_fn(|cx| Poll::Ready(a.as_mut().poll(cx).is_pending() && b.as_mut().poll(cx).is_pending())).await;
            completion.publish(Err("completed".into()));
            let results = tokio::time::timeout(Duration::from_secs(5), async { tokio::join!(a, b) }).await;
            (registered, results)
        });
        let blocking_result = finished_rx.recv_timeout(Duration::from_secs(5));
        if blocking_result.is_err() { completion.changed.notify_all(); }
        join_bounded(waiter).unwrap();
        assert!(blocking_registered && registered);
        assert_eq!(blocking_result.expect("blocking completion waiter timed out"), Err("completed".into()));
        let (a, b) = results.expect("async completion waiters timed out");
        assert_eq!(a, Err("completed".into())); assert_eq!(a, b);
        assert_eq!(completion.wait_blocking(), Err("completed".into()));
    }
    #[test]
    fn stopping_status_precedes_admission_close() {
        let mut random = [0u8; 16]; getrandom::fill(&mut random).unwrap();
        let path = std::env::temp_dir().join(format!("mindtrail-status-{random:?}.sqlite"));
        let store = std::sync::Arc::new(super::super::store::Store::open(&path).unwrap());
        let id = store.store_id().to_owned();
        let access = StoreAccess::new(store);
        let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap(); listener.set_nonblocking(true).unwrap();
        let http = super::super::http::HttpState::with_access(access.clone(), id.clone(), address, "test-token", |_| {});
        let admission = http.admission();
        let status = std::sync::Arc::new(std::sync::Mutex::new(GatewayStatus::Ready { store_id: id }));
        let worker_status = status.clone(); let worker_access = access.clone(); let worker_admission = admission.clone();
        let (stop_tx, stop_rx) = oneshot::channel();
        let (entered_tx, entered_rx) = mpsc::channel(); let (release_tx, release_rx) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            runtime.block_on(async {
                let listener = tokio::net::TcpListener::from_std(listener).unwrap();
                let result = begin_shutdown_with_hook(listener, super::super::http::router(http), worker_access, worker_admission, stop_rx, || {
                    *worker_status.lock().unwrap() = GatewayStatus::Unavailable { message: "gateway stopping".into() };
                    entered_tx.send(()).unwrap();
                    release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                }).await.unwrap();
                if let Some(drain) = result { drain.finish(Duration::from_secs(2)).await.unwrap(); }
            });
        });
        stop_tx.send(()).unwrap();
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(*status.lock().unwrap(), GatewayStatus::Unavailable { message: "gateway stopping".into() });
        assert!(admission.load(std::sync::atomic::Ordering::Acquire));
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        assert!(runtime.block_on(access.run(|_| Ok(()))).is_ok());
        release_tx.send(()).unwrap();
        join_bounded(thread).unwrap();
        assert!(!admission.load(std::sync::atomic::Ordering::Acquire));
        assert_eq!(runtime.block_on(access.run(|_| Ok(()))).unwrap_err().status(), 503);
        assert_eq!(*status.lock().unwrap(), GatewayStatus::Unavailable { message: "gateway stopping".into() });
        drop(runtime); drop(access); std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn token_generation_encodes_all_bytes_and_handles_failure() {
        let token = generate_token_with(|bytes| {
            for (index, byte) in bytes.iter_mut().enumerate() { *byte = index as u8; }
            Ok(())
        }).unwrap();
        assert_eq!(token, "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f");
        let random = generate_token().unwrap();
        assert_eq!(random.len(), 64);
        assert!(random.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
        let error = generate_token_with(|bytes| { bytes.fill(0xff); Err(()) }).unwrap_err();
        assert_eq!(error.to_string(), "failed to generate gateway token");
    }
    #[test]
    fn keep_alive_rejects_requests_after_stop() {
        use std::io::{Read, Write};
        let mut random = [0u8; 16]; getrandom::fill(&mut random).unwrap();
        let path = std::env::temp_dir().join(format!("mindtrail-keepalive-{random:?}.sqlite"));
        let store = std::sync::Arc::new(super::super::store::Store::open(&path).unwrap());
        let id = store.store_id().to_owned();
        let access = StoreAccess::new(store);
        let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap(); listener.set_nonblocking(true).unwrap();
        let state = super::super::http::HttpState::with_access(access.clone(), id, address, "test-token", |_| {});
        let admission = state.admission();
        let (stop_tx, stop_rx) = oneshot::channel();
        let (finish_tx, finish_rx) = oneshot::channel();
        let (phase_tx, phase_rx) = mpsc::channel();
        let server = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            runtime.block_on(async {
                let listener = tokio::net::TcpListener::from_std(listener).unwrap();
                let router = super::super::http::router(state);
                let drain = begin_shutdown(listener, router.clone(), access, admission, stop_rx).await.unwrap();
                use tower::ServiceExt;
                let request = axum::http::Request::builder().uri("/v1/health").header("host", address.to_string()).header("authorization", "Bearer test-token").body(axum::body::Body::empty()).unwrap();
                let response = tokio::time::timeout(Duration::from_secs(5), router.oneshot(request)).await.expect("router request timed out").unwrap();
                assert_eq!(response.status(), axum::http::StatusCode::SERVICE_UNAVAILABLE);
                let _ = phase_tx.send(());
                let _ = tokio::time::timeout(Duration::from_secs(5), finish_rx).await;
                if let Some(drain) = drain { let _ = drain.finish(Duration::from_millis(200)).await; }
            });
            runtime.shutdown_timeout(Duration::from_secs(1));
        });
        let mut stream = std::net::TcpStream::connect(address).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        stream.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
        let request = format!("GET /v1/health HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer test-token\r\n\r\n");
        stream.write_all(request.as_bytes()).unwrap();
        let mut response = [0u8; 4096];
        let mut received = Vec::new();
        let header_end = loop {
            let count = stream.read(&mut response).expect("health response header timed out or failed");
            assert!(count != 0, "connection closed before response header");
            received.extend_from_slice(&response[..count]);
            assert!(received.len() <= 65536, "response header exceeds limit");
            if let Some(index) = received.windows(4).position(|part| part == b"\r\n\r\n") { break index + 4; }
        };
        let header = std::str::from_utf8(&received[..header_end]).unwrap();
        assert!(header.starts_with("HTTP/1.1 200 "));
        let length: usize = header.lines().find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length").then(|| value.trim().parse().unwrap())
        }).expect("missing Content-Length");
        assert!(length <= 65536, "response body exceeds limit");
        while received.len() < header_end + length {
            let count = stream.read(&mut response).expect("health response body timed out or failed");
            assert!(count != 0, "connection closed before response body");
            received.extend_from_slice(&response[..count]);
        }
        assert_eq!(received.len(), header_end + length);
        stop_tx.send(()).unwrap();
        phase_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let sent = stream.write_all(request.as_bytes()).is_ok();
        let _ = finish_tx.send(());
        if sent {
            match stream.read(&mut response) {
                Ok(0) => {},
                Ok(count) => assert!(!String::from_utf8_lossy(&response[..count]).contains("200 OK")),
                Err(error) => assert!(matches!(error.kind(), std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted | std::io::ErrorKind::BrokenPipe)),
            }
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while !server.is_finished() { assert!(Instant::now() < deadline); std::thread::yield_now(); }
        server.join().unwrap(); drop(stream);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn drain_timeout_is_observable() {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let drain = HttpDrain(Box::pin(std::future::pending()));
        assert!(matches!(runtime.block_on(drain.finish(Duration::from_millis(10))), Err(ServeError::DrainTimeout)));
    }
    #[test]
    fn serve_handles_health_and_bounded_half_header_shutdown() {
        use std::io::{Read, Write};
        struct ObservedListener { inner: tokio::net::TcpListener, accepted: mpsc::Sender<()> }
        impl axum::serve::Listener for ObservedListener {
            type Io = tokio::net::TcpStream;
            type Addr = std::net::SocketAddr;
            async fn accept(&mut self) -> (Self::Io, Self::Addr) {
                let connection = axum::serve::Listener::accept(&mut self.inner).await;
                let _ = self.accepted.send(());
                connection
            }
            fn local_addr(&self) -> std::io::Result<Self::Addr> { self.inner.local_addr() }
        }
        let mut random = [0u8; 16]; getrandom::fill(&mut random).unwrap();
        let path = std::env::temp_dir().join(format!("mindtrail-serve-{random:?}.sqlite"));
        let store = std::sync::Arc::new(super::super::store::Store::open(&path).unwrap());
        let id = store.store_id().to_owned();
        let access = StoreAccess::new(store);
        let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let state = super::super::http::HttpState::with_access(access.clone(), id.clone(), address, "test-token", |_| {});
        let admission = state.admission();
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let (stop_tx, stop_rx) = oneshot::channel();
        let listener = runtime.block_on(async { tokio::net::TcpListener::from_std(listener).unwrap() });
        let (accepted_tx, accepted_rx) = mpsc::channel();
        let server = runtime.spawn(begin_shutdown(ObservedListener { inner: listener, accepted: accepted_tx }, super::super::http::router(state), access.clone(), admission, stop_rx));
        let client = std::thread::spawn(move || {
            let mut stream = std::net::TcpStream::connect(address).unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            write!(stream, "GET /v1/health HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer test-token\r\nConnection: close\r\n\r\n").unwrap();
            let mut response = String::new(); stream.read_to_string(&mut response).unwrap();
            assert!(response.contains("200 OK")); assert!(response.contains(&id));
            let mut partial = std::net::TcpStream::connect(address).unwrap();
            partial.write_all(b"GET /v1/health HTTP/1.1\r\nHost:").unwrap();
            partial
        });
        let mut partial = runtime.block_on(async { tokio::time::timeout(Duration::from_secs(5), async { while !client.is_finished() { tokio::task::yield_now().await; } }).await.unwrap(); client.join().unwrap() });
        runtime.block_on(async { tokio::time::timeout(Duration::from_secs(5), async {
            let mut count = 0;
            while count < 2 { match accepted_rx.try_recv() { Ok(()) => count += 1, Err(mpsc::TryRecvError::Empty) => tokio::task::yield_now().await, Err(mpsc::TryRecvError::Disconnected) => panic!("listener stopped") } }
            tokio::task::yield_now().await;
        }).await.unwrap(); });
        stop_tx.send(()).unwrap();
        let result = runtime.block_on(async { match tokio::time::timeout(Duration::from_secs(3), server).await.unwrap().unwrap() { Ok(Some(drain)) => drain.finish(Duration::from_millis(200)).await, Ok(None) => Ok(()), Err(error) => Err(error) } });
        assert!(matches!(result, Err(ServeError::DrainTimeout)));
        assert_eq!(runtime.block_on(access.run(|_| Ok(()))).unwrap_err().status(), 503);
        drop(runtime);
        partial.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut byte = [0u8; 1];
        match partial.read(&mut byte) { Ok(0) => {}, Err(error) if matches!(error.kind(), std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted) => {}, other => panic!("connection remained open: {other:?}") }
        drop(partial); drop(access);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn listener_retries_occupied_port_and_reports_exhaustion() {
        let mut seen = Vec::new();
        let result = bind_listener_with(41..=42, |address| {
            seen.push(address.port());
            if address.port() == 41 { Err(std::io::ErrorKind::AddrInUse.into()) } else { Ok(address.port()) }
        }).unwrap();
        assert_eq!(result, 42);
        assert_eq!(seen, vec![41, 42]);
        let error = bind_listener_with(41..=42, |_| -> std::io::Result<()> { Err(std::io::ErrorKind::AddrInUse.into()) }).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::AddrInUse);
        let listener = bind_listener(0..=0).unwrap();
        assert_eq!(listener.local_addr().unwrap().ip(), std::net::Ipv4Addr::LOCALHOST);
    }
    #[test]
    fn listener_stops_on_non_contention_error() {
        let mut calls = 0;
        let result: std::io::Result<()> = bind_listener_with(7391..=7490, |address| {
            calls += 1;
            assert_eq!(*address.ip(), std::net::Ipv4Addr::LOCALHOST);
            Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "injected"))
        });
        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::PermissionDenied);
        assert_eq!(calls, 1);
    }
    struct ChildFixture { child: Option<std::process::Child>, base: PathBuf }
    impl Drop for ChildFixture {
        fn drop(&mut self) {
            if let Some(child) = &mut self.child {
                let mut exited = matches!(child.try_wait(), Ok(Some(_)));
                if !exited {
                    if let Err(error) = child.kill() { eprintln!("test child termination failed: {error}"); }
                    let deadline = Instant::now() + Duration::from_secs(5);
                    while Instant::now() < deadline {
                        match child.try_wait() {
                            Ok(Some(_)) => { exited = true; break; }
                            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                            Err(error) => { eprintln!("test child status failed: {error}"); break; }
                        }
                    }
                }
                if !exited { eprintln!("test child could not be confirmed exited; fixture retained: {}", self.base.display()); return; }
            }
            if let Err(error) = std::fs::remove_dir_all(&self.base) { eprintln!("test fixture cleanup failed: {error}"); }
        }
    }
    fn isolated(name: &str) -> Option<PathBuf> {
        const FIXTURE: &str = "MINDTRAIL_SUPERVISOR_ISOLATED_FIXTURE";
        if let Some(path) = std::env::var_os(FIXTURE) { return Some(path.into()); }
        let mut random = [0u8; 16]; getrandom::fill(&mut random).unwrap();
        let base = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailIsolated-{random:?}"));
        std::fs::create_dir(&base).unwrap();
        let mut fixture = ChildFixture { child: None, base: base.clone() };
        fixture.child = Some(std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", name]).env(FIXTURE, &base).spawn().unwrap());
        let child = fixture.child.as_mut().unwrap();
        let deadline = Instant::now() + Duration::from_secs(35);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() { break Some(status); }
            if Instant::now() >= deadline { break None; }
            std::thread::sleep(Duration::from_millis(10));
        };
        if name.ends_with("cancel_after_ready_sent") && status.is_some_and(|status| status.success()) {
            let state = StateDirectory::acquire(&base.join("state")).unwrap();
            assert!(state.lock().is_ok());
        }
        assert!(status.is_some_and(|status| status.success()), "isolated test failed or timed out");
        None
    }
    fn join_bounded(thread: std::thread::JoinHandle<()>) -> Result<(), &'static str> {
        let deadline = Instant::now() + Duration::from_secs(8);
        while !thread.is_finished() {
            if Instant::now() >= deadline { return Err("supervisor thread did not exit"); }
            std::thread::sleep(Duration::from_millis(10));
        }
        thread.join().map_err(|_| "supervisor panicked")
    }
    #[test]
    fn isolated_http_supervisor_waits_for_active_work() {
        use std::io::{Read, Write};
        let Some(base) = isolated("gateway::runtime::tests::isolated_http_supervisor_waits_for_active_work") else { return; };
        let path = base.join("state");
        let worker_path = path.clone();
        let (ready_tx, ready_rx) = mpsc::channel();
        let (stop_tx, stop_rx) = oneshot::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let (drain_tx, drain_rx) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            let state = StateDirectory::acquire(&worker_path).unwrap();
            let lock = state.lock().unwrap();
            let mut store = lock.open_store().unwrap();
            let id = store.store_id().to_owned();
            let guard = store.access_guard();
            let access = guard.access();
            let token = generate_token().unwrap();
            let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
            let address = listener.local_addr().unwrap();
            listener.set_nonblocking(true).unwrap();
            let http = super::super::http::HttpState::with_access(access.clone(), id, address, token.clone(), |_| {});
            let admission = http.admission();
            let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            ready_tx.send((address, token, access.clone())).unwrap();
            let result = runtime.block_on(async {
                let listener = tokio::net::TcpListener::from_std(listener).unwrap();
                match begin_shutdown(listener, super::super::http::router(http), access, admission, stop_rx).await {
                    Ok(Some(drain)) => drain.finish(Duration::from_secs(2)).await,
                    Ok(None) => Ok(()),
                    Err(error) => Err(error),
                }
            });
            runtime.shutdown_timeout(Duration::from_secs(1));
            let _ = drain_tx.send(result);
            drop(guard); drop(store); drop(lock); drop(state);
            let _ = done_tx.send(());
        });
        let (address, token, access) = ready_rx.recv_timeout(Duration::from_secs(8)).unwrap();
        let mut stream = std::net::TcpStream::connect_timeout(&address, Duration::from_secs(5)).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        stream.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
        write!(stream, "GET /v1/health HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n").unwrap();
        let mut response = String::new(); stream.read_to_string(&mut response).unwrap();
        assert!(response.starts_with("HTTP/1.1 200 "));
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let worker_access = access.clone();
        let worker = runtime.spawn(async move { worker_access.run(move |_| {
            started_tx.send(()).unwrap();
            let _ = release_rx.recv_timeout(Duration::from_secs(15));
            Ok(())
        }).await });
        runtime.block_on(async { tokio::time::timeout(Duration::from_secs(5), async {
            loop { if started_rx.try_recv().is_ok() { break; } tokio::task::yield_now().await; }
        }).await.unwrap(); });
        stop_tx.send(()).unwrap();
        runtime.block_on(async { tokio::time::timeout(Duration::from_secs(5), async {
            loop { if let Err(error) = access.run(|_| Ok(())).await { assert_eq!(error.status(), 503); break; } tokio::task::yield_now().await; }
        }).await.unwrap(); });
        drain_rx.recv_timeout(Duration::from_secs(5)).expect("drain result missing").expect("HTTP drain failed");
        assert!(std::net::TcpStream::connect_timeout(&address, Duration::from_secs(2)).is_err(), "listener still accepts connections");
        let contender = StateDirectory::acquire(&path).unwrap();
        assert!(matches!(contender.lock(), Err(super::super::windows_security::FileLockError::Contended)));
        assert!(matches!(done_rx.recv_timeout(Duration::from_millis(200)), Err(mpsc::RecvTimeoutError::Timeout)));
        release_tx.send(()).unwrap();
        runtime.block_on(async { tokio::time::timeout(Duration::from_secs(5), worker).await.unwrap().unwrap().unwrap(); });
        done_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        join_bounded(thread).unwrap();
        assert!(contender.lock().is_ok());
        assert!(!path.join("endpoint.json").exists());
    }
    #[test]
    fn unsafe_existing_endpoint_blocks_database_and_listener() {
        let Some(base) = isolated("gateway::runtime::tests::unsafe_existing_endpoint_blocks_database_and_listener") else { return; };
        let path = base.join("state");
        drop(StateDirectory::acquire(&path).unwrap());
        let target = path.join("endpoint.json");
        std::fs::write(&target, b"old endpoint").unwrap();
        std::fs::hard_link(&target, path.join("linked-endpoint")).unwrap();
        let (ready_tx, ready_rx) = oneshot::channel();
        let (_stop_tx, stop_rx) = oneshot::channel();
        let (registered_tx, registered_rx) = mpsc::channel();
        let worker_path = path.clone();
        let thread = std::thread::spawn(move || {
            let result = supervise_with_hook(worker_path, stop_rx, ready_tx, None, std::sync::Arc::new(|_| {}), || {}, move |listener| {
                registered_tx.send(()).unwrap();
                tokio::net::TcpListener::from_std(listener)
            }, None);
            let (sender, error) = result.err().expect("unsafe endpoint must fail");
            sender.send(Err(error)).ok();
        });
        join_bounded(thread).unwrap();
        let error = ready_rx.blocking_recv().unwrap().err().expect("no Ready");
        assert_eq!(error.stage, "endpoint preflight");
        assert!(matches!(registered_rx.try_recv(), Err(mpsc::TryRecvError::Disconnected)));
        assert!(!path.join("mindtrail.sqlite3").exists());
        assert_eq!(std::fs::read(&target).unwrap(), b"old endpoint");
    }
    #[test]
    fn publication_sharing_conflict_preserves_old_endpoint_and_releases_port() {
        use std::os::windows::fs::OpenOptionsExt;
        let Some(base) = isolated("gateway::runtime::tests::publication_sharing_conflict_preserves_old_endpoint_and_releases_port") else { return; };
        let path = base.join("state");
        drop(StateDirectory::acquire(&path).unwrap());
        let target = path.join("endpoint.json");
        std::fs::write(&target, b"old endpoint").unwrap();
        let blocker = std::fs::OpenOptions::new().read(true).share_mode(3).open(&target).unwrap();
        let (ready_tx, ready_rx) = oneshot::channel();
        let (_stop_tx, stop_rx) = oneshot::channel();
        let (address_tx, address_rx) = mpsc::channel();
        let worker_path = path.clone();
        let thread = std::thread::spawn(move || {
            let result = supervise_with_hook(worker_path, stop_rx, ready_tx, None, std::sync::Arc::new(|_| {}), || {}, move |listener| {
                address_tx.send(listener.local_addr().unwrap()).unwrap();
                tokio::net::TcpListener::from_std(listener)
            }, None);
            let (sender, error) = result.err().expect("publication must fail");
            sender.send(Err(error)).ok();
        });
        join_bounded(thread).unwrap();
        let address = address_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let error = ready_rx.blocking_recv().unwrap().err().expect("no Ready");
        assert_eq!(error.stage, "publication");
        assert_eq!(std::fs::read(&target).unwrap(), b"old endpoint");
        let temp_files: Vec<_> = std::fs::read_dir(&path).unwrap().map(|entry| entry.unwrap().file_name()).filter(|name| { let name = name.to_string_lossy(); name.starts_with("endpoint-") && name.ends_with(".tmp") }).collect();
        assert!(temp_files.is_empty(), "publication left temporary endpoint files: {temp_files:?}");
        assert!(std::net::TcpListener::bind(address).is_ok());
        drop(blocker);
    }
    #[test]
    fn listener_registration_failure_precedes_ready() {
        let Some(base) = isolated("gateway::runtime::tests::listener_registration_failure_precedes_ready") else { return; };
        let path = base.join("state");
        let (ready_tx, ready_rx) = oneshot::channel();
        let (_stop_tx, stop_rx) = oneshot::channel();
        let worker_path = path.clone();
        let thread = std::thread::spawn(move || {
            let result = supervise_with_hook(worker_path, stop_rx, ready_tx, None, std::sync::Arc::new(|_| {}), || {}, |_| Err(std::io::Error::from_raw_os_error(10022)), None);
            let (sender, error) = result.err().expect("registration must fail");
            sender.send(Err(error)).ok();
        });
        join_bounded(thread).unwrap();
        let error = ready_rx.blocking_recv().unwrap().err().expect("no Ready on failure");
        assert_eq!(error.stage, "listener");
        assert!(error.message.contains("10022"));
        let state = StateDirectory::acquire(&path).unwrap();
        assert!(state.lock().is_ok());
    }
    #[test]
    fn published_endpoint_serves_and_rotates_on_restart() {
        use std::io::{Read, Write};
        let Some(base) = isolated("gateway::runtime::tests::published_endpoint_serves_and_rotates_on_restart") else { return; };
        let path = base.join("state");
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let mut previous: Option<(String, String)> = None;
        for _ in 0..2 {
            let supervisor = runtime.block_on(async { tokio::time::timeout(Duration::from_secs(5), Supervisor::start(&path)).await.unwrap().unwrap() });
            let endpoint: serde_json::Value = serde_json::from_slice(&std::fs::read(path.join("endpoint.json")).unwrap()).unwrap();
            assert_eq!(supervisor.status(), GatewayStatus::Ready { store_id: supervisor.store_id().to_owned() });
            let reader = supervisor.reader();
            let status = supervisor.status.clone();
            let token = endpoint["token"].as_str().unwrap().to_owned();
            let id = endpoint["store_id"].as_str().unwrap().to_owned();
            let address = std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, endpoint["port"].as_u64().unwrap() as u16));
            if let Some((old_token, old_id)) = &previous { assert_ne!(&token, old_token); assert_eq!(&id, old_id); }
            let mut stream = std::net::TcpStream::connect_timeout(&address, Duration::from_secs(5)).unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            stream.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
            write!(stream, "GET /v1/health HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n").unwrap();
            let mut response = String::new(); stream.read_to_string(&mut response).unwrap();
            assert!(response.starts_with("HTTP/1.1 200 "));
            runtime.block_on(async { tokio::time::timeout(Duration::from_secs(8), supervisor.shutdown()).await.unwrap().unwrap(); });
            assert!(!path.join("endpoint.json").exists());
            assert_eq!(*status.lock().unwrap(), GatewayStatus::Unavailable { message: "gateway stopped".into() });
            assert_eq!(runtime.block_on(reader.sessions(None, 10)).unwrap_err().status(), 503);
            previous = Some((token, id));
        }
    }
    #[test]
    fn supervisor_serves_health_on_isolated_port() {
        use std::io::{Read, Write};
        let Some(base) = isolated("gateway::runtime::tests::supervisor_serves_health_on_isolated_port") else { return; };
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let (accepted_tx, accepted_rx) = mpsc::channel();
        let supervisor = runtime.block_on(async { tokio::time::timeout(Duration::from_secs(5), Supervisor::start_inner(&base.join("state"), || {}, None, None, Some(accepted_tx))).await.unwrap().unwrap() });
        let address = supervisor.ready.address;
        let mut partial = std::net::TcpStream::connect_timeout(&address, Duration::from_secs(5)).unwrap();
        partial.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
        write!(partial, "GET /v1/health HTTP/1.1\r\nHost: {address}\r\n").unwrap();
        accepted_rx.recv_timeout(Duration::from_secs(5)).expect("partial connection was not accepted");
        let mut stream = std::net::TcpStream::connect_timeout(&address, Duration::from_secs(5)).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        stream.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
        write!(stream, "GET /v1/health HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {}\r\nConnection: close\r\n\r\n", supervisor.ready.token).unwrap();
        let mut response = String::new(); stream.read_to_string(&mut response).unwrap();
        assert!(response.starts_with("HTTP/1.1 200 "));
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let access = supervisor.access();
        let worker = runtime.spawn(async move { access.run(move |_| {
            started_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(15)).unwrap();
            Ok(())
        }).await });
        runtime.block_on(async { tokio::time::timeout(Duration::from_secs(5), async {
            while started_rx.try_recv().is_err() { tokio::task::yield_now().await; }
        }).await.unwrap(); });
        use std::os::windows::fs::OpenOptionsExt;
        let blocker = std::fs::OpenOptions::new().read(true).share_mode(3).open(base.join("state").join("endpoint.json")).unwrap();
        let shutdown = runtime.spawn(supervisor.shutdown());
        runtime.block_on(async { tokio::time::timeout(Duration::from_secs(5), async {
            loop { if std::net::TcpStream::connect_timeout(&address, Duration::from_millis(100)).is_err() { break; } tokio::time::sleep(Duration::from_millis(10)).await; }
        }).await.unwrap(); });
        let state = StateDirectory::acquire(&base.join("state")).unwrap();
        assert!(matches!(state.lock(), Err(super::super::windows_security::FileLockError::Contended)));
        runtime.block_on(async { tokio::time::sleep(Duration::from_secs(3)).await; });
        assert!(!shutdown.is_finished());
        partial.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        let mut byte = [0u8; 1];
        match partial.read(&mut byte) {
            Ok(0) => {},
            Err(error) if matches!(error.kind(), std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted) => {},
            other => panic!("partial connection remained open: {other:?}"),
        }
        release_tx.send(()).unwrap();
        runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(5), worker).await.unwrap().unwrap().unwrap();
            let error = tokio::time::timeout(Duration::from_secs(5), shutdown).await.unwrap().unwrap().unwrap_err();
            assert!(error.contains("HTTP drain timed out"));
            assert!(error.contains("endpoint withdrawal failed"));
        });
        assert!(state.lock().is_ok());
        drop(blocker);
        drop(partial);
    }
    #[test]
    fn cancel_after_ready_sent() {
        let Some(base) = isolated("gateway::runtime::tests::cancel_after_ready_sent") else { return; };
        let path = base.join("state");
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let (thread_tx, thread_rx) = mpsc::channel();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = oneshot::channel();
        let task = runtime.spawn(async move {
            Supervisor::start_inner(&path, || {}, Some(thread_tx), Some((entered_tx, release_rx)), None).await
        });
        let entered = runtime.block_on(async { tokio::time::timeout(Duration::from_secs(8), async {
            loop { match entered_rx.try_recv() {
                Ok(access) => return Some(access),
                Err(mpsc::TryRecvError::Disconnected) => return None,
                Err(mpsc::TryRecvError::Empty) => tokio::task::yield_now().await,
            }}
        }).await.unwrap_or(None) });
        let thread = thread_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        task.abort();
        let cancelled = runtime.block_on(async { matches!(task.await, Err(e) if e.is_cancelled()) });
        drop(release_tx);
        let result = join_bounded(thread);
        let access = entered.expect("ready handoff was not reached");
        assert!(cancelled);
        result.unwrap();
        assert_eq!(runtime.block_on(access.run(|_| Ok(()))).unwrap_err().status(), 503);
        assert!(!base.join("state").join("endpoint.json").exists());
        runtime.shutdown_timeout(Duration::from_secs(1));
    }
    #[test]
    fn drop_and_panic_wait_for_active_work() {
        let Some(root) = isolated("gateway::runtime::tests::drop_and_panic_wait_for_active_work") else { return; };
        for signal in [1, 2, 3, 4] {
            let mut random = [0u8; 16]; getrandom::fill(&mut random).unwrap();
            let base = root.join(format!("MindTrailSupervisorDrop-{random:?}"));
            std::fs::create_dir(&base).unwrap();
            struct Cleanup(PathBuf);
            impl Drop for Cleanup { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }
            let _cleanup = Cleanup(base.clone());
            let path = base.join("state");
            let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            let (thread_tx, thread_rx) = mpsc::channel();
            let mut supervisor = runtime.block_on(async { tokio::time::timeout(Duration::from_secs(5), Supervisor::start_inner(&path, || {}, Some(thread_tx), None, None)).await.unwrap().unwrap() });
            let thread = thread_rx.recv_timeout(Duration::from_secs(1)).unwrap();
            let access = supervisor.access();
            let reader = supervisor.reader();
            let status = supervisor.status.clone();
            let worker_access = access.clone();
            let (started_tx, started_rx) = mpsc::channel();
            let (release_tx, release_rx) = mpsc::channel();
            let worker = runtime.spawn(async move { worker_access.run(move |_| {
                let _ = started_tx.send(());
                let _ = release_rx.recv_timeout(Duration::from_secs(15));
                Ok(())
            }).await });
            let started = runtime.block_on(async { tokio::time::timeout(Duration::from_secs(5), async {
                loop { match started_rx.try_recv() { Ok(()) => return true, Err(mpsc::TryRecvError::Disconnected) => return false, Err(mpsc::TryRecvError::Empty) => tokio::task::yield_now().await } }
            }).await.unwrap_or(false) });
            let _ = supervisor.control.stop.take().unwrap().send(signal);
            let closed = runtime.block_on(async { tokio::time::timeout(Duration::from_secs(5), async {
                loop { if access.run(|_| Ok(())).await.is_err() { break; } tokio::task::yield_now().await; }
            }).await.is_ok() });
            runtime.block_on(async { tokio::time::timeout(Duration::from_secs(5), async {
                loop { if *status.lock().unwrap() == (GatewayStatus::Unavailable { message: "gateway stopping".into() }) { break; } tokio::task::yield_now().await; }
            }).await.unwrap(); });
            assert_eq!(runtime.block_on(reader.sessions(None, 10)).unwrap_err().status(), 503);
            let contender = StateDirectory::acquire(&path);
            let held = contender.as_ref().map(|state| matches!(state.lock(), Err(super::super::windows_security::FileLockError::Contended))).unwrap_or(false);
            let early_result = runtime.block_on(async { tokio::time::timeout(Duration::from_millis(200), supervisor.wait_completion()).await });
            let early = early_result.is_ok();
            let _ = release_tx.send(());
            let (worker_result, completion) = runtime.block_on(async {
                let worker_result = tokio::time::timeout(Duration::from_secs(5), worker).await;
                let completion = match early_result { Ok(value) => Ok(value), Err(_) => tokio::time::timeout(Duration::from_secs(5), supervisor.wait_completion()).await };
                (worker_result, completion)
            });
            let lock_reacquired = completion.is_ok() && contender.as_ref().map(|state| state.lock().is_ok()).unwrap_or(false);
            let joined = join_bounded(thread);
            assert!(started && closed && held && !early);
            assert!(matches!(worker_result, Ok(Ok(Ok(())))));
            let completion = completion.unwrap();
            match signal {
                1 => assert_eq!(completion, Ok(())),
                2 => assert!(completion.is_err()),
                3 => assert_eq!(completion, Err("HTTP drain timed out".to_owned())),
                4 => assert_eq!(completion, Err("HTTP service failed: injected serve failure".to_owned())),
                _ => unreachable!(),
            }
            let expected = match signal { 1 => "gateway stopped", 2 => "gateway supervisor stopped unexpectedly", 3 => "HTTP drain timed out", 4 => "HTTP service failed: injected serve failure", _ => unreachable!() };
            assert_eq!(*status.lock().unwrap(), GatewayStatus::Unavailable { message: expected.into() });
            assert!(joined.is_ok());
            assert!(lock_reacquired);
            runtime.shutdown_timeout(Duration::from_secs(1));
        }
    }
    #[test]
    fn cancelled_start_receiver_releases_lock() {
        let Some(root) = isolated("gateway::runtime::tests::cancelled_start_receiver_releases_lock") else { return; };
        let mut random = [0u8; 16]; getrandom::fill(&mut random).unwrap();
        let base = root.join(format!("MindTrailCancelStart-{random:?}"));
        std::fs::create_dir(&base).unwrap();
        struct Cleanup(PathBuf);
        impl Drop for Cleanup { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }
        let _cleanup = Cleanup(base.clone());
        let path = base.join("state");
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (thread_tx, thread_rx) = mpsc::channel();
        let worker_path = path.clone();
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let task = runtime.spawn(async move {
            Supervisor::start_inner(&worker_path, move || {
                let _ = entered_tx.send(());
                let _ = release_rx.recv_timeout(Duration::from_secs(5));
            }, Some(thread_tx), None, None).await
        });
        let entered = runtime.block_on(async { tokio::time::timeout(Duration::from_secs(8), async {
            loop { match entered_rx.try_recv() {
                Ok(()) => return true,
                Err(mpsc::TryRecvError::Disconnected) => return false,
                Err(mpsc::TryRecvError::Empty) => tokio::task::yield_now().await,
            }}
        }).await.unwrap_or(false) });
        let thread = thread_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        if !entered {
            drop(release_tx); task.abort();
            let _ = join_bounded(thread);
            panic!("supervisor failed to reach handoff");
        }
        task.abort();
        runtime.block_on(async { assert!(matches!(task.await, Err(e) if e.is_cancelled())); });
        let released = release_tx.send(());
        let joined = join_bounded(thread);
        assert!(released.is_ok());
        joined.unwrap();
        assert!(!path.join("endpoint.json").exists());
        let state = StateDirectory::acquire(&path).unwrap();
        assert!(state.lock().is_ok());
    }
    #[test]
    fn shutdown_waits_for_work_and_holds_cross_process_lock() {
        const CHILD: &str = "MINDTRAIL_SUPERVISOR_CHILD_FIXTURE";
        if let Some(base) = std::env::var_os(CHILD) {
            let base = PathBuf::from(base);
            let path = base.join("state");
            let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            let mut supervisor = runtime.block_on(async { tokio::time::timeout(Duration::from_secs(5), Supervisor::start(&path)).await.unwrap().unwrap() });
            let access = supervisor.access();
            let (started_tx, started_rx) = mpsc::channel();
            let (release_tx, release_rx) = mpsc::channel();
            let worker_access = access.clone();
            let worker = runtime.spawn(async move { worker_access.run(move |_| {
                let _ = started_tx.send(());
                let _ = release_rx.recv_timeout(Duration::from_secs(20));
                Ok(())
            }).await });
            runtime.block_on(async { tokio::time::timeout(Duration::from_secs(5), async {
                loop { match started_rx.try_recv() { Ok(()) => break, Err(mpsc::TryRecvError::Disconnected) => panic!("worker exited"), Err(mpsc::TryRecvError::Empty) => tokio::task::yield_now().await } }
            }).await.unwrap(); });
            supervisor.request_shutdown();
            runtime.block_on(async { tokio::time::timeout(Duration::from_secs(5), async {
                loop { if let Err(error) = access.run(|_| Ok(())).await { assert_eq!(error.status(), 503); break; } tokio::task::yield_now().await; }
            }).await.unwrap(); });
            let shutdown = runtime.spawn(supervisor.shutdown());
            runtime.block_on(async { assert!(tokio::time::timeout(Duration::from_millis(200), async { while !shutdown.is_finished() { tokio::task::yield_now().await; } }).await.is_err()); });
            std::fs::write(base.join("blocked"), b"ready").unwrap();
            let deadline = Instant::now() + Duration::from_secs(10);
            while !base.join("release").exists() { assert!(Instant::now() < deadline); std::thread::sleep(Duration::from_millis(10)); }
            release_tx.send(()).unwrap();
            runtime.block_on(async { tokio::time::timeout(Duration::from_secs(5), worker).await.unwrap().unwrap().unwrap(); tokio::time::timeout(Duration::from_secs(5), shutdown).await.unwrap().unwrap().unwrap(); });
            runtime.shutdown_timeout(Duration::from_secs(1));
            return;
        }
        let mut random = [0u8; 16]; getrandom::fill(&mut random).unwrap();
        let base = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailSupervisor-{random:?}"));
        std::fs::create_dir(&base).unwrap();
        let mut fixture = ChildFixture { child: None, base: base.clone() };
        fixture.child = Some(std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "gateway::runtime::tests::shutdown_waits_for_work_and_holds_cross_process_lock"])
            .env(CHILD, &base).spawn().unwrap());
        let deadline = Instant::now() + Duration::from_secs(12);
        while !base.join("blocked").exists() {
            if let Some(status) = fixture.child.as_mut().unwrap().try_wait().unwrap() { panic!("child exited before barrier: {status}"); }
            assert!(Instant::now() < deadline, "child barrier timed out");
            std::thread::sleep(Duration::from_millis(10));
        }
        let state = StateDirectory::acquire(&base.join("state")).unwrap();
        assert!(matches!(state.lock(), Err(super::super::windows_security::FileLockError::Contended)));
        std::fs::write(base.join("release"), b"go").unwrap();
        let deadline = Instant::now() + Duration::from_secs(8);
        let status = loop {
            if let Some(status) = fixture.child.as_mut().unwrap().try_wait().unwrap() { break status; }
            assert!(Instant::now() < deadline, "child exit timed out");
            std::thread::sleep(Duration::from_millis(10));
        };
        assert!(status.success());
        assert!(state.lock().is_ok());
        drop(state);
    }
    #[test]
    fn startup_failure_reports_stage() {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let result = runtime.block_on(async { tokio::time::timeout(Duration::from_secs(5), Supervisor::start(Path::new("relative-path"))).await }).unwrap();
        let error = match result { Ok(_) => panic!("relative path accepted"), Err(e) => e };
        assert_eq!(error.stage, "directory");
        assert!(!error.message.is_empty());
    }
}





use std::sync::Mutex;
use serde::Serialize;
use super::http::ReadService;
use super::store::{EventPage, SessionSummary};

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(crate) enum DesktopStatus {
    Ready { store_id: String },
    Unavailable { message: String },
}

#[derive(Serialize)]
pub(crate) struct SessionsResponse { store_id: String, sessions: Vec<SessionSummary> }
#[derive(Serialize)]
pub(crate) struct EventsResponse { store_id: String, page: EventPage }

pub(crate) struct DesktopState { core: std::sync::Arc<Core>, #[cfg(windows)] worker: Mutex<Option<std::sync::mpsc::Sender<()>>>, #[cfg(all(test, windows))] test_shutdown: Mutex<Option<(u8, usize)>> }
pub(crate) struct Core { inner: Mutex<Manager>, changed: tokio::sync::Notify, #[cfg(all(test, windows))] before_command: Mutex<Option<(std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>)>>, #[cfg(all(test, windows))] fault: std::sync::atomic::AtomicU8, #[cfg(all(test, windows))] fail_spawn: std::sync::atomic::AtomicUsize, #[cfg(all(test, windows))] stop_before_command: std::sync::atomic::AtomicBool, #[cfg(all(test, windows))] stopped_before_command: std::sync::atomic::AtomicBool, #[cfg(all(test, windows))] before_start_panic: Mutex<Option<(std::sync::mpsc::Sender<super::access::StoreAccess>, std::sync::mpsc::Receiver<()>)>>, #[cfg(all(test, windows))] before_install: Mutex<Option<(std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>, u8)>> }
impl std::ops::Deref for DesktopState { type Target = Core; fn deref(&self) -> &Core { &self.core } }
struct Manager { slot: Slot, exiting: bool, starting: usize, closing: bool, errors: ErrorLog, result: Option<Result<(), String>>, #[cfg(windows)] fatal: Option<String>, #[cfg(windows)] retry_fault: Option<String>, #[cfg(windows)] fault_history: ErrorLog, #[cfg(windows)] recovering: usize, #[cfg(windows)] pending_close: Option<super::runtime::Supervisor>, #[cfg(windows)] queue: std::collections::VecDeque<Command>, #[cfg(windows)] stranded: Vec<Stranded> }
#[cfg(windows)]
enum Command { Start(std::path::PathBuf, tokio::sync::oneshot::Sender<Result<(), String>>, super::runtime::SessionNotifier), Close, Recover(Stranded) }
#[cfg(windows)]
struct Stranded { owner: Cleanup, starting: bool }
#[cfg(windows)]
enum Cleanup { Pending(super::runtime::PendingStart), Ready(super::runtime::Supervisor) }
#[cfg(windows)]
impl Cleanup {
    fn request(&mut self) { match self { Self::Pending(p) => p.request_shutdown(), Self::Ready(s) => s.request_shutdown() } }
    fn finish(&mut self) -> Result<(), String> {
        match self {
            Self::Pending(p) => { p.request_shutdown(); p.wait_completion_blocking() }
            Self::Ready(s) => { s.request_shutdown(); s.wait_completion_blocking() }
        }
    }
}
// Keep the earliest diagnostics, and explicitly count anything that does not
// fit. Both the number of entries and the size of each entry are bounded.
#[derive(Default)]
struct ErrorLog { entries: Vec<String>, count: u64, omitted: u64 }
impl ErrorLog {
    fn bounded(error: String) -> String {
        let mut chars = error.chars();
        let mut message: String = chars.by_ref().take(512).collect();
        if chars.next().is_some() { message.push_str("… [diagnostic truncated]"); }
        message
    }
    fn push(&mut self, error: String) { self.push_bounded(Self::bounded(error)); }
    fn push_bounded(&mut self, error: String) {
        self.count = self.count.saturating_add(1);
        if self.entries.len() == 32 { self.omitted = self.omitted.saturating_add(1); return; }
        self.entries.push(error);
    }
    fn is_empty(&self) -> bool { self.count == 0 }
    fn render(&self) -> String {
        let mut result = self.entries.join("; ");
        if self.omitted > 0 { result.push_str(&format!("; {} additional diagnostics omitted", self.omitted)); }
        result
    }
}
enum Slot {
    Unavailable(String),
    #[cfg(windows)] Ready(super::runtime::Supervisor),
    #[cfg(test)] TestReady { reader: ReadService, store_id: String },
}
impl Manager {
    #[cfg(windows)]
    fn publish_fault(&mut self, error: String, recoverable: bool) {
        let error = ErrorLog::bounded(error);
        self.fault_history.push_bounded(error.clone());
        self.errors.push_bounded(error.clone());
        if recoverable { self.retry_fault = Some(error); }
        else if self.fatal.is_none() { self.fatal = Some(format!("Unrecoverable: {error}")); }
    }
    fn finish_if_ready(&mut self) {
        if self.exiting && self.starting == 0 && !self.closing && self.result.is_none()
            && { #[cfg(windows)] { self.fatal.is_none() && self.recovering == 0 } #[cfg(not(windows))] { true } } {
            self.result = Some(if self.errors.is_empty() { Ok(()) } else { Err(self.errors.render()) });
        }
    }
}
impl Default for DesktopState {
    fn default() -> Self {
        #[cfg(windows)] let message = "Gateway has not started. Restart the application or check the gateway data directory.";
        #[cfg(not(windows))] let message = "The local gateway is currently available on Windows only.";
        Self { core: std::sync::Arc::new(Core { inner: Mutex::new(Manager { slot: Slot::Unavailable(message.into()), exiting: false, starting: 0, closing: false, errors: ErrorLog::default(), result: None, #[cfg(windows)] fatal: None, #[cfg(windows)] retry_fault: None, #[cfg(windows)] fault_history: ErrorLog::default(), #[cfg(windows)] recovering: 0, #[cfg(windows)] pending_close: None, #[cfg(windows)] queue: std::collections::VecDeque::new(), #[cfg(windows)] stranded: Vec::new() }), changed: tokio::sync::Notify::new(), #[cfg(all(test, windows))] before_command: Mutex::new(None), #[cfg(all(test, windows))] fault: std::sync::atomic::AtomicU8::new(0), #[cfg(all(test, windows))] fail_spawn: std::sync::atomic::AtomicUsize::new(0), #[cfg(all(test, windows))] stop_before_command: std::sync::atomic::AtomicBool::new(false), #[cfg(all(test, windows))] stopped_before_command: std::sync::atomic::AtomicBool::new(false), #[cfg(all(test, windows))] before_start_panic: Mutex::new(None), #[cfg(all(test, windows))] before_install: Mutex::new(None) }), #[cfg(windows)] worker: Mutex::new(None), #[cfg(all(test, windows))] test_shutdown: Mutex::new(None) }
    }
}
impl DesktopState {
    pub(crate) async fn connection_read_check(&self) -> Result<(), String> {
        self.connection_read_check_with_hook(|| {}).await
    }
    async fn connection_read_check_with_hook(&self, hook: impl FnOnce()) -> Result<(), String> {
        let (reader, store_id)=self.snapshot()?;
        reader.sessions(None,1).await.map_err(|_|"gateway_unavailable".to_owned())?;
        hook();
        match self.status() { DesktopStatus::Ready { store_id: current } if current == store_id => Ok(()), _ => Err("identity_changed".into()) }
    }
    pub(crate) async fn semantic_snapshot(&self, context: crate::semantic::types::SemanticContext) -> Result<crate::semantic::types::SemanticInput, String> {
        let (reader, expected_store) = self.snapshot()?;
        if context.store_id != expected_store { return Err("invalid_context".into()); }
        let (store_id, events) = reader.semantic_prefix(context.session_id.clone(), context.prefix_seq).await.map_err(|_| "snapshot_unavailable".to_owned())?;
        crate::semantic::snapshot::build_input(context, &store_id, events).map_err(str::to_owned)
    }
    #[cfg(all(test, windows))]
    pub(crate) fn test_shutdown_mode(&self, mode: u8) { self.test_shutdown.lock().unwrap().get_or_insert((mode, 0)).0 = mode; self.changed.notify_waiters(); }
    #[cfg(all(test, windows))]
    pub(crate) fn test_shutdown_calls(&self) -> usize { self.test_shutdown.lock().unwrap().unwrap().1 }
    #[cfg(all(test, windows))]
    pub(crate) fn test_lock_available(path: &std::path::Path) -> std::io::Result<bool> {
        let dir = super::windows_security::StateDirectory::acquire(path)?;
        match dir.lock() {
            Ok(_) => Ok(true),
            Err(super::windows_security::FileLockError::Contended) => Ok(false),
            Err(super::windows_security::FileLockError::System(error)) => Err(error),
        }
    }
    #[cfg(all(test, windows))]
    pub(crate) fn test_pause_before_install(&self, entered: std::sync::mpsc::Sender<()>, release: std::sync::mpsc::Receiver<()>) {
        *self.before_install.lock().unwrap() = Some((entered, release, 255));
    }
    #[cfg(all(test, windows))]
    pub(crate) async fn test_hold_work(&self, entered: std::sync::mpsc::Sender<()>, release: std::sync::mpsc::Receiver<()>) {
        let access = match &self.inner.lock().unwrap().slot { Slot::Ready(s) => s.access(), _ => panic!("gateway not ready") };
        access.run(move |_| { let _ = entered.send(()); let _ = release.recv_timeout(std::time::Duration::from_secs(20)); Ok(()) }).await.unwrap();
    }
    #[cfg(windows)]
    fn sender(&self) -> Result<std::sync::mpsc::Sender<()>, String> {
        let mut worker = self.worker.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(sender) = worker.as_ref() { return Ok(sender.clone()); }
        #[cfg(test)]
        if let Ok(n) = self.core.fail_spawn.fetch_update(std::sync::atomic::Ordering::SeqCst, std::sync::atomic::Ordering::SeqCst, |n| n.checked_sub(1)) { return Err(format!("Gateway worker creation failed (injected {n}).")); }
        let (sender, receiver) = std::sync::mpsc::channel();
        let core = self.core.clone();
        std::thread::Builder::new().name("gateway-desktop".into()).spawn(move || {
            while receiver.recv().is_ok() {
                loop {
                    #[cfg(test)]
                    if let Some((entered, release)) = core.before_command.lock().unwrap_or_else(|e| e.into_inner()).take() {
                        let _ = entered.send(());
                        let _ = release.recv_timeout(std::time::Duration::from_secs(10));
                    }
                    #[cfg(test)]
                    if core.stop_before_command.swap(false, std::sync::atomic::Ordering::SeqCst) {
                        drop(receiver);
                        core.stopped_before_command.store(true, std::sync::atomic::Ordering::SeqCst);
                        return;
                    }
                    let command = core.inner.lock().unwrap_or_else(|e| e.into_inner()).queue.pop_front();
                    let Some(command) = command else { break; };
                    let was_recovery = matches!(&command, Command::Recover(_));
                    let starting = match &command { Command::Start(..) => true, Command::Close => false, Command::Recover(item) => item.starting };
                    let mut cleanup = None;
                    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| core.run(command, &mut cleanup))).is_err() {
                        if let Some(owner) = cleanup.as_mut() { owner.request(); }
                        let mut state = core.inner.lock().unwrap_or_else(|e| e.into_inner());
                        let recoverable = if let Some(owner) = cleanup.take() {
                            state.stranded.push(Stranded { owner, starting });
                            if !was_recovery { state.recovering += 1; }
                            true
                        } else { false };
                        state.errors.push("Gateway worker failed.".into());
                        state.publish_fault("Gateway worker failed before cleanup was confirmed.".into(), recoverable);
                        drop(state);
                        core.changed.notify_waiters();
                    }
                }
            }
        }).map_err(|e| format!("Gateway worker failed: {e}"))?;
        *worker = Some(sender.clone());
        Ok(sender)
    }
    #[cfg(windows)]
    fn wake_worker(&self, sender: &std::sync::mpsc::Sender<()>) -> Result<bool, String> {
        if sender.send(()).is_ok() { return Ok(false); }
        {
            let mut worker = self.worker.lock().unwrap_or_else(|e| e.into_inner());
            // Another caller may already have installed a live replacement.
            if let Some(current) = worker.as_ref() {
                if current.send(()).is_ok() { return Ok(true); }
            }
            *worker = None;
        }
        let replacement = self.sender()?;
        replacement.send(()).map_err(|_| "Gateway worker failed.".to_owned())?;
        Ok(true)
    }
    #[cfg(windows)]
    pub(super) async fn start(self: &std::sync::Arc<Self>, path: &std::path::Path) -> Result<(), String> {
        self.start_with_notify(path, std::sync::Arc::new(|_| {})).await
    }
    #[cfg(windows)]
    pub(crate) async fn start_with_notify(self: &std::sync::Arc<Self>, path: &std::path::Path, notify: super::runtime::SessionNotifier) -> Result<(), String> {
        let (reply, received) = tokio::sync::oneshot::channel();
        {
            let mut state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            if state.exiting { return Err("Gateway is stopping.".into()); }
            let sender = self.sender()?;
            state.starting += 1;
            state.queue.push_back(Command::Start(path.to_owned(), reply, notify));
            match self.wake_worker(&sender) {
                Ok(false) => {},
                Ok(true) => state.errors.push("Gateway worker failed; recovered.".into()),
                Err(error) => {
                    state.errors.push(error.clone());
                    state.queue.pop_back();
                    state.starting -= 1;
                    return Err(format!("Gateway cleanup is unconfirmed; retry is required: {error}"));
                }
            }
        }
        received.await.unwrap_or_else(|_| Err("Gateway startup task failed.".into()))
    }
    #[cfg(windows)]
    pub(crate) async fn shutdown(self: &std::sync::Arc<Self>) -> Result<(), String> {
        #[cfg(test)]
        {
            let mut injection = self.test_shutdown.lock().unwrap();
            if let Some((mode, calls)) = injection.as_mut() {
                *calls += 1;
                let panic_now = *mode == 3;
                if panic_now { *mode = 0; }
                drop(injection);
                if panic_now { panic!("injected shutdown panic"); }
                return Err("injected shutdown diagnostic".into());
            }
        }
        let initial_fault_index;
        {
            let mut state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            initial_fault_index = state.fault_history.count;
            if !state.exiting {
                state.exiting = true;
                if let Slot::Ready(mut supervisor) = std::mem::replace(&mut state.slot, Slot::Unavailable("Gateway is stopping.".into())) {
                    supervisor.request_shutdown();
                    state.closing = true;
                    state.pending_close = Some(supervisor);
                    state.queue.push_back(Command::Close);
                }
            }
            if !state.stranded.is_empty() {
                let stranded = std::mem::take(&mut state.stranded);
                for item in stranded { state.queue.push_back(Command::Recover(item)); }
            }
            if !state.queue.is_empty() {
                let wake = self.sender().and_then(|sender| self.wake_worker(&sender));
                match wake {
                    Ok(recovered) => {
                        if recovered { state.errors.push("Gateway worker failed; recovered.".into()); }
                        state.retry_fault = None;
                    }
                    Err(error) => {
                        let diagnostic = format!("Gateway cleanup is unconfirmed; retry is required: {error}");
                        state.publish_fault(diagnostic.clone(), true);
                        let returned = match &state.fatal { Some(fatal) => format!("{fatal}; {diagnostic}"), None => diagnostic };
                        drop(state);
                        self.changed.notify_waiters();
                        return Err(returned);
                    }
                }
            }
            state.finish_if_ready();
        }
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            {
                let state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(result) = state.result.clone() { return result; }
                if let Some(fatal) = &state.fatal { return Err(format!("{fatal}; {}", state.fault_history.render())); }
                if state.fault_history.count > initial_fault_index { return Err(state.fault_history.render()); }
                if let Some(error) = &state.retry_fault { return Err(error.clone()); }
            }
            notified.await;
        }
    }
    #[cfg(windows)]
    pub(crate) fn confirmed_shutdown_result(&self) -> Option<Result<(), String>> {
        #[cfg(test)]
        if let Some((mode, _)) = *self.test_shutdown.lock().unwrap() { return (mode == 2).then(|| Err("injected shutdown diagnostic".into())); }
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).result.clone()
    }
    #[cfg(windows)]
    pub(crate) fn shutdown_is_fatal(&self) -> bool {
        #[cfg(test)]
        if let Some((mode, _)) = *self.test_shutdown.lock().unwrap() { return mode == 1; }
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).fatal.is_some()
    }
    #[cfg(windows)]
    pub(crate) fn report_unconfirmed_shutdown(&self, message: &str) {
        let mut state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if state.result.is_none() {
            state.slot = Slot::Unavailable(format!("Gateway shutdown is unconfirmed. Keep the application open and contact support. {message}"));
        }
    }
    #[cfg(windows)]
    pub(crate) async fn wait_confirmed_shutdown(&self) -> Result<(), String> {
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if let Some(result) = self.confirmed_shutdown_result() { return result; }
            notified.await;
        }
    }
    pub(crate) fn record_start_error(&self, message: String) {
        let mut state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if !state.exiting && matches!(state.slot, Slot::Unavailable(_)) { state.slot = Slot::Unavailable(format!("{message} Check the gateway data directory and restart the application.")); }
    }
    #[cfg(test)]
    fn set_unavailable(&self, message: String) {
        let old = std::mem::replace(&mut self.inner.lock().unwrap_or_else(|e| e.into_inner()).slot, Slot::Unavailable(message));
        drop(old);
    }
    pub(crate) fn status(&self) -> DesktopStatus {
        let state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        match &state.slot {
            Slot::Unavailable(message) => DesktopStatus::Unavailable { message: message.clone() },
            #[cfg(windows)] Slot::Ready(supervisor) => match supervisor.status() {
                super::runtime::GatewayStatus::Ready { store_id } => DesktopStatus::Ready { store_id },
                super::runtime::GatewayStatus::Unavailable { message } => DesktopStatus::Unavailable { message },
            },
            #[cfg(test)] Slot::TestReady { store_id, .. } => DesktopStatus::Ready { store_id: store_id.clone() },
        }
    }
    fn snapshot(&self) -> Result<(ReadService, String), String> {
        let state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        match &state.slot {
            Slot::Unavailable(message) => Err(message.clone()),
            #[cfg(windows)] Slot::Ready(supervisor) => match supervisor.status() {
                super::runtime::GatewayStatus::Ready { store_id } => Ok((supervisor.reader(), store_id)),
                super::runtime::GatewayStatus::Unavailable { message } => Err(message),
            },
            #[cfg(test)] Slot::TestReady { reader, store_id } => Ok((reader.clone(), store_id.clone())),
        }
    }
}

#[cfg(windows)]
impl Core {
    fn run(&self, command: Command, cleanup: &mut Option<Cleanup>) {
        let (path, reply, starting, recovering) = match command {
            Command::Start(path, reply, notify) => (Some((path, notify)), Some(reply), true, false),
            Command::Close => (None, None, false, false),
            Command::Recover(item) => { *cleanup = Some(item.owner); (None, None, item.starting, true) },
        };
        if !starting && !recovering { *cleanup = self.inner.lock().unwrap_or_else(|e| e.into_inner()).pending_close.take().map(Cleanup::Ready); }
        if !starting && !recovering && cleanup.is_none() {
            let mut state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            state.errors.push("Gateway close ownership missing.".into());
            state.publish_fault("Gateway close ownership missing; cleanup is unconfirmed.".into(), false);
            drop(state);
            self.changed.notify_waiters();
            return;
        }
        #[cfg(test)]
        if !starting && !recovering && self.fault.compare_exchange(6, 0, std::sync::atomic::Ordering::SeqCst, std::sync::atomic::Ordering::SeqCst).is_ok() { panic!("injected outer panic"); }
        let mut outcome = Ok(());
        let mut startup_completion = None;
        let operation = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            #[cfg(test)]
            if !starting && !recovering && self.fault.compare_exchange(2, 0, std::sync::atomic::Ordering::SeqCst, std::sync::atomic::Ordering::SeqCst).is_ok() { panic!("injected close panic"); }
            if let Some((path, notify)) = path {
                match super::runtime::Supervisor::begin_start_with_notify(&path, notify) {
                    Err(e) => outcome = Err(format!("Gateway {} failed: {}", e.stage, e.message)),
                    Ok(pending) => {
                        *cleanup = Some(Cleanup::Pending(pending));
                        let ready = match cleanup.as_mut().unwrap() { Cleanup::Pending(p) => p.wait_ready_blocking(), _ => unreachable!() };
                        match ready {
                            Err(e) => {
                                startup_completion = Some(format!("{}: {}", e.stage, e.message));
                                outcome = Err(format!("Gateway {} failed: {}", e.stage, e.message));
                            }
                            Ok(supervisor) => {
                                *cleanup = Some(Cleanup::Ready(supervisor));
                                #[cfg(test)]
                                if self.fault.compare_exchange(1, 0, std::sync::atomic::Ordering::SeqCst, std::sync::atomic::Ordering::SeqCst).is_ok() {
                                    if let Some((entered, release)) = self.before_start_panic.lock().unwrap_or_else(|e| e.into_inner()).take() {
                                        if let Some(Cleanup::Ready(s)) = cleanup.as_ref() { let _ = entered.send(s.access()); }
                                        let _ = release.recv_timeout(std::time::Duration::from_secs(10));
                                    }
                                    panic!("injected startup panic");
                                }
                                #[cfg(test)]
                                if let Some((entered, release, signal)) = self.before_install.lock().unwrap_or_else(|e| e.into_inner()).take() {
                                    let _ = entered.send(());
                                    let _ = release.recv_timeout(std::time::Duration::from_secs(10));
                                    if signal != 255 { if let Some(Cleanup::Ready(s)) = cleanup.as_mut() { s.request_shutdown_injected(signal); } }
                                }
                                let mut state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
                                if state.exiting { outcome = Err("Gateway is stopping.".into()); }
                                else {
                                    let supervisor = match cleanup.take().unwrap() { Cleanup::Ready(s) => s, _ => unreachable!() };
                                    let old = std::mem::replace(&mut state.slot, Slot::Ready(supervisor));
                                    if let Slot::Ready(old) = old { *cleanup = Some(Cleanup::Ready(old)); }
                                }
                            }
                        }
                    }
                }
                if let Err(error) = &outcome {
                    let mut state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
                    if !state.exiting && matches!(state.slot, Slot::Unavailable(_)) { state.slot = Slot::Unavailable(error.clone()); }
                }
            }
        }));
        #[cfg(test)]
        if starting && cleanup.is_some() && self.fault.compare_exchange(8, 0, std::sync::atomic::Ordering::SeqCst, std::sync::atomic::Ordering::SeqCst).is_ok() { panic!("injected outer startup panic"); }
        let panic_error = operation.is_err().then(|| if starting { "Gateway startup task failed." } else { "Gateway shutdown task failed." }.to_owned());
        // The owner stays outside the unwind boundary, so a failed wait cannot
        // destroy the only control handle. A subsequent wait proves completion.
        if let Some(owner) = cleanup.as_mut() { owner.request(); }
        let mut wait_panicked = false;
        let mut failures = 0;
        let cleanup_result = loop {
            let waited = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                #[cfg(test)]
                if self.fault.load(std::sync::atomic::Ordering::SeqCst) == 5 || self.fault.compare_exchange(3, 0, std::sync::atomic::Ordering::SeqCst, std::sync::atomic::Ordering::SeqCst).is_ok() { panic!("injected cleanup panic"); }
                cleanup.as_mut().map(Cleanup::finish).unwrap_or(Ok(()))
            }));
            match waited {
                Ok(result) => break result,
                Err(_) => {
                    wait_panicked = true;
                    failures += 1;
                    if failures >= 2 {
                        let error = "Gateway cleanup is unconfirmed after repeated wait failure.".to_owned();
                        let mut state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
                        if let Some(owner) = cleanup.take() { state.stranded.push(Stranded { owner, starting }); if !recovering { state.recovering += 1; } }
                        state.errors.push("Gateway cleanup task failed.".into());
                        state.publish_fault(error.clone(), true);
                        drop(state);
                        self.changed.notify_waiters();
                        if let Some(reply) = reply { let _ = reply.send(Err(error)); }
                        return;
                    }
                }
            }
        };
        {
            let mut state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(error) = &panic_error { state.errors.push(error.clone()); }
            if wait_panicked { state.errors.push("Gateway cleanup task failed.".into()); }
            if let Err(error) = &cleanup_result { if startup_completion.as_ref() != Some(error) { state.errors.push(error.clone()); } }
            if recovering { state.recovering -= 1; if state.recovering == 0 { state.retry_fault = None; } }
            if starting { state.starting -= 1; } else { state.closing = false; }
            state.finish_if_ready();
        }
        self.changed.notify_waiters();
        if let Some(reply) = reply { let _ = reply.send(if let Some(error) = panic_error { Err(error) } else if wait_panicked { Err("Gateway cleanup task failed.".into()) } else { outcome.and(cleanup_result) }); }
    }
}
#[tauri::command]
pub(crate) fn gateway_status(window: tauri::WebviewWindow, state: tauri::State<'_, std::sync::Arc<DesktopState>>) -> Result<DesktopStatus, String> {
    require_workbench(&window)?;
    Ok(state.status())
}
fn require_workbench(window: &tauri::WebviewWindow) -> Result<(), String> {
    if window.label() == "main" { Ok(()) } else { Err("Gateway commands are only available in the workbench.".into()) }
}
#[tauri::command]
pub(crate) async fn list_sessions(window: tauri::WebviewWindow, state: tauri::State<'_, std::sync::Arc<DesktopState>>, after: Option<String>, limit: u16) -> Result<SessionsResponse, String> {
    require_workbench(&window)?;
    let (reader, store_id) = state.snapshot()?;
    let sessions = reader.sessions(after, limit).await.map_err(|_| "Unable to read sessions. Check that the gateway is running.".to_owned())?;
    Ok(SessionsResponse { store_id, sessions })
}
#[tauri::command]
pub(crate) async fn read_events(window: tauri::WebviewWindow, state: tauri::State<'_, std::sync::Arc<DesktopState>>, session: String, since: u64, limit: u16) -> Result<EventsResponse, String> {
    require_workbench(&window)?;
    let (reader, store_id) = state.snapshot()?;
    let page = reader.events(session, since, limit).await.map_err(|_| "Unable to read events. Check that the gateway is running.".to_owned())?;
    Ok(EventsResponse { store_id, page })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    #[cfg(windows)] use std::future::Future;
    use super::super::access::StoreAccess;
    use super::super::store::Store;
    #[cfg(windows)]
    #[test]
    fn cancelled_start_and_concurrent_shutdown_settle() {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let manager = Arc::new(DesktopState::default());
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        *manager.before_command.lock().unwrap() = Some((entered_tx, release_rx));
        runtime.block_on(async {
            {
                let start = manager.start(std::path::Path::new("relative-path"));
                tokio::pin!(start);
                std::future::poll_fn(|cx| { assert!(start.as_mut().poll(cx).is_pending()); std::task::Poll::Ready(()) }).await;
            }
            entered_rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
            let shutdown = manager.shutdown();
            tokio::pin!(shutdown);
            std::future::poll_fn(|cx| { assert!(shutdown.as_mut().poll(cx).is_pending()); std::task::Poll::Ready(()) }).await;
            release_tx.send(()).unwrap();
            let (a, b) = tokio::time::timeout(std::time::Duration::from_secs(10), async { tokio::join!(shutdown, manager.shutdown()) }).await.unwrap();
            assert_eq!(a, b);
            assert!(a.is_ok());
        });
        let state = manager.inner.lock().unwrap();
        assert_eq!(state.starting, 0);
        assert!(!state.closing);
    }
    #[cfg(windows)]
    #[test]
    fn start_after_shutdown_does_not_create_worker() {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let manager = Arc::new(DesktopState::default());
        runtime.block_on(async {
            manager.shutdown().await.unwrap();
            manager.fail_spawn.store(1, std::sync::atomic::Ordering::SeqCst);
            assert_eq!(manager.start(std::path::Path::new("relative-path")).await.unwrap_err(), "Gateway is stopping.");
            assert_eq!(manager.fail_spawn.load(std::sync::atomic::Ordering::SeqCst), 1);
            assert!(manager.worker.lock().unwrap().is_none());
            assert!(manager.shutdown().await.is_ok());
        });
    }
    #[cfg(windows)]
    #[test]
    fn accepted_start_survives_worker_exit_and_repeated_spawn_failure() {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let manager = Arc::new(DesktopState::default());
        let (entered_tx, entered_rx) = std::sync::mpsc::channel(); let (release_tx, release_rx) = std::sync::mpsc::channel();
        *manager.before_command.lock().unwrap() = Some((entered_tx, release_rx));
        manager.stop_before_command.store(true, std::sync::atomic::Ordering::SeqCst);
        runtime.block_on(async {
            {
                let start = manager.start(std::path::Path::new("relative-path")); tokio::pin!(start);
                std::future::poll_fn(|cx| { assert!(start.as_mut().poll(cx).is_pending()); std::task::Poll::Ready(()) }).await;
            }
            entered_rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
            let waiter = manager.shutdown(); let delayed = manager.shutdown(); tokio::pin!(waiter); tokio::pin!(delayed);
            std::future::poll_fn(|cx| { assert!(waiter.as_mut().poll(cx).is_pending()); assert!(delayed.as_mut().poll(cx).is_pending()); std::task::Poll::Ready(()) }).await;
            release_tx.send(()).unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(5), async { while !manager.stopped_before_command.load(std::sync::atomic::Ordering::SeqCst) { tokio::task::yield_now().await; } }).await.unwrap();
            manager.fail_spawn.store(2, std::sync::atomic::Ordering::SeqCst);
            for attempt in 0..2 {
                let error = manager.shutdown().await.unwrap_err();
                if attempt == 1 { let combined = waiter.as_mut().await.unwrap_err(); assert!(combined.contains("injected 2") && combined.contains("injected 1")); }
                assert!(error.contains("cleanup is unconfirmed") && error.contains("retry"));
                let state = manager.inner.lock().unwrap();
                assert_eq!(state.starting, 1); assert_eq!(state.queue.len(), 1); assert!(state.result.is_none());
            }
            let result = tokio::time::timeout(std::time::Duration::from_secs(5), manager.shutdown()).await.unwrap().unwrap_err();
            assert!(result.contains("worker creation failed"));
            let delayed_result = delayed.await.unwrap_err();
            assert_eq!(delayed_result, result);
            assert!(delayed_result.contains("injected 2") && delayed_result.contains("injected 1"));
            assert_eq!(manager.shutdown().await.unwrap_err(), result);
            assert_eq!(manager.inner.lock().unwrap().starting, 0);
        });
    }
    #[cfg(windows)]
    #[test]
    fn full_history_preserves_unrecoverable_fault_and_counts_rebuild_once() {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let manager = Arc::new(DesktopState::default());
        let (entered_tx, entered_rx) = std::sync::mpsc::channel(); let (release_tx, release_rx) = std::sync::mpsc::channel();
        *manager.before_command.lock().unwrap() = Some((entered_tx, release_rx));
        manager.stop_before_command.store(true, std::sync::atomic::Ordering::SeqCst);
        runtime.block_on(async {
            {
                let start = manager.start(std::path::Path::new("relative-path")); tokio::pin!(start);
                std::future::poll_fn(|cx| { assert!(start.as_mut().poll(cx).is_pending()); std::task::Poll::Ready(()) }).await;
            }
            entered_rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap(); release_tx.send(()).unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(5), async { while !manager.stopped_before_command.load(std::sync::atomic::Ordering::SeqCst) { tokio::task::yield_now().await; } }).await.unwrap();
            manager.fail_spawn.store(35, std::sync::atomic::Ordering::SeqCst);
            for n in 1..=35 {
                assert!(manager.shutdown().await.unwrap_err().contains("cleanup is unconfirmed"));
                let state = manager.inner.lock().unwrap();
                assert_eq!(state.errors.count, n); assert_eq!(state.fault_history.count, n);
                assert!(state.result.is_none());
            }
            manager.core.run(Command::Close, &mut None);
            manager.fail_spawn.store(1, std::sync::atomic::Ordering::SeqCst);
            let immediate = manager.shutdown().await.unwrap_err();
            assert!(immediate.contains("Unrecoverable: Gateway close ownership missing"));
            assert!(immediate.contains("cleanup is unconfirmed") && immediate.contains("retry is required"));
            assert!(manager.inner.lock().unwrap().result.is_none());
            let result = manager.shutdown().await.unwrap_err();
            assert!(result.contains("Unrecoverable: Gateway close ownership missing"));
            assert!(result.contains("additional diagnostics omitted"));
            assert!(manager.inner.lock().unwrap().result.is_none());
        });
    }
    #[cfg(windows)]
    #[test]
    fn unrecoverable_fault_survives_recovery_of_another_transaction() {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let manager = Arc::new(DesktopState::default());
        let pending = super::super::runtime::Supervisor::begin_start(std::path::Path::new("relative-path")).unwrap();
        {
            let mut state = manager.inner.lock().unwrap();
            state.starting = 1; state.closing = true; state.recovering = 1;
            state.stranded.push(Stranded { owner: Cleanup::Pending(pending), starting: true });
            state.publish_fault("Gateway cleanup is unconfirmed; retry is required.".into(), true);
        }
        manager.core.run(Command::Close, &mut None);
        runtime.block_on(async {
            let (a, b) = tokio::join!(manager.shutdown(), manager.shutdown());
            assert!(a.unwrap_err().contains("close ownership missing"));
            assert!(b.unwrap_err().contains("close ownership missing"));
            tokio::time::timeout(std::time::Duration::from_secs(5), async { while manager.inner.lock().unwrap().recovering != 0 { tokio::task::yield_now().await; } }).await.unwrap();
            assert!(manager.shutdown().await.unwrap_err().contains("close ownership missing"));
        });
        let state = manager.inner.lock().unwrap();
        assert!(state.fatal.is_some()); assert!(state.result.is_none()); assert!(state.closing);
    }
    #[cfg(windows)]
    #[test]
    fn disconnected_worker_restarts_and_reports_failure() {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let manager = Arc::new(DesktopState::default());
        let (sender, receiver) = std::sync::mpsc::channel();
        drop(receiver);
        *manager.worker.lock().unwrap() = Some(sender);
        runtime.block_on(async {
            assert!(manager.start(std::path::Path::new("relative-path")).await.is_err());
            let result = tokio::time::timeout(std::time::Duration::from_secs(5), manager.shutdown()).await.unwrap().unwrap_err();
            assert!(result.contains("Gateway worker failed"));
            assert_eq!(manager.shutdown().await.unwrap_err(), result);
        });
        assert_eq!(manager.inner.lock().unwrap().starting, 0);
    }
    #[cfg(windows)]
    #[test]
    fn close_panics_wait_for_real_lock_release() {
        const FIXTURE: &str = "MINDTRAIL_DESKTOP_PANIC_FIXTURE";
        let Some(base) = std::env::var_os(FIXTURE) else {
            let mut random = [0u8; 16]; getrandom::fill(&mut random).unwrap();
            let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailDesktopPanic-{random:?}"));
            std::fs::create_dir(&base).unwrap();
            let mut child = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "gateway::desktop::tests::close_panics_wait_for_real_lock_release"])
                .env(FIXTURE, &base).spawn().unwrap();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(35);
            let status = loop {
                if let Some(status) = child.try_wait().unwrap() { break status; }
                if std::time::Instant::now() >= deadline { let _ = child.kill(); let _ = child.wait(); panic!("child timed out"); }
                std::thread::sleep(std::time::Duration::from_millis(10));
            };
            assert!(status.success());
            std::fs::remove_dir_all(base).unwrap();
            return;
        };
        let base = std::path::PathBuf::from(base);
        for fault in [2, 3, 4, 5, 6, 7] {
            let path = base.join(format!("state-{fault}"));
            let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            let manager = Arc::new(DesktopState::default());
            runtime.block_on(async {
                let (notified_tx, notified_rx) = std::sync::mpsc::channel();
                manager.start_with_notify(&path, std::sync::Arc::new(move |session| { let _ = notified_tx.send(session.to_owned()); })).await.unwrap();
                if fault == 2 {
                    use std::io::{Read, Write};
                    let endpoint: serde_json::Value = serde_json::from_slice(&std::fs::read(path.join("endpoint.json")).unwrap()).unwrap();
                    let port = endpoint["port"].as_u64().unwrap() as u16;
                    let token = endpoint["token"].as_str().unwrap();
                    let address = std::net::SocketAddr::from(([127, 0, 0, 1], port));
                    let mut socket = std::net::TcpStream::connect_timeout(&address, std::time::Duration::from_secs(5)).unwrap();
                    socket.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
                    let body = r#"[{"v":1,"session":"notification-session","seq":1,"ts":"2024-01-01T00:00:00Z","type":"task.start","payload":{"title":"T","goal":"G","project":"P"}}]"#;
                    write!(socket, "POST /v1/events HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {token}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
                    let mut response = String::new(); socket.read_to_string(&mut response).unwrap();
                    assert!(response.starts_with("HTTP/1.1 200"));
                    assert_eq!(notified_rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap(), "notification-session");
                }
                let access = match &manager.inner.lock().unwrap().slot { Slot::Ready(s) => s.access(), _ => panic!("not ready") };
                let (entered_tx, entered_rx) = std::sync::mpsc::channel();
                let (release_tx, release_rx) = std::sync::mpsc::channel();
                let work_access = access.clone();
                let work = tokio::spawn(async move { work_access.run(move |_| { entered_tx.send(()).unwrap(); let _ = release_rx.recv(); Ok(()) }).await });
                tokio::time::timeout(std::time::Duration::from_secs(5), async { while entered_rx.try_recv().is_err() { tokio::task::yield_now().await; } }).await.unwrap();
                if fault == 4 || fault == 7 {
                    let (sender, receiver) = std::sync::mpsc::channel();
                    drop(receiver);
                    *manager.worker.lock().unwrap() = Some(sender);
                    if fault == 7 { manager.fail_spawn.store(1, std::sync::atomic::Ordering::SeqCst); }
                } else {
                    manager.fault.store(fault, std::sync::atomic::Ordering::SeqCst);
                }
                {
                    let shutdown = manager.shutdown();
                    tokio::pin!(shutdown);
                    std::future::poll_fn(|cx| { let result = shutdown.as_mut().poll(cx); if fault == 7 { assert!(matches!(result, std::task::Poll::Ready(Err(_)))); } else if fault < 5 { assert!(result.is_pending()); } std::task::Poll::Ready(()) }).await;
                }
                tokio::time::timeout(std::time::Duration::from_secs(5), async { while access.run(|_| Ok(())).await.is_ok() { tokio::task::yield_now().await; } }).await.unwrap();
                let directory = super::super::windows_security::StateDirectory::acquire(&path).unwrap();
                assert!(directory.lock().is_err());
                if fault == 5 || fault == 6 {
                    tokio::time::timeout(std::time::Duration::from_secs(5), async { loop {
                        if manager.inner.lock().unwrap().retry_fault.is_some() { break; }
                        tokio::task::yield_now().await;
                    }}).await.unwrap();
                    let state = manager.inner.lock().unwrap();
                    assert_eq!(state.stranded.len(), 1); assert!(state.result.is_none());
                    drop(state);
                    manager.fault.store(0, std::sync::atomic::Ordering::SeqCst);
                }
                let first = manager.shutdown(); let second = manager.shutdown();
                tokio::pin!(first); tokio::pin!(second);
                std::future::poll_fn(|cx| { assert!(first.as_mut().poll(cx).is_pending()); assert!(second.as_mut().poll(cx).is_pending()); std::task::Poll::Ready(()) }).await;
                release_tx.send(()).unwrap();
                work.await.unwrap().unwrap();
                let (a, b) = tokio::time::timeout(std::time::Duration::from_secs(10), async { tokio::join!(first, second) }).await.unwrap();
                let error = a.unwrap_err(); assert_eq!(b.unwrap_err(), error);
                assert!(error.contains(match fault { 2 => "shutdown task failed", 3 => "cleanup task failed", 4 => "worker failed", 5 => "cleanup task failed", 6 => "worker failed", _ => "worker creation failed" }));
                assert_eq!(manager.shutdown().await.unwrap_err(), error);
                tokio::time::timeout(std::time::Duration::from_secs(5), async { while directory.lock().is_err() { tokio::task::yield_now().await; } }).await.unwrap();
            });
        }
        // A genuine directory-lock startup error reaches its caller and status,
        // but does not turn the losing manager's later exit into a failure.
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let owner = Arc::new(DesktopState::default()); let losing = Arc::new(DesktopState::default());
        let contested = base.join("contested");
        runtime.block_on(async {
            owner.start(&contested).await.unwrap();
            let error = losing.start(&contested).await.unwrap_err();
            assert!(error.starts_with("Gateway lock failed:"), "{error}");
            assert_eq!(losing.status(), DesktopStatus::Unavailable { message: error });
            losing.shutdown().await.unwrap(); owner.shutdown().await.unwrap();
        });
        // Replacing an installed instance transfers its cleanup to the start
        // transaction; shutdown of the replacement must wait for both owners.
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let manager = Arc::new(DesktopState::default());
        let old_path = base.join("replace-old"); let new_path = base.join("replace-new");
        runtime.block_on(async {
            manager.start(&old_path).await.unwrap();
            let (access, old_id) = match &manager.inner.lock().unwrap().slot { Slot::Ready(s) => (s.access(), s.store_id().to_owned()), _ => panic!("not ready") };
            let (entered_tx, entered_rx) = std::sync::mpsc::channel(); let (release_tx, release_rx) = std::sync::mpsc::channel();
            let work = tokio::spawn(async move { access.run(move |_| { entered_tx.send(()).unwrap(); let _ = release_rx.recv(); Ok(()) }).await });
            tokio::time::timeout(std::time::Duration::from_secs(5), async { while entered_rx.try_recv().is_err() { tokio::task::yield_now().await; } }).await.unwrap();
            let other = manager.clone(); let path = new_path.clone();
            let replacement = tokio::spawn(async move { other.start(&path).await });
            tokio::time::timeout(std::time::Duration::from_secs(5), async { loop {
                let installed = matches!(manager.status(), DesktopStatus::Ready { store_id } if store_id != old_id);
                if installed { break; } tokio::task::yield_now().await;
            }}).await.unwrap();
            assert!(tokio::time::timeout(std::time::Duration::from_millis(100), manager.shutdown()).await.is_err());
            let old = super::super::windows_security::StateDirectory::acquire(&old_path).unwrap();
            assert!(old.lock().is_err());
            release_tx.send(()).unwrap(); work.await.unwrap().unwrap();
            replacement.await.unwrap().unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(10), manager.shutdown()).await.unwrap().unwrap();
            assert!(old.lock().is_ok());
            let new = super::super::windows_security::StateDirectory::acquire(&new_path).unwrap();
            assert!(new.lock().is_ok());
        });
        // Panic before the replacement transaction requests stop. The outer
        // catcher must stop its displaced supervisor before any shutdown call.
        let manager = Arc::new(DesktopState::default());
        let old_path = base.join("outer-old"); let new_path = base.join("outer-new");
        runtime.block_on(async {
            manager.start(&old_path).await.unwrap();
            let access = match &manager.inner.lock().unwrap().slot { Slot::Ready(s) => s.access(), _ => panic!("not ready") };
            let (entered_tx, entered_rx) = std::sync::mpsc::channel(); let (release_tx, release_rx) = std::sync::mpsc::channel();
            let old_access = access.clone();
            let work = tokio::spawn(async move { old_access.run(move |_| { entered_tx.send(()).unwrap(); let _ = release_rx.recv(); Ok(()) }).await });
            tokio::time::timeout(std::time::Duration::from_secs(5), async { while entered_rx.try_recv().is_err() { tokio::task::yield_now().await; } }).await.unwrap();
            manager.fault.store(8, std::sync::atomic::Ordering::SeqCst);
            let other = manager.clone(); let path = new_path.clone();
            let replacement = tokio::spawn(async move { other.start(&path).await });
            assert!(replacement.await.unwrap().is_err());
            tokio::time::timeout(std::time::Duration::from_secs(5), async { while access.run(|_| Ok(())).await.is_ok() { tokio::task::yield_now().await; } }).await.unwrap();
            let old = super::super::windows_security::StateDirectory::acquire(&old_path).unwrap();
            assert!(old.lock().is_err());
            let shutdown = manager.shutdown(); tokio::pin!(shutdown);
            std::future::poll_fn(|cx| { assert!(shutdown.as_mut().poll(cx).is_pending()); std::task::Poll::Ready(()) }).await;
            release_tx.send(()).unwrap(); work.await.unwrap().unwrap();
            let error = tokio::time::timeout(std::time::Duration::from_secs(10), shutdown).await.unwrap().unwrap_err();
            assert!(error.contains("worker failed")); assert!(old.lock().is_ok());
        });
        let manager = Arc::new(DesktopState::default());
        let installed_path = base.join("late-installed"); let late_path = base.join("late-pending");
        runtime.block_on(async {
            manager.start(&installed_path).await.unwrap();
            let (entered_tx, entered_rx) = std::sync::mpsc::channel(); let (release_tx, release_rx) = std::sync::mpsc::channel();
            *manager.before_install.lock().unwrap() = Some((entered_tx, release_rx, 4));
            let other = manager.clone(); let path = late_path.clone();
            let late = tokio::spawn(async move { other.start(&path).await });
            tokio::time::timeout(std::time::Duration::from_secs(5), async { while entered_rx.try_recv().is_err() { tokio::task::yield_now().await; } }).await.unwrap();
            {
                let mut state = manager.inner.lock().unwrap();
                match &mut state.slot { Slot::Ready(s) => s.request_shutdown_injected(2), _ => panic!("installed instance missing") }
            }
            {
                let shutdown = manager.shutdown(); tokio::pin!(shutdown);
                std::future::poll_fn(|cx| { assert!(shutdown.as_mut().poll(cx).is_pending()); std::task::Poll::Ready(()) }).await;
            }
            let first = manager.shutdown(); let second = manager.shutdown();
            tokio::pin!(first); tokio::pin!(second);
            std::future::poll_fn(|cx| { assert!(first.as_mut().poll(cx).is_pending()); assert!(second.as_mut().poll(cx).is_pending()); std::task::Poll::Ready(()) }).await;
            release_tx.send(()).unwrap();
            assert_eq!(late.await.unwrap().unwrap_err(), "Gateway is stopping.");
            let (a, b) = tokio::time::timeout(std::time::Duration::from_secs(10), async { tokio::join!(first, second) }).await.unwrap();
            let error = a.unwrap_err(); assert_eq!(b.unwrap_err(), error);
            assert!(error.contains("gateway supervisor stopped unexpectedly"));
            assert!(error.contains("HTTP service failed: injected serve failure"));
            assert_eq!(manager.shutdown().await.unwrap_err(), error);
            let installed = super::super::windows_security::StateDirectory::acquire(&installed_path).unwrap();
            let late = super::super::windows_security::StateDirectory::acquire(&late_path).unwrap();
            assert!(installed.lock().is_ok()); assert!(late.lock().is_ok());
        });
    }
    #[cfg(windows)]
    #[test]
    fn startup_panic_waits_for_active_lock() {
        const FIXTURE: &str = "MINDTRAIL_DESKTOP_START_PANIC";
        let Some(base) = std::env::var_os(FIXTURE) else {
            let mut random = [0u8; 16]; getrandom::fill(&mut random).unwrap();
            let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailDesktopStart-{random:?}"));
            std::fs::create_dir(&base).unwrap();
            let mut child = std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact", "gateway::desktop::tests::startup_panic_waits_for_active_lock"]).env(FIXTURE, &base).spawn().unwrap();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            let status = loop { if let Some(status) = child.try_wait().unwrap() { break status; } if std::time::Instant::now() >= deadline { let _ = child.kill(); let _ = child.wait(); panic!("child timed out"); } std::thread::sleep(std::time::Duration::from_millis(10)); };
            assert!(status.success()); std::fs::remove_dir_all(base).unwrap(); return;
        };
        let path = std::path::PathBuf::from(base).join("state");
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let manager = Arc::new(DesktopState::default());
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (panic_tx, panic_rx) = std::sync::mpsc::channel();
        *manager.before_start_panic.lock().unwrap() = Some((ready_tx, panic_rx));
        manager.fault.store(1, std::sync::atomic::Ordering::SeqCst);
        runtime.block_on(async {
            let start_manager = manager.clone(); let start_path = path.clone();
            let start = tokio::spawn(async move { start_manager.start(&start_path).await });
            let access = tokio::time::timeout(std::time::Duration::from_secs(5), async { loop { if let Ok(access) = ready_rx.try_recv() { break access; } tokio::task::yield_now().await; } }).await.unwrap();
            let (entered_tx, entered_rx) = std::sync::mpsc::channel(); let (release_tx, release_rx) = std::sync::mpsc::channel();
            let worker_access = access.clone();
            let work = tokio::spawn(async move { worker_access.run(move |_| { entered_tx.send(()).unwrap(); let _ = release_rx.recv(); Ok(()) }).await });
            tokio::time::timeout(std::time::Duration::from_secs(5), async { while entered_rx.try_recv().is_err() { tokio::task::yield_now().await; } }).await.unwrap();
            panic_tx.send(()).unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(5), async { while access.run(|_| Ok(())).await.is_ok() { tokio::task::yield_now().await; } }).await.unwrap();
            let directory = super::super::windows_security::StateDirectory::acquire(&path).unwrap();
            assert!(directory.lock().is_err());
            assert!(tokio::time::timeout(std::time::Duration::from_millis(100), manager.shutdown()).await.is_err());
            release_tx.send(()).unwrap(); work.await.unwrap().unwrap();
            assert!(start.await.unwrap().unwrap_err().contains("startup task failed"));
            let (a, b) = tokio::time::timeout(std::time::Duration::from_secs(10), async { tokio::join!(manager.shutdown(), manager.shutdown()) }).await.unwrap();
            assert_eq!(a, b); assert!(a.unwrap_err().contains("startup task failed"));
            assert!(directory.lock().is_ok());
        });
    }
    #[test]
    fn error_log_has_bounded_entries_and_reports_omissions() {
        let mut log = ErrorLog::default();
        log.push("界".repeat(1000));
        for n in 0..50 { log.push(format!("failure {n}")); }
        assert_eq!(log.entries.len(), 32);
        assert!(log.entries[0].contains("diagnostic truncated"));
        assert_eq!(log.count, 51);
        assert!(log.render().contains("19 additional diagnostics omitted"));
        assert!(log.render().len() < 6000);
    }
    #[test]
    fn command_payloads_serialize_without_credentials() {
        let ready = DesktopStatus::Ready { store_id: "store-id".into() };
        assert_eq!(serde_json::to_value(ready).unwrap(), serde_json::json!({"status":"ready", "store_id":"store-id"}));
        let unavailable = DesktopStatus::Unavailable { message: "unavailable".into() };
        assert_eq!(serde_json::to_value(unavailable).unwrap(), serde_json::json!({"status":"unavailable", "message":"unavailable"}));
        let sessions = SessionsResponse { store_id: "store-id".into(), sessions: vec![] };
        assert_eq!(serde_json::to_value(sessions).unwrap(), serde_json::json!({"store_id":"store-id", "sessions":[]}));
        let events = EventsResponse { store_id: "store-id".into(), page: EventPage { events: vec![], last_seq: 0, status: "open".into() } };
        assert_eq!(serde_json::to_value(events).unwrap(), serde_json::json!({"store_id":"store-id", "page":{"events":[], "last_seq":0, "status":"open"}}));
        let error: Result<SessionsResponse, String> = Err("unavailable".into());
        assert_eq!(serde_json::to_value(error).unwrap(), serde_json::json!({"Err":"unavailable"}));
    }
    #[test]
    fn replacement_keeps_reader_and_identity_paired_and_old_reader_closes() {
        let mut random = [0u8; 16]; getrandom::fill(&mut random).unwrap();
        let path = std::env::temp_dir().join(format!("mindtrail-desktop-{random:?}.sqlite"));
        let store = Arc::new(Store::open(&path).unwrap());
        let id = store.store_id().to_owned();
        let access = StoreAccess::new(store);
        let state = Arc::new(DesktopState::default());
        state.inner.lock().unwrap().slot = Slot::TestReady { reader: ReadService::from_access(access.clone()), store_id: id.clone() };
        let (reader, snapshot_id) = state.snapshot().unwrap();
        let other = state.clone();
        std::thread::spawn(move || other.set_unavailable("replacement".into())).join().unwrap();
        assert_eq!(snapshot_id, id);
        assert_eq!(state.status(), DesktopStatus::Unavailable { message: "replacement".into() });
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        assert!(runtime.block_on(reader.sessions(None, 10)).is_ok());
        access.close(); access.wait_idle();
        assert_eq!(runtime.block_on(reader.sessions(None, 10)).unwrap_err().status(), 503);
        drop(reader); drop(access); drop(runtime); std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn connection_read_check_detects_identity_change_without_writing_events() {
        let mut random=[0u8;16]; getrandom::fill(&mut random).unwrap();
        let path=std::env::temp_dir().join(format!("mindtrail-connection-{random:?}.sqlite"));
        let store=Arc::new(Store::open(&path).unwrap()); let id=store.store_id().to_owned();
        let access=StoreAccess::new(store); let state=Arc::new(DesktopState::default());
        state.inner.lock().unwrap().slot=Slot::TestReady { reader:ReadService::from_access(access.clone()), store_id:id };
        let runtime=tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        assert!(runtime.block_on(state.connection_read_check()).is_ok());
        let other=state.clone();
        assert_eq!(runtime.block_on(state.connection_read_check_with_hook(|| other.set_unavailable("changed".into()))),Err("identity_changed".into()));
        let db=rusqlite::Connection::open(&path).unwrap();
        assert_eq!(db.query_row("SELECT COUNT(*) FROM sessions",[],|r|r.get::<_,i64>(0)).unwrap(),0);
        assert_eq!(db.query_row("SELECT COUNT(*) FROM events",[],|r|r.get::<_,i64>(0)).unwrap(),0);
        drop(db); access.close(); access.wait_idle(); drop(access); drop(runtime); std::fs::remove_file(path).unwrap();
    }
}












































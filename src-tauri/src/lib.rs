use std::sync::Arc;
use tauri::{Emitter, Manager};
use gateway::desktop::DesktopState;
mod semantic;
mod connection;
mod distribution;

fn app_builder(
    resolve_path: impl Fn(&tauri::AppHandle) -> Result<std::path::PathBuf, String> + Send + Sync + 'static,
) -> tauri::Builder<tauri::Wry> {
    app_builder_with_state(resolve_path, Arc::new(DesktopState::default()))
}

fn app_builder_with_state(
    resolve_path: impl Fn(&tauri::AppHandle) -> Result<std::path::PathBuf, String> + Send + Sync + 'static,
    state: Arc<DesktopState>,
) -> tauri::Builder<tauri::Wry> {
    tauri::Builder::default()
        .setup(move |app| {
            app.manage(state.clone());
            let semantic = Arc::new(semantic::service::SemanticService::default());
            match semantic::openai_compatible::from_environment() {
                Ok(config) => { let _ = semantic.configure(config); },
                Err(error) => semantic.set_config_error(error),
            }
            app.manage(semantic);
            #[cfg(windows)]
            match resolve_path(app.handle()) {
                Ok(path) => {
                    let handle = app.handle().clone();
                    let notify = Arc::new(move |session: &str| {
                        if let Some(window) = handle.get_webview_window("main") {
                            let _ = window.emit_to(tauri::EventTarget::webview_window("main"), "mindtrail:session-updated", session);
                        }
                    });
                    tauri::async_runtime::spawn(async move {
                        if let Err(error) = state.start_with_notify(&path, notify).await { state.record_start_error(error); }
                    });
                }
                Err(error) => state.record_start_error(error),
            }
            #[cfg(not(windows))]
            let _ = resolve_path;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            gateway::desktop::gateway_status,
            gateway::desktop::list_sessions,
            gateway::desktop::read_events,
            semantic::commands::semantic_status,
            semantic::commands::prepare_insight,
            semantic::commands::generate_insight,
            semantic::commands::cancel_insight,
            semantic::commands::release_insight,
            connection::connection_status,
            connection::connection_check
        ])
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = app_builder(|handle| handle.path().home_dir().map(|path| path.join(".path-tracker"))
        .map_err(|_| "Unable to locate the user home directory.".to_owned()))
        .build(tauri::generate_context!())
        .expect("error while running MindTrail");
    #[cfg(windows)]
    let exit_state = Arc::new(ExitState::default());
    app.run(move |handle, event| {
        #[cfg(windows)]
        handle_exit_event(handle, event, &exit_state);
        #[cfg(not(windows))]
        let _ = (handle, event);
    });
}

#[cfg(windows)]
#[derive(Default)]
struct ExitState {
    phase: std::sync::atomic::AtomicU8,
    code: std::sync::Mutex<Option<i32>>,
    watching: std::sync::atomic::AtomicBool,
}

#[cfg(windows)]
struct ExitTaskGuard(Arc<ExitState>);
#[cfg(windows)]
impl ExitTaskGuard {
    fn finish_unconfirmed(&self, fatal: bool) {
        use std::sync::atomic::Ordering;
        if fatal { let _ = self.0.phase.compare_exchange(1, 3, Ordering::AcqRel, Ordering::Acquire); }
    }
}
#[cfg(windows)]
impl Drop for ExitTaskGuard {
    fn drop(&mut self) {
        use std::sync::atomic::Ordering;
        let _ = self.0.phase.compare_exchange(1, 0, Ordering::AcqRel, Ordering::Acquire);
    }
}
#[cfg(windows)]
struct ExitWatchGuard(Arc<ExitState>);
#[cfg(windows)]
impl Drop for ExitWatchGuard {
    fn drop(&mut self) { self.0.watching.store(false, std::sync::atomic::Ordering::Release); }
}

#[cfg(windows)]
fn finish_exit(state: &ExitState, handle: &tauri::AppHandle, code: i32, result: Result<(), String>) {
    use std::sync::atomic::Ordering;
    if state.phase.fetch_update(Ordering::AcqRel, Ordering::Acquire, |phase| (phase != 2 && phase != 3).then_some(2)).is_ok() {
        if let Err(error) = result { use std::io::Write; let _ = writeln!(std::io::stderr().lock(), "Gateway shutdown: {error}"); }
        handle.exit(code);
    }
}

#[cfg(windows)]
fn handle_exit_event(handle: &tauri::AppHandle, event: tauri::RunEvent, exit_state: &Arc<ExitState>) {
    if let tauri::RunEvent::ExitRequested { api, code, .. } = event {
        handle.state::<Arc<semantic::service::SemanticService>>().cancel_all();
        use std::sync::atomic::Ordering;
        if exit_state.phase.load(Ordering::Acquire) == 2 { return; }
        api.prevent_exit();
        let mut first_code = exit_state.code.lock().unwrap();
        let code = *first_code.get_or_insert(code.unwrap_or(0));
        if exit_state.phase.compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire).is_ok() {
            drop(first_code);
            let exit_state = exit_state.clone();
            let handle = handle.clone();
            let state = handle.state::<Arc<DesktopState>>().inner().clone();
            if !exit_state.watching.swap(true, Ordering::AcqRel) {
                let watcher_state = state.clone();
                let watcher_exit = exit_state.clone();
                let watcher_handle = handle.clone();
                let guard = ExitWatchGuard(watcher_exit.clone());
                tauri::async_runtime::spawn(async move {
                    let _guard = guard;
                    let result = watcher_state.wait_confirmed_shutdown().await;
                    finish_exit(&watcher_exit, &watcher_handle, code, result);
                });
            }
            let guard = ExitTaskGuard(exit_state.clone());
            tauri::async_runtime::spawn(async move {
                let guard = guard;
                let mut attempts = 0;
                loop {
                    let result = state.shutdown().await;
                    if let Some(confirmed) = state.confirmed_shutdown_result() {
                        finish_exit(&exit_state, &handle, code, confirmed);
                        break;
                    }
                    attempts += 1;
                    if state.shutdown_is_fatal() || attempts >= 3 {
                        let error = result.err().unwrap_or_else(|| "Cleanup has not been confirmed.".into());
                        state.report_unconfirmed_shutdown(&error);
                        { use std::io::Write; let _ = writeln!(std::io::stderr().lock(), "Gateway shutdown is unconfirmed; keep the application open and contact support: {error}"); }
                        // A later request can retry recoverable dispatch failures.
                        guard.finish_unconfirmed(state.shutdown_is_fatal());
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                }
            });
        }
    }
}

pub mod protocol;
pub mod gateway;
#[cfg(all(feature = "producer", windows))]
pub mod producer;

#[cfg(all(test, windows))]
mod app_tests {
    use super::*;
    use std::time::{Duration, Instant};

    // Keep each path component open without delete sharing so it cannot be replaced
    // while the child uses path-based APIs. Never follow a reparse point.
    fn lock_fixture_path(path: &std::path::Path) -> std::io::Result<Vec<std::fs::File>> {
        use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
        use windows_sys::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, FILE_SHARE_WRITE};
        let mut current = std::path::PathBuf::new();
        let mut locks = Vec::new();
        for part in path.components() {
            current.push(part);
            if !current.is_absolute() { continue; }
            let file = std::fs::OpenOptions::new().read(true)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
                .open(&current)?;
            let meta = file.metadata()?;
            if !meta.is_dir() || meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "fixture path contains a reparse point or non-directory"));
            }
            locks.push(file);
        }
        Ok(locks)
    }

    fn fixture_identity(file: &std::fs::File) -> std::io::Result<String> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION};
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        if unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, &mut info) } == 0 { return Err(std::io::Error::last_os_error()); }
        Ok(format!("{:08x}{:08x}{:08x}", info.dwVolumeSerialNumber, info.nFileIndexHigh, info.nFileIndexLow))
    }

    fn fixture_file(path: &std::path::Path, create: bool) -> std::io::Result<std::fs::File> {
        use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
        use windows_sys::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, FILE_SHARE_WRITE};
        let file = std::fs::OpenOptions::new().read(true).write(true).create_new(create)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE).custom_flags(FILE_FLAG_OPEN_REPARSE_POINT).open(path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "invalid fixture file"));
        }
        Ok(file)
    }

    fn fixture_read(file: &mut std::fs::File) -> std::io::Result<String> {
        use std::io::{Read, Seek, SeekFrom};
        file.seek(SeekFrom::Start(0))?;
        let mut text = String::new(); file.read_to_string(&mut text)?; Ok(text)
    }

    fn fixture_write(file: &std::sync::Mutex<std::fs::File>, text: impl AsRef<[u8]>) -> std::io::Result<()> {
        use std::io::{Seek, SeekFrom, Write};
        let mut file = file.lock().unwrap();
        file.set_len(0)?; file.seek(SeekFrom::Start(0))?; file.write_all(text.as_ref())?; file.flush()
    }

    fn fixture_path(suffix: &str) -> std::path::PathBuf {
        assert!(suffix.len() == 32 && suffix.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)), "invalid fixture identifier");
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailAppEvent-{suffix}"))
    }

    #[test]
    fn cancelled_exit_task_releases_processing_phase_without_confirming() {
        use std::sync::atomic::Ordering;
        let state = Arc::new(ExitState::default());
        state.phase.store(1, Ordering::Release);
        let guard = ExitTaskGuard(state.clone());
        let future = async move { let _guard = guard; std::future::pending::<()>().await; };
        drop(future);
        assert_eq!(state.phase.load(Ordering::Acquire), 0);
    }

    #[test]
    fn retry_cannot_claim_processing_until_previous_guard_is_dropped() {
        use std::sync::atomic::Ordering;
        let state = Arc::new(ExitState::default());
        state.phase.store(1, Ordering::Release);
        let previous = ExitTaskGuard(state.clone());
        // Pause precisely after the recoverable branch and before its guard drops.
        previous.finish_unconfirmed(false);
        assert!(state.phase.compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire).is_err());
        drop(previous);
        assert!(state.phase.compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire).is_ok());
        let current = ExitTaskGuard(state.clone());
        assert!(state.phase.compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire).is_err());
        assert_eq!(state.phase.load(Ordering::Acquire), 1);
        drop(current);
        assert_eq!(state.phase.load(Ordering::Acquire), 0);
    }

    #[test]
    fn hidden_wry_shutdown_faults_preserve_exit_contract() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        const CHILD: &str = "MINDTRAIL_EXIT_FAULT_CASE";
        if let Ok(mode) = std::env::var(CHILD) {
            let state = Arc::new(DesktopState::default());
            state.test_shutdown_mode(match mode.as_str() { "recoverable" | "late" => 0, "fatal" => 1, "confirmed" => 2, "panic" => 3, _ => panic!("bad mode") });
            let mut context = tauri::generate_context!();
            for window in &mut context.config_mut().app.windows { window.visible = false; }
            let app = app_builder_with_state(|_| Err("fixture startup disabled".into()), state.clone()).any_thread().build(context).unwrap();
            let exit_state = Arc::new(ExitState::default());
            let requests = Arc::new(AtomicUsize::new(0));
            let mut started = false;
            app.run(move |handle, event| {
                if let tauri::RunEvent::ExitRequested { code, .. } = &event {
                    if exit_state.phase.load(Ordering::Acquire) == 2 {
                        if *code != Some(23) || (mode == "recoverable" && requests.load(Ordering::Acquire) < 2) { std::process::exit(1); }
                    } else { requests.fetch_add(1, Ordering::AcqRel); }
                }
                if matches!(event, tauri::RunEvent::Ready) && !started {
                    started = true;
                    let handle = handle.clone(); let state = state.clone(); let exit_state = exit_state.clone(); let mode = mode.clone(); let requests = requests.clone();
                    std::thread::spawn(move || {
                        let ok = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            handle.exit(23);
                            if mode == "confirmed" { return; }
                            let deadline = Instant::now() + Duration::from_secs(12);
                            let expected_phase = if mode == "fatal" { 3 } else { 0 };
                            while state.test_shutdown_calls() < if mode == "fatal" || mode == "panic" { 1 } else { 3 } || exit_state.phase.load(Ordering::Acquire) != expected_phase {
                                assert!(Instant::now() < deadline, "shutdown did not reach expected phase");
                                std::thread::sleep(Duration::from_millis(10));
                            }
                            assert_eq!(state.test_shutdown_calls(), if mode == "fatal" || mode == "panic" { 1 } else { 3 });
                            if mode == "fatal" {
                                handle.exit(42);
                                let deadline = Instant::now() + Duration::from_secs(5);
                                while requests.load(Ordering::Acquire) < 2 { assert!(Instant::now() < deadline); std::thread::sleep(Duration::from_millis(10)); }
                                std::thread::sleep(Duration::from_millis(1500));
                                assert_eq!(state.test_shutdown_calls(), 1);
                                assert_eq!(exit_state.phase.load(Ordering::Acquire), 3);
                                std::process::exit(0);
                            }
                            if mode == "panic" || mode == "recoverable" {
                                handle.exit(42);
                                let deadline = Instant::now() + Duration::from_secs(5);
                                while requests.load(Ordering::Acquire) < 2 { assert!(Instant::now() < deadline); std::thread::sleep(Duration::from_millis(10)); }
                            }
                            state.test_shutdown_mode(2);
                        })).is_ok();
                        if !ok { std::process::exit(1); }
                    });
                }
                handle_exit_event(handle, event, &exit_state);
            });
            panic!("app returned before exit");
        }
        for mode in ["recoverable", "fatal", "confirmed", "late", "panic"] {
            let mut random = [0u8; 16]; getrandom::fill(&mut random).unwrap();
            let suffix: String = random.iter().map(|b| format!("{b:02x}")).collect();
            let root = fixture_path(&suffix);
            let _parent_locks = lock_fixture_path(root.parent().unwrap()).unwrap();
            std::fs::create_dir(&root).unwrap();
            let _locks = lock_fixture_path(&root).unwrap();
            let mut child = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "app_tests::hidden_wry_shutdown_faults_preserve_exit_contract"])
                .env(CHILD, mode).env("WEBVIEW2_USER_DATA_FOLDER", root.join("webview")).spawn().unwrap();
            let deadline = Instant::now() + Duration::from_secs(30);
            let status = loop {
                if let Some(status) = child.try_wait().unwrap() { break status; }
                if Instant::now() >= deadline { let _ = child.kill(); let _ = child.wait(); panic!("{mode} timed out; fixture retained: {}", root.display()); }
                std::thread::sleep(Duration::from_millis(10));
            };
            assert_eq!(status.code(), Some(if mode == "fatal" { 0 } else { 23 }), "{mode}; fixture retained: {}", root.display());
        }
    }

    #[test]
    fn hidden_wry_exit_waits_for_startup_and_active_work() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        const CHILD: &str = "MINDTRAIL_EXIT_FIXTURE";
        if std::env::var_os(CHILD).is_none() {
            for mode in ["startup", "active"] {
                let mut random = [0u8; 16]; getrandom::fill(&mut random).unwrap();
                let suffix: String = random.iter().map(|b| format!("{b:02x}")).collect();
                let root = fixture_path(&suffix);
                let _parent_locks = lock_fixture_path(root.parent().unwrap()).unwrap();
                std::fs::create_dir(&root).unwrap();
                let locks = lock_fixture_path(&root).unwrap();
                let identity = fixture_identity(locks.last().unwrap()).unwrap();
                let mut nonce = [0u8; 32]; getrandom::fill(&mut nonce).unwrap();
                let marker: String = nonce.iter().map(|b| format!("{b:02x}")).collect();
                let marker_file = fixture_file(&root.join("fixture-marker"), true).unwrap();
                let marker_id = fixture_identity(&marker_file).unwrap();
                fixture_write(&std::sync::Mutex::new(marker_file.try_clone().unwrap()), &marker).unwrap();
                let mut report_file = fixture_file(&root.join("exit-report.txt"), true).unwrap();
                let report_id = fixture_identity(&report_file).unwrap();
                assert!(std::fs::rename(root.join("fixture-marker"), root.join("marker-replaced")).is_err());
                assert!(std::fs::rename(root.join("exit-report.txt"), root.join("report-replaced")).is_err());
                let mut child = std::process::Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", "app_tests::hidden_wry_exit_waits_for_startup_and_active_work"])
                    .env(CHILD, &suffix).env("MINDTRAIL_TEST_FIXTURE_ID", identity)
                    .env("MINDTRAIL_TEST_FIXTURE_MARKER", marker).env("MINDTRAIL_MARKER_ID", marker_id).env("MINDTRAIL_REPORT_ID", report_id).env("MINDTRAIL_EXIT_MODE", mode)
                    .env("WEBVIEW2_USER_DATA_FOLDER", root.join("webview"))
                    .spawn().unwrap();
                let deadline = Instant::now() + Duration::from_secs(50);
                let status = loop {
                    if let Some(status) = child.try_wait().unwrap() { break status; }
                    if Instant::now() >= deadline { let _ = child.kill(); let _ = child.wait(); panic!("exit fixture timed out: {}", root.display()); }
                    std::thread::sleep(Duration::from_millis(10));
                };
                let report = fixture_read(&mut report_file).unwrap_or_else(|_| "no report".into());
                assert!(status.success() && report == "success", "{mode}: {status}; {report}; fixture {}", root.display());
                assert!(DesktopState::test_lock_available(&root.join("state")).unwrap(), "{mode}: lock not released");
            }
            return;
        }
        let root = fixture_path(&std::env::var(CHILD).unwrap());
        let _locks = lock_fixture_path(&root).unwrap();
        assert_eq!(fixture_identity(_locks.last().unwrap()).unwrap(), std::env::var("MINDTRAIL_TEST_FIXTURE_ID").unwrap());
        let marker = std::env::var("MINDTRAIL_TEST_FIXTURE_MARKER").unwrap();
        let mut marker_file = fixture_file(&root.join("fixture-marker"), false).unwrap();
        assert_eq!(fixture_identity(&marker_file).unwrap(), std::env::var("MINDTRAIL_MARKER_ID").unwrap());
        assert_eq!(fixture_read(&mut marker_file).unwrap(), marker);
        let mode = std::env::var("MINDTRAIL_EXIT_MODE").unwrap();
        assert!(mode == "startup" || mode == "active");
        let path = root.join("state");
        let report_file = fixture_file(&root.join("exit-report.txt"), false).unwrap();
        assert_eq!(fixture_identity(&report_file).unwrap(), std::env::var("MINDTRAIL_REPORT_ID").unwrap());
        let report = Arc::new(std::sync::Mutex::new(report_file));
        fixture_write(&report, "building").unwrap();
        let state = Arc::new(DesktopState::default());
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let mut entered_rx = Some(entered_rx);
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let mut release_rx = Some(release_rx);
        if mode == "startup" { state.test_pause_before_install(entered_tx.clone(), release_rx.take().unwrap()); }
        let injected = path.clone();
        let mut context = tauri::generate_context!();
        for window in &mut context.config_mut().app.windows { window.visible = false; }
        let app = app_builder_with_state(move |_| Ok(injected.clone()), state.clone()).any_thread().build(context).expect("build app");
        let exits = Arc::new(AtomicUsize::new(0));
        let exit_state = Arc::new(ExitState::default());
        let mut started = false;
        app.run(move |handle, event| {
            if let tauri::RunEvent::ExitRequested { .. } = &event {
                if exit_state.phase.load(Ordering::Acquire) == 2 {
                    if exits.load(Ordering::Acquire) < 2 || !matches!(DesktopState::test_lock_available(&path), Ok(true)) {
                        let _ = fixture_write(&report, "final exit before lock release"); std::process::exit(1);
                    }
                    fixture_write(&report, "success").unwrap();
                } else { exits.fetch_add(1, Ordering::AcqRel); }
            }
            if matches!(event, tauri::RunEvent::Ready) && !started {
                started = true;
                let handle = handle.clone(); let state = state.clone(); let mode = mode.clone();
                let entered_tx = entered_tx.clone(); let exits = exits.clone(); let path = path.clone(); let report = report.clone();
                let entered_rx = entered_rx.take().unwrap();
                let release_rx = release_rx.take(); let release_tx = release_tx.clone();
                std::thread::spawn(move || {
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        if mode == "active" {
                            let deadline = Instant::now() + Duration::from_secs(15);
                            while !matches!(state.status(), gateway::desktop::DesktopStatus::Ready { .. }) {
                                assert!(Instant::now() < deadline, "gateway readiness timeout: {:?}", state.status());
                                std::thread::sleep(Duration::from_millis(10));
                            }
                            let state = state.clone();
                            tauri::async_runtime::spawn(async move { state.test_hold_work(entered_tx, release_rx.unwrap()).await; });
                        }
                        entered_rx.recv_timeout(Duration::from_secs(15)).unwrap_or_else(|_| panic!("gate not reached; gateway status: {:?}", state.status()));
                        handle.exit(0); handle.exit(0);
                        let deadline = Instant::now() + Duration::from_secs(5);
                        while exits.load(Ordering::Acquire) < 2 { assert!(Instant::now() < deadline, "repeated exit not handled"); std::thread::sleep(Duration::from_millis(10)); }
                        let (tx, rx) = std::sync::mpsc::channel();
                        handle.run_on_main_thread(move || { let _ = tx.send(()); }).unwrap();
                        rx.recv_timeout(Duration::from_secs(5)).expect("event loop blocked");
                        assert!(!DesktopState::test_lock_available(&path).expect("lock probe failed"), "lock released before gate");
                    }));
                    let _ = release_tx.send(());
                    if let Err(error) = result {
                        let message = error.downcast_ref::<String>().map(String::as_str).or_else(|| error.downcast_ref::<&str>().copied()).unwrap_or("unknown failure");
                        let _ = fixture_write(&report, format!("exit checks failed: {message}"));
                        std::process::exit(1);
                    }
                });
            }
            handle_exit_event(handle, event, &exit_state);
        });
        panic!("app returned before final exit");
    }

    #[test]
    fn actual_tauri_builder_keeps_workbench_on_isolated_start_failure() {
        const CHILD: &str = "MINDTRAIL_HIDDEN_APP_TEST";
        if std::env::var_os(CHILD).is_none() {
            let mut child = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "app_tests::actual_tauri_builder_keeps_workbench_on_isolated_start_failure"])
                .env(CHILD, "1").spawn().unwrap();
            let deadline = Instant::now() + Duration::from_secs(30);
            let status = loop {
                if let Some(status) = child.try_wait().unwrap() { break status; }
                if Instant::now() >= deadline { let _ = child.kill(); let _ = child.wait(); panic!("hidden Tauri fixture timed out"); }
                std::thread::sleep(Duration::from_millis(10));
            };
            assert!(status.success(), "hidden Tauri fixture failed");
            return;
        }
        let mut context = tauri::generate_context!();
        for window in &mut context.config_mut().app.windows { window.visible = false; }
        let app = app_builder(|_| Ok(std::path::PathBuf::from("relative-invalid-test-path")))
            .any_thread().build(context).expect("build Tauri app");
        let mut started = false;
        app.run(move |handle, event| {
            if matches!(event, tauri::RunEvent::Ready) && !started {
                started = true;
                let handle = handle.clone();
                std::thread::spawn(move || {
                    let deadline = Instant::now() + Duration::from_secs(15);
                    let result = loop {
                        let state = handle.state::<Arc<DesktopState>>();
                        match state.status() {
                            gateway::desktop::DesktopStatus::Unavailable { message } if message.contains("Gateway directory failed") && message.contains("restart") => {
                                break handle.get_webview_window("main").is_some();
                            }
                            _ if Instant::now() >= deadline => break false,
                            _ => std::thread::sleep(Duration::from_millis(10)),
                        }
                    };
                    if !result { eprintln!("workbench or actionable startup status missing"); }
                    handle.exit(if result { 0 } else { 1 });
                });
            }
        });
        panic!("Tauri app returned without reporting its result");
    }
    #[test]
    fn desktop_ui_isolated_fixture() {
        let Ok(suffix) = std::env::var("MINDTRAIL_UI_FIXTURE") else { return; };
        let root = fixture_path(&suffix);
        assert!(root.join("fixture-marker").is_file());
        let state_path = root.join("state");
        let injected = state_path.clone();
        let mut context = tauri::generate_context!();
        for window in &mut context.config_mut().app.windows { window.visible = false; }
        let app = app_builder(move |_| Ok(injected.clone())).any_thread().build(context).expect("build isolated UI");
        let mut started = false;
        let exit_state = Arc::new(ExitState::default());
        app.run(move |handle, event| {
            if matches!(event, tauri::RunEvent::Ready) && !started {
                started = true;
                let handle = handle.clone();
                let root = root.clone();
                std::thread::spawn(move || {
                    let timeout = std::env::var("MINDTRAIL_UI_TIMEOUT_SECS").ok().and_then(|value| value.parse::<u64>().ok()).filter(|value| (30..=600).contains(value)).unwrap_or(90);
                    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout);
                    while !root.join("stop").is_file() && std::time::Instant::now() < deadline { std::thread::sleep(std::time::Duration::from_millis(100)); }
                    handle.get_webview_window("main").expect("main window").close().expect("close window");
                });
            }
            if matches!(event, tauri::RunEvent::ExitRequested { .. }) { let _ = std::fs::write(root.join("exit-requested"), "yes"); }
            if matches!(event, tauri::RunEvent::Exit) {
                let confirmed = handle.state::<Arc<DesktopState>>().confirmed_shutdown_result().is_some()
                    && exit_state.phase.load(std::sync::atomic::Ordering::Acquire) == 2;
                let _ = std::fs::write(root.join("shutdown-result"), if confirmed { "exited" } else { "unconfirmed" });
            }
            handle_exit_event(handle, event, &exit_state);
        });
    }

    #[test]
    fn hidden_tauri_window_receives_session_event_from_real_gateway() {
        const CHILD: &str = "MINDTRAIL_HIDDEN_EVENT_APP_TEST";
        if std::env::var_os(CHILD).is_none() {
            let mut random = [0u8; 16]; getrandom::fill(&mut random).unwrap();
            let suffix: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
            let root = fixture_path(&suffix);
            let _parent_locks = lock_fixture_path(root.parent().unwrap()).expect("lock fixture parent");
            std::fs::create_dir(&root).unwrap();
            let locks = lock_fixture_path(&root).expect("lock fixture path");
            let identity = fixture_identity(locks.last().unwrap()).unwrap();
            let mut nonce = [0u8; 32]; getrandom::fill(&mut nonce).unwrap();
            let marker: String = nonce.iter().map(|byte| format!("{byte:02x}")).collect();
            let marker_file = fixture_file(&root.join("fixture-marker"), true).unwrap();
                let marker_id = fixture_identity(&marker_file).unwrap();
                fixture_write(&std::sync::Mutex::new(marker_file.try_clone().unwrap()), &marker).unwrap();
                let mut report_file = fixture_file(&root.join("test-stage.txt"), true).unwrap();
                let report_id = fixture_identity(&report_file).unwrap();
                assert!(std::fs::rename(root.join("fixture-marker"), root.join("marker-replaced")).is_err());
                assert!(std::fs::rename(root.join("test-stage.txt"), root.join("report-replaced")).is_err());
            let mut child = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "app_tests::hidden_tauri_window_receives_session_event_from_real_gateway"])
                .env(CHILD, &suffix).env("MINDTRAIL_TEST_FIXTURE_ID", identity).env("MINDTRAIL_TEST_FIXTURE_MARKER", marker).env("MINDTRAIL_MARKER_ID", marker_id).env("MINDTRAIL_REPORT_ID", report_id).spawn().unwrap();
            let deadline = Instant::now() + Duration::from_secs(40);
            let status = loop {
                if let Some(status) = child.try_wait().unwrap() { break status; }
                if Instant::now() >= deadline { let _ = child.kill(); let _ = child.wait(); panic!("hidden event fixture timed out; isolated fixture retained: {}", root.display()); }
                std::thread::sleep(Duration::from_millis(10));
            };
            eprintln!("isolated app event fixture retained: {}", root.display());
            let stage = fixture_read(&mut report_file).unwrap_or_else(|_| "no stage report".into());
            assert!(status.success() && stage == "success", "hidden event fixture failed: {status}; {stage}");
            return;
        }
        use tauri::Listener;
        let suffix = std::env::var(CHILD).expect("fixture identifier must be Unicode");
        let root = fixture_path(&suffix);
        let _locks = lock_fixture_path(&root).expect("lock fixture path");
        assert_eq!(fixture_identity(_locks.last().unwrap()).unwrap(), std::env::var("MINDTRAIL_TEST_FIXTURE_ID").expect("missing fixture identity"), "fixture identity changed");
        let marker = std::env::var("MINDTRAIL_TEST_FIXTURE_MARKER").expect("missing fixture marker");
        assert!(marker.len() == 64 && marker.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)), "invalid fixture marker");
        let mut marker_file = fixture_file(&root.join("fixture-marker"), false).unwrap();
        assert_eq!(fixture_identity(&marker_file).unwrap(), std::env::var("MINDTRAIL_MARKER_ID").unwrap());
        assert_eq!(fixture_read(&mut marker_file).unwrap(), marker);
        let report_file = fixture_file(&root.join("test-stage.txt"), false).unwrap();
        assert_eq!(fixture_identity(&report_file).unwrap(), std::env::var("MINDTRAIL_REPORT_ID").unwrap());
        let report = Arc::new(std::sync::Mutex::new(report_file));
        fixture_write(&report, "building app").unwrap();
        let state_path = root.join("state");
        let injected = state_path.clone();
        let mut context = tauri::generate_context!();
        for window in &mut context.config_mut().app.windows { window.visible = false; }
        let app = app_builder(move |_| Ok(injected.clone())).any_thread().build(context).expect("build Tauri app");
        fixture_write(&report, "app built; awaiting ready").unwrap();
        let mut started = false;
        app.run(move |handle, event| {
            if matches!(event, tauri::RunEvent::Ready) && !started {
                started = true;
                fixture_write(&report, "ready; creating second window").unwrap();
                let window = handle.get_webview_window("main").expect("workbench");
                let other = tauri::WebviewWindowBuilder::new(handle, "other", tauri::WebviewUrl::default()).visible(false).build().expect("second window");
                fixture_write(&report, "windows built; awaiting gateway").unwrap();
                let (other_tx, other_rx) = std::sync::mpsc::channel();
                other.listen("mindtrail:session-updated", move |event| { let _ = other_tx.send(event.payload().to_owned()); });
                let (event_tx, event_rx) = std::sync::mpsc::channel();
                window.listen("mindtrail:session-updated", move |event| { let _ = event_tx.send(event.payload().to_owned()); });
                let handle = handle.clone(); let path = state_path.clone(); let report = report.clone();
                std::thread::spawn(move || {
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        use std::io::{Read, Write};
                        let deadline = Instant::now() + Duration::from_secs(15);
                        loop {
                            let status = handle.state::<Arc<DesktopState>>().status();
                            if matches!(status, gateway::desktop::DesktopStatus::Ready { .. }) { break; }
                            if Instant::now() >= deadline { let _ = fixture_write(&report, format!("gateway readiness timeout: {status:?}")); panic!("gateway readiness timeout: {status:?}"); }
                            std::thread::sleep(Duration::from_millis(10));
                        }
                        fixture_write(&report, "gateway ready; sending event").unwrap();
                        let data: serde_json::Value = serde_json::from_slice(&std::fs::read(path.join("endpoint.json")).unwrap()).unwrap();
                        let port = data["port"].as_u64().unwrap() as u16;
                        let token = data["token"].as_str().unwrap();
                        let address = std::net::SocketAddr::from(([127, 0, 0, 1], port));
                        let mut socket = std::net::TcpStream::connect_timeout(&address, Duration::from_secs(5)).unwrap();
                        socket.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                        let body = r#"[{"v":1,"session":"workbench-event","seq":1,"ts":"2024-01-01T00:00:00Z","type":"task.start","payload":{"title":"T","goal":"G","project":"P"}}]"#;
                        write!(socket, "POST /v1/events HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {token}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
                        let mut response = String::new(); socket.read_to_string(&mut response).unwrap();
                        assert!(response.starts_with("HTTP/1.1 200"));
                        fixture_write(&report, "event accepted; awaiting listener").unwrap();
                        let payload = event_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                        assert_eq!(serde_json::from_str::<String>(&payload).unwrap(), "workbench-event");
                        assert!(matches!(other_rx.recv_timeout(Duration::from_millis(500)), Err(std::sync::mpsc::RecvTimeoutError::Timeout)), "other window received session event");
                        fixture_write(&report, "listener assertions passed; shutting down").unwrap();
                    })).is_ok();
                    let state = handle.state::<Arc<DesktopState>>().inner().clone();
                    let shutdown = tauri::async_runtime::block_on(state.shutdown());
                    let success = result && shutdown.is_ok();
                    if success { let _ = fixture_write(&report, "success"); }
                    if !success { eprintln!("gateway event or shutdown failed"); }
                    handle.exit(if success { 0 } else { 1 });
                });
            }
        });
        panic!("Tauri app returned without reporting its result");
    }
}

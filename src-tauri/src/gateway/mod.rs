pub mod store;
pub mod http;
#[cfg(test)]
mod recovery;
#[cfg(windows)]
mod windows_security;
#[cfg(all(windows, feature = "producer"))]
pub(crate) use windows_security::{with_producer_queue, read_producer_discovery};
mod discovery;

mod access;
#[cfg(windows)]
mod runtime;
pub(crate) mod desktop;

// Synchronous lifecycle operations retain supervision until cleanup completes.
// Callers should run these blocking methods outside an async executor thread.
#[cfg(windows)]
pub mod lifecycle {
    use std::path::Path;
    struct StartGuard(Option<super::runtime::PendingStart>);
    impl Drop for StartGuard {
        fn drop(&mut self) {
            if let Some(pending) = &mut self.0 {
                pending.request_shutdown();
                let _ = pending.wait_completion_blocking();
            }
        }
    }
    pub struct Gateway(Option<super::runtime::Supervisor>);
    impl Gateway {
        pub fn start(path: &Path) -> Result<Self, String> {
            let pending = super::runtime::Supervisor::begin_start(path)
                .map_err(|error| format!("Gateway {} failed: {}", error.stage, error.message))?;
            let mut guard = StartGuard(Some(pending));
            match guard.0.as_mut().unwrap().wait_ready_blocking() {
                Ok(supervisor) => { guard.0.take(); Ok(Self(Some(supervisor))) }
                Err(error) => {
                    let pending = guard.0.as_mut().unwrap();
                    pending.request_shutdown();
                    let completion = pending.wait_completion_blocking();
                    let ordinary_error = format!("{}: {}", error.stage, error.message);
                    let message = format!("Gateway {} failed: {}", error.stage, error.message);
                    let message = match completion {
                        Err(cleanup) if cleanup != ordinary_error => format!("{message}; cleanup failed: {cleanup}"),
                        _ => message,
                    };
                    guard.0.take();
                    Err(message)
                }
            }
        }
        pub fn shutdown(mut self) -> Result<(), String> {
            let supervisor = self.0.as_mut().expect("gateway already closed");
            supervisor.request_shutdown();
            let result = supervisor.wait_completion_blocking();
            self.0.take();
            result
        }
    }
    impl Drop for Gateway {
        fn drop(&mut self) {
            if let Some(mut supervisor) = self.0.take() {
                supervisor.request_shutdown();
                let _ = supervisor.wait_completion_blocking();
            }
        }
    }
    #[cfg(test)]
    mod tests {
        use super::*;
        use std::{path::PathBuf, sync::mpsc, time::{Duration, Instant}};
        use std::os::windows::{fs::{MetadataExt, OpenOptionsExt}, io::AsRawHandle};
        use windows_sys::Win32::{Foundation::HANDLE, Storage::FileSystem::{FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_ID_INFO, FILE_READ_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, FileIdInfo, GetFileInformationByHandleEx}};
        fn root_guard(path: &Path) -> std::io::Result<(std::fs::File, (u64, [u8; 16]))> {
            let name = path.file_name().and_then(|n| n.to_str()).ok_or_else(|| std::io::Error::other("fixture name"))?;
            let suffix = name.strip_prefix("MindTrailSyncDrop-").ok_or_else(|| std::io::Error::other("fixture prefix"))?;
            if suffix.len() != 32 || !suffix.bytes().all(|b| b.is_ascii_hexdigit()) || path != Path::new(env!("CARGO_MANIFEST_DIR")).join(name) { return Err(std::io::Error::other("fixture outside workspace")); }
            let metadata = std::fs::symlink_metadata(path)?;
            if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 { return Err(std::io::Error::other("invalid fixture root")); }
            let handle = std::fs::OpenOptions::new().access_mode(FILE_READ_ATTRIBUTES).share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT).open(path)?;
            let mut id = FILE_ID_INFO::default();
            if unsafe { GetFileInformationByHandleEx(handle.as_raw_handle() as HANDLE, FileIdInfo, (&mut id as *mut FILE_ID_INFO).cast(), std::mem::size_of_val(&id) as u32) } == 0 { return Err(std::io::Error::last_os_error()); }
            Ok((handle, (id.VolumeSerialNumber, id.FileId.Identifier)))
        }
        struct Fixture { root: PathBuf, guard: Option<std::fs::File>, id: (u64, [u8; 16]), child: Option<std::process::Child> }
        impl Drop for Fixture {
            fn drop(&mut self) {
                if let Some(child) = &mut self.child {
                    if child.try_wait().ok().flatten().is_none() {
                        let _ = child.kill();
                        let deadline = Instant::now() + Duration::from_secs(5);
                        while child.try_wait().ok().flatten().is_none() && Instant::now() < deadline { std::thread::sleep(Duration::from_millis(10)); }
                        if child.try_wait().ok().flatten().is_none() { eprintln!("fixture retained: child exit unconfirmed"); return; }
                    }
                }
                match root_guard(&self.root) { Ok((_guard, id)) if id == self.id => {}, _ => { eprintln!("fixture retained: root changed"); return; } }
                // A recursive path deletion after releasing the handle could
                // follow a replacement. Preserve the fixture instead.
                eprintln!("fixture retained (identity-bound recursive cleanup unavailable): {}", self.root.display());
            }
        }

        #[test]
        fn drop_waits_for_real_active_store_work() {
            const FIXTURE: &str = "MINDTRAIL_SYNC_DROP_FIXTURE";
            let Some(value) = std::env::var_os(FIXTURE) else {
                let mut random = [0u8; 16]; getrandom::fill(&mut random).unwrap();
                let suffix: String = random.iter().map(|b| format!("{b:02x}")).collect();
                let root = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailSyncDrop-{suffix}"));
                std::fs::create_dir(&root).unwrap();
                let (guard, id) = root_guard(&root).unwrap();
                let mut fixture = Fixture { root, guard: Some(guard), id, child: None };
                fixture.child = Some(std::process::Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", "gateway::lifecycle::tests::drop_waits_for_real_active_store_work"])
                    .env(FIXTURE, &fixture.root).spawn().unwrap());
                let deadline = Instant::now() + Duration::from_secs(30);
                let status = loop {
                    if let Some(status) = fixture.child.as_mut().unwrap().try_wait().unwrap() { break status; }
                    assert!(Instant::now() < deadline, "fixture child timed out");
                    std::thread::sleep(Duration::from_millis(10));
                };
                assert!(status.success());
                return;
            };
            let root = PathBuf::from(value);
            let (_root_guard, _identity) = root_guard(&root).expect("invalid fixture root");
            let path = root.join("state");
            let gateway = Gateway::start(&path).unwrap();
            let access = gateway.0.as_ref().unwrap().access();
            let (entered_tx, entered_rx) = mpsc::channel();
            let (release_tx, release_rx) = mpsc::channel();
            let worker_access = access.clone();
            let worker = std::thread::spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
                runtime.block_on(worker_access.run(move |_| {
                    entered_tx.send(()).unwrap();
                    let _ = release_rx.recv_timeout(Duration::from_secs(15));
                    Ok(())
                }))
            });
            entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            let (done_tx, done_rx) = mpsc::channel();
            let dropper = std::thread::spawn(move || { drop(gateway); let _ = done_tx.send(()); });
            let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            while runtime.block_on(access.run(|_| Ok(()))).is_ok() {
                assert!(Instant::now() < deadline, "shutdown did not close admission");
                std::thread::yield_now();
            }
            assert!(done_rx.recv_timeout(Duration::from_millis(100)).is_err(), "Drop returned while real work was active");
            let error = match Gateway::start(&path) { Ok(_) => panic!("lock released while work active"), Err(error) => error };
            assert!(error.starts_with("Gateway lock failed:"), "unexpected stage: {error}");
            release_tx.send(()).unwrap();
            done_rx.recv_timeout(Duration::from_secs(10)).expect("Drop did not complete");
            worker.join().unwrap().unwrap(); dropper.join().unwrap();
            let restarted = Gateway::start(&path).expect("lock not released after Drop");
            restarted.shutdown().unwrap();
        }
    }
}







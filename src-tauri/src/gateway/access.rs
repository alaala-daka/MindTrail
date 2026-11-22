use std::sync::{Arc, Condvar, Mutex};
use super::store::Store;

/// Coordinates shutdown with work that may outlive its requesting future.
#[derive(Clone)]
pub(super) struct StoreAccess(Arc<Inner>);
struct Inner { state: Mutex<State>, idle: Condvar, permits: Arc<tokio::sync::Semaphore> }
struct State { store: Option<Arc<Store>>, active: usize }

pub(super) struct StoreLease {
    // Fields drop in declaration order: the store must go before the counter.
    store: Arc<Store>,
    _permit: tokio::sync::OwnedSemaphorePermit,
    _activity: Activity,
}
struct Activity(Arc<Inner>);

impl StoreAccess {
    pub(super) fn new(store: Arc<Store>) -> Self {
        Self(Arc::new(Inner { state: Mutex::new(State { store: Some(store), active: 0 }), idle: Condvar::new(), permits: Arc::new(tokio::sync::Semaphore::new(8)) }))
    }
    pub(super) fn permits(&self) -> Arc<tokio::sync::Semaphore> { self.0.permits.clone() }
    fn acquire(&self) -> Option<StoreLease> {
        let permit = self.0.permits.clone().try_acquire_owned().ok()?;
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        let store = state.store.as_ref()?.clone();
        state.active += 1;
        Some(StoreLease { store, _permit: permit, _activity: Activity(self.0.clone()) })
    }
    pub(super) async fn run<T: Send + 'static>(&self, operation: impl FnOnce(&Store) -> Result<T, crate::protocol::ProtocolError> + Send + 'static) -> Result<T, crate::protocol::ProtocolError> {
        let lease = self.acquire().ok_or_else(|| crate::protocol::temporary_storage_error("busy or closed"))?;
        tokio::task::spawn_blocking(move || operation(lease.store())).await.map_err(|_| crate::protocol::temporary_storage_error("storage worker failed"))?
    }
    pub(super) fn close(&self) {
        self.0.state.lock().unwrap_or_else(|e| e.into_inner()).store.take();
    }
    pub(super) fn wait_idle(&self) {
        self.wait_idle_with_hook(|| {});
    }
    fn wait_idle_with_hook(&self, mut waiting: impl FnMut()) {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        while state.active != 0 {
            waiting();
            state = self.0.idle.wait(state).unwrap_or_else(|e| e.into_inner());
        }
    }
}
impl StoreLease {
    pub(super) fn store(&self) -> &Store { &self.store }
}
impl Drop for Activity {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        state.active -= 1;
        if state.active == 0 { self.0.idle.notify_all(); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::mpsc, time::Duration};

    async fn wait_started(rx: &mpsc::Receiver<()>, timeout: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            match rx.try_recv() {
                Ok(()) => return true,
                Err(mpsc::TryRecvError::Disconnected) => return false,
                Err(mpsc::TryRecvError::Empty) if tokio::time::Instant::now() >= deadline => return false,
                Err(mpsc::TryRecvError::Empty) => tokio::task::yield_now().await,
            }
        }
    }

    #[test]
    fn startup_wait_is_bounded_for_timeout_and_disconnect() {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let (tx, rx) = mpsc::channel();
        assert!(!runtime.block_on(wait_started(&rx, Duration::from_millis(10))));
        drop(tx);
        assert!(!runtime.block_on(wait_started(&rx, Duration::from_secs(1))));
    }

    fn access() -> (StoreAccess, std::path::PathBuf) {
        let mut bytes = [0; 16];
        getrandom::fill(&mut bytes).unwrap();
        let path = std::env::temp_dir().join(format!("mindtrail-access-{bytes:?}.sqlite"));
        (StoreAccess::new(Arc::new(Store::open(&path).unwrap())), path)
    }

    #[test]
    fn close_rejects_new_work_and_waits_for_detached_work() {
        let (access, path) = access();
        let lease = access.acquire().unwrap();
        let (release_tx, release_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || { release_rx.recv().unwrap(); drop(lease); });
        access.close();
        assert!(access.acquire().is_none());
        let (done_tx, done_rx) = mpsc::channel();
        let (waiting_tx, waiting_rx) = mpsc::channel();
        let waiter = std::thread::spawn(move || { access.wait_idle_with_hook(|| { waiting_tx.send(()).unwrap(); }); done_tx.send(()).unwrap(); });
        waiting_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(done_rx.try_recv().is_err());
        release_tx.send(()).unwrap();
        done_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        worker.join().unwrap(); waiter.join().unwrap();
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn panic_releases_one_of_multiple_leases() {
        let (access, path) = access();
        let first = access.acquire().unwrap();
        let second = access.acquire().unwrap();
        access.close();
        let worker = std::thread::spawn(move || { let _lease = first; panic!("simulated worker panic"); });
        assert!(worker.join().is_err());
        assert_eq!(access.0.state.lock().unwrap().active, 1);
        let (waiting_tx, waiting_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let waiter = std::thread::spawn(move || {
            access.wait_idle_with_hook(|| { waiting_tx.send(()).unwrap(); });
            done_tx.send(()).unwrap();
        });
        waiting_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(done_rx.try_recv().is_err());
        drop(second);
        done_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        waiter.join().unwrap();
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn cancelled_future_keeps_lease_and_permit_until_worker_finishes() {
        let (access, path) = access();
        let permits = access.permits();
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let worker_access = access.clone();
        let task = runtime.spawn(async move {
            worker_access.run(move |_| {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Ok(())
            }).await
        });
        let started = runtime.block_on(wait_started(&started_rx, Duration::from_secs(5)));
        if !started { drop(release_tx); task.abort(); runtime.shutdown_timeout(Duration::from_secs(1)); panic!("worker did not start"); }
        task.abort();
        runtime.block_on(async { assert!(task.await.unwrap_err().is_cancelled()); });
        access.close();
        assert!(access.acquire().is_none());
        assert_eq!(permits.available_permits(), 7);
        let (waiting_tx, waiting_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let waiter = std::thread::spawn(move || {
            access.wait_idle_with_hook(|| { waiting_tx.send(()).unwrap(); });
            done_tx.send(()).unwrap();
        });
        waiting_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(done_rx.try_recv().is_err());
        release_tx.send(()).unwrap();
        done_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(permits.available_permits(), 8);
        waiter.join().unwrap();
        drop(runtime);
        std::fs::remove_file(path).unwrap();
    }
}

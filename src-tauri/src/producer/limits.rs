use std::{sync::{Arc,Mutex},time::{Duration,Instant}};

struct State { in_flight:usize, credit:u128, last:Instant }
#[derive(Clone)]
pub struct Limits(Arc<Mutex<State>>);
pub struct Permit(Arc<Mutex<State>>);
impl Limits {
    pub fn new(now:Instant)->Self {Self(Arc::new(Mutex::new(State{in_flight:0,credit:20_000_000_000,last:now})))}
    /// Reserve at dispatch, before spawning an SDK request future.
    pub fn try_acquire(&self,now:Instant)->Option<Permit> {
        let mut state=self.0.lock().unwrap_or_else(|e|e.into_inner());
        let elapsed=now.checked_duration_since(state.last).unwrap_or(Duration::ZERO);
        state.credit=state.credit.saturating_add(elapsed.as_nanos().saturating_mul(20)).min(20_000_000_000);state.last=state.last.max(now);
        if state.in_flight>=16 || state.credit<1_000_000_000 {return None}
        state.credit-=1_000_000_000;state.in_flight+=1;Some(Permit(self.0.clone()))
    }
}
impl Drop for Permit {fn drop(&mut self) {let mut state=self.0.lock().unwrap_or_else(|e|e.into_inner());state.in_flight-=1;}}

#[cfg(test)] mod tests {
    use super::*;
    #[test] fn concurrent_limit_releases_on_drop() {let now=Instant::now();let limit=Limits::new(now);let mut held:Vec<_>=(0..16).map(|_|limit.try_acquire(now).unwrap()).collect();assert!(limit.try_acquire(now).is_none());held.pop();assert!(limit.try_acquire(now).is_some());}
    #[test] fn burst_refills_at_twenty_per_second() {let now=Instant::now();let limit=Limits::new(now);for _ in 0..20 {drop(limit.try_acquire(now).unwrap());}assert!(limit.try_acquire(now).is_none());assert!(limit.try_acquire(now+Duration::from_millis(49)).is_none());assert!(limit.try_acquire(now+Duration::from_millis(50)).is_some());}
}

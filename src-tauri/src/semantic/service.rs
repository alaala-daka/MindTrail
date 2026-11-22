use std::{collections::{HashMap, VecDeque}, sync::{Arc, Mutex}, time::{Duration, SystemTime, UNIX_EPOCH}};
use super::{provider::SemanticProvider, types::*, validation::validate_result};

pub(crate) const PROMPT_VERSION: &str = "mindtrail-insight-v1";
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderOutput { sections: Vec<SemanticSection> }
#[derive(Clone)]
pub struct ProviderConfig { pub provider: String, pub model: String, pub endpoint: String, pub origin: String, pub epoch: u64, pub implementation: Arc<dyn SemanticProvider> }
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct CacheKey { context: SemanticContext, provider: String, model: String, endpoint: String, epoch: u64, prompt_version: String }
struct Ticket { input: SemanticInput, config: ProviderConfig, expires_at: u64, bytes: usize }
struct Request { sender: tokio::sync::watch::Sender<bool>, ticket_id: String, context: SemanticContext }
#[derive(Default)]
struct State { config: Option<ProviderConfig>, config_error: Option<SemanticError>, tickets: HashMap<String, Ticket>, ticket_bytes: usize, requests: HashMap<String, Request>, cache: HashMap<CacheKey, (SemanticResult, usize)>, order: VecDeque<CacheKey>, cache_bytes: usize }
#[derive(Default)]
pub struct SemanticService { state: Mutex<State> }
struct ActiveRequest<'a> { service: &'a SemanticService, id: String, armed: bool }
impl Drop for ActiveRequest<'_> {
    fn drop(&mut self) { if self.armed { if let Ok(mut state) = self.service.state.lock() { state.requests.remove(&self.id); } } }
}

fn now() -> Result<u64, SemanticError> { SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).map_err(|_| SemanticError::InvalidConfig) }
fn random_id() -> Result<String, SemanticError> { let mut bytes = [0u8; 32]; getrandom::fill(&mut bytes).map_err(|_| SemanticError::InvalidConfig)?; Ok(bytes.iter().map(|b| format!("{b:02x}")).collect()) }
fn key(input: &SemanticInput, config: &ProviderConfig) -> CacheKey { CacheKey { context: input.context.clone(), provider: config.provider.clone(), model: config.model.clone(), endpoint: config.endpoint.clone(), epoch: config.epoch, prompt_version: PROMPT_VERSION.into() } }
impl SemanticService {
    pub fn status(&self) -> Result<ProviderStatus, SemanticError> {
        let state = self.state.lock().map_err(|_| SemanticError::Busy)?;
        if let Some(error) = state.config_error { return Err(error); }
        Ok(match &state.config { Some(c) => ProviderStatus { configured: true, provider: Some(c.provider.clone()), model: Some(c.model.clone()), endpoint_origin: Some(c.origin.clone()) }, None => ProviderStatus { configured: false, provider: None, model: None, endpoint_origin: None } })
    }
    pub fn configure(&self, config: Option<ProviderConfig>) -> Result<(), SemanticError> {
        let mut state = self.state.lock().map_err(|_| SemanticError::Busy)?;
        for request in state.requests.values() { let _ = request.sender.send(true); }
        state.tickets.clear(); state.ticket_bytes = 0; state.cache.clear(); state.order.clear(); state.cache_bytes = 0; state.config_error = None; state.config = config; Ok(())
    }
    pub fn set_config_error(&self, error: SemanticError) { if let Ok(mut state) = self.state.lock() { state.config_error = Some(error); } }
    pub fn prepare(&self, input: SemanticInput) -> Result<Preview, SemanticError> { self.prepare_at(input, now()?) }
    fn prepare_at(&self, input: SemanticInput, time: u64) -> Result<Preview, SemanticError> {
        if input.evidence.is_empty() { return Err(SemanticError::InsufficientEvidence); }
        let bytes = serde_json::to_vec(&input).map_err(|_| SemanticError::InvalidContext)?.len();
        if bytes > 256 * 1024 { return Err(SemanticError::InputTooLarge); }
        let mut state = self.state.lock().map_err(|_| SemanticError::Busy)?;
        let expired: Vec<_> = state.tickets.iter().filter(|(_, t)| t.expires_at <= time).map(|(id, _)| id.clone()).collect();
        for id in expired { if let Some(t) = state.tickets.remove(&id) { state.ticket_bytes -= t.bytes; } }
        let config = state.config.clone().ok_or(SemanticError::NotConfigured)?;
        if state.tickets.len() >= 8 || state.ticket_bytes + bytes > 2 * 1024 * 1024 { return Err(SemanticError::Busy); }
        let id = random_id()?; let expires_at = time.checked_add(300).ok_or(SemanticError::InvalidContext)?;
        let preview = Preview { ticket_id: id.clone(), context: input.context.clone(), provider: config.provider.clone(), model: config.model.clone(), prompt_version: PROMPT_VERSION.into(), event_count: input.events.len(), evidence_count: input.evidence.len(), byte_count: bytes, events: input.events.clone(), evidence: input.evidence.clone(), expires_at };
        state.ticket_bytes += bytes; state.tickets.insert(id, Ticket { input, config, expires_at, bytes }); Ok(preview)
    }
    pub fn cancel(&self, request_id: &str) -> Result<(), SemanticError> {
        let state = self.state.lock().map_err(|_| SemanticError::Busy)?;
        if let Some(request) = state.requests.get(request_id) { let _ = request.sender.send(true); } Ok(())
    }
    pub fn release(&self, ticket_id: &str, context: &SemanticContext) -> Result<(), SemanticError> {
        let mut state = self.state.lock().map_err(|_| SemanticError::Busy)?;
        if let Some(ticket) = state.tickets.get(ticket_id) {
            if &ticket.input.context != context { return Err(SemanticError::InvalidContext); }
            let removed = state.tickets.remove(ticket_id).ok_or(SemanticError::InvalidContext)?;
            state.ticket_bytes -= removed.bytes;
        }
        for request in state.requests.values() {
            if request.ticket_id == ticket_id {
                if &request.context != context { return Err(SemanticError::InvalidContext); }
                let _ = request.sender.send(true);
            }
        }
        Ok(())
    }
    pub fn cancel_all(&self) { if let Ok(state) = self.state.lock() { for request in state.requests.values() { let _ = request.sender.send(true); } } }
    pub async fn generate(&self, ticket_id: &str, request_id: &str) -> Result<SemanticResult, SemanticError> {
        self.generate_with_timeout(ticket_id, request_id, Duration::from_secs(30)).await
    }
    async fn generate_with_timeout(&self, ticket_id: &str, request_id: &str, timeout: Duration) -> Result<SemanticResult, SemanticError> {
        if request_id.is_empty() || request_id.len() > 128 { return Err(SemanticError::InvalidContext); }
        let (ticket, cache_key, mut cancel) = {
            let mut state = self.state.lock().map_err(|_| SemanticError::Busy)?;
            if state.requests.len() >= 2 || state.requests.contains_key(request_id) { return Err(SemanticError::Busy); }
            let ticket = state.tickets.remove(ticket_id).ok_or(SemanticError::InvalidContext)?; state.ticket_bytes -= ticket.bytes;
            if ticket.expires_at <= now()? { return Err(SemanticError::InvalidContext); }
            let cache_key = key(&ticket.input, &ticket.config);
            if let Some((result, _)) = state.cache.get(&cache_key) { return Ok(result.clone()); }
            let (sender, receiver) = tokio::sync::watch::channel(false); state.requests.insert(request_id.to_owned(), Request { sender, ticket_id: ticket_id.to_owned(), context: ticket.input.context.clone() });
            (ticket, cache_key, receiver)
        };
        let mut active = ActiveRequest { service: self, id: request_id.to_owned(), armed: true };
        let output = tokio::select! {
            result = tokio::time::timeout(timeout, ticket.config.implementation.generate(&ticket.input)) => match result { Ok(value) => value, Err(_) => Err(SemanticError::Timeout) },
            _ = cancel.changed() => Err(SemanticError::Cancelled),
        };
        let parsed = output.and_then(|raw| {
            let value = crate::protocol::raw::parse_strict_value(raw.as_bytes(), 128 * 1024).map_err(|_| SemanticError::InvalidOutput)?;
            let output: ProviderOutput = serde_json::from_value(value).map_err(|_| SemanticError::InvalidOutput)?;
            if output.sections.is_empty() { return Err(SemanticError::InsufficientEvidence); }
            let result = SemanticResult { context: ticket.input.context.clone(), provider: ticket.config.provider.clone(), model: ticket.config.model.clone(), prompt_version: PROMPT_VERSION.into(), sections: output.sections };
            validate_result(&ticket.input, &result).map_err(|_| SemanticError::InvalidReference)?; Ok(result)
        });
        let mut state = self.state.lock().map_err(|_| SemanticError::Busy)?;
        let request = state.requests.remove(request_id).ok_or(SemanticError::Cancelled)?;
        active.armed = false;
        if *request.sender.borrow() || state.config.as_ref().is_none_or(|c| key(&ticket.input, c) != cache_key) { return Err(SemanticError::Cancelled); }
        let result = parsed?;
        let bytes = serde_json::to_vec(&result).map_err(|_| SemanticError::InvalidOutput)?.len();
        while state.cache.len() >= 32 || state.cache_bytes + bytes > 4 * 1024 * 1024 { if let Some(old) = state.order.pop_front() { if let Some((_, n)) = state.cache.remove(&old) { state.cache_bytes -= n; } } else { return Err(SemanticError::Busy); } }
        if let Some((_, old_bytes)) = state.cache.remove(&cache_key) { state.cache_bytes -= old_bytes; state.order.retain(|item| item != &cache_key); }
        state.cache_bytes += bytes; state.order.push_back(cache_key.clone()); state.cache.insert(cache_key, (result.clone(), bytes)); Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use serde_json::json;

    struct Fake { calls: AtomicUsize, dropped: AtomicUsize, entered: tokio::sync::Notify, gate: Option<Arc<tokio::sync::Notify>>, bad: bool }
    struct Dropped<'a>(&'a AtomicUsize);
    impl Drop for Dropped<'_> { fn drop(&mut self) { self.0.fetch_add(1, Ordering::SeqCst); } }
    impl SemanticProvider for Fake {
        fn generate<'a>(&'a self, input: &'a SemanticInput) -> std::pin::Pin<Box<dyn std::future::Future<Output=Result<String, SemanticError>> + Send + 'a>> {
            Box::pin(async move {
                let _dropped = Dropped(&self.dropped);
                self.calls.fetch_add(1, Ordering::SeqCst); self.entered.notify_one();
                if let Some(gate) = &self.gate { gate.notified().await; }
                Ok(json!({"sections":[{"title":"T","text":"text","citations":[{"kind":"quote","session_id":input.context.session_id,"node_id":"n","id":if self.bad { "s:99" } else { "s:3" }}],"related_node_ids":[]}]}).to_string())
            })
        }
    }
    struct Oversize;
    impl SemanticProvider for Oversize {
        fn generate<'a>(&'a self, _: &'a SemanticInput) -> std::pin::Pin<Box<dyn std::future::Future<Output=Result<String, SemanticError>> + Send + 'a>> {
            Box::pin(async { Ok("x".repeat(256*1024+1)) })
        }
    }
    struct Large;
    impl SemanticProvider for Large {
        fn generate<'a>(&'a self, input: &'a SemanticInput) -> std::pin::Pin<Box<dyn std::future::Future<Output=Result<String, SemanticError>> + Send + 'a>> {
            Box::pin(async move {
                let mut section = json!({"title":"T","text":"","citations":[{"kind":"quote","session_id":"s","node_id":"n","id":"s:3"}],"related_node_ids":[]});
                let overhead = json!({"sections":vec![section.clone();16]}).to_string().len();
                section["text"] = json!("x".repeat(((128*1024-overhead)/16).min(8192)));
                let _ = input;
                Ok(json!({"sections":vec![section;16]}).to_string())
            })
        }
    }
    fn input() -> SemanticInput {
        let context = SemanticContext { store_id:"a".into(), session_id:"s".into(), node_id:"n".into(), prefix_seq:3 };
        let events = vec![json!({"session":"s","seq":1,"type":"task.start","payload":{}}),json!({"session":"s","seq":2,"type":"capability.intent","payload":{"id":"n"}}),json!({"session":"s","seq":3,"type":"capability.done","payload":{"id":"n","quote":"q"}})];
        super::super::snapshot::build_input(context, "a", events).unwrap()
    }
    fn config(fake: Arc<Fake>) -> ProviderConfig { ProviderConfig { provider:"mock".into(), model:"m".into(), endpoint:"https://example.invalid".into(), origin:"https://example.invalid".into(), epoch:1, implementation:fake } }
    fn fake(gate: Option<Arc<tokio::sync::Notify>>, bad: bool) -> Arc<Fake> { Arc::new(Fake { calls:AtomicUsize::new(0), dropped:AtomicUsize::new(0), entered:tokio::sync::Notify::new(), gate, bad }) }
    fn runtime() -> tokio::runtime::Runtime { tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap() }

    #[test]
    fn semantic_prepare_never_calls_provider_and_ticket_is_single_use() {
        runtime().block_on(async {
            let svc = SemanticService::default(); let f = fake(None, false); svc.configure(Some(config(f.clone()))).unwrap();
            let p = svc.prepare(input()).unwrap(); assert_eq!(f.calls.load(Ordering::SeqCst), 0);
            assert!(svc.generate(&p.ticket_id, "r1").await.is_ok());
            assert!(matches!(svc.generate(&p.ticket_id, "r2").await, Err(SemanticError::InvalidContext)));
            let cached = svc.prepare(input()).unwrap(); assert!(svc.generate(&cached.ticket_id, "r3").await.is_ok()); assert_eq!(f.calls.load(Ordering::SeqCst), 1);
            let expired = svc.prepare_at(input(), 0).unwrap(); assert!(matches!(svc.generate(&expired.ticket_id, "r4").await, Err(SemanticError::InvalidContext)));
        });
    }
    #[test]
    fn semantic_cancel_prevents_cache_publish() {
        runtime().block_on(async {
            let svc = Arc::new(SemanticService::default()); let f = fake(Some(Arc::new(tokio::sync::Notify::new())), false); svc.configure(Some(config(f.clone()))).unwrap();
            let p = svc.prepare(input()).unwrap(); let task_svc = svc.clone();
            let task = tokio::spawn(async move { task_svc.generate(&p.ticket_id, "r").await });
            f.entered.notified().await; svc.cancel("r").unwrap();
            assert!(matches!(task.await.unwrap(), Err(SemanticError::Cancelled)));
            assert!(svc.state.lock().unwrap().cache.is_empty());
        });
    }
    #[test]
    fn semantic_rejects_bad_citation_and_separates_context_keys() {
        runtime().block_on(async {
            let svc = SemanticService::default(); let f = fake(None, true); svc.configure(Some(config(f))).unwrap();
            let p = svc.prepare(input()).unwrap(); assert!(matches!(svc.generate(&p.ticket_id, "r").await, Err(SemanticError::InvalidReference)));
            assert!(svc.state.lock().unwrap().cache.is_empty());
            let a = input(); let mut b = a.clone(); b.context.store_id = "other".into();
            let cfg = config(fake(None, false)); assert_ne!(key(&a, &cfg), key(&b, &cfg));
            b = a.clone(); b.context.prefix_seq += 1; assert_ne!(key(&a, &cfg), key(&b, &cfg));
            let mut cfg2 = cfg.clone(); cfg2.epoch += 1; assert_ne!(key(&a, &cfg), key(&a, &cfg2));
            cfg2 = cfg.clone(); cfg2.model = "another".into(); assert_ne!(key(&a, &cfg), key(&a, &cfg2));
            cfg2 = cfg.clone(); cfg2.endpoint = "https://other.invalid".into(); assert_ne!(key(&a, &cfg), key(&a, &cfg2));
            assert!(svc.state.lock().unwrap().requests.is_empty());
        });
    }
    #[test]
    fn semantic_concurrency_and_configuration_change_release_requests() {
        runtime().block_on(async {
            let svc = Arc::new(SemanticService::default()); let f = fake(Some(Arc::new(tokio::sync::Notify::new())), false); svc.configure(Some(config(f.clone()))).unwrap();
            let p1=svc.prepare(input()).unwrap(); let p2=svc.prepare(input()).unwrap(); let p3=svc.prepare(input()).unwrap();
            let s1=svc.clone(); let t1=tokio::spawn(async move { s1.generate(&p1.ticket_id,"one").await }); f.entered.notified().await;
            assert!(matches!(svc.generate(&p2.ticket_id,"one").await,Err(SemanticError::Busy)));
            let s2=svc.clone(); let t2=tokio::spawn(async move { s2.generate(&p2.ticket_id,"two").await }); f.entered.notified().await;
            assert!(matches!(svc.generate(&p3.ticket_id,"three").await,Err(SemanticError::Busy)));
            svc.configure(Some(config(fake(None,false)))).unwrap();
            assert!(matches!(t1.await.unwrap(),Err(SemanticError::Cancelled)));
            assert!(matches!(t2.await.unwrap(),Err(SemanticError::Cancelled)));
            assert!(svc.state.lock().unwrap().requests.is_empty());
            let p=svc.prepare(input()).unwrap(); assert!(svc.generate(&p.ticket_id,"three").await.is_ok());
        });
    }
    #[test]
    fn semantic_dropped_future_releases_slot() {
        runtime().block_on(async {
            let svc=Arc::new(SemanticService::default()); let f=fake(Some(Arc::new(tokio::sync::Notify::new())),false); svc.configure(Some(config(f.clone()))).unwrap();
            let p=svc.prepare(input()).unwrap(); let worker=svc.clone(); let task=tokio::spawn(async move {worker.generate(&p.ticket_id,"drop").await});
            f.entered.notified().await; task.abort(); let _=task.await;
            assert!(svc.state.lock().unwrap().requests.is_empty());
        });
    }
    #[test]
    fn semantic_timeout_drops_provider_and_releases_slot() {
        runtime().block_on(async {
            let svc=SemanticService::default(); let f=fake(Some(Arc::new(tokio::sync::Notify::new())),false); svc.configure(Some(config(f.clone()))).unwrap();
            let p=svc.prepare(input()).unwrap();
            assert!(matches!(svc.generate_with_timeout(&p.ticket_id,"timeout",Duration::ZERO).await,Err(SemanticError::Timeout)));
            assert_eq!(f.calls.load(Ordering::SeqCst),1); assert_eq!(f.dropped.load(Ordering::SeqCst),1);
            let state=svc.state.lock().unwrap(); assert!(state.requests.is_empty()); assert!(state.cache.is_empty()); drop(state);
            let another=svc.prepare(input()).unwrap(); assert!(matches!(svc.generate_with_timeout(&another.ticket_id,"timeout",Duration::ZERO).await,Err(SemanticError::Timeout)));
        });
    }
    #[test]
    fn semantic_ticket_and_cache_entry_limits_are_enforced() {
        runtime().block_on(async {
            let svc=SemanticService::default(); let f=fake(None,false); svc.configure(Some(config(f))).unwrap();
            let tickets: Vec<_> = (0..8).map(|_|svc.prepare(input()).unwrap()).collect();
            assert!(matches!(svc.prepare(input()),Err(SemanticError::Busy)));
            assert!(svc.state.lock().unwrap().ticket_bytes <= 2*1024*1024);
            svc.generate(&tickets[0].ticket_id,"first").await.unwrap();
            assert!(svc.prepare(input()).is_ok());
            svc.configure(Some(config(fake(None,false)))).unwrap();
            for i in 0..33 {
                let mut current=input(); current.context.store_id=format!("store-{i}");
                let p=svc.prepare(current).unwrap(); svc.generate(&p.ticket_id,&format!("request-{i}")).await.unwrap();
            }
            let state=svc.state.lock().unwrap();
            assert_eq!(state.cache.len(),32); assert_eq!(state.order.len(),32);
            assert!(!state.cache.keys().any(|k|k.context.store_id=="store-0"));
            assert!(state.cache_bytes <= 4*1024*1024);
        });
    }
    #[test]
    fn semantic_oversize_input_and_output_are_not_cached() {
        runtime().block_on(async {
            let svc=SemanticService::default(); let mut cfg=config(fake(None,false)); cfg.implementation=Arc::new(Oversize); svc.configure(Some(cfg)).unwrap();
            let mut too_large=input(); too_large.events.push(json!("x".repeat(256*1024)));
            assert!(matches!(svc.prepare(too_large),Err(SemanticError::InputTooLarge)));
            let p=svc.prepare(input()).unwrap();
            assert!(matches!(svc.generate(&p.ticket_id,"large").await,Err(SemanticError::InvalidOutput)));
            let state=svc.state.lock().unwrap(); assert!(state.cache.is_empty()); assert!(state.requests.is_empty());
        });
    }
    #[test]
    fn semantic_cache_evicts_on_byte_budget_before_entry_limit() {
        runtime().block_on(async {
            let svc=SemanticService::default(); let mut cfg=config(fake(None,false)); cfg.implementation=Arc::new(Large); svc.configure(Some(cfg)).unwrap();
            for i in 0..32 {
                let mut current=input(); current.context.store_id=format!("large-store-{i}");
                let p=svc.prepare(current).unwrap(); svc.generate(&p.ticket_id,&format!("large-{i}")).await.unwrap();
            }
            let state=svc.state.lock().unwrap();
            assert!(state.cache.len() < 32); assert!(state.cache_bytes <= 4*1024*1024);
            assert!(!state.cache.keys().any(|key|key.context.store_id=="large-store-0"));
        });
    }
    #[test]
    fn semantic_release_is_context_bound_and_atomic_with_generation() {
        runtime().block_on(async {
            let svc=Arc::new(SemanticService::default()); let f=fake(Some(Arc::new(tokio::sync::Notify::new())),false); svc.configure(Some(config(f.clone()))).unwrap();
            for _ in 0..12 { let p=svc.prepare(input()).unwrap(); svc.release(&p.ticket_id,&p.context).unwrap(); }
            assert!(svc.state.lock().unwrap().tickets.is_empty());
            let p=svc.prepare(input()).unwrap(); let mut wrong=p.context.clone(); wrong.store_id="other".into();
            assert!(matches!(svc.release(&p.ticket_id,&wrong),Err(SemanticError::InvalidContext)));
            assert_eq!(svc.state.lock().unwrap().tickets.len(),1);
            svc.release(&p.ticket_id,&p.context).unwrap();
            assert!(matches!(svc.generate(&p.ticket_id,"before").await,Err(SemanticError::InvalidContext)));
            assert_eq!(f.calls.load(Ordering::SeqCst),0);
            let p=svc.prepare(input()).unwrap(); let id=p.ticket_id.clone(); let context=p.context.clone(); let worker=svc.clone();
            let task=tokio::spawn(async move { worker.generate(&id,"during").await }); f.entered.notified().await;
            assert!(matches!(svc.release(&p.ticket_id,&wrong),Err(SemanticError::InvalidContext)));
            assert!(!task.is_finished());
            svc.release(&p.ticket_id,&context).unwrap();
            assert!(matches!(task.await.unwrap(),Err(SemanticError::Cancelled)));
            let state=svc.state.lock().unwrap(); assert!(state.tickets.is_empty()); assert!(state.requests.is_empty()); assert!(state.cache.is_empty());
        });
    }
}

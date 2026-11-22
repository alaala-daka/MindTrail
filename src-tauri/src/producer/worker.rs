use std::{collections::HashMap,path::Path};
use super::{gateway::{Attempt,EndpointSnapshot,GatewayClient},queue::{Entry,Queue}};

#[derive(Default)]
struct Retry { attempts:u32, due:u64 }
#[derive(Default)]
pub struct Worker { retries:HashMap<String,Retry>, last:Option<String>, unauthorized:Option<EndpointSnapshot>, failed_post:Option<EndpointSnapshot>, observed:Option<EndpointSnapshot> }
#[derive(Debug, PartialEq, Eq)]
pub enum Tick { Idle, Deferred, Committed, Rejected, Unauthorized, Error(&'static str) }
pub enum Command {
    AdmitMcp { session:String, operation_id:String, kind:String, payload:serde_json::Value, reply:std::sync::mpsc::Sender<Result<i64,&'static str>> },
    AdmitBatch { batch_id:String, bytes:Vec<u8>, reply:std::sync::mpsc::Sender<Result<i64,&'static str>> },
    Get { id:i64, reply:std::sync::mpsc::Sender<Result<Option<Entry>,&'static str>> },
    Watch { id:i64, deadline:std::time::Instant, reply:std::sync::mpsc::Sender<Result<Option<Entry>,&'static str>> },
    Wake,
    Shutdown,
    #[cfg(test)] Pause { ready:std::sync::mpsc::Sender<()>, resume:std::sync::mpsc::Receiver<()> },
}
pub fn channel() -> (std::sync::mpsc::SyncSender<Command>,std::sync::mpsc::Receiver<Command>) { std::sync::mpsc::sync_channel(64) }
enum NetworkEvent {
    Bind(String,tokio::sync::oneshot::Sender<Result<(), &'static str>>),
    Done(Entry,Option<EndpointSnapshot>,Option<EndpointSnapshot>,NetworkResult),
}
enum NetworkResult { Outcome(Attempt), Error(&'static str), Unauthorized(EndpointSnapshot) }
#[cfg(test)]
#[derive(Clone)]
struct Hooks { done:std::sync::mpsc::Sender<()>, panic_once:std::sync::Arc<std::sync::atomic::AtomicBool> }
async fn network_round(root:std::path::PathBuf, entry:Entry, mode:super::queue::QueueMode, endpoint:EndpointSnapshot, mut failed:Option<EndpointSnapshot>, tx:std::sync::mpsc::Sender<NetworkEvent>, #[cfg(test)] hook:Option<Hooks>) {
    #[cfg(test)] if hook.as_ref().is_some_and(|h|h.panic_once.swap(false,std::sync::atomic::Ordering::SeqCst)) {panic!("injected network task failure")}
    let client=GatewayClient::new(std::time::Duration::from_secs(2));
    let mut current=endpoint; let mut posts=0;
    let result=loop {
        if let Err(e)=client.health(&current).await {
            break if e=="gateway_unauthorized" && failed.as_ref()==Some(&current) {NetworkResult::Unauthorized(current.clone())} else {NetworkResult::Error(e)};
        }
        let (bind,answer)=tokio::sync::oneshot::channel();
        if tx.send(NetworkEvent::Bind(current.store_id().to_owned(),bind)).is_err() {return}
        match answer.await {Ok(Ok(()))=>{},Ok(Err(e))=>break NetworkResult::Error(e),Err(_)=>return}
        posts+=1;
        #[cfg(feature="producer-test-fixture")] super::faults::barrier("before_post");
        match client.submit(&current,mode,&entry).await {
            Attempt::Unauthorized => {
                if failed.as_ref()==Some(&current) || posts==2 {break NetworkResult::Unauthorized(current.clone())}
                failed=Some(current.clone());
                current=match EndpointSnapshot::read(&root) {Ok(e)=>e,Err(e)=>break NetworkResult::Error(e)};
            },
            result=>break NetworkResult::Outcome(result),
        }
    };
    if tx.send(NetworkEvent::Done(entry,Some(current),failed,result)).is_ok() {
        #[cfg(test)] if let Some(hook)=hook {let _=hook.done.send(());}
    }
}
fn finish_network(queue:&mut Queue,worker:&mut Worker,entry:Entry,failed:Option<EndpointSnapshot>,result:NetworkResult,now:u64,jitter:u64) -> Result<(), &'static str> {
    worker.failed_post=failed;
    match result {
        NetworkResult::Outcome(Attempt::Committed(v))=>{#[cfg(feature="producer-test-fixture")] super::faults::barrier("before_receipt_commit");queue.save_outcome(entry.id,"committed",&v.to_string())?;#[cfg(feature="producer-test-fixture")] super::faults::barrier("after_receipt_commit");worker.failed_post=None;worker.retries.remove(&entry.session);},
        NetworkResult::Outcome(Attempt::Rejected(v))=>{queue.save_outcome(entry.id,"rejected",&v.to_string())?;worker.failed_post=None;worker.retries.remove(&entry.session);},
        NetworkResult::Unauthorized(endpoint)=>worker.unauthorized=Some(endpoint),
        NetworkResult::Outcome(_) | NetworkResult::Error(_)=>worker.defer(&entry.session,now,jitter),
    }
    Ok(())
}
/// Runs on the thread that owns the queue and its directory lock.
pub fn run(queue:&mut Queue, root:&Path, commands:std::sync::mpsc::Receiver<Command>) -> Result<(), &'static str> {
    run_inner(queue,root,commands,#[cfg(test)] None)
}
fn run_inner(queue:&mut Queue, root:&Path, commands:std::sync::mpsc::Receiver<Command>, #[cfg(test)] hook:Option<Hooks>) -> Result<(), &'static str> {
    let mut worker=Worker::default(); let started=std::time::Instant::now();
    let (events,event_rx)=std::sync::mpsc::channel();
    let mut network:Option<(std::thread::JoinHandle<()>,std::sync::Arc<std::sync::atomic::AtomicBool>)>=None;
    let mut next_poll=std::time::Instant::now();
    let mut waiters:Vec<(i64,std::time::Instant,std::sync::mpsc::Sender<Result<Option<Entry>,&'static str>>)>=Vec::new();
    loop {
        let delay=if network.is_some() || !waiters.is_empty() {std::time::Duration::from_millis(25)} else {next_poll.saturating_duration_since(std::time::Instant::now())};
        match commands.recv_timeout(delay) {
            Ok(Command::AdmitMcp {session,operation_id,kind,payload,reply}) => {let _=reply.send(queue.admit_mcp(&session,&operation_id,&kind,&payload));next_poll=std::time::Instant::now();},
            Ok(Command::AdmitBatch {batch_id,bytes,reply}) => {let _=reply.send(queue.admit_batch(&batch_id,&bytes));next_poll=std::time::Instant::now();},
            Ok(Command::Get {id,reply}) => {let _=reply.send(queue.get_for_caller(id));},
            Ok(Command::Watch {id,deadline,reply}) => {
                let current=queue.get_for_caller(id);
                waiters.retain(|(_,until,_)| *until > std::time::Instant::now());
                if matches!(&current,Ok(Some(entry)) if entry.status=="pending") && deadline > std::time::Instant::now() {
                    if waiters.len()==64 {let _=reply.send(Err("busy"));} else {waiters.push((id,deadline,reply));}
                } else {let _=reply.send(current);}
            },
            Ok(Command::Wake) => {next_poll=std::time::Instant::now();},
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {},
            #[cfg(test)] Ok(Command::Pause {ready,resume}) => {let _=ready.send(());let _=resume.recv();},
            Ok(Command::Shutdown) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                if let Some((thread,cancel))=network.take() {cancel.store(true,std::sync::atomic::Ordering::Relaxed);let _=thread.join();}
                while let Ok(event)=event_rx.try_recv() {if let NetworkEvent::Done(entry,_,failed,result)=event {finish_network(queue,&mut worker,entry,failed,result,0,0)?;}}
                return Ok(())
            },
        }
        let mut random=[0u8;8]; let _=getrandom::fill(&mut random); let jitter=u64::from_le_bytes(random);
        let now=started.elapsed().as_millis().min(u64::MAX as u128) as u64;
        while let Ok(event)=event_rx.try_recv() {match event {
            NetworkEvent::Bind(store,reply)=>{let _=reply.send(queue.bind_if_unset(&store));},
            NetworkEvent::Done(entry,_endpoint,failed,result)=>{
                if let Some((thread,_))=network.take() {let _=thread.join();}
                finish_network(queue,&mut worker,entry,failed,result,now,jitter)?;next_poll=std::time::Instant::now();
            },
        }}
        if network.is_none() && std::time::Instant::now()>=next_poll {
            next_poll=std::time::Instant::now()+std::time::Duration::from_secs(1);
            let discovery=EndpointSnapshot::read(root);
            if let Ok(endpoint)=&discovery {if worker.observed.as_ref()!=Some(endpoint) {worker.retries.clear();worker.observed=Some(endpoint.clone());}}
            let heads=queue.session_heads()?; if let Some(index)=worker.choose(&heads,now) {
                let entry=heads[index].clone(); worker.last=Some(entry.session.clone());
                match discovery {
                    Ok(endpoint) if worker.unauthorized.as_ref()!=Some(&endpoint)=>{
                        let root=root.to_owned(); let events=events.clone(); let mode=queue.mode(); let failed=worker.failed_post.clone();
                        let cancel=std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)); let stop=cancel.clone();
                        #[cfg(test)] let hook=hook.clone();
                        let thread=std::thread::spawn(move || {
                            let Ok(rt)=tokio::runtime::Builder::new_current_thread().enable_all().build() else {let _=events.send(NetworkEvent::Done(entry,None,failed,NetworkResult::Error("worker_unavailable")));return};
                            let failure_entry=entry.clone();let failure_failed=failed.clone();let failure_events=events.clone();
                            let mut task=rt.spawn(network_round(root,entry,mode,endpoint,failed,events,#[cfg(test)] hook));
                            loop {if stop.load(std::sync::atomic::Ordering::Relaxed) {task.abort();break} match rt.block_on(async {tokio::time::timeout(std::time::Duration::from_millis(20),&mut task).await}) {Ok(Ok(()))=>break,Ok(Err(_))=>{let _=failure_events.send(NetworkEvent::Done(failure_entry,None,failure_failed,NetworkResult::Error("worker_unavailable")));break},Err(_)=>{}}}
                        });
                        network=Some((thread,cancel));
                    },
                    Ok(_)=>{},Err(_)=>worker.defer(&entry.session,now,jitter),
                }
            }
        }
        waiters.retain(|(id,deadline,reply)| { if *deadline<=std::time::Instant::now() {return false} let current=queue.get_for_caller(*id); if matches!(&current,Ok(Some(entry)) if entry.status=="pending") {true} else {let _=reply.send(current);false} });
    }
}
impl Worker {
    fn choose(&self, heads:&[Entry], now:u64) -> Option<usize> {
        if heads.is_empty() { return None }
        let start=self.last.as_ref().and_then(|s|heads.iter().position(|e|&e.session==s)).map_or(0,|i|(i+1)%heads.len());
        (0..heads.len()).map(|i|(start+i)%heads.len()).find(|&i|heads[i].status=="pending" && self.retries.get(&heads[i].session).is_none_or(|r|r.due<=now))
    }
    fn defer(&mut self, session:&str, now:u64, jitter:u64) {
        let retry=self.retries.entry(session.to_owned()).or_default(); let base=250u64.saturating_mul(1u64 << retry.attempts.min(7)).min(30_000);
        retry.attempts=retry.attempts.saturating_add(1); retry.due=now.saturating_add(base.saturating_add(jitter%(base/4+1)).min(30_000));
    }
    pub async fn tick(&mut self, queue:&mut Queue, root:&Path, client:&GatewayClient, now_ms:u64, jitter:u64)->Tick {
        let discovery=EndpointSnapshot::read(root);
        if let Ok(endpoint)=&discovery { if self.observed.as_ref()!=Some(endpoint) { self.retries.clear(); self.observed=Some(endpoint.clone()); } }
        let heads=match queue.session_heads() {Ok(v)=>v,Err(e)=>return Tick::Error(e)};
        let Some(index)=self.choose(&heads,now_ms) else {return if heads.is_empty(){Tick::Idle}else{Tick::Deferred}};
        let entry=&heads[index]; self.last=Some(entry.session.clone());
        let endpoint=match discovery {Ok(v)=>v,Err(e)=>{self.defer(&entry.session,now_ms,jitter);return Tick::Error(e)}};
        if self.unauthorized.as_ref()==Some(&endpoint) {return Tick::Unauthorized}
        if let Err(e)=client.health(&endpoint).await {if e=="gateway_unauthorized" && self.failed_post.as_ref()==Some(&endpoint) {self.unauthorized=Some(endpoint);return Tick::Unauthorized} self.defer(&entry.session,now_ms,jitter);return Tick::Error(e)}
        if let Err(e)=queue.bind_if_unset(endpoint.store_id()) {return Tick::Error(e)}
        self.unauthorized=None;
        let mut result=client.submit(&endpoint,queue.mode(),entry).await;
        let mut last_endpoint=endpoint;
        if matches!(result,Attempt::Unauthorized) {
            if self.failed_post.as_ref()==Some(&last_endpoint) {self.unauthorized=Some(last_endpoint);return Tick::Unauthorized}
            self.failed_post=Some(last_endpoint.clone());
            let fresh=match EndpointSnapshot::read(root) {Ok(v)=>v,Err(e)=>{self.defer(&entry.session,now_ms,jitter);return Tick::Error(e)}};
            if let Err(e)=client.health(&fresh).await {if e=="gateway_unauthorized" && fresh==last_endpoint {self.unauthorized=Some(fresh);return Tick::Unauthorized} self.defer(&entry.session,now_ms,jitter);return Tick::Error(e)}
            if let Err(e)=queue.bind_if_unset(fresh.store_id()) {return Tick::Error(e)}
            result=client.submit(&fresh,queue.mode(),entry).await; last_endpoint=fresh;
        }
        match result {
            Attempt::Committed(value) => match queue.save_outcome(entry.id,"committed",&value.to_string()) {Ok(())=>{self.failed_post=None;self.retries.remove(&entry.session);Tick::Committed},Err(e)=>Tick::Error(e)},
            Attempt::Rejected(value) => match queue.save_outcome(entry.id,"rejected",&value.to_string()) {Ok(())=>{self.failed_post=None;self.retries.remove(&entry.session);Tick::Rejected},Err(e)=>Tick::Error(e)},
            Attempt::Unauthorized => {self.unauthorized=Some(last_endpoint);Tick::Unauthorized},
            Attempt::Retry => {self.defer(&entry.session,now_ms,jitter);Tick::Deferred}
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read,Write};
    use super::super::queue::{with_queue,QueueMode};
    fn entry(id:i64,session:&str,status:&str)->Entry {Entry{id,session:session.into(),request:vec![],status:status.into(),outcome:None}}
    #[test] fn fair_rotation_skips_backoff_and_rejected_heads() {
        let heads=vec![entry(1,"a","pending"),entry(2,"b","pending"),entry(3,"c","rejected")];
        let mut worker=Worker::default(); assert_eq!(worker.choose(&heads,0),Some(0)); worker.last=Some("a".into()); worker.defer("a",0,0); assert_eq!(worker.choose(&heads,0),Some(1)); worker.last=Some("b".into()); assert_eq!(worker.choose(&heads,0),Some(1)); assert_eq!(worker.choose(&heads,250),Some(0));
        for _ in 0..20 {worker.defer("a",1000,u64::MAX);} assert!(worker.retries["a"].due<=31_000);
    }
    #[test] fn command_channel_is_bounded() {
        let (sender,_receiver)=channel(); for _ in 0..64 { assert!(sender.try_send(Command::Wake).is_ok()); }
        assert!(matches!(sender.try_send(Command::Wake),Err(std::sync::mpsc::TrySendError::Full(_))));
    }
    #[test] fn running_worker_returns_storage_error_instead_of_silent_idle() {
        let root=fixture();let (sender,commands)=channel();sender.send(Command::Wake).unwrap();
        let result=with_queue(&root,QueueMode::Mcp,"sample",|q| {q.inject_storage_failure();run(q,&root,commands)});
        assert_eq!(result,Err("queue_corrupt".to_owned()));
    }
    #[test] fn shutdown_persists_already_received_success_and_rejection() {
        use std::time::Duration;
        for (status,body,expected) in [(200,receipt("op"),"committed"),(409,r#"{"error":{"code":"task_closed","message":"closed","index":0,"last_seq":1}}"#.into(),"rejected")] {
            let root=fixture();let listener=std::net::TcpListener::bind("127.0.0.1:0").unwrap();let port=listener.local_addr().unwrap().port();
            let (post,seen)=std::sync::mpsc::channel();let (release,gate)=std::sync::mpsc::channel();
            let server=std::thread::spawn(move || {for (code,response) in [(200,health()),(status,body)] {let (mut s,_)=listener.accept().unwrap();s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();let mut head=Vec::new();let mut b=[0u8];while !head.ends_with(b"\r\n\r\n") {s.read_exact(&mut b).unwrap();head.push(b[0]);}let head=String::from_utf8(head).unwrap();let len=head.lines().find_map(|l|l.to_ascii_lowercase().strip_prefix("content-length: ").and_then(|v|v.parse::<usize>().ok())).unwrap_or(0);let mut input=vec![0;len];s.read_exact(&mut input).unwrap();if code!=200 || !input.is_empty() {post.send(()).unwrap();gate.recv_timeout(Duration::from_secs(5)).unwrap();}write!(s,"HTTP/1.1 {code} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",response.len()).unwrap();}});
            let other=root.clone();let (sender,commands)=channel();let (ready,started)=std::sync::mpsc::channel();let (done,done_rx)=std::sync::mpsc::channel();
            let hook=Hooks{done,panic_once:std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false))};
            let thread=std::thread::spawn(move || with_queue(&other,QueueMode::Mcp,"sample",|q| {publish(&other,port,&"a".repeat(64));let id=q.admit_mcp("s","op","task.start",&serde_json::json!({"title":"T","goal":"G","project":"P"}))?;ready.send(id).unwrap();run_inner(q,&other,commands,Some(hook))}));
            let id=started.recv_timeout(Duration::from_secs(5)).unwrap();seen.recv_timeout(Duration::from_secs(5)).unwrap();
            let (paused,barrier)=std::sync::mpsc::channel();let (resume,wait)=std::sync::mpsc::channel();sender.send(Command::Pause{ready:paused,resume:wait}).unwrap();barrier.recv_timeout(Duration::from_secs(5)).unwrap();sender.send(Command::Shutdown).unwrap();release.send(()).unwrap();done_rx.recv_timeout(Duration::from_secs(5)).unwrap();resume.send(()).unwrap();
            assert!(thread.join().unwrap().is_ok());server.join().unwrap();with_queue(&root,QueueMode::Mcp,"sample",|q| {let entry=q.get(id)?.unwrap();assert_eq!(entry.status,expected);let actual:serde_json::Value=serde_json::from_str(entry.outcome.as_deref().unwrap()).unwrap();let wanted=if expected=="committed" {serde_json::from_str(&receipt("op")).unwrap()} else {serde_json::json!({"code":"task_closed","index":0,"last_seq":1})};assert_eq!(actual,wanted);Ok(())}).unwrap();
        }
    }
    #[test] fn network_task_panic_releases_slot_and_retries() {
        use std::time::Duration;
        let root=fixture();let (port,server)=fake_server(vec![(200,health()),(200,receipt("op"))]);let other=root.clone();let (sender,commands)=channel();let (ready,started)=std::sync::mpsc::channel();let (done,done_rx)=std::sync::mpsc::channel();
        let hook=Hooks{done,panic_once:std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true))};
        let thread=std::thread::spawn(move || with_queue(&other,QueueMode::Mcp,"sample",|q| {publish(&other,port,&"a".repeat(64));let id=q.admit_mcp("s","op","task.start",&serde_json::json!({"title":"T","goal":"G","project":"P"}))?;ready.send(id).unwrap();run_inner(q,&other,commands,Some(hook))}));
        let id=started.recv_timeout(Duration::from_secs(5)).unwrap();done_rx.recv_timeout(Duration::from_secs(8)).unwrap();let (reply,rx)=std::sync::mpsc::channel();sender.send(Command::Watch{id,deadline:std::time::Instant::now()+Duration::from_secs(3),reply}).unwrap();assert_eq!(rx.recv_timeout(Duration::from_secs(3)).unwrap().unwrap().unwrap().status,"committed");sender.send(Command::Shutdown).unwrap();assert!(thread.join().unwrap().is_ok());assert_eq!(server.join().unwrap(),2);
    }
    #[test] fn expired_watchers_release_capacity_for_same_pending_entry() {
        use std::time::{Duration,Instant};
        let root=fixture(); let other=root.clone(); let (sender,commands)=channel(); let (ready,started)=std::sync::mpsc::channel();
        let thread=std::thread::spawn(move || with_queue(&other,QueueMode::Mcp,"sample",|q| {let id=q.admit_mcp("s","op","task.start",&serde_json::json!({"title":"T","goal":"G","project":"P"}))?;ready.send(id).unwrap();run(q,&other,commands)}));
        let id=started.recv_timeout(Duration::from_secs(5)).unwrap();
        let deadline=Instant::now()+Duration::from_secs(2);
        for _ in 0..64 {let (reply,rx)=std::sync::mpsc::channel();sender.send(Command::Watch{id,deadline,reply}).unwrap();drop(rx);}
        let (reply,rx)=std::sync::mpsc::channel();sender.send(Command::Get{id,reply}).unwrap();rx.recv_timeout(Duration::from_secs(2)).unwrap().unwrap();
        let (reply,overflow)=std::sync::mpsc::channel();sender.send(Command::Watch{id,deadline,reply}).unwrap();assert_eq!(overflow.recv_timeout(Duration::from_secs(1)).unwrap().err(),Some("busy"));
        while Instant::now()<=deadline {std::thread::sleep(Duration::from_millis(10));}
        let (reply,watch)=std::sync::mpsc::channel();sender.send(Command::Watch{id,deadline:Instant::now()+Duration::from_secs(5),reply}).unwrap();
        let (reply,rx)=std::sync::mpsc::channel();sender.send(Command::Get{id,reply}).unwrap();assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap().unwrap().unwrap().status,"pending");
        assert!(matches!(watch.try_recv(),Err(std::sync::mpsc::TryRecvError::Empty)));sender.send(Command::Shutdown).unwrap();assert!(thread.join().unwrap().is_ok());
    }
    #[test] fn stalled_health_does_not_block_local_commands_or_shutdown() {
        use std::time::Duration;
        let root=fixture(); let listener=std::net::TcpListener::bind("127.0.0.1:0").unwrap(); let port=listener.local_addr().unwrap().port();
        let (connected,barrier)=std::sync::mpsc::channel(); let server=std::thread::spawn(move || {let (mut stream,_)=listener.accept().unwrap();stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();let mut data=[0u8;4096];let _=stream.read(&mut data).unwrap();connected.send(()).unwrap();loop {match stream.read(&mut data) {Ok(0)=>break,Ok(_)=>{},Err(e)=>panic!("connection did not close: {e}")}}});
        let other=root.clone(); let (sender,commands)=channel(); let (ready,started)=std::sync::mpsc::channel();
        let thread=std::thread::spawn(move || with_queue(&other,QueueMode::Mcp,"sample",|q| {let payload=serde_json::json!({"title":"T","goal":"G","project":"P"});let id=q.admit_mcp("done","old","task.start",&payload)?;q.save_outcome(id,"committed",&receipt("old"))?;q.admit_mcp("pending","new","task.start",&payload)?;publish(&other,port,&"a".repeat(64));ready.send(id).unwrap();run(q,&other,commands)}));
        let id=started.recv_timeout(Duration::from_secs(5)).unwrap();barrier.recv_timeout(Duration::from_secs(5)).unwrap();
        let (reply,rx)=std::sync::mpsc::channel();sender.send(Command::Get{id,reply}).unwrap();assert_eq!(rx.recv_timeout(Duration::from_millis(700)).unwrap().unwrap().unwrap().status,"committed");
        let (reply,rx)=std::sync::mpsc::channel();sender.send(Command::AdmitMcp{session:"other".into(),operation_id:"local".into(),kind:"task.start".into(),payload:serde_json::json!({"title":"T","goal":"G","project":"P"}),reply}).unwrap();rx.recv_timeout(Duration::from_millis(700)).unwrap().unwrap();
        sender.send(Command::Shutdown).unwrap();let (done,wait)=std::sync::mpsc::channel();std::thread::spawn(move || {done.send(thread.join()).unwrap();});assert!(wait.recv_timeout(Duration::from_millis(700)).unwrap().unwrap().is_ok());server.join().unwrap();
    }
    #[test] fn post_401_followed_by_health_401_pauses_same_snapshot() {
        let root=fixture();let (port,server)=fake_server(vec![(200,health()),(401,"{}".into()),(401,"{}".into()),(200,health()),(200,receipt("op"))]);
        let rt=tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        with_queue(&root,QueueMode::Mcp,"sample",|q| {publish(&root,port,&"a".repeat(64));q.admit_mcp("s","op","task.start",&serde_json::json!({"title":"T","goal":"G","project":"P"}))?;let mut worker=Worker::default();let client=GatewayClient::new(std::time::Duration::from_secs(3));rt.block_on(async {assert_eq!(worker.tick(q,&root,&client,0,0).await,Tick::Unauthorized);assert_eq!(worker.tick(q,&root,&client,1000,0).await,Tick::Unauthorized);publish(&root,port,&"c".repeat(64));assert_eq!(worker.tick(q,&root,&client,2000,0).await,Tick::Committed);});Ok(())}).unwrap();assert_eq!(server.join().unwrap(),5);
    }
    #[test] fn background_pauses_mixed_401_and_resumes_changed_credentials() {
        use std::time::Duration;
        let root=fixture();let (third,barrier)=std::sync::mpsc::channel();let (port,server)=fake_server_notify(vec![(200,health()),(401,"{}".into()),(401,"{}".into()),(200,health()),(200,receipt("op"))],Some(third));
        let other=root.clone();let (sender,commands)=channel();let (ready,started)=std::sync::mpsc::channel();
        let thread=std::thread::spawn(move || with_queue(&other,QueueMode::Mcp,"sample",|q| {publish(&other,port,&"a".repeat(64));let id=q.admit_mcp("s","op","task.start",&serde_json::json!({"title":"T","goal":"G","project":"P"}))?;ready.send(id).unwrap();run(q,&other,commands)}));
        let id=started.recv_timeout(Duration::from_secs(5)).unwrap();barrier.recv_timeout(Duration::from_secs(5)).unwrap();std::thread::sleep(Duration::from_millis(800));
        let (reply,rx)=std::sync::mpsc::channel();sender.send(Command::Get{id,reply}).unwrap();assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap().unwrap().unwrap().status,"pending");
        publish(&root,port,&"c".repeat(64));let (reply,watch)=std::sync::mpsc::channel();sender.send(Command::Watch{id,deadline:std::time::Instant::now()+Duration::from_secs(5),reply}).unwrap();
        assert_eq!(watch.recv_timeout(Duration::from_secs(5)).unwrap().unwrap().unwrap().status,"committed");sender.send(Command::Shutdown).unwrap();assert!(thread.join().unwrap().is_ok());assert_eq!(server.join().unwrap(),5);
    }
    fn fixture() -> std::path::PathBuf {
        let mut random=[0u8;16]; getrandom::fill(&mut random).unwrap(); let suffix:String=random.iter().map(|b|format!("{b:02x}")).collect();
        let base=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailWorker-{suffix}")); std::fs::create_dir(&base).unwrap(); base.join("state")
    }
    fn fake_server(replies:Vec<(u16,String)>) -> (u16,std::thread::JoinHandle<usize>) {
        fake_server_notify(replies,None)
    }
    fn fake_server_notify(replies:Vec<(u16,String)>, third:Option<std::sync::mpsc::Sender<()>>) -> (u16,std::thread::JoinHandle<usize>) {
        let listener=std::net::TcpListener::bind("127.0.0.1:0").unwrap(); listener.set_nonblocking(true).unwrap(); let port=listener.local_addr().unwrap().port();
        let worker=std::thread::spawn(move || { let mut count=0; for (status,body) in replies { let deadline=std::time::Instant::now()+std::time::Duration::from_secs(15); let mut s=loop { match listener.accept() { Ok((s,_))=>break s, Err(e) if e.kind()==std::io::ErrorKind::WouldBlock && std::time::Instant::now()<deadline=>std::thread::yield_now(), Err(e)=>panic!("fake gateway: {e}") } }; s.set_nonblocking(false).unwrap(); s.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap(); let mut head=Vec::new(); let mut byte=[0]; while !head.ends_with(b"\r\n\r\n") { s.read_exact(&mut byte).unwrap(); head.push(byte[0]); } let head=String::from_utf8(head).unwrap(); let len=head.lines().find_map(|l|l.to_ascii_lowercase().strip_prefix("content-length: ").map(str::to_owned)).and_then(|s|s.parse::<usize>().ok()).unwrap_or(0); let mut body_input=vec![0;len]; s.read_exact(&mut body_input).unwrap(); write!(s,"HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap(); count+=1; if count==3 {if let Some(tx)=&third {let _=tx.send(());}} } count }); (port,worker)
    }
    fn publish(root:&Path,port:u16,token:&str) { let value=serde_json::json!({"port":port,"token":token,"version":1,"store_id":"b".repeat(64)}); std::fs::write(root.join("endpoint.json"),serde_json::to_vec(&value).unwrap()).unwrap(); }
    fn health() -> String { serde_json::json!({"version":1,"store_id":"b".repeat(64)}).to_string() }
    fn receipt(op:&str)->String { serde_json::json!({"accepted":1,"duplicates":0,"last_seq":1,"results":[{"operation_id":op,"seq":1}]}).to_string() }
    #[test] fn retries_503_fairly_and_preserves_rejected_head() {
        let root=fixture(); let (port,server)=fake_server(vec![(200,health()),(503,"{}".into()),(200,health()),(200,receipt("b1")),(200,health()),(409,r#"{"error":{"code":"task_closed","message":"closed","index":0,"last_seq":1}}"#.into())]);
        let rt=tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let (a,b)=with_queue(&root,QueueMode::Mcp,"sample",|q| {
            publish(&root,port,&"a".repeat(64)); let payload=serde_json::json!({"title":"T","goal":"G","project":"P"});
            let a=q.admit_mcp("a","a1","task.start",&payload)?; q.admit_mcp("a","a2","task.start",&payload)?; let b=q.admit_mcp("b","b1","task.start",&payload)?;
            let mut worker=Worker::default(); let client=GatewayClient::new(std::time::Duration::from_secs(3));
            rt.block_on(async {assert_eq!(worker.tick(q,&root,&client,0,0).await,Tick::Deferred); assert_eq!(worker.tick(q,&root,&client,0,0).await,Tick::Committed); assert_eq!(worker.tick(q,&root,&client,250,0).await,Tick::Rejected);}); Ok((a,b))
        }).unwrap();
        with_queue(&root,QueueMode::Mcp,"sample",|q| {assert_eq!(q.get(a)?.unwrap().status,"rejected"); assert_eq!(q.get(b)?.unwrap().status,"committed"); assert_eq!(q.session_heads()?.len(),1); Ok(())}).unwrap(); assert_eq!(server.join().unwrap(),6);
    }
    #[test] fn unauthorized_pauses_until_credentials_change() {
        let root=fixture(); let (port,server)=fake_server(vec![(200,health()),(401,"{}".into()),(200,health()),(401,"{}".into()),(200,health()),(200,receipt("op"))]);
        let rt=tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        with_queue(&root,QueueMode::Mcp,"sample",|q| { publish(&root,port,&"a".repeat(64)); q.admit_mcp("s","op","task.start",&serde_json::json!({"title":"T","goal":"G","project":"P"}))?; let mut worker=Worker::default(); let client=GatewayClient::new(std::time::Duration::from_secs(3));
            rt.block_on(async {assert_eq!(worker.tick(q,&root,&client,0,0).await,Tick::Unauthorized); assert_eq!(worker.tick(q,&root,&client,1000,0).await,Tick::Unauthorized); publish(&root,port,&"c".repeat(64)); assert_eq!(worker.tick(q,&root,&client,2000,0).await,Tick::Committed);}); Ok(()) }).unwrap();
        assert_eq!(server.join().unwrap(),6);
    }
    #[test] fn real_gateway_restarts_same_store_and_rejects_other_store() {
        let root=fixture(); let mut gateway=Some(crate::gateway::lifecycle::Gateway::start(&root).unwrap());
        let rt=tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        with_queue(&root,QueueMode::Mcp,"sample",|q| {
            let payload=serde_json::json!({"title":"T","goal":"G","project":"P"});
            let first=q.admit_mcp("a","one","task.start",&payload)?;
            let mut worker=Worker::default(); let client=GatewayClient::new(std::time::Duration::from_secs(3));
            assert_eq!(rt.block_on(worker.tick(q,&root,&client,0,0)),Tick::Committed); assert_eq!(q.get(first)?.unwrap().status,"committed");
            let second=q.admit_mcp("b","two","task.start",&payload)?;
            gateway.take().unwrap().shutdown().unwrap();
            let other_root=root.parent().unwrap().join("other"); let other=crate::gateway::lifecycle::Gateway::start(&other_root).unwrap();
            let replacement=std::fs::read(other_root.join("endpoint.json")).unwrap(); std::fs::write(root.join("endpoint.json"),replacement).unwrap();
            assert_eq!(rt.block_on(worker.tick(q,&root,&client,1000,0)),Tick::Error("store_changed")); assert_eq!(q.get(second)?.unwrap().status,"pending");
            other.shutdown().unwrap(); gateway=Some(crate::gateway::lifecycle::Gateway::start(&root).unwrap());
            assert_eq!(rt.block_on(worker.tick(q,&root,&client,2000,0)),Tick::Committed); assert_eq!(q.get(second)?.unwrap().status,"committed"); Ok(())
        }).unwrap();
        gateway.take().unwrap().shutdown().unwrap(); eprintln!("isolated worker fixture retained: {}",root.parent().unwrap().display());
    }
    #[test] fn worker_polls_after_offline_admission_and_exits_on_disconnect() {
        let root=fixture(); let thread_root=root.clone(); let (sender,commands)=channel(); let (ready_tx,ready_rx)=std::sync::mpsc::channel();
        let thread=std::thread::spawn(move || with_queue(&thread_root,QueueMode::Mcp,"sample",|q| {ready_tx.send(()).unwrap(); run(q,&thread_root,commands)}));
        ready_rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
        let (reply,answer)=std::sync::mpsc::channel(); assert!(sender.try_send(Command::AdmitMcp {session:"s".into(),operation_id:"op".into(),kind:"task.start".into(),payload:serde_json::json!({"title":"T","goal":"G","project":"P"}),reply}).is_ok());
        let id=answer.recv_timeout(std::time::Duration::from_secs(5)).unwrap().unwrap();
        let (watch,updated)=std::sync::mpsc::channel(); assert!(sender.try_send(Command::Watch{id,deadline:std::time::Instant::now()+std::time::Duration::from_secs(15),reply:watch}).is_ok());
        let (reply,answer)=std::sync::mpsc::channel(); assert!(sender.try_send(Command::Get{id,reply}).is_ok()); assert_eq!(answer.recv_timeout(std::time::Duration::from_secs(5)).unwrap().unwrap().unwrap().status,"pending");
        let (port,server)=fake_server(vec![(200,health()),(200,receipt("op"))]); publish(&root,port,&"a".repeat(64));
        let (done_tx,done_rx)=std::sync::mpsc::channel(); std::thread::spawn(move || {let _=done_tx.send(server.join());});
        assert_eq!(done_rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap().unwrap(),2);
        assert_eq!(updated.recv_timeout(std::time::Duration::from_secs(5)).unwrap().unwrap().unwrap().status,"committed");
        let (reply,answer)=std::sync::mpsc::channel(); assert!(sender.try_send(Command::Get{id,reply}).is_ok()); assert_eq!(answer.recv_timeout(std::time::Duration::from_secs(5)).unwrap().unwrap().unwrap().status,"committed");
        drop(sender); assert!(thread.join().unwrap().is_ok());
    }
    #[test] fn discovery_change_resets_backoff_before_deadline() {
        let root=fixture(); let (port,server)=fake_server(vec![(200,health()),(200,receipt("op"))]);
        let rt=tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        with_queue(&root,QueueMode::Mcp,"sample",|q| {
            q.admit_mcp("s","op","task.start",&serde_json::json!({"title":"T","goal":"G","project":"P"}))?;
            let mut worker=Worker::default(); let client=GatewayClient::new(std::time::Duration::from_secs(3));
            assert_eq!(rt.block_on(worker.tick(q,&root,&client,0,0)),Tick::Error("discovery_missing"));
            publish(&root,port,&"a".repeat(64));
            assert_eq!(rt.block_on(worker.tick(q,&root,&client,1,0)),Tick::Committed); Ok(())
        }).unwrap(); assert_eq!(server.join().unwrap(),2);
    }
}



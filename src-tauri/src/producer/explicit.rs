use std::{io::Read,path::Path,time::Duration};
use serde_json::{json,Value};
use super::{gateway::{Attempt,EndpointSnapshot,GatewayClient},queue::{Queue,QueueMode,Entry}};

#[derive(Default)] struct AuthState {failed:Option<EndpointSnapshot>,paused:Option<EndpointSnapshot>}
async fn submit(queue:&mut Queue,root:&Path,client:&GatewayClient,entry:&Entry,auth:&mut AuthState)->Result<Attempt,&'static str> {
    let mut endpoint=EndpointSnapshot::read(root)?;if auth.paused.as_ref()==Some(&endpoint){return Err("gateway_unauthorized")}
    if let Err(e)=client.health(&endpoint).await {if e=="gateway_unauthorized" && auth.failed.as_ref()==Some(&endpoint) {auth.paused=Some(endpoint);return Err("gateway_unauthorized")}return Err(e)}queue.bind_if_unset(endpoint.store_id())?;
    #[cfg(feature="producer-test-fixture")] super::faults::barrier("before_post");
    let mut result=client.submit(&endpoint,QueueMode::Explicit,entry).await;
    if matches!(result,Attempt::Unauthorized) {
        if auth.failed.as_ref()==Some(&endpoint) {auth.paused=Some(endpoint);return Err("gateway_unauthorized")}
        auth.failed=Some(endpoint.clone());endpoint=EndpointSnapshot::read(root)?;
        if let Err(e)=client.health(&endpoint).await {if e=="gateway_unauthorized" && auth.failed.as_ref()==Some(&endpoint){auth.paused=Some(endpoint);return Err("gateway_unauthorized")}return Err(e)}
        queue.bind_if_unset(endpoint.store_id())?;result=client.submit(&endpoint,QueueMode::Explicit,entry).await;
        if matches!(result,Attempt::Unauthorized) {auth.paused=Some(endpoint);return Err("gateway_unauthorized")}
    }
    if matches!(result,Attempt::Committed(_)|Attempt::Rejected(_)){auth.failed=None;}
    Ok(result)
}
async fn drive(queue:&mut Queue,root:&Path)->Result<(), &'static str> {
    let client=GatewayClient::new(Duration::from_secs(2));let mut auth=AuthState::default();let mut deferred=std::collections::HashSet::new();let mut gaps=std::collections::HashSet::new();let mut last:Option<String>=None;
    loop {
        let heads=queue.session_heads()?;let start=last.as_ref().and_then(|s|heads.iter().position(|e|&e.session==s)).map_or(0,|i|(i+1)%heads.len());let Some(index)=(0..heads.len()).map(|i|(start+i)%heads.len()).find(|&i|!deferred.contains(&heads[i].session)) else {return Ok(())};let head=heads[index].clone();
        if head.status=="rejected" {if !gaps.insert((head.id,head.outcome.clone())) {deferred.insert(head.session);continue}if queue.prepare_gap(head.id)? {continue}deferred.insert(head.session);continue}
        let repair=match queue.next_repair(&head.session) {Ok(v)=>v,Err("session_blocked")=>{deferred.insert(head.session);continue},Err(e)=>return Err(e)};
        let entry=repair.as_ref().map(|(_,entry)|entry).unwrap_or(&head);
        last=Some(head.session.clone());
        match submit(queue,root,&client,entry,&mut auth).await {
            Ok(Attempt::Committed(value))=>if let Some((id,_))=repair {queue.finish_repair(id,"committed")?;#[cfg(feature="producer-test-fixture")] super::faults::barrier("after_repair_commit");} else {#[cfg(feature="producer-test-fixture")] super::faults::barrier("before_receipt_commit");queue.save_outcome(head.id,"committed",&value.to_string())?;#[cfg(feature="producer-test-fixture")] super::faults::barrier("after_receipt_commit");},
            Ok(Attempt::Rejected(value))=>if let Some((id,_))=repair {queue.finish_repair(id,"rejected")?;deferred.insert(head.session);} else {queue.save_outcome(head.id,"rejected",&value.to_string())?;if value["code"]!="seq_gap" {deferred.insert(head.session);}},
            Ok(Attempt::Unauthorized)|Err("gateway_unauthorized")=>return Ok(()),
            _=>{deferred.insert(head.session);},
        }
    }
}
fn parse(args:&[String])->Result<(&str,Option<&str>),&'static str> {
    let mut queue=None;let mut batch=None;let mut i=0;while i<args.len(){let flag=args[i].as_str();let value=args.get(i+1).ok_or("invalid_arguments")?;match flag {"--queue" if queue.is_none()=>queue=Some(value.as_str()),"--batch-id" if batch.is_none()=>batch=Some(value.as_str()),_=>return Err("invalid_arguments")}i+=2;}
    let queue=queue.filter(|s|super::queue::valid_queue_name(s)).ok_or("invalid_arguments")?;if batch.is_some_and(|s|s.trim().is_empty()){return Err("invalid_arguments")}Ok((queue,batch))
}
fn static_error(code:&str)->Value {json!({"error":{"code":code,"message":"The operation could not be completed."}})}
pub fn entry(command:&str,args:&[String])->Result<(), &'static str> {
    let result=(||->Result<Value,String> {
        let (name,batch)=parse(args).map_err(str::to_owned)?;
        if (command=="submit-events" && batch.is_none()) || (command=="flush" && batch.is_some()) {return Err("invalid_arguments".into())}
        let bytes=if command=="submit-events" {let mut data=Vec::new();std::io::stdin().take((crate::protocol::MAX_BODY_BYTES+1) as u64).read_to_end(&mut data).map_err(|_|"invalid_request")?;if data.len()>crate::protocol::MAX_BODY_BYTES{return Err("payload_too_large".into())}Some(data)}else{None};
        let root=super::state_root().map_err(str::to_owned)?;
        super::queue::with_queue(&root,QueueMode::Explicit,name,|queue| {
            let id=match &bytes {Some(bytes)=>Some(queue.admit_batch(batch.unwrap(),bytes)?),None=>None};
            let rt=tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|_|"worker_unavailable")?;
            match rt.block_on(async {tokio::time::timeout(Duration::from_secs(2),drive(queue,&root)).await}) {Ok(Err(e))=>return Err(e),_=>{}}
            if let Some(id)=id {let entry=queue.get(id)?.ok_or("queue_corrupt")?;
                if entry.status=="rejected" {let mut err:Value=serde_json::from_str(entry.outcome.as_deref().ok_or("queue_corrupt")?).map_err(|_|"queue_corrupt")?;err["message"]=json!("The gateway rejected this operation.");return Ok(json!({"error":err}))}
                if entry.status=="committed" {let receipt:Value=serde_json::from_str(entry.outcome.as_deref().ok_or("queue_corrupt")?).map_err(|_|"queue_corrupt")?;return Ok(json!({"status":"committed","batch_id":batch.unwrap(),"session":entry.session,"receipt":receipt}))}
                match queue.get_for_caller(id) {Err("session_blocked")=>return Ok(static_error("session_blocked")),Err(e)=>return Err(e),_=>{}}
                match queue.next_repair(&entry.session) {Err("session_blocked")=>return Ok(static_error("session_blocked")),Err(e)=>return Err(e),_=>{}}
                Ok(json!({"status":"queued","batch_id":batch.unwrap(),"session":entry.session}))
            } else {let (committed,pending,blocked)=queue.explicit_counts()?;Ok(json!({"committed":committed,"pending":pending,"blocked":blocked}))}
        })
    })();
    let value=match result {Ok(v)=>v,Err(e)=>static_error(&e)};let failed=value.get("error").is_some();println!("{}",value);if failed {Err("reported_error")}else{Ok(())}
}

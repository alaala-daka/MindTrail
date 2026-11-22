use mindtrail_lib::gateway::store::Store;
use std::path::PathBuf;
fn path() -> PathBuf {
    let mut bytes = [0u8; 16]; getrandom::fill(&mut bytes).unwrap();
    std::env::temp_dir().join(format!("mindtrail-{:?}.sqlite", bytes))
}
#[test]
fn identity_persists_and_is_unique() {
    let a = path(); let b = path();
    let id = Store::open(&a).unwrap().store_id().to_owned();
    assert_eq!(Store::open(&a).unwrap().store_id(), id);
    assert_ne!(Store::open(&b).unwrap().store_id(), id);
    std::fs::remove_file(a).unwrap(); std::fs::remove_file(b).unwrap();
}
#[test]
fn newer_version_is_preserved() {
    let p = path();
    let conn = rusqlite::Connection::open(&p).unwrap();
    conn.execute_batch("CREATE TABLE sentinel (value TEXT); INSERT INTO sentinel VALUES ('kept'); PRAGMA user_version=99;").unwrap(); drop(conn);
    assert!(Store::open(&p).is_err());
    let conn = rusqlite::Connection::open(&p).unwrap();
    let mode: String = conn.pragma_query_value(None, "journal_mode", |r| r.get(0)).unwrap(); assert_eq!(mode.to_lowercase(), "delete");
    let value: String = conn.query_row("SELECT value FROM sentinel", [], |r| r.get(0)).unwrap();
    assert_eq!(value, "kept");
    let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0)).unwrap(); assert_eq!(version, 99);
    drop(conn); std::fs::remove_file(p).unwrap();
}
#[test]
fn failed_migration_rolls_back_all_ddl() {
    let p = path();
    let conn = rusqlite::Connection::open(&p).unwrap();
    conn.execute_batch("CREATE TABLE operations (value TEXT); INSERT INTO operations VALUES ('kept');").unwrap();
    drop(conn);
    assert!(Store::open(&p).is_err());
    let conn = rusqlite::Connection::open(&p).unwrap();
    let count: i64 = conn.query_row("SELECT count(*) FROM sqlite_master WHERE name IN ('store_meta','sessions','events','nodes')", [], |r| r.get(0)).unwrap();
    assert_eq!(count, 0);
    let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
    assert_eq!(version, 0);
    let value: String = conn.query_row("SELECT value FROM operations", [], |r| r.get(0)).unwrap();
    assert_eq!(value, "kept");
    drop(conn); std::fs::remove_file(p).unwrap();
}


#[test]
fn transactional_writes_and_retries() {
    use mindtrail_lib::protocol::{decode_explicit,decode_append};
    let p=path(); let store=Store::open(&p).unwrap();
    let explicit=r#"[{"v":1,"session":"e","seq":1,"ts":"2024-01-01T00:00:00Z","type":"task.start","payload":{"title":"T","goal":"G","project":"P"}},{"v":1,"session":"e","seq":2,"ts":"2024-01-01T00:00:00Z","type":"capability.intent","payload":{"id":"c","title":"C"}},{"v":1,"session":"e","seq":3,"ts":"2024-01-01T00:00:00Z","type":"task.done","payload":{}}]"#;
    let append=r#"[{"v":1,"session":"a","operation_id":"1","ts":"2024-01-01T00:00:00Z","type":"task.start","payload":{"title":"T","goal":"G","project":"P"}},{"v":1,"session":"a","operation_id":"2","ts":"2024-01-01T00:00:00Z","type":"capability.intent","payload":{"id":"c","title":"C"}},{"v":1,"session":"a","operation_id":"3","ts":"2024-01-01T00:00:00Z","type":"task.done","payload":{}}]"#;
    let e=decode_explicit(explicit.as_bytes()).unwrap(); let a=decode_append(append.as_bytes()).unwrap();
    assert_eq!(store.write_explicit(&e).unwrap().accepted,3); assert_eq!(store.write_append(&a).unwrap().accepted,3);
    drop(store); let store=Store::open(&p).unwrap();
    assert_eq!(store.write_explicit(&e).unwrap().duplicates,3); assert_eq!(store.write_append(&a).unwrap().duplicates,3);
    let changed=explicit.replace("2024-01-01T00:00:00Z","2024-01-02T00:00:00Z");
    assert_eq!(store.write_explicit(&decode_explicit(changed.as_bytes()).unwrap()).unwrap_err().code,"seq_conflict");
    let changed=append.replace("2024-01-01T00:00:00Z","2024-01-02T00:00:00Z");
    assert_eq!(store.write_append(&decode_append(changed.as_bytes()).unwrap()).unwrap_err().code,"operation_conflict");
    let cross=append.replace("\"session\":\"a\"","\"session\":\"e\"");
    assert_eq!(store.write_append(&decode_append(cross.as_bytes()).unwrap()).unwrap_err().code,"write_mode_conflict");
    drop(store); std::fs::remove_file(p).unwrap();
}
#[test]
fn invalid_tail_rolls_back_and_gap_is_reported() {
    use mindtrail_lib::protocol::decode_explicit;
    let p=path(); let store=Store::open(&p).unwrap();
    let bad=r#"[{"v":1,"session":"s","seq":1,"ts":"2024-01-01T00:00:00Z","type":"task.start","payload":{"title":"T","goal":"G","project":"P"}},{"v":1,"session":"s","seq":2,"ts":"2024-01-01T00:00:00Z","type":"capability.done","payload":{"id":"missing"}}]"#;
    assert_eq!(store.write_explicit(&decode_explicit(bad.as_bytes()).unwrap()).unwrap_err().code,"invalid_reference");
    let conn=rusqlite::Connection::open(&p).unwrap(); let n:i64=conn.query_row("SELECT count(*) FROM sessions",[],|r|r.get(0)).unwrap(); assert_eq!(n,0); drop(conn);
    let gap=r#"[{"v":1,"session":"s","seq":3,"ts":"2024-01-01T00:00:00Z","type":"task.done","payload":{}}]"#;
    let err=store.write_explicit(&decode_explicit(gap.as_bytes()).unwrap()).unwrap_err(); assert_eq!((err.code,err.expected_seq,err.offending_seq),("seq_gap",Some(1),Some(3)));
    drop(store); std::fs::remove_file(p).unwrap();
}

fn append_json(session: &str, id: &str, kind: &str, payload: &str) -> String {
    format!(r#"{{"v":1,"session":"{session}","operation_id":"{id}","ts":"2024-01-01T00:00:00Z","type":"{kind}","payload":{payload}}}"#)
}
fn append_batch(items: &[String]) -> mindtrail_lib::protocol::AppendBatch {
    mindtrail_lib::protocol::decode_append(format!("[{}]",items.join(",")).as_bytes()).unwrap()
}
#[test]
fn replaced_database_rejects_both_write_modes() {
    let a=path(); let b=path(); let store=Store::open(&a).unwrap(); let other=Store::open(&b).unwrap(); drop(other);
    std::fs::remove_file(&a).unwrap(); std::fs::copy(&b,&a).unwrap(); std::fs::remove_file(&b).unwrap();
    assert_eq!(store.sessions(None,1).unwrap_err().status(),503); assert_eq!(store.events("s",0,1).unwrap_err().status(),503);
    let batch=append_batch(&[append_json("s","1","task.start",r#"{"title":"T","goal":"G","project":"P"}"#)]);
    assert_eq!(store.write_append(&batch).unwrap_err().status(),503);
    let explicit=r#"[{"v":1,"session":"s","seq":1,"ts":"2024-01-01T00:00:00Z","type":"task.start","payload":{"title":"T","goal":"G","project":"P"}}]"#;
    assert_eq!(store.write_explicit(&mindtrail_lib::protocol::decode_explicit(explicit.as_bytes()).unwrap()).unwrap_err().status(),503);
    let c=rusqlite::Connection::open(&a).unwrap(); let n:i64=c.query_row("SELECT count(*) FROM sessions",[],|r|r.get(0)).unwrap(); assert_eq!(n,0); drop(c); drop(store); std::fs::remove_file(a).unwrap();
}
#[test]
fn sql_failure_and_bad_tail_leave_no_partial_writes() {
    let p=path(); let store=Store::open(&p).unwrap();
    let start=append_batch(&[append_json("s","1","task.start",r#"{"title":"T","goal":"G","project":"P"}"#)]); store.write_append(&start).unwrap();
    let good=append_json("s","2","capability.intent",r#"{"id":"c","title":"C"}"#);
    let bad=append_json("s","3","capability.done",r#"{"id":"missing"}"#);
    let err=store.write_append(&append_batch(&[good.clone(),bad])).unwrap_err(); assert_eq!((err.code,err.index,err.last_seq),("invalid_reference",Some(1),Some(1)));
    let c=rusqlite::Connection::open(&p).unwrap(); c.execute_batch("CREATE TRIGGER fail_op BEFORE INSERT ON operations WHEN NEW.operation_id='3' BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    let batch=append_batch(&[good,append_json("s","3","task.done","{}")]);
    assert_eq!(store.write_append(&batch).unwrap_err().status(),503);
    for table in ["events","operations"] {let n:i64=c.query_row(&format!("SELECT count(*) FROM {table}"),[],|r|r.get(0)).unwrap(); assert_eq!(n,1);}
    let n:i64=c.query_row("SELECT count(*) FROM nodes",[],|r|r.get(0)).unwrap(); assert_eq!(n,0);
    let state:(String,i64)=c.query_row("SELECT status,last_seq FROM sessions",[],|r|Ok((r.get(0)?,r.get(1)?))).unwrap(); assert_eq!(state,("running".into(),1));
    c.execute_batch("DROP TRIGGER fail_op").unwrap(); let receipt=store.write_append(&batch).unwrap(); assert_eq!(receipt.last_seq,3); assert_eq!(receipt.results[0].seq,2); assert_eq!(receipt.results[1].seq,3);
    drop(c); drop(store); std::fs::remove_file(p).unwrap();
}
#[test]
fn lock_timeout_then_retry_and_concurrent_append() {
    let p=path(); let store=Store::open(&p).unwrap();
    let start=append_batch(&[append_json("s","1","task.start",r#"{"title":"T","goal":"G","project":"P"}"#)]);
    let explicit=r#"[{"v":1,"session":"e","seq":1,"ts":"2024-01-01T00:00:00Z","type":"task.start","payload":{"title":"T","goal":"G","project":"P"}}]"#;
    let e=mindtrail_lib::protocol::decode_explicit(explicit.as_bytes()).unwrap();
    let c=rusqlite::Connection::open(&p).unwrap(); c.execute_batch("BEGIN IMMEDIATE").unwrap();
    assert_eq!(store.write_append(&start).unwrap_err().status(),503); assert_eq!(store.write_explicit(&e).unwrap_err().status(),503);
    c.execute_batch("COMMIT").unwrap(); store.write_append(&start).unwrap(); store.write_explicit(&e).unwrap(); drop(c);
    let barrier=std::sync::Arc::new(std::sync::Barrier::new(3));
    let prepared:Vec<_>=(2..4).map(|i| (Store::open(&p).unwrap(),append_batch(&[append_json("s",&i.to_string(),"capability.intent",&format!(r#"{{"id":"c{i}","title":"C"}}"#))]))).collect();
    let handles:Vec<_>=prepared.into_iter().map(|(s,b)| {let barrier=barrier.clone(); std::thread::spawn(move || {barrier.wait(); s.write_append(&b).unwrap().results[0].seq})}).collect();
    barrier.wait(); let mut seq:Vec<_>=handles.into_iter().map(|h|h.join().unwrap()).collect(); seq.sort(); assert_eq!(seq,vec![2,3]); drop(store); std::fs::remove_file(p).unwrap();
}
#[test]
fn structurally_equal_reordered_json_is_duplicate() {
    use mindtrail_lib::protocol::decode_explicit;
    let p=path(); let store=Store::open(&p).unwrap();
    let e1=r#"[{"v":1,"session":"e","seq":1,"ts":"2024-01-01T00:00:00Z","type":"task.start","payload":{"title":"T","goal":"G","project":"P"}}]"#;
    let e2=r#"[ { "payload":{"project":"P","goal":"G","title":"T"},"type":"task.start","ts":"2024-01-01T00:00:00Z","seq":1,"session":"e","v":1 } ]"#;
    store.write_explicit(&decode_explicit(e1.as_bytes()).unwrap()).unwrap();
    let a1=append_batch(&[append_json("a","1","task.start",r#"{"title":"T","goal":"G","project":"P"}"#)]); store.write_append(&a1).unwrap(); drop(store);
    let store=Store::open(&p).unwrap(); assert_eq!(store.write_explicit(&decode_explicit(e2.as_bytes()).unwrap()).unwrap().duplicates,1);
    let a2=r#"[{"payload":{"project":"P","goal":"G","title":"T"},"type":"task.start","ts":"2024-01-01T00:00:00Z","operation_id":"1","session":"a","v":1}]"#;
    assert_eq!(store.write_append(&mindtrail_lib::protocol::decode_append(a2.as_bytes()).unwrap()).unwrap().results[0].seq,1);
    let changed=e2.replace("\"title\":\"T\"","\"title\":\"X\""); let err=store.write_explicit(&decode_explicit(changed.as_bytes()).unwrap()).unwrap_err(); assert_eq!((err.code,err.index,err.last_seq),("seq_conflict",Some(0),Some(1)));
    let changed=a2.replace("\"title\":\"T\"","\"title\":\"X\""); let err=store.write_append(&mindtrail_lib::protocol::decode_append(changed.as_bytes()).unwrap()).unwrap_err(); assert_eq!((err.code,err.index,err.last_seq),("operation_conflict",Some(0),Some(1)));
    drop(store); std::fs::remove_file(p).unwrap();
}


#[test]
fn paging_survives_reopen_and_sorts_binary() {
    let p=path(); let store=Store::open(&p).unwrap();
    for session in ["z","A","a"] {let batch=append_batch(&[append_json(session,"1","task.start",r#"{"title":"T","goal":"G","project":"P"}"#),append_json(session,"2","task.done","{}")]); store.write_append(&batch).unwrap();}
    drop(store); let store=Store::open(&p).unwrap();
    assert_eq!(store.sessions(None,2).unwrap().iter().map(|s|s.session.as_str()).collect::<Vec<_>>(),vec!["A","a"]);
    assert_eq!(store.sessions(Some("a"),2).unwrap()[0].session,"z");
    let page=store.events("a",0,1).unwrap(); assert_eq!((page.events.len(),page.last_seq,page.status.as_str()),(1,2,"done"));
    assert_eq!(store.events("a",1,2).unwrap().events[0]["seq"],2);
    assert!(store.events("a",u64::MAX,2).unwrap().events.is_empty());
    assert_eq!(store.events("missing",0,1).unwrap_err().status(),404); assert_eq!(store.sessions(None,0).unwrap_err().status(),400);
    drop(store); std::fs::remove_file(p).unwrap();
}

#[test]
fn corrupt_events_and_status_are_storage_errors() {
    let p=path(); let store=Store::open(&p).unwrap();
    let batch=append_batch(&[append_json("s","1","task.start",r#"{"title":"T","goal":"G","project":"P"}"#)]); store.write_append(&batch).unwrap();
    let c=rusqlite::Connection::open(&p).unwrap();
    let original:String=c.query_row("SELECT event_json FROM events",[],|r|r.get(0)).unwrap();
    for corrupt in ["null".to_string(), format!("{},{}", original, original.replace("\"seq\":1","\"seq\":2")), format!("{} trailing", original), original.replace("\"seq\":1","\"seq\":2"), original.replace("\"session\":\"s\"","\"session\":\"x\""), original.replace("task.start","unknown")] {
        c.execute("UPDATE events SET event_json=?1",[corrupt]).unwrap(); assert_eq!(store.events("s",0,10).unwrap_err().status(),503);
    }
    c.execute("UPDATE events SET event_json=?1",[original]).unwrap(); c.execute("UPDATE sessions SET status='broken'",[]).unwrap();
    assert_eq!(store.sessions(None,10).unwrap_err().status(),503); assert_eq!(store.events("s",0,10).unwrap_err().status(),503);
    drop(c); drop(store); std::fs::remove_file(p).unwrap();
}

#[test]
fn near_limit_event_remains_readable_after_number_normalization() {
    use mindtrail_lib::protocol::{decode_explicit,MAX_BODY_BYTES};
    let p=path(); let store=Store::open(&p).unwrap();
    let start=r#"[{"v":1,"session":"s","seq":1,"ts":"2024-01-01T00:00:00Z","type":"task.start","payload":{"title":"T","goal":"G","project":"P"}}]"#;
    store.write_explicit(&decode_explicit(start.as_bytes()).unwrap()).unwrap();
    let prefix=r#"[{"v":1,"session":"s","seq":2,"ts":"2024-01-01T00:00:00Z","type":"file.change","payload":{"id":"f","path":"p","action":"modify","stats":{"add":1e15,"del":1e15},"quote":""#;
    let suffix=r#""}}]"#; let body=format!("{}{}{}",prefix,"x".repeat(MAX_BODY_BYTES-prefix.len()-suffix.len()),suffix);
    assert_eq!(body.len(),MAX_BODY_BYTES); store.write_explicit(&decode_explicit(body.as_bytes()).unwrap()).unwrap();
    let c=rusqlite::Connection::open(&p).unwrap(); let len:i64=c.query_row("SELECT length(event_json) FROM events WHERE seq=2",[],|r|r.get(0)).unwrap(); assert!(len>MAX_BODY_BYTES as i64); drop(c);
    let page=store.events("s",1,1).unwrap(); assert_eq!(page.events[0]["seq"],2);
    drop(store); std::fs::remove_file(p).unwrap();
}
#[test]
fn missing_page_facts_and_corrupt_snapshots_fail() {
    use mindtrail_lib::protocol::decode_explicit;
    let p=path(); let store=Store::open(&p).unwrap();
    let e=r#"[{"v":1,"session":"e","seq":1,"ts":"2024-01-01T00:00:00Z","type":"task.start","payload":{"title":"T","goal":"G","project":"P"}},{"v":1,"session":"e","seq":2,"ts":"2024-01-01T00:00:00Z","type":"capability.intent","payload":{"id":"c","title":"C"}},{"v":1,"session":"e","seq":3,"ts":"2024-01-01T00:00:00Z","type":"task.done","payload":{}}]"#;
    let batch=decode_explicit(e.as_bytes()).unwrap(); store.write_explicit(&batch).unwrap();
    assert_eq!(store.events("e",0,2).unwrap().events.len(),2); assert!(store.events("e",4,2).unwrap().events.is_empty());
    let c=rusqlite::Connection::open(&p).unwrap(); let saved:String=c.query_row("SELECT event_json FROM events WHERE session='e' AND seq=2",[],|r|r.get(0)).unwrap();
    c.execute("DELETE FROM events WHERE session='e' AND seq=2",[]).unwrap(); assert_eq!(store.events("e",1,2).unwrap_err().status(),503); assert_eq!(store.write_explicit(&batch).unwrap_err().status(),503);
    c.execute("INSERT INTO events VALUES ('e',2,?1)",[saved]).unwrap(); c.execute("DELETE FROM events WHERE session='e' AND seq=3",[]).unwrap(); assert_eq!(store.events("e",2,2).unwrap_err().status(),503);
    c.execute("UPDATE events SET event_json='null' WHERE session='e' AND seq=1",[]).unwrap(); assert_eq!(store.write_explicit(&batch).unwrap_err().status(),503);
    drop(c); drop(store); std::fs::remove_file(p).unwrap();
}
#[test]
fn corrupt_operation_mapping_and_identity_fail() {
    let p=path(); let store=Store::open(&p).unwrap();
    let batch=append_batch(&[append_json("s","1","task.start",r#"{"title":"T","goal":"G","project":"P"}"#),append_json("s","2","task.done","{}")]); store.write_append(&batch).unwrap();
    let c=rusqlite::Connection::open(&p).unwrap();
    c.execute_batch("PRAGMA foreign_keys=OFF; DROP INDEX operations_session_seq").unwrap();
    c.execute("UPDATE operations SET seq=2 WHERE operation_id='1'",[]).unwrap(); assert_eq!(store.write_append(&batch).unwrap_err().status(),503);
    c.execute("UPDATE operations SET seq=1 WHERE operation_id='1'",[]).unwrap(); c.execute("DELETE FROM events WHERE seq=1",[]).unwrap(); assert_eq!(store.write_append(&batch).unwrap_err().status(),503);
    c.execute("INSERT INTO store_meta VALUES ('bad')",[]).unwrap(); assert_eq!(store.sessions(None,1).unwrap_err().status(),503); assert!(Store::open(&p).is_err());
    c.execute("DELETE FROM store_meta",[]).unwrap(); c.execute("INSERT INTO store_meta VALUES ('')",[]).unwrap(); assert!(Store::open(&p).is_err());
    drop(c); drop(store); std::fs::remove_file(&p).unwrap();
    let missing=path(); let stale=Store::open(&missing).unwrap(); std::fs::remove_file(&missing).unwrap(); assert_eq!(stale.sessions(None,1).unwrap_err().status(),503); assert!(!missing.exists());
}


#[test]
fn identical_operations_cannot_share_a_sequence() {
    let p=path(); let store=Store::open(&p).unwrap();
    let start=append_json("s","1","task.start",r#"{"title":"T","goal":"G","project":"P"}"#);
    let a=append_json("s","2","capability.intent",r#"{"id":"c","title":"C"}"#);
    let b=append_json("s","3","capability.intent",r#"{"id":"c","title":"C"}"#);
    store.write_append(&append_batch(&[start,a.clone(),b.clone()])).unwrap();
    let c=rusqlite::Connection::open(&p).unwrap();
    assert!(c.execute("UPDATE operations SET seq=3 WHERE operation_id='2'",[]).is_err());
    c.execute_batch("DROP INDEX operations_session_seq; UPDATE operations SET seq=3 WHERE operation_id='2'").unwrap();
    assert_eq!(store.write_append(&append_batch(&[a])).unwrap_err().status(),503);
    assert_eq!(store.events("s",0,10).unwrap_err().status(),503);
    assert!(Store::open(&p).is_err());
    c.execute("UPDATE operations SET seq=2 WHERE operation_id='2'",[]).unwrap();
    drop(c); drop(store);
    let reopened=Store::open(&p).unwrap();
    assert_eq!(reopened.write_append(&append_batch(&[b])).unwrap().results[0].seq,3);
    drop(reopened); std::fs::remove_file(p).unwrap();
}

#[test]
fn unique_but_reassigned_mapping_is_rejected() {
    let p=path(); let store=Store::open(&p).unwrap();
    let a=append_json("s","2","capability.intent",r#"{"id":"c","title":"C"}"#);
    let b=append_json("s","3","capability.intent",r#"{"id":"c","title":"C"}"#);
    store.write_append(&append_batch(&[append_json("s","1","task.start",r#"{"title":"T","goal":"G","project":"P"}"#),a.clone(),b.clone()])).unwrap();
    let c=rusqlite::Connection::open(&p).unwrap();
    c.execute_batch("DELETE FROM operations WHERE operation_id='3'; UPDATE operations SET seq=3 WHERE operation_id='2'").unwrap();
    assert_eq!(store.write_append(&append_batch(&[a])).unwrap_err().status(),503);
    assert_eq!(store.write_append(&append_batch(&[b])).unwrap_err().status(),503);
    drop(c); drop(store); std::fs::remove_file(p).unwrap();
}

#[test]
fn legacy_migration_rejects_unverifiable_append_and_preserves_data() {
    let p=path(); let store=Store::open(&p).unwrap();
    let a=append_batch(&[append_json("s","1","task.start",r#"{"title":"T","goal":"G","project":"P"}"#)]);
    store.write_append(&a).unwrap(); drop(store);
    let c=rusqlite::Connection::open(&p).unwrap();
    c.execute_batch("DROP TABLE event_owners; PRAGMA user_version=1").unwrap(); drop(c);
    assert_eq!(Store::open(&p).err().unwrap().status(),503);
    let c=rusqlite::Connection::open(&p).unwrap();
    assert_eq!(c.pragma_query_value(None,"user_version",|r|r.get::<_,i64>(0)).unwrap(),1);
    assert_eq!(c.query_row("SELECT COUNT(*) FROM events",[],|r|r.get::<_,i64>(0)).unwrap(),1);
    c.execute_batch("DELETE FROM operations; DELETE FROM events; DELETE FROM sessions").unwrap(); drop(c);
    let migrated=Store::open(&p).unwrap(); assert_eq!(migrated.write_append(&a).unwrap().results[0].seq,1);
    drop(migrated); std::fs::remove_file(p).unwrap();
}

#[test]
fn legacy_explicit_history_survives_migration_and_reopen() {
    use mindtrail_lib::protocol::decode_explicit;
    let p=path(); let store=Store::open(&p).unwrap();
    let raw=br#"[{"v":1,"session":"legacy","seq":1,"ts":"2024-01-01T00:00:00Z","type":"task.start","payload":{"title":"T","goal":"G","project":"P"}},{"v":1,"session":"legacy","seq":2,"ts":"2024-01-01T00:00:00Z","type":"task.done","payload":{}}]"#;
    let batch=decode_explicit(raw).unwrap();
    store.write_explicit(&batch).unwrap();
    let id=store.store_id().to_owned();
    let before=store.events("legacy",0,10).unwrap();
    drop(store);
    let c=rusqlite::Connection::open(&p).unwrap();
    c.execute_batch("DROP TABLE event_owners; DROP INDEX operations_session_seq; PRAGMA user_version=1").unwrap();
    drop(c);
    let migrated=Store::open(&p).unwrap();
    assert_eq!(migrated.store_id(),id);
    assert_eq!(migrated.events("legacy",0,10).unwrap(),before);
    drop(migrated);
    let reopened=Store::open(&p).unwrap();
    assert_eq!(reopened.events("legacy",0,10).unwrap(),before);
    let receipt=reopened.write_explicit(&batch).unwrap();
    assert_eq!((receipt.accepted,receipt.duplicates,receipt.last_seq),(0,2,2));
    drop(reopened); std::fs::remove_file(p).unwrap();
}

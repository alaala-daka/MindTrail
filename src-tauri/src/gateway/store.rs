use std::{path::{Path, PathBuf}, time::Duration};
use rusqlite::{Connection, TransactionBehavior};
use crate::protocol::{ProtocolError, temporary_storage_error};

pub struct Store { pub(crate) path: PathBuf, store_id: String }
impl Store {
    pub fn open(path: &Path) -> Result<Self, ProtocolError> {
        Self::open_with_hook(path, || {})
    }
    fn open_with_hook(path: &Path, before_transaction: impl FnOnce()) -> Result<Self, ProtocolError> {
        Self::open_checked(path, before_transaction, || Ok(()))
    }
    pub(super) fn open_with_postcheck(path: &Path, after_commit: impl FnOnce() -> std::io::Result<()>) -> Result<Self, ProtocolError> {
        Self::open_checked(path, || {}, after_commit)
    }
    fn open_checked(path: &Path, before_transaction: impl FnOnce(), after_commit: impl FnOnce() -> std::io::Result<()>) -> Result<Self, ProtocolError> {
        let mut conn = Connection::open(path).map_err(storage)?;
        conn.busy_timeout(Duration::from_millis(250)).map_err(storage)?;
        // Check the schema version before making any persistent changes.
        let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0)).map_err(storage)?;
        if version > 2 { return Err(temporary_storage_error("unsupported database version")); }
        conn.pragma_update(None, "foreign_keys", "ON").map_err(storage)?;
        let wal: String = conn.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0)).map_err(storage)?;
        conn.pragma_update(None, "synchronous", "FULL").map_err(storage)?;
        let fk: i64 = conn.pragma_query_value(None, "foreign_keys", |r| r.get(0)).map_err(storage)?;
        let sync: i64 = conn.pragma_query_value(None, "synchronous", |r| r.get(0)).map_err(storage)?;
        if !wal.eq_ignore_ascii_case("wal") || fk != 1 || sync != 2 { return Err(temporary_storage_error("required SQLite settings unavailable")); }
        before_transaction();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(storage)?;
        let version: i64 = tx.pragma_query_value(None, "user_version", |r| r.get(0)).map_err(storage)?;
        if version > 2 { return Err(temporary_storage_error("unsupported database version")); }
        if version == 0 {
            tx.execute_batch("CREATE TABLE store_meta (store_id TEXT NOT NULL);
CREATE TABLE sessions (session TEXT COLLATE BINARY NOT NULL PRIMARY KEY, title TEXT NOT NULL, project TEXT NOT NULL, mode TEXT NOT NULL, status TEXT NOT NULL, last_seq INTEGER NOT NULL);
CREATE TABLE events (session TEXT COLLATE BINARY NOT NULL, seq INTEGER NOT NULL, event_json TEXT NOT NULL, PRIMARY KEY(session, seq), FOREIGN KEY(session) REFERENCES sessions(session));
CREATE TABLE operations (session TEXT COLLATE BINARY NOT NULL, operation_id TEXT COLLATE BINARY NOT NULL, input_json TEXT NOT NULL, seq INTEGER NOT NULL, PRIMARY KEY(session, operation_id), FOREIGN KEY(session, seq) REFERENCES events(session, seq));
CREATE TABLE nodes (session TEXT COLLATE BINARY NOT NULL, id TEXT COLLATE BINARY NOT NULL, kind TEXT NOT NULL, PRIMARY KEY(session, id), FOREIGN KEY(session) REFERENCES sessions(session));").map_err(storage)?;
            let mut bytes = [0u8; 32];
            getrandom::fill(&mut bytes).map_err(storage)?;
            let id: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
            tx.execute("INSERT INTO store_meta (store_id) VALUES (?1)", [&id]).map_err(storage)?;

        }
        if version <= 1 {
            let legacy_append: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM sessions WHERE mode='append') OR EXISTS(SELECT 1 FROM operations)", [], |r| r.get(0)).map_err(storage)?;
            if legacy_append { return Err(temporary_storage_error("legacy append ownership cannot be verified")); }
            tx.execute_batch("CREATE UNIQUE INDEX IF NOT EXISTS operations_session_seq ON operations(session, seq); CREATE TABLE event_owners (session TEXT COLLATE BINARY NOT NULL, seq INTEGER NOT NULL, operation_id TEXT COLLATE BINARY NOT NULL, PRIMARY KEY(session,seq), UNIQUE(session,operation_id), FOREIGN KEY(session,seq) REFERENCES events(session,seq))").map_err(storage)?;
            tx.pragma_update(None, "user_version", 2).map_err(storage)?;
        }
        tx.execute_batch("CREATE UNIQUE INDEX IF NOT EXISTS operations_session_seq ON operations(session, seq)").map_err(storage)?;
        let store_id = read_identity(&tx)?;
        tx.commit().map_err(storage)?;
        after_commit().map_err(storage)?;
        Ok(Self { path: path.to_owned(), store_id })
    }
    pub fn store_id(&self) -> &str { &self.store_id }
}
fn storage(e: impl std::fmt::Display) -> ProtocolError { temporary_storage_error(e.to_string()) }

#[cfg(test)]
pub(super) fn test_writer_pragmas(path: &Path) -> (String, i64) {
    let c = write_connection(path).unwrap();
    (c.pragma_query_value(None, "journal_mode", |r| r.get(0)).unwrap(), c.pragma_query_value(None, "synchronous", |r| r.get(0)).unwrap())
}




#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    #[test]
    fn version_is_rechecked_after_another_transaction() {
        let mut random = [0u8; 16]; getrandom::fill(&mut random).unwrap();
        let path = std::env::temp_dir().join(format!("mindtrail-race-{:?}.sqlite", random));
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE sentinel (value TEXT); INSERT INTO sentinel VALUES ('kept');").unwrap();
        let (ready_tx, ready_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        let worker_path = path.clone();
        let worker = std::thread::spawn(move || Store::open_with_hook(&worker_path, || {
            ready_tx.send(()).unwrap(); resume_rx.recv().unwrap();
        }).is_err());
        ready_rx.recv().unwrap();
        conn.execute_batch("BEGIN IMMEDIATE; PRAGMA user_version=99; COMMIT;").unwrap();
        resume_tx.send(()).unwrap();
        assert!(worker.join().unwrap());
        let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
        assert_eq!(version, 99);
        let value: String = conn.query_row("SELECT value FROM sentinel", [], |r| r.get(0)).unwrap();
        assert_eq!(value, "kept");
        drop(conn); std::fs::remove_file(path).unwrap();
    }
}

use crate::protocol::{AppendBatch, AppendReceipt, ExplicitBatch, ExplicitReceipt, SessionMetadata, SessionSnapshot, SessionStatus, WriteMode, NodeKind, WritePlan, plan_append, plan_explicit};
use rusqlite::{OptionalExtension, params, Transaction};
use serde_json::Value;

fn write_connection(path: &Path) -> Result<Connection, ProtocolError> {
    let c = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE).map_err(storage)?;
    c.busy_timeout(Duration::from_millis(250)).map_err(storage)?;
    c.pragma_update(None, "foreign_keys", "ON").map_err(storage)?;
    c.pragma_update(None, "synchronous", "FULL").map_err(storage)?;
    let fk: i64 = c.pragma_query_value(None, "foreign_keys", |r| r.get(0)).map_err(storage)?;
    let sync: i64 = c.pragma_query_value(None, "synchronous", |r| r.get(0)).map_err(storage)?;
    let wal: String = c.pragma_query_value(None, "journal_mode", |r| r.get(0)).map_err(storage)?;
    if fk != 1 || sync != 2 || !wal.eq_ignore_ascii_case("wal") { return Err(temporary_storage_error("required SQLite settings unavailable")); }
    Ok(c)
}
struct Snapshot<'a, 'b> { tx: &'a Transaction<'b>, metadata: SessionMetadata }
impl SessionSnapshot for Snapshot<'_, '_> {
    fn metadata(&self) -> &SessionMetadata { &self.metadata }
    fn event(&self, seq: u64) -> Result<Option<Value>, ProtocolError> {
        let json: Option<String> = self.tx.query_row("SELECT event_json FROM events WHERE session=?1 AND seq=?2", params![self.metadata.session, seq], |r| r.get(0)).optional().map_err(storage)?;
        if json.is_none() && seq <= self.metadata.last_seq { return Err(temporary_storage_error("stored event missing")); }
        json.map(|s| checked_event(&s, &self.metadata.session, seq, self.metadata.last_seq)).transpose()
    }
    fn operation(&self, id: &str) -> Result<Option<(Value,u64)>, ProtocolError> {
        let row: Option<(String,u64)> = self.tx.query_row("SELECT input_json,seq FROM operations WHERE session=?1 AND operation_id=?2", params![self.metadata.session,id], |r| Ok((r.get(0)?,r.get(1)?))).optional().map_err(storage)?;
                if row.is_none() {
            let owner_exists: bool = self.tx.query_row("SELECT EXISTS(SELECT 1 FROM event_owners WHERE session=?1 AND operation_id=?2)", params![self.metadata.session,id], |r| r.get(0)).map_err(storage)?;
            if owner_exists { return Err(temporary_storage_error("operation mapping missing")); }
        }
        row.map(|(s,n)| {
            let op=crate::protocol::decode_stored_operation(s.as_bytes()).map_err(|_| temporary_storage_error("invalid stored operation"))?;
            if op.session()!=self.metadata.session || op.operation_id()!=id || n==0 || n>self.metadata.last_seq { return Err(temporary_storage_error("stored operation key mismatch")); }
            let owner: Option<String> = self.tx.query_row("SELECT operation_id FROM event_owners WHERE session=?1 AND seq=?2", params![self.metadata.session,n], |r| r.get(0)).optional().map_err(storage)?;
            if owner.as_deref() != Some(id) { return Err(temporary_storage_error("mapped event owner mismatch")); }
            let event=self.event(n)?.ok_or_else(|| temporary_storage_error("mapped event missing"))?;
            if op.with_seq(n).as_ref()!=Some(&event) { return Err(temporary_storage_error("mapped event mismatch")); }
            Ok((op.value().clone(),n))
        }).transpose()
    }
    fn node_kind(&self, id: &str) -> Result<Option<NodeKind>, ProtocolError> {
        let kind: Option<String> = self.tx.query_row("SELECT kind FROM nodes WHERE session=?1 AND id=?2", params![self.metadata.session,id], |r| r.get(0)).optional().map_err(storage)?;
        kind.map(|k| match k.as_str() { "capability"=>Ok(NodeKind::Capability), "api"=>Ok(NodeKind::Api), "file"=>Ok(NodeKind::File), _=>Err(temporary_storage_error("invalid stored node kind")) }).transpose()
    }
}
fn snapshot<'a,'b>(tx: &'a Transaction<'b>, session: &str) -> Result<Option<Snapshot<'a,'b>>, ProtocolError> {

    let row: Option<(String,String,String,String,u64)> = tx.query_row("SELECT title,project,status,mode,last_seq FROM sessions WHERE session=?1", [session], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional().map_err(storage)?;
    row.map(|(title,project,status,mode,last_seq)| Ok(Snapshot {tx,metadata:SessionMetadata {session:session.into(),title,project,last_seq,status:match status.as_str(){"running"=>SessionStatus::Running,"done"=>SessionStatus::Done,_=>return Err(temporary_storage_error("invalid stored status"))},mode:match mode.as_str(){"explicit"=>WriteMode::Explicit,"append"=>WriteMode::Append,_=>return Err(temporary_storage_error("invalid stored mode"))}}})).transpose()
}
fn persist<R>(tx: &Transaction<'_>, plan: WritePlan<R>) -> Result<R, ProtocolError> {
    let m = &plan.metadata;
    let mode = match m.mode {WriteMode::Explicit=>"explicit",WriteMode::Append=>"append"};
    let status = match m.status {SessionStatus::Running=>"running",SessionStatus::Done=>"done"};
    tx.execute("INSERT INTO sessions(session,title,project,mode,status,last_seq) VALUES (?1,?2,?3,?4,?5,?6) ON CONFLICT(session) DO UPDATE SET title=excluded.title,project=excluded.project,mode=excluded.mode,status=excluded.status,last_seq=excluded.last_seq", params![m.session,m.title,m.project,mode,status,m.last_seq]).map_err(storage)?;
    for e in plan.events { let seq = e["seq"].as_u64().ok_or_else(|| temporary_storage_error("missing event sequence"))?; tx.execute("INSERT INTO events VALUES (?1,?2,?3)",params![m.session,seq,serde_json::to_string(&e).map_err(storage)?]).map_err(storage)?; }
    for op in plan.operations { tx.execute("INSERT INTO event_owners VALUES (?1,?2,?3)",params![m.session,op.seq,op.operation_id]).map_err(storage)?; tx.execute("INSERT INTO operations VALUES (?1,?2,?3,?4)",params![m.session,op.operation_id,serde_json::to_string(&op.input).map_err(storage)?,op.seq]).map_err(storage)?; }
    for (id,kind) in plan.node_updates { let kind = match kind {NodeKind::Capability=>"capability",NodeKind::Api=>"api",NodeKind::File=>"file"}; tx.execute("INSERT INTO nodes VALUES (?1,?2,?3) ON CONFLICT(session,id) DO UPDATE SET kind=excluded.kind",params![m.session,id,kind]).map_err(storage)?; }
    Ok(plan.receipt)
}
impl Store {
    pub fn write_explicit(&self, batch: &ExplicitBatch) -> Result<ExplicitReceipt,ProtocolError> {
        let mut c=write_connection(&self.path)?; let tx=c.transaction_with_behavior(TransactionBehavior::Immediate).map_err(storage)?;
        verify_identity(&tx, &self.store_id)?;
        let snap=snapshot(&tx,batch.items()[0].session())?; let plan=plan_explicit(batch,snap.as_ref().map(|s| s as &dyn SessionSnapshot))?; drop(snap);
        let receipt=persist(&tx,plan)?; tx.commit().map_err(storage)?; Ok(receipt)
    }
    pub fn write_append(&self, batch: &AppendBatch) -> Result<AppendReceipt,ProtocolError> {
        let mut c=write_connection(&self.path)?; let tx=c.transaction_with_behavior(TransactionBehavior::Immediate).map_err(storage)?;
        verify_identity(&tx, &self.store_id)?;
        let snap=snapshot(&tx,batch.items()[0].session())?; let plan=plan_append(batch,snap.as_ref().map(|s| s as &dyn SessionSnapshot))?; drop(snap);
        let receipt=persist(&tx,plan)?;
        #[cfg(test)] super::recovery::checkpoint("before_commit");
        tx.commit().map_err(storage)?;
        #[cfg(test)] super::recovery::checkpoint("after_commit");
        Ok(receipt)
    }
}

fn verify_identity(tx: &Transaction<'_>, expected: &str) -> Result<(), ProtocolError> {
    let version: i64 = tx.pragma_query_value(None, "user_version", |r| r.get(0)).map_err(storage)?;
    let id = read_identity(tx)?;
    if version != 2 || id != expected { return Err(temporary_storage_error("database identity or version changed")); }
    Ok(())
}

#[derive(Debug, Clone, serde::Serialize, PartialEq)]
pub struct SessionSummary { pub session:String, pub title:String, pub project:String, pub status:String, pub last_seq:u64 }
#[derive(Debug, Clone, serde::Serialize, PartialEq)]
pub struct EventPage { pub events:Vec<Value>, pub last_seq:u64, pub status:String }
fn check_limit(limit:u16)->Result<(),ProtocolError> { if !(1..=200).contains(&limit) {Err(crate::protocol::invalid_query_error())} else {Ok(())} }
impl Store {
    /// Return a bounded, consistent prefix from one SQLite read transaction.
    pub fn semantic_prefix(&self, session: &str, prefix: u64) -> Result<Vec<Value>, ProtocolError> {
        self.semantic_prefix_with_hook(session, prefix, || {})
    }
    fn semantic_prefix_with_hook(&self, session: &str, prefix: u64, hook: impl FnOnce()) -> Result<Vec<Value>, ProtocolError> {
        if prefix == 0 || prefix > 2000 { return Err(crate::protocol::invalid_query_error()); }
        let mut conn = write_connection(&self.path)?;
        let tx = conn.transaction().map_err(storage)?;
        verify_identity(&tx, &self.store_id)?;
        let meta: Option<(u64, String)> = tx.query_row("SELECT last_seq,mode FROM sessions WHERE session=?1", [session], |row| Ok((row.get(0)?, row.get(1)?))).optional().map_err(storage)?;
        let (last_seq, mode) = meta.ok_or_else(crate::protocol::not_found_error)?;
        if prefix > last_seq { return Err(crate::protocol::invalid_query_error()); }
        hook();
        let mut stmt = tx.prepare("SELECT seq,length(CAST(event_json AS BLOB)),event_json FROM events WHERE session=?1 AND seq<=?2 ORDER BY seq").map_err(storage)?;
        let mut rows = stmt.query(params![session, prefix as i64]).map_err(storage)?;
        let mut result = Vec::new();
        let mut total = 0usize;
        while let Some(row) = rows.next().map_err(storage)? {
            let seq: u64 = row.get(0).map_err(storage)?;
            if seq != result.len() as u64 + 1 { return Err(temporary_storage_error("stored event gap")); }
            let bytes: usize = row.get(1).map_err(storage)?;
            total = total.checked_add(bytes).ok_or_else(crate::protocol::invalid_query_error)?;
            if total > 256 * 1024 { return Err(crate::protocol::invalid_query_error()); }
            let json: String = row.get(2).map_err(storage)?;
            if mode == "append" {
                let count: i64 = tx.query_row("SELECT COUNT(*) FROM event_owners o JOIN operations p ON p.session=o.session AND p.seq=o.seq AND p.operation_id=o.operation_id WHERE o.session=?1 AND o.seq=?2", params![session,seq], |r| r.get(0)).map_err(storage)?;
                if count != 1 { return Err(temporary_storage_error("event owner mismatch")); }
            }
            result.push(checked_event(&json, session, seq, last_seq)?);
        }
        if result.len() as u64 != prefix { return Err(temporary_storage_error("stored event gap")); }
        drop(rows); drop(stmt); tx.commit().map_err(storage)?;
        Ok(result)
    }
    pub fn sessions(&self, after:Option<&str>, limit:u16)->Result<Vec<SessionSummary>,ProtocolError> {
        check_limit(limit)?; let mut c=write_connection(&self.path)?; let tx=c.transaction().map_err(storage)?; verify_identity(&tx,&self.store_id)?;
        let sql=if after.is_some() {"SELECT session,title,project,status,last_seq FROM sessions WHERE session > ?1 COLLATE BINARY ORDER BY session COLLATE BINARY LIMIT ?2"} else {"SELECT session,title,project,status,last_seq FROM sessions ORDER BY session COLLATE BINARY LIMIT ?2"};
        let mut stmt=tx.prepare(sql).map_err(storage)?;
        let rows=stmt.query_map(params![after,limit], |r| Ok(SessionSummary {session:r.get(0)?,title:r.get(1)?,project:r.get(2)?,status:r.get(3)?,last_seq:r.get(4)?})).map_err(storage)?;
        let result=rows.collect::<Result<Vec<_>,_>>().map_err(storage)?; for row in &result { validate_status(&row.status)?; } drop(stmt); tx.commit().map_err(storage)?; Ok(result)
    }
    pub fn events(&self, session:&str, since:u64, limit:u16)->Result<EventPage,ProtocolError> { self.events_with_hook(session,since,limit,|| {}) }
    fn events_with_hook(&self, session:&str, since:u64, limit:u16, hook:impl FnOnce())->Result<EventPage,ProtocolError> {
        check_limit(limit)?; let mut c=write_connection(&self.path)?; let tx=c.transaction().map_err(storage)?; verify_identity(&tx,&self.store_id)?;
        let meta:Option<(u64,String,String)>=tx.query_row("SELECT last_seq,status,mode FROM sessions WHERE session=?1",[session],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(storage)?;
        let (last_seq,status,mode)=meta.ok_or_else(crate::protocol::not_found_error)?; validate_status(&status)?; hook();
        let mut stmt=tx.prepare("SELECT seq,event_json FROM events WHERE session=?1 AND seq>?2 ORDER BY seq LIMIT ?3").map_err(storage)?;
        let mut rows=stmt.query(params![session,since.min(i64::MAX as u64) as i64,limit]).map_err(storage)?; let mut events=Vec::new();
        while let Some(row)=rows.next().map_err(storage)? { let seq:u64=row.get(0).map_err(storage)?; let json:String=row.get(1).map_err(storage)?; let expected=since.checked_add(events.len() as u64 + 1).ok_or_else(|| temporary_storage_error("invalid event sequence"))?; if seq!=expected { return Err(temporary_storage_error("stored event gap")); } if mode == "append" { let count: i64 = tx.query_row("SELECT COUNT(*) FROM event_owners o JOIN operations p ON p.session=o.session AND p.seq=o.seq AND p.operation_id=o.operation_id WHERE o.session=?1 AND o.seq=?2", params![session,seq], |r| r.get(0)).map_err(storage)?; if count != 1 { return Err(temporary_storage_error("event owner mismatch")); } } events.push(checked_event(&json,session,seq,last_seq)?); }
        drop(rows); drop(stmt); let remaining=last_seq.saturating_sub(since); if (events.len() as u64) < remaining.min(limit as u64) { return Err(temporary_storage_error("stored event gap")); } tx.commit().map_err(storage)?; Ok(EventPage {events,last_seq,status})
    }
}


#[cfg(test)]
mod paging_tests {
    use super::*;
    use crate::protocol::decode_append;
    #[test]
    fn semantic_prefix_never_reads_future_events() {
        let mut bytes=[0u8;16]; getrandom::fill(&mut bytes).unwrap();
        let path=std::env::temp_dir().join(format!("mindtrail-semantic-{:?}.sqlite",bytes));
        let store=Store::open(&path).unwrap();
        let start=br#"[{"v":1,"session":"s","operation_id":"1","ts":"2024-01-01T00:00:00Z","type":"task.start","payload":{"title":"T","goal":"G","project":"P"}}]"#;
        let done=br#"[{"v":1,"session":"s","operation_id":"2","ts":"2024-01-01T00:00:00Z","type":"task.done","payload":{}}]"#;
        store.write_append(&decode_append(start).unwrap()).unwrap();
        let other=Store::open(&path).unwrap();
        let prefix=store.semantic_prefix_with_hook("s",1,|| { other.write_append(&decode_append(done).unwrap()).unwrap(); }).unwrap();
        assert_eq!(prefix.len(),1); assert_eq!(prefix[0]["seq"],1);
        assert!(store.semantic_prefix("s",0).is_err());
        assert!(store.semantic_prefix("s",3).is_err());
        assert!(store.semantic_prefix("s",2001).is_err());
        assert!(store.semantic_prefix("missing",1).is_err());
        let conn=Connection::open(&path).unwrap();
        conn.execute("DELETE FROM event_owners WHERE session='s' AND seq=1", []).unwrap();
        assert!(store.semantic_prefix("s",1).is_err());
        conn.execute("INSERT INTO event_owners(session,seq,operation_id) VALUES('s',1,'1')", []).unwrap();
        conn.execute("UPDATE events SET event_json=?1 WHERE session='s' AND seq=1", ["x".repeat(256 * 1024 + 1)]).unwrap();
        assert!(store.semantic_prefix("s",1).is_err());
        conn.execute("UPDATE store_meta SET store_id=?1", ["0".repeat(64)]).unwrap();
        assert!(store.semantic_prefix("s",1).is_err());
        drop(conn); drop(other); drop(store); std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn semantic_seven_event_prefix_is_contiguous() {
        let mut bytes=[0u8;16]; getrandom::fill(&mut bytes).unwrap();
        let path=std::env::temp_dir().join(format!("mindtrail-semantic-seven-{:?}.sqlite",bytes));
        let store=Store::open(&path).unwrap();
        let mut events=vec![serde_json::json!({"v":1,"session":"s","operation_id":"1","ts":"2024-01-01T00:00:00Z","type":"task.start","payload":{"title":"T","goal":"G","project":"P"}})];
        for i in 2..=6 { events.push(serde_json::json!({"v":1,"session":"s","operation_id":i.to_string(),"ts":"2024-01-01T00:00:00Z","type":"capability.intent","payload":{"id":format!("n{i}"),"title":"T"}})); }
        events.push(serde_json::json!({"v":1,"session":"s","operation_id":"7","ts":"2024-01-01T00:00:00Z","type":"task.done","payload":{}}));
        store.write_append(&decode_append(&serde_json::to_vec(&events).unwrap()).unwrap()).unwrap();
        let prefix=store.semantic_prefix("s",3).unwrap();
        assert_eq!(prefix.iter().map(|v|v["seq"].as_u64().unwrap()).collect::<Vec<_>>(),vec![1,2,3]);
        assert!(store.semantic_prefix("s",8).is_err());
        drop(store); std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn semantic_accepts_two_thousand_events_and_rejects_more() {
        let mut bytes=[0u8;16]; getrandom::fill(&mut bytes).unwrap();
        let path=std::env::temp_dir().join(format!("mindtrail-semantic-limit-{:?}.sqlite",bytes));
        let store=Store::open(&path).unwrap();
        let mut events=vec![serde_json::json!({"v":1,"session":"s","operation_id":"1","ts":"2024-01-01T00:00:00Z","type":"task.start","payload":{"title":"T","goal":"G","project":"P"}}), serde_json::json!({"v":1,"session":"s","operation_id":"2","ts":"2024-01-01T00:00:00Z","type":"capability.intent","payload":{"id":"n","title":"T"}})];
        for i in 3..=2000 { events.push(serde_json::json!({"v":1,"session":"s","operation_id":i.to_string(),"ts":"2024-01-01T00:00:00Z","type":"capability.done","payload":{"id":"n"}})); }
        for chunk in events.chunks(200) { store.write_append(&decode_append(&serde_json::to_vec(chunk).unwrap()).unwrap()).unwrap(); }
        assert_eq!(store.semantic_prefix("s",2000).unwrap().len(),2000);
        assert!(store.semantic_prefix("s",2001).is_err());
        drop(store); std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn page_keeps_snapshot_during_concurrent_commit() {
        let mut bytes=[0u8;16]; getrandom::fill(&mut bytes).unwrap(); let path=std::env::temp_dir().join(format!("mindtrail-page-{:?}.sqlite",bytes));
        let store=Store::open(&path).unwrap();
        let start=br#"[{"v":1,"session":"s","operation_id":"1","ts":"2024-01-01T00:00:00Z","type":"task.start","payload":{"title":"T","goal":"G","project":"P"}}]"#;
        store.write_append(&decode_append(start).unwrap()).unwrap();
        let other=Store::open(&path).unwrap();
        let done=br#"[{"v":1,"session":"s","operation_id":"2","ts":"2024-01-01T00:00:00Z","type":"task.done","payload":{}}]"#;
        let page=store.events_with_hook("s",0,200,|| {other.write_append(&decode_append(done).unwrap()).unwrap();}).unwrap();
        assert_eq!((page.last_seq,page.status.as_str(),page.events.len()),(1,"running",1));
        let page=store.events("s",0,200).unwrap(); assert_eq!((page.last_seq,page.status.as_str(),page.events.len()),(2,"done",2));
        drop(other); drop(store); std::fs::remove_file(path).unwrap();
    }
}

fn validate_status(status:&str)->Result<(),ProtocolError> { match status { "running"|"done"=>Ok(()), _=>Err(temporary_storage_error("invalid stored status")) } }





fn read_identity(tx:&Transaction<'_>)->Result<String,ProtocolError> {
    let mut stmt=tx.prepare("SELECT store_id FROM store_meta LIMIT 2").map_err(storage)?;
    let ids=stmt.query_map([],|r|r.get::<_,String>(0)).map_err(storage)?.collect::<Result<Vec<_>,_>>().map_err(storage)?;
    if ids.len()!=1 || ids[0].len()!=64 || !ids[0].bytes().all(|b|b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) { return Err(temporary_storage_error("invalid store identity")); }
    Ok(ids.into_iter().next().unwrap())
}
fn checked_event(json:&str,session:&str,seq:u64,last_seq:u64)->Result<Value,ProtocolError> {
    let event=crate::protocol::decode_stored_event(json.as_bytes()).map_err(|_|temporary_storage_error("invalid stored event"))?;
    if event.session()!=session || event.seq()!=seq || seq==0 || seq>last_seq {return Err(temporary_storage_error("stored event key mismatch"));}
    Ok(event.value().clone())
}





/// Queue names are also Windows path components, so reserved device names are rejected.
pub fn valid_queue_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 64 || !name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-') { return false; }
    if matches!(name, "con" | "prn" | "aux" | "nul") { return false; }
    if name.len() == 4 && (name.starts_with("com") || name.starts_with("lpt")) && matches!(name.as_bytes()[3], b'1'..=b'9') { return false; }
    true
}

#[cfg(test)]
mod tests {
    use super::{valid_queue_name, with_queue, QueueMode};
    #[test]
    fn accepts_only_safe_queue_components() {
        for name in ["mcp", "a_1-2", &"a".repeat(64)] { assert!(valid_queue_name(name), "{name}"); }
        for name in ["", "A", "a/b", "a:b", "con", "prn", "aux", "nul", "com1", "com9", "lpt1", "lpt9", &"a".repeat(65)] { assert!(!valid_queue_name(name), "{name}"); }
        assert!(valid_queue_name("com0")); assert!(valid_queue_name("console"));
    }
    #[test]
    fn explicit_batch_reopens_and_conflicts_rollback() {
        let mut random = [0u8; 16]; getrandom::fill(&mut random).unwrap();
        let suffix: String = random.iter().map(|b| format!("{b:02x}")).collect();
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailQueue-{suffix}"));
        std::fs::create_dir(&base).unwrap(); let root = base.join("state");
        let data = br#"[{"v":1,"session":"s","seq":1,"ts":"2024-01-01T00:00:00Z","type":"task.start","payload":{"title":"T","goal":"G","project":"P"}}]"#;
        let id = with_queue(&root, QueueMode::Explicit, "sample", |q| q.admit_batch("b", data)).unwrap();
        with_queue(&root, QueueMode::Explicit, "sample", |q| {
            assert_eq!(q.get(id)?.unwrap().request, data);
            assert_eq!(q.admit_batch("b", data)?, id);
            let reordered=br#"[{"payload":{"project":"P","goal":"G","title":"T"},"type":"task.start","ts":"2024-01-01T00:00:00Z","seq":1,"session":"s","v":1}]"#;
            assert_eq!(q.admit_batch("b",reordered)?,id);
            assert_eq!(q.get(id)?.unwrap().request,data);
            let changed = String::from_utf8(data.to_vec()).unwrap().replace("\"title\":\"T\"", "\"title\":\"changed\"");
            assert_eq!(q.admit_batch("b", changed.as_bytes()), Err("batch_conflict"));
            assert_eq!(q.admit_batch("other", changed.as_bytes()), Err("seq_conflict"));
            assert!(q.get(id + 1)?.is_none());
            assert_eq!(q.explicit_prefix("s",1,2)?.len(),1);
            assert_eq!(q.explicit_prefix("s",1,3),Err("seq_gap"));
            assert_eq!(q.session_heads()?.len(),1);
            assert_eq!(q.save_outcome(id,"committed",r#"{"accepted":1,"duplicates":0,"last_seq":0}"#),Err("invalid_request"));
            assert_eq!(q.save_outcome(id,"committed",r#"{"accepted":2,"duplicates":0,"last_seq":1}"#),Err("invalid_request"));
            q.save_outcome(id,"committed",r#"{"accepted":1,"duplicates":0,"last_seq":1}"#)?;
            assert!(q.session_heads()?.is_empty());
            assert_eq!(q.get(id)?.unwrap().outcome.as_deref(),Some(r#"{"accepted":1,"duplicates":0,"last_seq":1}"#)); Ok(())
        }).unwrap();
        eprintln!("isolated queue fixture retained: {}",base.display());
    }
    #[test]
    fn gap_repair_is_atomic_and_survives_reopen() {
        let mut random=[0u8;16];getrandom::fill(&mut random).unwrap();let suffix:String=random.iter().map(|b|format!("{b:02x}")).collect();let base=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailRepair-{suffix}"));std::fs::create_dir(&base).unwrap();let root=base.join("state");
        let original=br#"[{"v":1,"session":"s","seq":1,"ts":"2024-01-01T00:00:00Z","type":"task.start","payload":{"title":"T","goal":"G","project":"P"}},{"v":1,"session":"s","seq":3,"ts":"2024-01-01T00:00:00Z","type":"task.done","payload":{}}]"#;
        let missing=br#"[{"v":1,"session":"s","seq":2,"ts":"2024-01-01T00:00:00Z","type":"capability.intent","payload":{"id":"c","title":"C"}}]"#;
        let id=with_queue(&root,QueueMode::Explicit,"sample",|q| {let id=q.admit_batch("original",original)?;q.save_outcome(id,"rejected",r#"{"code":"seq_gap","index":1,"last_seq":0,"expected_seq":2,"offending_seq":3}"#)?;assert!(!q.prepare_gap(id)?);assert!(q.next_repair("s")?.is_none());q.admit_batch("missing",missing)?;assert!(q.prepare_gap(id)?);assert_eq!(q.get(id)?.unwrap().status,"pending");Ok(id)}).unwrap();
        with_queue(&root,QueueMode::Explicit,"sample",|q| {assert_eq!(q.get(id)?.unwrap().request,original);let (repair_id,repair)=q.next_repair("s")?.unwrap();let batch=crate::protocol::decode_explicit(&repair.request).unwrap();assert_eq!(batch.items().iter().map(|e|e.seq()).collect::<Vec<_>>(),vec![1,2]);q.finish_repair(repair_id,"committed")?;Ok(())}).unwrap();
        with_queue(&root,QueueMode::Explicit,"sample",|q| {assert!(q.next_repair("s")?.is_none());assert_eq!(q.get(id)?.unwrap().status,"pending");Ok(())}).unwrap();
    }
    #[test]
    fn v1_migrates_without_rewriting_batches_and_source_corruption_is_rejected() {
        let mut random=[0u8;16];getrandom::fill(&mut random).unwrap();let suffix:String=random.iter().map(|b|format!("{b:02x}")).collect();let base=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailRepairMigration-{suffix}"));std::fs::create_dir(&base).unwrap();let root=base.join("state");
        let original=br#"[{"v":1,"session":"s","seq":1,"ts":"2024-01-01T00:00:00Z","type":"task.start","payload":{"title":"T","goal":"G","project":"P"}},{"v":1,"session":"s","seq":3,"ts":"2024-01-01T00:00:00Z","type":"task.done","payload":{}}]"#;
        let missing=br#"[{"v":1,"session":"s","seq":2,"ts":"2024-01-01T00:00:00Z","type":"capability.intent","payload":{"id":"c","title":"C"}}]"#;
        let id=with_queue(&root,QueueMode::Explicit,"sample",|q|q.admit_batch("original",original)).unwrap();let path=root.join("producers/explicit/sample/queue.sqlite3");
        let db=rusqlite::Connection::open(&path).unwrap();db.execute_batch("DROP TABLE repair_sources; PRAGMA user_version=1;").unwrap();drop(db);
        with_queue(&root,QueueMode::Explicit,"sample",|q| {assert_eq!(q.get(id)?.unwrap().request,original);let version:i64=q.conn.pragma_query_value(None,"user_version",|r|r.get(0)).map_err(super::failure)?;assert_eq!(version,2);q.save_outcome(id,"rejected",r#"{"code":"seq_gap","index":1,"last_seq":0,"expected_seq":2,"offending_seq":3}"#)?;q.admit_batch("missing",missing)?;assert!(q.prepare_gap(id)?);Ok(())}).unwrap();
        let db=rusqlite::Connection::open(&path).unwrap();db.pragma_update(None,"foreign_keys","OFF").unwrap();db.execute("UPDATE repair_sources SET entry_id=99999",[]).unwrap();drop(db);let before=std::fs::read(&path).unwrap();assert_eq!(with_queue(&root,QueueMode::Explicit,"sample",|_|Ok(())),Err("queue_corrupt".into()));assert_eq!(std::fs::read(&path).unwrap(),before);
    }
    #[test]
    fn v1_repairs_without_source_are_preserved_and_rejected() {
        let mut random=[0u8;16];getrandom::fill(&mut random).unwrap();let suffix:String=random.iter().map(|b|format!("{b:02x}")).collect();let base=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailRepairLegacy-{suffix}"));std::fs::create_dir(&base).unwrap();let root=base.join("state");
        let original=br#"[{"v":1,"session":"s","seq":1,"ts":"2024-01-01T00:00:00Z","type":"task.start","payload":{"title":"T","goal":"G","project":"P"}},{"v":1,"session":"s","seq":3,"ts":"2024-01-01T00:00:00Z","type":"task.done","payload":{}}]"#;let missing=br#"[{"v":1,"session":"s","seq":2,"ts":"2024-01-01T00:00:00Z","type":"capability.intent","payload":{"id":"c","title":"C"}}]"#;
        with_queue(&root,QueueMode::Explicit,"sample",|q| {let id=q.admit_batch("original",original)?;q.save_outcome(id,"rejected",r#"{"code":"seq_gap","index":1,"last_seq":0,"expected_seq":2,"offending_seq":3}"#)?;q.admit_batch("missing",missing)?;assert!(q.prepare_gap(id)?);Ok(())}).unwrap();let path=root.join("producers/explicit/sample/queue.sqlite3");let db=rusqlite::Connection::open(&path).unwrap();db.execute_batch("DROP TABLE repair_sources; PRAGMA user_version=1;").unwrap();drop(db);let before=std::fs::read(&path).unwrap();assert_eq!(with_queue(&root,QueueMode::Explicit,"sample",|_|Ok(())),Err("queue_corrupt".into()));assert_eq!(std::fs::read(&path).unwrap(),before);
    }
    #[test]
    fn mcp_v1_migration_preserves_original_operation_bytes() {
        let mut random=[0u8;16];getrandom::fill(&mut random).unwrap();let suffix:String=random.iter().map(|b|format!("{b:02x}")).collect();let base=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailMcpMigration-{suffix}"));std::fs::create_dir(&base).unwrap();let root=base.join("state");
        let (id,bytes)=with_queue(&root,QueueMode::Mcp,"sample",|q| {let id=q.admit_mcp("s","op","task.start",&serde_json::json!({"title":"T","goal":"G","project":"P"}))?;Ok((id,q.get(id)?.unwrap().request))}).unwrap();let path=root.join("producers/mcp/sample/queue.sqlite3");let db=rusqlite::Connection::open(&path).unwrap();db.execute_batch("DROP TABLE repair_sources; PRAGMA user_version=1;").unwrap();drop(db);
        with_queue(&root,QueueMode::Mcp,"sample",|q| {assert_eq!(q.get(id)?.unwrap().request,bytes);assert_eq!(q.admit_mcp("s","op","task.start",&serde_json::json!({"title":"T","goal":"G","project":"P"}))?,id);Ok(())}).unwrap();
    }
    #[test]
    fn mcp_retry_preserves_original_request_and_store_binding() {
        let mut random = [0u8; 16]; getrandom::fill(&mut random).unwrap();
        let suffix: String = random.iter().map(|b| format!("{b:02x}")).collect();
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailQueue-{suffix}"));
        std::fs::create_dir(&base).unwrap(); let root = base.join("state");
        let payload = serde_json::json!({"title":"T", "goal":"G", "project":"P"});
        let (id,request) = with_queue(&root, QueueMode::Mcp, "sample", |q| {
            q.bind_if_unset(&"a".repeat(64))?;
            let id=q.admit_mcp("s","op","task.start",&payload)?; Ok((id,q.get(id)?.unwrap().request))
        }).unwrap();
        with_queue(&root, QueueMode::Mcp, "sample", |q| {
            assert_eq!(q.admit_mcp("s","op","task.start",&payload)?,id);
            assert_eq!(q.get(id)?.unwrap().request,request);
            assert_eq!(q.admit_mcp("s","op","task.start",&serde_json::json!({"title":"changed","goal":"G","project":"P"})),Err("operation_conflict"));
            assert_eq!(q.bind_if_unset(&"b".repeat(64)),Err("store_changed"));
            assert_eq!(q.save_outcome(id,"committed",r#"{"accepted":1,"duplicates":0,"last_seq":1,"results":[{"operation_id":"other","seq":1}]}"#),Err("invalid_request"));
            assert_eq!(q.save_outcome(id,"committed",r#"{"accepted":1,"duplicates":0,"last_seq":1,"results":[{"operation_id":"op","seq":2}]}"#),Err("invalid_request"));
            q.save_outcome(id,"committed",r#"{"accepted":1,"duplicates":0,"last_seq":1,"results":[{"operation_id":"op","seq":1}]}"#)?; Ok(())
        }).unwrap();
        eprintln!("isolated queue fixture retained: {}",base.display());
    }
    #[test]
    fn queue_lock_is_independent_of_gateway_and_other_namespaces() {
        let mut random=[0u8;16]; getrandom::fill(&mut random).unwrap();
        let suffix:String=random.iter().map(|b|format!("{b:02x}")).collect();
        let base=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailQueue-{suffix}")); std::fs::create_dir(&base).unwrap();
        let root=base.join("state"); let gateway=crate::gateway::lifecycle::Gateway::start(&root).unwrap();
        with_queue(&root,QueueMode::Mcp,"sample",|_| {
            assert_eq!(with_queue(&root,QueueMode::Mcp,"sample",|_|Ok(())),Err("queue_busy".into()));
            assert!(with_queue(&root,QueueMode::Explicit,"sample",|_|Ok(())).is_ok());
            assert!(with_queue(&root,QueueMode::Mcp,"other",|_|Ok(())).is_ok()); Ok(())
        }).unwrap();
        assert!(with_queue(&root,QueueMode::Mcp,"sample",|_|Ok(())).is_ok());
        gateway.shutdown().unwrap(); eprintln!("isolated queue fixture retained: {}",base.display());
    }
    #[test]
    fn newer_schema_and_unsafe_links_are_rejected_without_modification() {
        let mut random=[0u8;16]; getrandom::fill(&mut random).unwrap();
        let suffix:String=random.iter().map(|b|format!("{b:02x}")).collect();
        let base=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailQueue-{suffix}")); std::fs::create_dir(&base).unwrap(); let root=base.join("state");
        with_queue(&root,QueueMode::Mcp,"sample",|_|Ok(())).unwrap();
        let db=root.join("producers/mcp/sample/queue.sqlite3");
        let conn=rusqlite::Connection::open(&db).unwrap(); conn.pragma_update(None,"user_version",99).unwrap(); drop(conn);
        let original=std::fs::read(&db).unwrap();
        assert_eq!(with_queue(&root,QueueMode::Mcp,"sample",|_|Ok(())),Err("queue_corrupt".into()));
        assert_eq!(std::fs::read(&db).unwrap(),original);
        let alias=base.join("alias"); std::fs::hard_link(&db,&alias).unwrap();
        assert_eq!(with_queue(&root,QueueMode::Mcp,"sample",|_|Ok(())),Err("queue_unavailable".into()));
        assert_eq!(std::fs::read(&alias).unwrap(),original);
        eprintln!("isolated queue fixture retained: {}",base.display());
    }
    #[test]
    fn corrupt_outcome_does_not_become_a_success() {
        let mut random=[0u8;16]; getrandom::fill(&mut random).unwrap();
        let suffix:String=random.iter().map(|b|format!("{b:02x}")).collect();
        let base=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailQueue-{suffix}")); std::fs::create_dir(&base).unwrap(); let root=base.join("state");
        with_queue(&root,QueueMode::Mcp,"sample",|q| {
            let id=q.admit_mcp("s","op","task.start",&serde_json::json!({"title":"T","goal":"G","project":"P"}))?;
            q.conn.execute("UPDATE entries SET status='committed', outcome=?1 WHERE id=?2",rusqlite::params![r#"{"x":1,"x":2}"#,id]).map_err(super::failure)?;
            assert_eq!(q.get(id),Err("queue_corrupt")); Ok(())
        }).unwrap();
        assert_eq!(with_queue(&root,QueueMode::Mcp,"sample",|_|Ok(())),Err("queue_corrupt".into()));
        eprintln!("isolated queue fixture retained: {}",base.display());
    }
    #[test]
    fn queue_lock_contends_across_processes_and_releases() {
        use std::io::{BufRead, Write};
        if let Ok(suffix)=std::env::var("MINDTRAIL_QUEUE_TEST_ID") {
            assert!(suffix.len()==32 && suffix.bytes().all(|b|b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
            let base=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailQueueProcess-{suffix}"));
            assert_eq!(std::fs::read_to_string(base.join("marker")).unwrap(),suffix);
            with_queue(&base.join("state"),QueueMode::Mcp,"sample",|_| {
                println!("QUEUE_LOCKED"); std::io::stdout().flush().unwrap();
                let mut line=String::new(); std::io::stdin().read_line(&mut line).unwrap(); Ok(())
            }).unwrap(); return;
        }
        let mut random=[0u8;16]; getrandom::fill(&mut random).unwrap(); let suffix:String=random.iter().map(|b|format!("{b:02x}")).collect();
        let base=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailQueueProcess-{suffix}")); std::fs::create_dir(&base).unwrap(); std::fs::write(base.join("marker"),&suffix).unwrap();
        let mut child=std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact","producer::queue::tests::queue_lock_contends_across_processes_and_releases","--nocapture"]).env("MINDTRAIL_QUEUE_TEST_ID",&suffix).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).spawn().unwrap();
        struct Guard(std::process::Child); impl Drop for Guard { fn drop(&mut self) { if self.0.try_wait().ok().flatten().is_none() { let _=self.0.kill(); let _=self.0.wait(); } } }
        let out=child.stdout.take().unwrap(); let mut child=Guard(child); let (tx,rx)=std::sync::mpsc::channel();
        std::thread::spawn(move || for line in std::io::BufReader::new(out).lines().map_while(Result::ok) { let _=tx.send(line); });
        loop { if rx.recv_timeout(std::time::Duration::from_secs(20)).expect("child lock timeout").contains("QUEUE_LOCKED") { break; } }
        assert_eq!(with_queue(&base.join("state"),QueueMode::Mcp,"sample",|_|Ok(())),Err("queue_busy".into()));
        writeln!(child.0.stdin.as_mut().unwrap(),"release").unwrap();
        let deadline=std::time::Instant::now()+std::time::Duration::from_secs(20);
        loop { if let Some(status)=child.0.try_wait().unwrap() { assert!(status.success()); break; } assert!(std::time::Instant::now()<deadline,"child exit timeout"); std::thread::yield_now(); }
        assert!(with_queue(&base.join("state"),QueueMode::Mcp,"sample",|_|Ok(())).is_ok());
        eprintln!("isolated queue fixture retained: {}",base.display());
    }
    #[test]
    fn unsafe_sidecar_links_are_rejected_before_sqlite_opens() {
        let mut random=[0u8;16]; getrandom::fill(&mut random).unwrap(); let suffix:String=random.iter().map(|b|format!("{b:02x}")).collect();
        let base=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailQueue-{suffix}")); std::fs::create_dir(&base).unwrap(); let root=base.join("state");
        with_queue(&root,QueueMode::Explicit,"sample",|_|Ok(())).unwrap();
        let dir=root.join("producers/explicit/sample");
        for (index,name) in ["queue.sqlite3-wal","queue.sqlite3-shm","queue.sqlite3-journal"].iter().enumerate() {
            let path=dir.join(name); let alias=base.join(format!("sentinel-{index}"));
            std::fs::write(&path,b"sentinel").unwrap(); std::fs::hard_link(&path,&alias).unwrap();
            assert_eq!(with_queue(&root,QueueMode::Explicit,"sample",|_|Ok(())),Err("queue_unavailable".into()));
            assert_eq!(std::fs::read(&alias).unwrap(),b"sentinel");
            std::fs::remove_file(&path).unwrap(); std::fs::remove_file(&alias).unwrap();
        }
        assert!(with_queue(&root,QueueMode::Explicit,"sample",|_|Ok(())).is_ok());
        eprintln!("isolated queue fixture retained: {}",base.display());
    }
    #[test]
    fn migration_failure_rolls_back_schema_and_can_retry() {
        let mut random=[0u8;16]; getrandom::fill(&mut random).unwrap(); let suffix:String=random.iter().map(|b|format!("{b:02x}")).collect();
        let base=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailQueue-{suffix}")); std::fs::create_dir(&base).unwrap(); let root=base.join("state");
        crate::gateway::with_producer_queue(&root,"mcp","sample",|path,preflight| {
            assert!(matches!(super::Queue::open_impl(path,QueueMode::Mcp,preflight,true),Err("queue_corrupt")));
            let conn=rusqlite::Connection::open(path).unwrap();
            let version:i64=conn.pragma_query_value(None,"user_version",|r|r.get(0)).unwrap(); assert_eq!(version,0);
            let tables:i64=conn.query_row("SELECT count(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",[],|r|r.get(0)).unwrap(); assert_eq!(tables,0);
        }).unwrap();
        assert!(with_queue(&root,QueueMode::Mcp,"sample",|_|Ok(())).is_ok());
        eprintln!("isolated queue fixture retained: {}",base.display());
    }
    #[test]
    fn unknown_schema_and_wrong_mode_are_rejected() {
        let mut random=[0u8;16]; getrandom::fill(&mut random).unwrap(); let suffix:String=random.iter().map(|b|format!("{b:02x}")).collect();
        let base=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailQueue-{suffix}")); std::fs::create_dir(&base).unwrap(); let root=base.join("state");
        with_queue(&root,QueueMode::Mcp,"sample",|_|Ok(())).unwrap();
        let path=root.join("producers/mcp/sample/queue.sqlite3");
        let conn=rusqlite::Connection::open(&path).unwrap(); conn.execute("UPDATE meta SET mode='explicit'",[]).unwrap(); drop(conn);
        assert_eq!(with_queue(&root,QueueMode::Mcp,"sample",|_|Ok(())),Err("queue_corrupt".into()));
        let conn=rusqlite::Connection::open(&path).unwrap(); let mode:String=conn.query_row("SELECT mode FROM meta",[],|r|r.get(0)).unwrap(); assert_eq!(mode,"explicit");
        conn.pragma_update(None,"user_version",0).unwrap(); drop(conn);
        assert_eq!(with_queue(&root,QueueMode::Mcp,"sample",|_|Ok(())),Err("queue_corrupt".into()));
        let conn=rusqlite::Connection::open(&path).unwrap(); let version:i64=conn.pragma_query_value(None,"user_version",|r|r.get(0)).unwrap(); assert_eq!(version,0);
        let mode:String=conn.query_row("SELECT mode FROM meta",[],|r|r.get(0)).unwrap(); assert_eq!(mode,"explicit");
        eprintln!("isolated queue fixture retained: {}",base.display());
    }
    #[test]
    fn orphaned_event_index_is_detected_on_reopen() {
        let mut random=[0u8;16]; getrandom::fill(&mut random).unwrap(); let suffix:String=random.iter().map(|b|format!("{b:02x}")).collect();
        let base=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailQueue-{suffix}")); std::fs::create_dir(&base).unwrap(); let root=base.join("state");
        with_queue(&root,QueueMode::Explicit,"sample",|q| {
            q.conn.execute("INSERT INTO explicit_events VALUES (?1,?2,?3)",rusqlite::params!["s",1,r#"{"v":1,"session":"s","seq":1,"ts":"2024-01-01T00:00:00Z","type":"task.start","payload":{"title":"T","goal":"G","project":"P"}}"#]).map_err(super::failure)?; Ok(())
        }).unwrap();
        assert_eq!(with_queue(&root,QueueMode::Explicit,"sample",|_|Ok(())),Err("queue_corrupt".into()));
        eprintln!("isolated queue fixture retained: {}",base.display());
    }
    #[test]
    fn malformed_repair_record_is_rejected_on_reopen() {
        let mut random=[0u8;16]; getrandom::fill(&mut random).unwrap(); let suffix:String=random.iter().map(|b|format!("{b:02x}")).collect();
        let base=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailQueue-{suffix}")); std::fs::create_dir(&base).unwrap(); let root=base.join("state");
        let request=br#"[{"v":1,"session":"s","seq":1,"ts":"2024-01-01T00:00:00Z","type":"task.start","payload":{"title":"T","goal":"G","project":"P"}}]"#;
        with_queue(&root,QueueMode::Explicit,"sample",|q| { q.admit_batch("b",request)?; q.conn.execute("INSERT INTO repairs(session,first_seq,end_exclusive,request,status) VALUES ('s',1,3,?1,'pending')",[request.as_slice()]).map_err(super::failure)?; Ok(()) }).unwrap();
        assert_eq!(with_queue(&root,QueueMode::Explicit,"sample",|_|Ok(())),Err("queue_corrupt".into()));
        eprintln!("isolated queue fixture retained: {}",base.display());
    }
    #[test]
    fn invalid_existing_delete_database_is_never_rewritten() {
        let mut random=[0u8;16]; getrandom::fill(&mut random).unwrap(); let suffix:String=random.iter().map(|b|format!("{b:02x}")).collect();
        let base=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailQueue-{suffix}")); std::fs::create_dir(&base).unwrap(); let root=base.join("state");
        for case in ["mode","receipt","index","negative","bytes"] {
            with_queue(&root,QueueMode::Mcp,case,|q| { q.admit_mcp("s","op","task.start",&serde_json::json!({"title":"T","goal":"G","project":"P"}))?; Ok(()) }).unwrap();
            let path=root.join("producers").join("mcp").join(case).join("queue.sqlite3");
            let conn=rusqlite::Connection::open(&path).unwrap(); let journal:String=conn.query_row("PRAGMA journal_mode=DELETE",[],|r|r.get(0)).unwrap(); assert_eq!(journal.to_lowercase(),"delete");
            match case { "mode"=>{conn.execute("UPDATE meta SET mode='explicit'",[]).unwrap();}, "receipt"=>{conn.execute("UPDATE entries SET status='committed',outcome='{}'",[]).unwrap();}, "index"=>{conn.execute("DROP INDEX op_key",[]).unwrap();}, "negative"=>{conn.pragma_update(None,"user_version",-1).unwrap();}, _=>{} } drop(conn);
            if case=="bytes" { std::fs::write(&path,b"not a sqlite database").unwrap(); }
            let before=std::fs::read(&path).unwrap();
            assert_eq!(with_queue(&root,QueueMode::Mcp,case,|_|Ok(())),Err("queue_corrupt".into()));
            assert_eq!(std::fs::read(&path).unwrap(),before,"{case}");
            if case!="bytes" { let conn=rusqlite::Connection::open_with_flags(&path,rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap(); let mode:String=conn.query_row("PRAGMA journal_mode",[],|r|r.get(0)).unwrap(); assert_eq!(mode.to_lowercase(),"delete"); }
        }
        eprintln!("isolated queue fixture retained: {}",base.display());
    }
    #[test]
    fn queue_junction_is_rejected_without_touching_target() {
        let mut random=[0u8;16]; getrandom::fill(&mut random).unwrap(); let suffix:String=random.iter().map(|b|format!("{b:02x}")).collect();
        let base=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailQueue-{suffix}")); std::fs::create_dir(&base).unwrap(); let root=base.join("state");
        with_queue(&root,QueueMode::Mcp,"sample",|_|Ok(())).unwrap();
        let target=base.join("target"); std::fs::create_dir(&target).unwrap(); std::fs::write(target.join("sentinel"),b"unchanged").unwrap();
        let junction=root.join("producers").join("mcp").join("link");
        let output=std::process::Command::new("cmd").args(["/C","mklink","/J"]).arg(&junction).arg(&target).output().unwrap(); assert!(output.status.success(), "junction creation failed: {} {}",String::from_utf8_lossy(&output.stdout),String::from_utf8_lossy(&output.stderr));
        assert_eq!(with_queue(&root,QueueMode::Mcp,"link",|_|Ok(())),Err("queue_unavailable".into()));
        assert_eq!(std::fs::read(target.join("sentinel")).unwrap(),b"unchanged");
        eprintln!("isolated queue fixture retained: {}",base.display());
    }
    #[test]
    fn explicit_tail_conflict_rolls_back_new_index_rows() {
        let mut random=[0u8;16]; getrandom::fill(&mut random).unwrap(); let suffix:String=random.iter().map(|b|format!("{b:02x}")).collect();
        let base=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailQueue-{suffix}")); std::fs::create_dir(&base).unwrap(); let root=base.join("state");
        let original=br#"[{"v":1,"session":"s","seq":1,"ts":"2024-01-01T00:00:00Z","type":"task.start","payload":{"title":"T","goal":"G","project":"P"}},{"v":1,"session":"s","seq":3,"ts":"2024-01-01T00:00:00Z","type":"task.done","payload":{}}]"#;
        let conflicting=br#"[{"v":1,"session":"s","seq":2,"ts":"2024-01-01T00:00:00Z","type":"capability.intent","payload":{"id":"c","title":"C"}},{"v":1,"session":"s","seq":3,"ts":"2024-01-01T00:00:00Z","type":"task.done","payload":{"summary":"changed"}}]"#;
        with_queue(&root,QueueMode::Explicit,"sample",|q| {
            q.admit_batch("original",original)?;
            assert_eq!(q.admit_batch("conflict",conflicting),Err("seq_conflict"));
            assert_eq!(q.explicit_prefix("s",2,3),Err("seq_gap")); Ok(())
        }).unwrap();
        with_queue(&root,QueueMode::Explicit,"sample",|q| { assert_eq!(q.explicit_prefix("s",2,3),Err("seq_gap")); Ok(()) }).unwrap();
        eprintln!("isolated queue fixture retained: {}",base.display());
    }
    #[test]
    fn rejected_head_blocks_later_entries_but_other_session_completes() {
        let mut random=[0u8;16]; getrandom::fill(&mut random).unwrap(); let suffix:String=random.iter().map(|b|format!("{b:02x}")).collect();
        let base=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("MindTrailQueue-{suffix}")); std::fs::create_dir(&base).unwrap(); let root=base.join("state");
        let (a,b)=with_queue(&root,QueueMode::Mcp,"sample",|q| {
            let start=serde_json::json!({"title":"T","goal":"G","project":"P"});
            let a=q.admit_mcp("a","a1","task.start",&start)?;
            q.admit_mcp("a","a2","capability.intent",&serde_json::json!({"id":"c","title":"C"}))?;
            let b=q.admit_mcp("b","b1","task.start",&start)?;
            q.save_outcome(a,"rejected",r#"{"code":"task_closed"}"#)?;
            q.save_outcome(b,"committed",r#"{"accepted":1,"duplicates":0,"last_seq":1,"results":[{"operation_id":"b1","seq":1}]}"#)?;
            Ok((a,b))
        }).unwrap();
        with_queue(&root,QueueMode::Mcp,"sample",|q| { let heads=q.session_heads()?; assert_eq!(heads.len(),1); assert_eq!(heads[0].id,a); assert_eq!(heads[0].status,"rejected"); assert_eq!(q.get_for_caller(a+1),Err("session_blocked")); assert_eq!(q.get_for_caller(b)?.unwrap().status,"committed"); Ok(()) }).unwrap();
        eprintln!("isolated queue fixture retained: {}",base.display());
    }
}

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use std::path::Path;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueMode { Mcp, Explicit }
impl QueueMode { fn label(self) -> &'static str { match self { Self::Mcp => "mcp", Self::Explicit => "explicit" } } }
#[derive(Clone, PartialEq)]
pub struct Entry { pub id: i64, pub session: String, pub request: Vec<u8>, pub status: String, pub outcome: Option<String> }
impl std::fmt::Debug for Entry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Entry").field("id",&self.id).field("session",&self.session).field("request",&"[redacted]").field("status",&self.status).field("outcome",&self.outcome.as_ref().map(|_|"[redacted]")).finish()
    }
}
pub struct Queue { conn: Connection, mode: QueueMode }
fn failure<T>(_: T) -> &'static str { "queue_corrupt" }
pub(super) fn valid_outcome(mode: QueueMode, entry: &Entry, status: &str, value: &serde_json::Value) -> bool {
    let Some(object)=value.as_object() else { return false; };
    let safe = |v: Option<&serde_json::Value>| v.and_then(|n|n.as_u64()).filter(|n|*n<=crate::protocol::MAX_SAFE_INTEGER);
    if status == "rejected" {
        let known=object.get("code").and_then(|v|v.as_str()).is_some_and(|v|matches!(v,"invalid_request"|"payload_too_large"|"invalid_reference"|"node_id_conflict"|"operation_conflict"|"seq_conflict"|"seq_exhausted"|"seq_gap"|"task_already_started"|"task_closed"|"task_not_started"|"write_mode_conflict"))
            && object.keys().all(|k| matches!(k.as_str(),"code"|"index"|"last_seq"|"expected_seq"|"offending_seq"))
            && object.iter().filter(|(k,_)|k.as_str()!="code").all(|(_,v)|safe(Some(v)).is_some());
        if !known{return false}
        if object.get("code").and_then(|v|v.as_str())==Some("seq_gap") {
            let (Some(index),Some(last),Some(expected),Some(offending))=(safe(object.get("index")),safe(object.get("last_seq")),safe(object.get("expected_seq")),safe(object.get("offending_seq"))) else {return false};
            let Ok(batch)=crate::protocol::decode_explicit(&entry.request) else {return false};
            return mode==QueueMode::Explicit && object.len()==5 && last<expected && expected<offending && usize::try_from(index).ok().and_then(|i|batch.items().get(i)).is_some_and(|e|e.seq()==offending);
        }
        return true;
    }
    if status != "committed" { return false; }
    let (Some(accepted),Some(duplicates),Some(last))=(safe(object.get("accepted")),safe(object.get("duplicates")),safe(object.get("last_seq"))) else {return false};
    match mode {
        QueueMode::Explicit => {
            let Ok(batch)=crate::protocol::decode_explicit(&entry.request) else {return false};
            object.len()==3 && accepted.checked_add(duplicates)==Some(batch.items().len() as u64) && last>=batch.items().last().unwrap().seq()
        }
        QueueMode::Mcp => {
            let Ok(batch)=crate::protocol::decode_append(&entry.request) else {return false};
            let Some(results)=object.get("results").and_then(|v|v.as_array()) else {return false};
            object.len()==4 && batch.items().len()==1 && accepted.checked_add(duplicates)==Some(1) && results.len()==1 && results[0].as_object().is_some_and(|r|r.len()==2 && r.get("operation_id").and_then(|v|v.as_str())==Some(batch.items()[0].operation_id()) && safe(r.get("seq")).is_some_and(|n|n>0 && n<=last))
        }
    }
}
impl Queue {
    #[cfg(test)] pub(super) fn inject_storage_failure(&self) {self.conn.execute_batch("DROP TABLE entries").unwrap();}
    pub(super) fn mode(&self) -> QueueMode { self.mode }
    fn open(path: &Path, mode: QueueMode, preflight: &dyn Fn() -> std::io::Result<()>) -> Result<Self, &'static str> {
        Self::open_impl(path,mode,preflight,#[cfg(test)] false)
    }
    fn open_impl(path: &Path, mode: QueueMode, preflight: &dyn Fn() -> std::io::Result<()>, #[cfg(test)] fail_migration: bool) -> Result<Self, &'static str> {
        preflight().map_err(failure)?;
        if path.exists() {
            let conn=Connection::open_with_flags(path,rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(failure)?;
            conn.busy_timeout(std::time::Duration::from_millis(250)).map_err(failure)?;
            let version:i64=conn.pragma_query_value(None,"user_version",|r|r.get(0)).map_err(failure)?;
            let integrity:String=conn.query_row("PRAGMA integrity_check",[],|r|r.get(0)).map_err(failure)?;
            if integrity != "ok" || !(0..=2).contains(&version) { return Err("queue_corrupt"); }
            if version == 0 {
                let count:i64=conn.query_row("SELECT count(*) FROM sqlite_master WHERE type IN ('table','view','trigger') AND name NOT LIKE 'sqlite_%'",[],|r|r.get(0)).map_err(failure)?;
                if count != 0 { return Err("queue_corrupt"); }
            } else {
                let queue=Self {conn,mode}; queue.validate_schema(version)?; queue.validate_metadata()?; queue.validate_data()?;
                if version==1 {let count:i64=queue.conn.query_row("SELECT count(*) FROM repairs",[],|r|r.get(0)).map_err(failure)?;if count!=0{return Err("queue_corrupt")}}
            }
        }
        let mut conn = Connection::open(path).map_err(failure)?;
        conn.busy_timeout(std::time::Duration::from_millis(250)).map_err(failure)?;
        let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0)).map_err(failure)?;
        if !(0..=2).contains(&version) { return Err("queue_corrupt"); }
        if version == 0 {
            let count: i64 = conn.query_row("SELECT count(*) FROM sqlite_master WHERE type IN ('table','view','trigger') AND name NOT LIKE 'sqlite_%'", [], |r| r.get(0)).map_err(failure)?;
            if count != 0 { return Err("queue_corrupt"); }
        }
        conn.pragma_update(None, "foreign_keys", "ON").map_err(failure)?;
        let wal: String = conn.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0)).map_err(failure)?;
        if !wal.eq_ignore_ascii_case("wal") { return Err("queue_corrupt"); }
        conn.pragma_update(None, "synchronous", "FULL").map_err(failure)?;
        let foreign_keys:i64=conn.pragma_query_value(None,"foreign_keys",|r|r.get(0)).map_err(failure)?;
        let synchronous:i64=conn.pragma_query_value(None,"synchronous",|r|r.get(0)).map_err(failure)?;
        if foreign_keys != 1 || synchronous != 2 { return Err("queue_corrupt"); }
        preflight().map_err(failure)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(failure)?;
        let current:i64=tx.pragma_query_value(None,"user_version",|r|r.get(0)).map_err(failure)?;
        if current != version { return Err("queue_corrupt"); }
        if version == 0 {
            tx.execute_batch("CREATE TABLE meta(singleton INTEGER PRIMARY KEY CHECK(singleton=1), mode TEXT NOT NULL CHECK(mode IN ('mcp','explicit')), bound_store_id TEXT); CREATE TABLE entries(id INTEGER PRIMARY KEY, session TEXT NOT NULL, operation_id TEXT, batch_id TEXT, normalized TEXT NOT NULL, request BLOB NOT NULL, status TEXT NOT NULL CHECK(status IN ('pending','committed','rejected')), outcome TEXT, CHECK((operation_id IS NULL) != (batch_id IS NULL)), CHECK((status='pending' AND outcome IS NULL) OR (status IN ('committed','rejected') AND outcome IS NOT NULL))); CREATE UNIQUE INDEX op_key ON entries(session,operation_id) WHERE operation_id IS NOT NULL; CREATE UNIQUE INDEX batch_key ON entries(batch_id) WHERE batch_id IS NOT NULL; CREATE TABLE explicit_events(session TEXT NOT NULL, seq INTEGER NOT NULL, canonical_event TEXT NOT NULL, PRIMARY KEY(session,seq)); CREATE TABLE repairs(id INTEGER PRIMARY KEY, session TEXT NOT NULL, first_seq INTEGER NOT NULL, end_exclusive INTEGER NOT NULL, request BLOB NOT NULL, status TEXT NOT NULL CHECK(status IN ('pending','committed','rejected'))); PRAGMA user_version=1;").map_err(failure)?;
            tx.execute("INSERT INTO meta(singleton,mode) VALUES (1,?1)", [mode.label()]).map_err(failure)?;
        }
        if version<=1 {
            let count:i64=tx.query_row("SELECT count(*) FROM repairs",[],|r|r.get(0)).map_err(failure)?;if count!=0{return Err("queue_corrupt")}
            tx.execute_batch("CREATE TABLE repair_sources(repair_id INTEGER PRIMARY KEY REFERENCES repairs(id), entry_id INTEGER NOT NULL REFERENCES entries(id), offending_seq INTEGER NOT NULL); PRAGMA user_version=2;").map_err(failure)?;
        }
        let modes: Vec<String> = { let mut stmt=tx.prepare("SELECT mode FROM meta").map_err(failure)?; let rows=stmt.query_map([], |r| r.get(0)).map_err(failure)?; rows.collect::<Result<_,_>>().map_err(failure)? };
        if modes != [mode.label()] { return Err("queue_corrupt"); }
        let bound:Option<String>=tx.query_row("SELECT bound_store_id FROM meta",[],|r|r.get(0)).map_err(failure)?;
        if bound.as_ref().is_some_and(|id|id.len()!=64 || !id.bytes().all(|b|b.is_ascii_digit() || (b'a'..=b'f').contains(&b))) { return Err("queue_corrupt"); }
        #[cfg(test)]
        if fail_migration { return Err("queue_corrupt"); }
        tx.commit().map_err(failure)?;
        let queue=Self { conn, mode };
        queue.validate_data()?;
        Ok(queue)
    }
    fn validate_metadata(&self) -> Result<(), &'static str> {
        let modes: Vec<String> = { let mut stmt=self.conn.prepare("SELECT mode FROM meta").map_err(failure)?; let rows=stmt.query_map([],|r|r.get(0)).map_err(failure)?; rows.collect::<Result<_,_>>().map_err(failure)? };
        if modes != [self.mode.label()] { return Err("queue_corrupt"); }
        let bound:Option<String>=self.conn.query_row("SELECT bound_store_id FROM meta",[],|r|r.get(0)).map_err(failure)?;
        if bound.as_ref().is_some_and(|id|id.len()!=64 || !id.bytes().all(|b|b.is_ascii_digit() || (b'a'..=b'f').contains(&b))) { return Err("queue_corrupt"); }
        Ok(())
    }
    fn validate_schema(&self,version:i64) -> Result<(), &'static str> {
        const EXPECTED: [(&str,&str);6] = [
            ("meta", "CREATE TABLE meta(singleton INTEGER PRIMARY KEY CHECK(singleton=1), mode TEXT NOT NULL CHECK(mode IN ('mcp','explicit')), bound_store_id TEXT)"),
            ("entries", "CREATE TABLE entries(id INTEGER PRIMARY KEY, session TEXT NOT NULL, operation_id TEXT, batch_id TEXT, normalized TEXT NOT NULL, request BLOB NOT NULL, status TEXT NOT NULL CHECK(status IN ('pending','committed','rejected')), outcome TEXT, CHECK((operation_id IS NULL) != (batch_id IS NULL)), CHECK((status='pending' AND outcome IS NULL) OR (status IN ('committed','rejected') AND outcome IS NOT NULL)))"),
            ("op_key", "CREATE UNIQUE INDEX op_key ON entries(session,operation_id) WHERE operation_id IS NOT NULL"),
            ("batch_key", "CREATE UNIQUE INDEX batch_key ON entries(batch_id) WHERE batch_id IS NOT NULL"),
            ("explicit_events", "CREATE TABLE explicit_events(session TEXT NOT NULL, seq INTEGER NOT NULL, canonical_event TEXT NOT NULL, PRIMARY KEY(session,seq))"),
            ("repairs", "CREATE TABLE repairs(id INTEGER PRIMARY KEY, session TEXT NOT NULL, first_seq INTEGER NOT NULL, end_exclusive INTEGER NOT NULL, request BLOB NOT NULL, status TEXT NOT NULL CHECK(status IN ('pending','committed','rejected')))"),
        ];
        for (name,expected) in EXPECTED {
            let actual:Option<String>=self.conn.query_row("SELECT sql FROM sqlite_master WHERE name=?1",[name],|r|r.get(0)).optional().map_err(failure)?;
            if actual.as_deref()!=Some(expected) { return Err("queue_corrupt"); }
        }
        let count:i64=self.conn.query_row("SELECT count(*) FROM sqlite_master WHERE name NOT LIKE 'sqlite_%' AND type IN ('table','index','trigger','view')",[],|r|r.get(0)).map_err(failure)?;
        if count!=if version==2 {7}else{6} { return Err("queue_corrupt"); }
        if version==2 {let sql:Option<String>=self.conn.query_row("SELECT sql FROM sqlite_master WHERE name='repair_sources'",[],|r|r.get(0)).optional().map_err(failure)?;if sql.as_deref()!=Some("CREATE TABLE repair_sources(repair_id INTEGER PRIMARY KEY REFERENCES repairs(id), entry_id INTEGER NOT NULL REFERENCES entries(id), offending_seq INTEGER NOT NULL)"){return Err("queue_corrupt")}}
        Ok(())
    }
    fn validate_data(&self) -> Result<(), &'static str> {
        let ids: Vec<i64> = { let mut stmt=self.conn.prepare("SELECT id FROM entries ORDER BY id").map_err(failure)?; let rows=stmt.query_map([],|r|r.get(0)).map_err(failure)?; rows.collect::<Result<_,_>>().map_err(failure)? };
        let mut expected = std::collections::BTreeMap::new();
        for id in ids {
            let entry=self.get(id)?.ok_or("queue_corrupt")?;
            if self.mode==QueueMode::Explicit {
                let batch=crate::protocol::decode_explicit(&entry.request).map_err(failure)?;
                for event in batch.items() {
                    let key=(event.session().to_owned(),event.seq());
                    if let Some(old)=expected.insert(key,event.value().clone()) { if old != *event.value() { return Err("queue_corrupt"); } }
                }
            }
        }
        let mut stmt=self.conn.prepare("SELECT session,seq,canonical_event FROM explicit_events ORDER BY session,seq").map_err(failure)?;
        let rows=stmt.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,u64>(1)?,r.get::<_,String>(2)?))).map_err(failure)?;
        let mut count=0usize;
        for row in rows { let (session,seq,json)=row.map_err(failure)?; let event=crate::protocol::decode_stored_event(json.as_bytes()).map_err(failure)?; if event.session()!=session || event.seq()!=seq || expected.get(&(session,seq))!=Some(event.value()) { return Err("queue_corrupt"); } count+=1; }
        if count != expected.len() { return Err("queue_corrupt"); }
        drop(stmt);
        let mut stmt=self.conn.prepare("SELECT id,session,first_seq,end_exclusive,request,status FROM repairs ORDER BY id").map_err(failure)?;
        let rows=stmt.query_map([],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,u64>(2)?,r.get::<_,u64>(3)?,r.get::<_,Vec<u8>>(4)?,r.get::<_,String>(5)?))).map_err(failure)?;
        for row in rows {
            let (id,session,first,end,request,status)=row.map_err(failure)?;
            if self.mode != QueueMode::Explicit || !matches!(status.as_str(),"pending"|"committed"|"rejected") || first==0 || end<=first || end>crate::protocol::MAX_SAFE_INTEGER+1 { return Err("queue_corrupt"); }
            let batch=crate::protocol::decode_explicit(&request).map_err(failure)?;
            if batch.items().len() as u64 != end-first || batch.items().iter().enumerate().any(|(i,e)|e.session()!=session || e.seq()!=first+i as u64 || expected.get(&(session.clone(),e.seq()))!=Some(e.value())) { return Err("queue_corrupt"); }
            let version:i64=self.conn.pragma_query_value(None,"user_version",|r|r.get(0)).map_err(failure)?;
            if version==2 {let source:Option<(i64,u64)>=self.conn.query_row("SELECT entry_id,offending_seq FROM repair_sources WHERE repair_id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(failure)?;let (source,offending)=source.ok_or("queue_corrupt")?;let source=self.get(source)?.ok_or("queue_corrupt")?;let batch=crate::protocol::decode_explicit(&source.request).map_err(failure)?;if source.session!=session || offending<end || !batch.items().iter().any(|e|e.seq()==offending){return Err("queue_corrupt")}}
        }
        drop(stmt);
        let version:i64=self.conn.pragma_query_value(None,"user_version",|r|r.get(0)).map_err(failure)?;
        if version==2 {
            let invalid:bool=self.conn.query_row("SELECT EXISTS(SELECT 1 FROM repairs r LEFT JOIN repair_sources s ON s.repair_id=r.id LEFT JOIN entries e ON e.id=s.entry_id WHERE s.repair_id IS NULL OR e.id IS NULL OR e.session!=r.session OR e.batch_id IS NULL) OR EXISTS(SELECT 1 FROM repair_sources s LEFT JOIN repairs r ON r.id=s.repair_id WHERE r.id IS NULL)",[],|r|r.get(0)).map_err(failure)?;
            if invalid{return Err("queue_corrupt")}
        }
        Ok(())
    }
    pub fn admit_batch(&mut self, batch_id: &str, bytes: &[u8]) -> Result<i64, &'static str> {
        if self.mode != QueueMode::Explicit || batch_id.trim().is_empty() { return Err("invalid_request"); }
        let batch = crate::protocol::decode_explicit(bytes).map_err(|_| "invalid_request")?;
        let normalized = serde_json::to_value(batch.items().iter().map(|x|x.value()).collect::<Vec<_>>()).map_err(failure)?;
        let session = batch.items()[0].session();
        let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(failure)?;
        let old: Option<(i64,String)> = tx.query_row("SELECT id,normalized FROM entries WHERE batch_id=?1", [batch_id], |r| Ok((r.get(0)?,r.get(1)?))).optional().map_err(failure)?;
        if let Some((id, value)) = old { let old=crate::protocol::raw::parse_strict_value(value.as_bytes(), 2*1024*1024).map_err(failure)?; if old != normalized { return Err("batch_conflict"); } drop(tx); self.get(id)?.ok_or("queue_corrupt")?; return Ok(id); }
        for event in batch.items() {
            let old: Option<String> = tx.query_row("SELECT canonical_event FROM explicit_events WHERE session=?1 AND seq=?2", params![session,event.seq()], |r|r.get(0)).optional().map_err(failure)?;
            if let Some(value)=old { let stored=crate::protocol::decode_stored_event(value.as_bytes()).map_err(failure)?; if stored.session()!=session || stored.seq()!=event.seq() { return Err("queue_corrupt"); } if stored.value() != event.value() { return Err("seq_conflict"); } }
            else { tx.execute("INSERT INTO explicit_events VALUES (?1,?2,?3)",params![session,event.seq(),serde_json::to_string(event.value()).map_err(failure)?]).map_err(failure)?; }
        }
        tx.execute("INSERT INTO entries(session,batch_id,normalized,request,status) VALUES (?1,?2,?3,?4,'pending')",params![session,batch_id,normalized.to_string(),bytes]).map_err(failure)?;
        let id=tx.last_insert_rowid();#[cfg(feature="producer-test-fixture")] super::faults::barrier("before_local_commit");tx.commit().map_err(failure)?;#[cfg(feature="producer-test-fixture")] super::faults::barrier("after_local_commit");Ok(id)
    }
    pub fn admit_mcp(&mut self, session: &str, operation_id: &str, kind: &str, payload: &serde_json::Value) -> Result<i64, &'static str> {
        if self.mode != QueueMode::Mcp || session.trim().is_empty() || operation_id.trim().is_empty() { return Err("invalid_request"); }
        let candidate = serde_json::json!({"v":1,"session":session,"operation_id":operation_id,"ts":"2000-01-01T00:00:00Z","type":kind,"payload":payload});
        let candidate_bytes = serde_json::to_vec(&vec![candidate]).map_err(failure)?;
        let batch = crate::protocol::decode_append(&candidate_bytes).map_err(|_| "invalid_request")?;
        let item = &batch.items()[0];
        let normalized = serde_json::json!({"type":item.kind(),"payload":item.payload()});
        let tx=self.conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(failure)?;
        let old:Option<(i64,String)>=tx.query_row("SELECT id,normalized FROM entries WHERE session=?1 AND operation_id=?2",params![session,operation_id],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(failure)?;
        if let Some((id,value))=old { let old=crate::protocol::raw::parse_strict_value(value.as_bytes(), 2*1024*1024).map_err(failure)?; if old!=normalized { return Err("operation_conflict"); } drop(tx); self.get(id)?.ok_or("queue_corrupt")?; return Ok(id); }
        let timestamp = time::OffsetDateTime::now_utc().format(&time::format_description::well_known::Rfc3339).map_err(failure)?;
        let event = serde_json::json!({"v":1,"session":session,"operation_id":operation_id,"ts":timestamp,"type":kind,"payload":item.payload()});
        let bytes = serde_json::to_vec(&vec![event]).map_err(failure)?;
        crate::protocol::decode_append(&bytes).map_err(failure)?;
        tx.execute("INSERT INTO entries(session,operation_id,normalized,request,status) VALUES (?1,?2,?3,?4,'pending')",params![session,operation_id,normalized.to_string(),bytes]).map_err(failure)?;
        let id=tx.last_insert_rowid();#[cfg(feature="producer-test-fixture")] super::faults::barrier("before_local_commit");tx.commit().map_err(failure)?;#[cfg(feature="producer-test-fixture")] super::faults::barrier("after_local_commit");Ok(id)
    }
    pub fn bind_if_unset(&mut self, store_id: &str) -> Result<(), &'static str> {
        if store_id.len()!=64 || !store_id.bytes().all(|b|b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) { return Err("invalid_request"); }
        let tx=self.conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(failure)?;
        let bound:Option<String>=tx.query_row("SELECT bound_store_id FROM meta",[],|r|r.get(0)).map_err(failure)?;
        if bound.as_ref().is_some_and(|id| id.len()!=64 || !id.bytes().all(|b|b.is_ascii_digit() || (b'a'..=b'f').contains(&b))) { return Err("queue_corrupt"); }
        if bound.as_deref().is_some_and(|id|id!=store_id) { return Err("store_changed"); }
        if bound.is_none() { tx.execute("UPDATE meta SET bound_store_id=?1",[store_id]).map_err(failure)?; }
        tx.commit().map_err(failure)
    }
    pub fn get(&self,id:i64) -> Result<Option<Entry>, &'static str> {
        let row: Option<(Entry,Option<String>,Option<String>,String)> = self.conn.query_row("SELECT id,session,request,status,outcome,operation_id,batch_id,normalized FROM entries WHERE id=?1",[id], |r|Ok((Entry {id:r.get(0)?,session:r.get(1)?,request:r.get(2)?,status:r.get(3)?,outcome:r.get(4)?},r.get(5)?,r.get(6)?,r.get(7)?))).optional().map_err(failure)?;
        if let Some((entry,operation_id,batch_id,normalized)) = &row {
            if entry.id <= 0 || entry.session.trim().is_empty() { return Err("queue_corrupt"); }
            if !matches!((&*entry.status, &entry.outcome), ("pending", None) | ("committed", Some(_)) | ("rejected", Some(_))) { return Err("queue_corrupt"); }
            if let Some(outcome) = &entry.outcome { let value=crate::protocol::raw::parse_strict_value(outcome.as_bytes(), 65536).map_err(failure)?; if !valid_outcome(self.mode,entry,&entry.status,&value) { return Err("queue_corrupt"); } }
            let stored=crate::protocol::raw::parse_strict_value(normalized.as_bytes(),2*1024*1024).map_err(failure)?;
            let (session, expected) = match self.mode {
                QueueMode::Mcp => { let b=crate::protocol::decode_append(&entry.request).map_err(failure)?; if b.items().len()!=1 || batch_id.is_some() || operation_id.as_deref()!=Some(b.items()[0].operation_id()) { return Err("queue_corrupt"); } let item=&b.items()[0]; (item.session().to_owned(),serde_json::json!({"type":item.kind(),"payload":item.payload()})) },
                QueueMode::Explicit => { let b=crate::protocol::decode_explicit(&entry.request).map_err(failure)?; if operation_id.is_some() || !batch_id.as_ref().is_some_and(|v|!v.trim().is_empty()) { return Err("queue_corrupt"); } for event in b.items() { let saved:Option<String>=self.conn.query_row("SELECT canonical_event FROM explicit_events WHERE session=?1 AND seq=?2",params![event.session(),event.seq()],|r|r.get(0)).optional().map_err(failure)?; let saved=saved.ok_or("queue_corrupt")?; if crate::protocol::decode_stored_event(saved.as_bytes()).map_err(failure)?.value()!=event.value() { return Err("queue_corrupt"); } } (b.items()[0].session().to_owned(),serde_json::to_value(b.items().iter().map(|v|v.value()).collect::<Vec<_>>()).map_err(failure)?) },
            };
            if session != entry.session || stored != expected { return Err("queue_corrupt"); }
        }
        Ok(row.map(|(entry,_,_,_)|entry))
    }
    pub(super) fn get_for_caller(&self,id:i64) -> Result<Option<Entry>, &'static str> {
        let entry=self.get(id)?;
        if let Some(entry)=&entry {
            if entry.status=="pending" {
                let blocked:bool=self.conn.query_row("SELECT EXISTS(SELECT 1 FROM entries WHERE session=?1 AND id<?2 AND status='rejected')",params![entry.session,id],|r|r.get(0)).map_err(failure)?;
                if blocked {return Err("session_blocked");}
            }
        }
        Ok(entry)
    }
    pub fn save_outcome(&mut self, id: i64, status: &str, outcome: &str) -> Result<(), &'static str> {
        if !matches!(status, "committed" | "rejected") { return Err("invalid_request"); }
        let value=crate::protocol::raw::parse_strict_value(outcome.as_bytes(), 65536).map_err(|_| "invalid_request")?;
        let entry=self.get(id)?.ok_or("not_found")?;
        if !valid_outcome(self.mode,&entry,status,&value) { return Err("invalid_request"); }
        let tx=self.conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(failure)?;
        let old: Option<(String,Option<String>)>=tx.query_row("SELECT status,outcome FROM entries WHERE id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(failure)?;
        match old { None => return Err("not_found"), Some((s, o)) if s != "pending" => { if s==status && o.as_deref().map(|v|crate::protocol::raw::parse_strict_value(v.as_bytes(),65536)).transpose().map_err(failure)? == Some(value) { return Ok(()); } return Err("outcome_conflict"); }, _ => {} }
        tx.execute("UPDATE entries SET status=?1,outcome=?2 WHERE id=?3",params![status,outcome,id]).map_err(failure)?; tx.commit().map_err(failure)
    }
    pub fn session_heads(&self) -> Result<Vec<Entry>, &'static str> {
        let mut stmt=self.conn.prepare("SELECT min(id) FROM entries WHERE status != 'committed' GROUP BY session ORDER BY min(id)").map_err(failure)?;
        let ids=stmt.query_map([],|r|r.get(0)).map_err(failure)?.collect::<Result<Vec<i64>,_>>().map_err(failure)?;
        ids.into_iter().map(|id|self.get(id)?.ok_or("queue_corrupt")).collect()
    }
    pub fn explicit_prefix(&self, session: &str, first: u64, end_exclusive: u64) -> Result<Vec<serde_json::Value>, &'static str> {
        if self.mode != QueueMode::Explicit || first==0 || first>crate::protocol::MAX_SAFE_INTEGER || end_exclusive < first || end_exclusive > crate::protocol::MAX_SAFE_INTEGER + 1 { return Err("invalid_request"); }
        let mut stmt=self.conn.prepare("SELECT seq,canonical_event FROM explicit_events WHERE session=?1 AND seq>=?2 AND seq<?3 ORDER BY seq").map_err(failure)?;
        let rows=stmt.query_map(params![session,first,end_exclusive],|r|Ok((r.get::<_,u64>(0)?,r.get::<_,String>(1)?))).map_err(failure)?;
        let mut result=Vec::new(); let mut expected=first;
        for row in rows { let (seq,value)=row.map_err(failure)?; if seq!=expected { return Err("seq_gap"); } let event=crate::protocol::decode_stored_event(value.as_bytes()).map_err(failure)?; if event.session()!=session || event.seq()!=seq { return Err("queue_corrupt"); } result.push(event.value().clone()); expected+=1; }
        if expected != end_exclusive { return Err("seq_gap"); } Ok(result)
    }
    /// Prepare repair chunks only after proving the entire missing prefix exists.
    /// A rejected gap remains durable until this transaction installs every chunk.
    pub(super) fn prepare_gap(&mut self,id:i64)->Result<bool,&'static str> {
        let entry=self.get(id)?.ok_or("queue_corrupt")?;
        if self.mode!=QueueMode::Explicit || entry.status!="rejected" {return Ok(false)}
        let gap:serde_json::Value=serde_json::from_str(entry.outcome.as_deref().ok_or("queue_corrupt")?).map_err(failure)?;
        if gap["code"]!="seq_gap" {return Ok(false)}
        let last=gap["last_seq"].as_u64().ok_or("queue_corrupt")?;let first=last.checked_add(1).ok_or("queue_corrupt")?;
        let expected_seq=gap["expected_seq"].as_u64().ok_or("queue_corrupt")?;let end=gap["offending_seq"].as_u64().ok_or("queue_corrupt")?;let index=gap["index"].as_u64().and_then(|n|usize::try_from(n).ok()).ok_or("queue_corrupt")?;
        let original=crate::protocol::decode_explicit(&entry.request).map_err(failure)?;
        if !(last<expected_seq && expected_seq<end) || original.items().get(index).is_none_or(|event|event.seq()!=end) {return Err("queue_corrupt")}
        let mut chunks:Vec<(u64,u64,Vec<u8>)>=Vec::new();let mut values=Vec::new();let mut expected=first;
        {
            let mut stmt=self.conn.prepare("SELECT seq,canonical_event FROM explicit_events WHERE session=?1 AND seq>=?2 AND seq<?3 ORDER BY seq").map_err(failure)?;
            let rows=stmt.query_map(params![entry.session,first,end],|r|Ok((r.get::<_,u64>(0)?,r.get::<_,String>(1)?))).map_err(failure)?;
            for row in rows {let (seq,text)=row.map_err(failure)?;if seq!=expected{return Ok(false)}let event=crate::protocol::decode_stored_event(text.as_bytes()).map_err(failure)?;if event.session()!=entry.session || event.seq()!=seq{return Err("queue_corrupt")}
                let value=event.value().clone();let mut candidate=values.clone();candidate.push(value.clone());let encoded=serde_json::to_vec(&candidate).map_err(failure)?;
                if candidate.len()>crate::protocol::MAX_BATCH_EVENTS || encoded.len()>crate::protocol::MAX_BODY_BYTES {
                    if values.is_empty(){return Err("queue_corrupt")}let bytes=serde_json::to_vec(&values).map_err(failure)?;chunks.push((seq-values.len() as u64,seq,bytes));values.clear();
                }
                values.push(value);expected=expected.checked_add(1).ok_or("queue_corrupt")?;
            }
        }
        if expected!=end {return Ok(false)}
        if !values.is_empty() {chunks.push((end-values.len() as u64,end,serde_json::to_vec(&values).map_err(failure)?));}
        let tx=self.conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(failure)?;
        for (start,stop,bytes) in chunks {crate::protocol::decode_explicit(&bytes).map_err(failure)?;tx.execute("INSERT INTO repairs(session,first_seq,end_exclusive,request,status) VALUES (?1,?2,?3,?4,'pending')",params![entry.session,start,stop,bytes]).map_err(failure)?;let repair_id=tx.last_insert_rowid();tx.execute("INSERT INTO repair_sources(repair_id,entry_id,offending_seq) VALUES (?1,?2,?3)",params![repair_id,id,end]).map_err(failure)?;}
        tx.execute("UPDATE entries SET status='pending',outcome=NULL WHERE id=?1 AND status='rejected'",[id]).map_err(failure)?;tx.commit().map_err(failure)?;Ok(true)
    }
    pub(super) fn next_repair(&self,session:&str)->Result<Option<(i64,Entry)>, &'static str> {
        let row:Option<(i64,Vec<u8>,String)>=self.conn.query_row("SELECT id,request,status FROM repairs WHERE session=?1 AND status!='committed' ORDER BY id LIMIT 1",[session],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(failure)?;
        match row {None=>Ok(None),Some((_,_,status)) if status=="rejected"=>Err("session_blocked"),Some((id,request,_))=>Ok(Some((id,Entry{id,session:session.into(),request,status:"pending".into(),outcome:None})))}
    }
    pub(super) fn finish_repair(&mut self,id:i64,status:&str)->Result<(), &'static str> {
        if !matches!(status,"committed"|"rejected"){return Err("invalid_request")}
        if self.conn.execute("UPDATE repairs SET status=?1 WHERE id=?2 AND status='pending'",params![status,id]).map_err(failure)?!=1{return Err("queue_corrupt")}Ok(())
    }
    pub(super) fn explicit_counts(&self)->Result<(u64,u64,u64), &'static str> {
        let mut counts=(0,0,0);let mut blocked=std::collections::HashSet::new();
        let mut stmt=self.conn.prepare("SELECT session,status FROM entries ORDER BY id").map_err(failure)?;
        let rows=stmt.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?))).map_err(failure)?;
        for row in rows {let (session,status)=row.map_err(failure)?;let repair_blocked:bool=self.conn.query_row("SELECT EXISTS(SELECT 1 FROM repairs WHERE session=?1 AND status='rejected')",[&session],|r|r.get(0)).map_err(failure)?;match status.as_str(){"committed"=>counts.0+=1,"rejected"=>{counts.2+=1;blocked.insert(session);},"pending" if blocked.contains(&session)||repair_blocked=>counts.2+=1,"pending"=>counts.1+=1,_=>return Err("queue_corrupt")}}
        Ok(counts)
    }
}
/// Call only inside this callback; the private directory lock remains held throughout.
pub(crate) fn with_queue<T>(root: &Path, mode: QueueMode, name: &str, f: impl FnOnce(&mut Queue) -> Result<T, &'static str>) -> Result<T, String> {
    crate::gateway::with_producer_queue(root,mode.label(),name,|path,preflight| {
        #[cfg(feature="producer-test-fixture")]
        let mut queue={
            let path_failed=std::cell::Cell::new(false);
            let observed=||preflight().map_err(|e| {path_failed.set(true);super::faults::mark_path_error(&e);e});
            Queue::open(path,mode,&observed).map_err(|e| {if !path_failed.get() {super::faults::process_marker("open-storage");}e})?
        };
        #[cfg(not(feature="producer-test-fixture"))]
        let mut queue=Queue::open(path,mode,preflight)?;
        f(&mut queue)
    }).map_err(|error| {#[cfg(feature="producer-test-fixture")] eprintln!("queue fixture I/O: {error}");if error.kind()==std::io::ErrorKind::WouldBlock { "queue_busy".to_owned() } else { "queue_unavailable".to_owned() }})?.map_err(str::to_owned)
}




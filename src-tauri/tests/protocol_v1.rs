use mindtrail_lib::protocol::{decode_append, decode_explicit, MAX_BODY_BYTES};
const EVENT: &str = r#"{"v":1,"session":"s","seq":1,"ts":"2024-02-29T00:00:00Z","type":"task.done","payload":{}}"#;
fn batch(event: &str) -> String { format!("[{event}]") }
#[test] fn fixture_and_append() {
 let b=decode_explicit(include_bytes!("../../protocol/fixtures/v1-events.json")).unwrap();
 assert_eq!(b.items().len(),7); assert_eq!(b.items()[0].ts(),"2026-11-13T10:00:00+08:00");
 let a=decode_append(br#"[{"v":1,"session":"s","operation_id":"o","ts":"2024-02-29T00:00:00Z","type":"task.done","payload":{}}]"#).unwrap();
 assert_eq!(a.items()[0].operation_id(),"o"); assert_eq!(a.items()[0].with_seq(1).unwrap()["seq"],1);
}
#[test] fn rejects_invalid_shapes_and_fields() {
 let cases=vec!["{}".into(),"[]".into(),"[{}] trailing".into(),"[{]".into(),format!("[{}]",vec![EVENT;201].join(",")),batch(&EVENT.replace("\"v\":1","\"v\":2")),batch(&EVENT.replace("task.done","unknown")),batch(&EVENT.replace("\"session\":\"s\"","\"session\":\"  \"")),batch(&EVENT.replace("\"payload\":{}","\"payload\":{\"summary\":null}")),batch(&EVENT.replace("\"payload\":{}","\"payload\":{\"extra\":1}")),batch(&EVENT.replace("\"seq\":1,","")),batch(&EVENT.replace("2024-02-29T00:00:00Z","2023-02-29T00:00:00Z")),batch(&EVENT.replace("2024-02-29T00:00:00Z","2024-02-29T00:00:00")),batch(&EVENT.replace("\"seq\":1","\"seq\":9007199254740992")),format!("[{EVENT},{EVENT}]")];
 for s in cases { assert_eq!(decode_explicit(s.as_bytes()).unwrap_err().status(),400,"{s}"); }
}
#[test] fn duplicate_keys_and_limits() {
 for s in [EVENT.replace("\"v\":1", "\"v\":1,\"v\":1"), EVENT.replace("\"payload\":{}", "\"payload\":{\"summary\":\"x\",\"summary\":\"y\"}"), EVENT.replace("\"type\":\"task.done\",\"payload\":{}", "\"type\":\"file.change\",\"payload\":{\"id\":\"x\",\"path\":\"p\",\"action\":\"create\",\"stats\":{\"add\":1,\"add\":2,\"del\":0}}") ] { let e=decode_explicit(batch(&s).as_bytes()).unwrap_err(); assert_eq!((e.status(),e.index),(400,Some(0))); }
 let mut b=batch(EVENT).into_bytes(); b.resize(MAX_BODY_BYTES,b' '); assert!(decode_explicit(&b).is_ok()); b.push(b' '); assert_eq!(decode_explicit(&b).unwrap_err().status(),413);
}
#[test] fn exact_numeric_values_and_identifiers() {
 for n in ["1.0000000000000001", "9007199254740991.1", "1e-999"] { let s=batch(&EVENT.replace("\"seq\":1",&format!("\"seq\":{n}"))); assert!(decode_explicit(s.as_bytes()).is_err(),"{n}"); }
 for n in ["1.0", "1e0", "100e-2"] { let s=batch(&EVENT.replace("\"seq\":1",&format!("\"seq\":{n}"))); assert_eq!(decode_explicit(s.as_bytes()).unwrap().items()[0].seq(),1); }
 let s=EVENT.replace("\"type\":\"task.done\",\"payload\":{}", "\"type\":\"api.expose\",\"payload\":{\"id\":\"x\",\"capabilityId\":\"  \",\"kind\":\"fn\",\"signature\":\"f()\"}"); assert!(decode_explicit(batch(&s).as_bytes()).is_err());
}
#[test] fn objects_cannot_impersonate_numbers_and_zero_is_exact() {
 for token in [r#"{"$serde_json::private::Number":"1"}"#,r#"{"x":0,"\u0024serde_json::private::Number":"1"}"#] {
  for field in ["seq", "v"] { let s=batch(&EVENT.replace(&format!("\"{field}\":1"),&format!("\"{field}\":{token}"))); assert!(decode_explicit(s.as_bytes()).is_err(),"{s}"); }
  let s=EVENT.replace("\"type\":\"task.done\",\"payload\":{}",&format!("\"type\":\"file.change\",\"payload\":{{\"id\":\"x\",\"path\":\"p\",\"action\":\"create\",\"stats\":{{\"add\":{token},\"del\":0}}}}")); assert!(decode_explicit(batch(&s).as_bytes()).is_err());
 }
 for n in ["-0.0","0e2147483648","0e-2147483649"] { let s=EVENT.replace("\"type\":\"task.done\",\"payload\":{}",&format!("\"type\":\"file.change\",\"payload\":{{\"id\":\"x\",\"path\":\"p\",\"action\":\"create\",\"stats\":{{\"add\":{n},\"del\":0}}}}")); assert_eq!(decode_explicit(batch(&s).as_bytes()).unwrap().items()[0].payload()["stats"]["add"],0); }
}
#[test] fn batch_rules_and_schema() {
 let invalid=[EVENT.replace("\"payload\":{}","\"payload\":{},\"extra\":0"), EVENT.replace("task.done\",\"payload\":{}","task.start\",\"payload\":{}"), EVENT.replace("task.done\",\"payload\":{}","call.edge\",\"payload\":{\"from\":\"a\",\"to\":\"b\",\"kind\":\"bad\"}"), EVENT.replace("00:00:00Z","00:00:61Z"), EVENT.replace("00:00:00Z","00:00:00+24:00")];
 for s in invalid { assert_eq!(decode_explicit(batch(&s).as_bytes()).unwrap_err().index,Some(0)); }
 for second in [EVENT.replace("\"session\":\"s\"","\"session\":\"S\"").replace("\"seq\":1","\"seq\":2"), EVENT.replace("\"seq\":1","\"seq\":0")] { let s=format!("[{EVENT},{second}]"); assert_eq!(decode_explicit(s.as_bytes()).unwrap_err().index,Some(1)); }
 let e=decode_explicit(batch(&EVENT.replace("\"seq\":1","\"seq\":9007199254740991")).as_bytes()).unwrap(); assert_eq!(e.items()[0].seq(),9007199254740991);
 let a=r#"{"v":1,"session":"s","operation_id":"x","ts":"2024-02-29T00:00:00Z","type":"task.done","payload":{}}"#; let err=decode_append(format!("[{a},{a}]").as_bytes()).unwrap_err(); assert_eq!((err.code,err.status(),err.index,err.last_seq,err.expected_seq,err.offending_seq),("invalid_request",400,Some(1),None,None,None));
 let v=decode_append(batch(a).as_bytes()).unwrap().items()[0].with_seq(1).unwrap(); assert!(v.get("operation_id").is_none());
 let s=format!("[{},{}]",EVENT,EVENT.replace("\"seq\":1","\"seq\":2").replace("00:00:00Z","01:00:00+01:00")); let b=decode_explicit(s.as_bytes()).unwrap(); assert_ne!(b.items()[0].ts(),b.items()[1].ts());
}
#[test] fn nesting_limit_is_bounded() {
 let deep = format!("[{}0{}]", "[".repeat(1000), "]".repeat(1000));
 let err = mindtrail_lib::protocol::raw::parse_batch(deep.as_bytes()).unwrap_err();
 assert_eq!((err.status,err.index),(400,Some(0)));
 let normal = format!("[{}0{}]", "[".repeat(20), "]".repeat(20));
 assert!(mindtrail_lib::protocol::raw::parse_batch(normal.as_bytes()).is_ok());
 let repeated = format!("[{}{{\"x\":1,\"x\":2}}{}]", "[".repeat(20), "]".repeat(20));
 assert_eq!(mindtrail_lib::protocol::raw::parse_batch(repeated.as_bytes()).unwrap_err().index,Some(0));
}
#[test] fn rejects_positive_descending_sequence(){let first=EVENT.replace("\"seq\":1","\"seq\":2");let err=decode_explicit(format!("[{first},{EVENT}]").as_bytes()).unwrap_err();assert_eq!(err.index,Some(1));}

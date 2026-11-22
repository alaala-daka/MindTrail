#![cfg(all(windows, feature="producer-test-fixture"))]
use mindtrail_lib::{producer::queue::valid_queue_name, protocol::raw::parse_strict_value};
#[test]
fn queue_identifiers_and_strict_json_are_enforced_at_public_boundary() {
    for name in ["", "COM1", "con", "lpt9", "../x", "a:b", &"x".repeat(65)] { assert!(!valid_queue_name(name)); }
    assert!(valid_queue_name("a_queue-1"));
    assert_eq!(parse_strict_value(br#"{"outer":{"x":1,"\u0078":2}}"#, 100).unwrap_err().status,400);
    assert_eq!(parse_strict_value(br#"{"a":1}"#, 6).unwrap_err().status,413);
    assert!(parse_strict_value(br#"{"a":1}"#, 7).is_ok());
}

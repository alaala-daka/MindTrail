#![cfg(all(windows, feature="producer-test-fixture"))]
#[test]
fn producer_without_command_rejects_before_opening_state() {
    let output=std::process::Command::new(env!("CARGO_BIN_EXE_mindtrail-producer")).env_remove("MINDTRAIL_STAGE0_ROOT").env_remove("MINDTRAIL_UI_FIXTURE").output().unwrap();
    assert!(!output.status.success()); assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("invalid_arguments"));
}

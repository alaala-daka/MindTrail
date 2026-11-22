use std::{path::PathBuf,time::{Duration,Instant}};

/// Fixed empty lifecycle markers, restricted to a verified test fixture.
pub fn process_marker(stage: &str) {
    use std::os::windows::fs::MetadataExt;
    let name = match stage {"main"=>"producer-main", "args"=>"producer-args", "before-open"=>"producer-before-open", "opened"=>"producer-opened", "open-path-security"=>"producer-open-path-security", "open-path-other"=>"producer-open-path-other", "open-lock"=>"producer-open-lock", "open-storage"=>"producer-open-storage", "stage-root"=>"producer-stage-root", "stage-producers"=>"producer-stage-producers", "stage-mode"=>"producer-stage-mode", "stage-queue"=>"producer-stage-queue", "stage-preflight"=>"producer-stage-preflight", "stage-parent-chain"=>"producer-stage-parent-chain", "stage-leaf"=>"producer-stage-leaf", "os-2"=>"producer-os-2", "os-3"=>"producer-os-3", "os-5"=>"producer-os-5", "os-6"=>"producer-os-6", "os-32"=>"producer-os-32", "os-33"=>"producer-os-33", "os-87"=>"producer-os-87", "os-123"=>"producer-os-123", "os-183"=>"producer-os-183", "os-267"=>"producer-os-267", "os-995"=>"producer-os-995", "os-1314"=>"producer-os-1314", "os-1332"=>"producer-os-1332", "os-other"=>"producer-os-other", "os-unavailable"=>"producer-os-unavailable", "kind-not-found"=>"producer-kind-not-found", "kind-invalid-input"=>"producer-kind-invalid-input", "kind-permission"=>"producer-kind-permission", "kind-other"=>"producer-kind-other", _=>return};
    if std::env::var("MINDTRAIL_TEST_PROCESS_MARKERS").ok().as_deref()!=Some("1") {return}
    let Some(root)=std::env::var_os("MINDTRAIL_TEST_ROOT").map(PathBuf::from) else{return};
    let Some(fixture)=root.parent() else{return};
    let Some(name_part)=fixture.file_name().and_then(|v|v.to_str()) else{return};
    let Some(id)=name_part.strip_prefix("MindTrailAppEvent-") else{return};
    if root.file_name().and_then(|v|v.to_str())!=Some("state") || id.len()!=32 || !id.bytes().all(|b|b.is_ascii_digit()||(b'a'..=b'f').contains(&b)) {return}
    let marker=fixture.join("fixture-marker");
    for path in [&root, &fixture.to_path_buf(), &marker] {
        let Ok(meta)=std::fs::symlink_metadata(path) else{return};
        if meta.file_attributes() & 0x400 != 0 {return}
    }
    if std::fs::metadata(&marker).ok().is_none_or(|meta| !meta.is_file() || meta.len()!=32) {return}
    if std::fs::read_to_string(&marker).ok().as_deref()!=Some(id) {return}
    let Some(parent)=fixture.parent() else{return};
    let (Ok(actual),Ok(expected))=(std::fs::canonicalize(parent),std::fs::canonicalize(env!("CARGO_MANIFEST_DIR"))) else{return};
    if actual!=expected {return}
    let _=std::fs::OpenOptions::new().write(true).create_new(true).open(root.join(name));
}

/// Classify a fixture-only path/preflight error without writing its contents.
pub(crate) fn mark_path_error(error: &std::io::Error) {
    process_marker(path_error_class(error));
    process_marker(match error.kind() {std::io::ErrorKind::NotFound=>"kind-not-found",std::io::ErrorKind::InvalidInput=>"kind-invalid-input",std::io::ErrorKind::PermissionDenied=>"kind-permission",_=>"kind-other"});
    mark_os_error(error);
}

pub(crate) fn mark_os_error(error: &std::io::Error) {
    // A later wrapped error may also emit unavailable after an inner raw code;
    // markers are observations at fixed boundaries, not an exclusive verdict.
    process_marker(os_error_label(error.raw_os_error()));
}

fn os_error_label(code: Option<i32>) -> &'static str {match code {Some(2)=>"os-2",Some(3)=>"os-3",Some(5)=>"os-5",Some(6)=>"os-6",Some(32)=>"os-32",Some(33)=>"os-33",Some(87)=>"os-87",Some(123)=>"os-123",Some(183)=>"os-183",Some(267)=>"os-267",Some(995)=>"os-995",Some(1314)=>"os-1314",Some(1332)=>"os-1332",Some(_)=>"os-other",None=>"os-unavailable"}}

fn path_error_class(error: &std::io::Error) -> &'static str {
    let text=error.to_string().to_ascii_lowercase();
    if error.kind()==std::io::ErrorKind::PermissionDenied || ["dacl", "ancestor ace", "reparse", "unexpected principal", "multiple hard links"].iter().any(|v|text.contains(v)) {
        "open-path-security"
    } else {
        "open-path-other"
    }
}

#[cfg(test)]
mod tests {
    use super::{os_error_label,path_error_class};
    #[test]
    fn path_error_classes_do_not_equate_permission_with_specific_acl_cause() {
        assert_eq!(path_error_class(&std::io::Error::new(std::io::ErrorKind::PermissionDenied,"synthetic")), "open-path-security");
        assert_eq!(path_error_class(&std::io::Error::other("synthetic")), "open-path-other");
    }
    #[test]
    fn operating_system_codes_are_exact_and_finite() {
        for (code,label) in [(2,"os-2"),(3,"os-3"),(5,"os-5"),(6,"os-6"),(32,"os-32"),(33,"os-33"),(87,"os-87"),(123,"os-123"),(183,"os-183"),(267,"os-267"),(995,"os-995"),(1314,"os-1314"),(1332,"os-1332")] {assert_eq!(os_error_label(Some(code)),label);}
        assert_eq!(os_error_label(Some(268)),"os-other");
        assert_eq!(os_error_label(None),"os-unavailable");
    }
}

/// Test-only, process-local barrier. The parent owns the isolated directory and
/// kills or releases only the child it started. No event data is written here.
pub(crate) fn barrier(stage:&str) {
    if std::env::var("MINDTRAIL_TEST_BARRIER_STAGE").ok().as_deref()!=Some(stage) {return}
    let Some(root)=std::env::var_os("MINDTRAIL_TEST_ROOT") else {return};
    let dir=PathBuf::from(root).parent().map(|p|p.join("barrier"));let Some(dir)=dir else{return};
    if std::fs::write(dir.join("ready"),stage.as_bytes()).is_err(){return}
    let deadline=Instant::now()+Duration::from_secs(45);
    while !dir.join("release").is_file() && Instant::now()<deadline {std::thread::sleep(Duration::from_millis(5));}
}

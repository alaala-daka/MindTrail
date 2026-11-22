#[cfg(not(windows))]
compile_error!("The producer currently supports Windows only.");

#[cfg(windows)]
fn main() {
    #[cfg(feature="producer-test-fixture")]
    mindtrail_lib::producer::fixture_process_marker("main");
    if let Err(message) = mindtrail_lib::producer::run() {
        if message != "reported_error" {eprintln!("{message}");}
        std::process::exit(1);
    }
}

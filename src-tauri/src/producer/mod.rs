pub mod queue;
pub(crate) mod gateway;
pub(crate) mod worker;
pub(crate) mod tools;
pub(crate) mod framing;
pub(crate) mod limits;
pub(crate) mod mcp_handler;
pub(crate) mod mcp_stdio;
pub(crate) mod explicit;
#[cfg(feature="producer-test-fixture")]
pub(crate) mod faults;
#[cfg(feature="producer-test-fixture")]
pub fn fixture_process_marker(stage: &str) { faults::process_marker(stage); }
fn state_root()->Result<std::path::PathBuf,&'static str> {
    #[cfg(feature="producer-test-fixture")]
    if let Some(root)=std::env::var_os("MINDTRAIL_TEST_ROOT") {return Ok(root.into())}
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::{UI::Shell::{SHGetKnownFolderPath,FOLDERID_Profile},System::Com::CoTaskMemFree};
    unsafe {let mut path=std::ptr::null_mut();if SHGetKnownFolderPath(&FOLDERID_Profile,0,std::ptr::null_mut(),&mut path)<0 || path.is_null() {return Err("state_unavailable")}
        let mut len=0;while *path.add(len)!=0 {len+=1;}let value=std::ffi::OsString::from_wide(std::slice::from_raw_parts(path,len));CoTaskMemFree(path.cast());Ok(std::path::PathBuf::from(value).join(".path-tracker"))}
}
pub fn run() -> Result<(), &'static str> {
    use rmcp::ServiceExt;
    let mut args=std::env::args().skip(1);let command=args.next().ok_or("invalid_arguments")?;if matches!(command.as_str(),"submit-events"|"flush") {return explicit::entry(&command,&args.collect::<Vec<_>>())}if command!="mcp" {return Err("invalid_arguments")}
    let name=match args.next() {None=>"mcp".to_owned(),Some(flag) if flag=="--queue"=>args.next().ok_or("invalid_arguments")?,Some(_)=>return Err("invalid_arguments")};
    if args.next().is_some() || !queue::valid_queue_name(&name) {return Err("invalid_arguments")}
    #[cfg(feature="producer-test-fixture")]
    fixture_process_marker("args");
    let root=state_root()?;let worker_root=root.clone();let (commands,receiver)=worker::channel();let (ready,started)=std::sync::mpsc::channel();
    #[cfg(feature="producer-test-fixture")]
    fixture_process_marker("before-open");
    let thread=std::thread::spawn(move || {let result=queue::with_queue(&worker_root,queue::QueueMode::Mcp,&name,|q| {#[cfg(feature="producer-test-fixture")] fixture_process_marker("opened");let _=ready.send(());worker::run(q,&worker_root,receiver)});if let Err(e)=&result {eprintln!("producer worker: {e}");}result});
    if started.recv().is_err() {return match thread.join() {Ok(Err(e)) if e=="queue_busy"=>Err("queue_busy"),Ok(Err(e)) if e=="queue_corrupt"=>Err("queue_corrupt"),_=>Err("queue_unavailable")}}
    let runtime=tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|_|"worker_unavailable")?;
    let transport=match mcp_stdio::StrictTransport::start() {Ok(t)=>t,Err(e)=>{let _=commands.send(worker::Command::Shutdown);let _=thread.join();return Err(e)}};let output=transport.drain_handle();let handler=mcp_handler::Handler{commands:commands.clone(),active:transport.active.clone()};
    let served=runtime.block_on(async {match handler.serve(transport).await {Ok(service)=>service.waiting().await.map_err(|_|"mcp_unavailable"),Err(_)=>Err("mcp_unavailable")}});
    let _=commands.send(worker::Command::Shutdown);drop(commands);
    let stopped=thread.join().map_err(|_|"worker_unavailable")?.map_err(|_|"queue_unavailable");
    let drained=output.drain(std::time::Duration::from_secs(2));served.and(stopped).and(drained)
}

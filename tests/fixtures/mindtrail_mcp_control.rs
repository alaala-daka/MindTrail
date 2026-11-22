//! Synthetic stdio handshake control. No event data is persisted.
use std::io::{self, BufRead, Write};
use std::path::PathBuf;

fn marker(root: &PathBuf, name: &str) {
    let _ = std::fs::OpenOptions::new().write(true).create_new(true).open(root.join(name));
}

fn compact(input: &str) -> String {
    let (mut quoted, mut escaped) = (false, false);
    let mut out = String::new();
    for ch in input.chars() {
        if quoted {
            out.push(ch);
            if escaped { escaped = false; } else if ch == '\\' { escaped = true; } else if ch == '"' { quoted = false; }
        } else if ch == '"' { quoted = true; out.push(ch); } else if !ch.is_whitespace() { out.push(ch); }
    }
    out
}

fn id(input: &str) -> Option<&str> {
    let rest = input.split_once("\"id\":")?.1;
    if rest.starts_with('"') {
        let mut escaped = false;
        for (i, ch) in rest.char_indices().skip(1) {
            if escaped { escaped = false; } else if ch == '\\' { escaped = true; } else if ch == '"' { return Some(&rest[..=i]); }
        }
        None
    } else {
        let end = rest.find([',', '}'])?;
        let token = &rest[..end];
        if !token.is_empty() && token.bytes().all(|b| b.is_ascii_digit() || b == b'-') { Some(token) } else { None }
    }
}

fn main() {
    let Some(root) = std::env::var_os("MINDTRAIL_TEST_ROOT").map(PathBuf::from) else { return };
    let Some(fixture) = root.parent() else { return };
    let Some(name) = fixture.file_name().and_then(|s| s.to_str()) else { return };
    let Some(suffix) = name.strip_prefix("MindTrailAppEvent-") else { return };
    if root.file_name().and_then(|s| s.to_str()) != Some("state") || suffix.len() != 32 || !suffix.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)) || !fixture.join("fixture-marker").is_file() { return }
    marker(&root, "control-started");
    if ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY"].iter().all(|key| std::env::var(key).is_ok_and(|v| v.is_empty()))
        && std::env::var("NO_PROXY").is_ok_and(|v| v == "*") {
        marker(&root, "control-proxy-isolated");
    }
    eprintln!("MINDTRAIL_SYNTHETIC_STDERR_MARKER");
    let stdin = io::stdin(); let mut stdout = io::stdout().lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break }; if line.len() > 1_048_576 { break }
        let line = compact(&line);
        let (kind, payload) = if line.contains("\"method\":\"initialize\"") {
            marker(&root, "control-initialize-request");
            ("initialize", r#"{"protocolVersion":"2025-11-25","capabilities":{"tools":{}},"serverInfo":{"name":"synthetic-control","version":"1"}}"#.to_string())
        } else if line.contains("\"method\":\"tools/list\"") {
            marker(&root, "control-tools-request");
            let tools = ["trace_task_start","trace_intent","trace_api","trace_file","trace_call","trace_done","trace_task_done"].map(|name| format!(r#"{{"name":"{name}","description":"synthetic","inputSchema":{{"type":"object"}}}}"#));
            ("tools", format!(r#"{{"tools":[{}]}}"#, tools.join(",")))
        } else { continue };
        let Some(id) = id(&line) else { break };
        if writeln!(stdout, r#"{{"jsonrpc":"2.0","id":{id},"result":{payload}}}"#).is_err() || stdout.flush().is_err() { break }
        marker(&root, if kind == "initialize" { "control-initialize-response" } else { "control-tools-response" });
    }
    marker(&root, "control-exited");
}

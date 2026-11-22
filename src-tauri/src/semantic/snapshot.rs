use super::types::{SemanticContext, SemanticEvidence, SemanticInput};
use serde_json::Value;

pub(crate) fn projected_nodes(events: &[Value]) -> std::collections::HashSet<String> {
    let mut kinds = std::collections::HashMap::new();
    for event in events {
        let kind = event.get("type").and_then(Value::as_str).unwrap_or("");
        let payload = &event["payload"];
        match kind {
            "task.start" => { kinds.insert("root".to_owned(), "task"); },
            "capability.intent" => { if let Some(id) = payload.get("id").and_then(Value::as_str) { kinds.insert(id.to_owned(), "capability"); } },
            "api.expose" | "file.change" => {
                let parent = payload.get("capabilityId").and_then(Value::as_str);
                if !parent.is_some_and(|id| kinds.get(id) == Some(&"capability")) { kinds.entry("unassigned".to_owned()).or_insert("group"); }
                if let Some(id) = payload.get("id").and_then(Value::as_str) { kinds.insert(id.to_owned(), if kind == "api.expose" { "api" } else { "file" }); }
            },
            _ => {}
        }
    }
    kinds.into_keys().collect()
}

pub fn build_input(context: SemanticContext, store_id: &str, events: Vec<Value>) -> Result<SemanticInput, &'static str> {
    if context.store_id != store_id || context.prefix_seq == 0 || events.len() as u64 != context.prefix_seq || [&context.store_id, &context.session_id, &context.node_id].iter().any(|s| s.is_empty() || s.len() > 256) { return Err("invalid_context"); }
    let nodes = projected_nodes(&events);
    let mut current_nodes = std::collections::HashSet::new();
    let mut evidence = Vec::new();
    for (index, event) in events.iter().enumerate() {
        if event.get("session").and_then(Value::as_str) != Some(context.session_id.as_str()) || event.get("seq").and_then(Value::as_u64) != Some(index as u64 + 1) { return Err("invalid_events"); }
        let kind = event.get("type").and_then(Value::as_str).ok_or("invalid_events")?;
        let payload = event.get("payload").and_then(Value::as_object).ok_or("invalid_events")?;
        let node = match kind { "task.start" | "task.done" => Some("root"), "capability.intent" | "capability.done" | "api.expose" | "file.change" => payload.get("id").and_then(Value::as_str), _ => None };
        if let Some(id) = node {
            if matches!(kind, "task.start" | "capability.intent" | "api.expose" | "file.change") { current_nodes.insert(id.to_owned()); }
            if current_nodes.contains(id) && let Some(text) = payload.get("quote").and_then(Value::as_str) {
                evidence.push(SemanticEvidence { id: format!("{}:{}", context.session_id, index + 1), node_id: id.to_owned(), seq: index as u64 + 1, text: text.to_owned() });
            }
        }
    }
    if !nodes.contains(&context.node_id) { return Err("invalid_node"); }
    let input = SemanticInput { context, events, evidence };
    if serde_json::to_vec(&input).map_err(|_| "invalid_events")?.len() > 256 * 1024 { return Err("input_too_large"); }
    Ok(input)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn semantic_snapshot_handles_unassigned_and_limits_complete_input() {
        let ctx = SemanticContext { store_id: "a".into(), session_id: "s".into(), node_id: "unassigned".into(), prefix_seq: 2 };
        let events = vec![json!({"session":"s","seq":1,"type":"task.start","payload":{}}), json!({"session":"s","seq":2,"type":"file.change","payload":{"id":"f"}})];
        assert!(build_input(ctx.clone(), "a", events.clone()).is_ok());
        assert!(build_input(ctx.clone(), "other", events.clone()).is_err());
        let mut missing = ctx.clone(); missing.node_id = "missing".into(); assert!(build_input(missing, "a", events.clone()).is_err());
        let mut large = events; large[1]["payload"]["quote"] = json!("x".repeat(140_000));
        assert_eq!(build_input(ctx, "a", large).unwrap_err(), "input_too_large");
    }
}

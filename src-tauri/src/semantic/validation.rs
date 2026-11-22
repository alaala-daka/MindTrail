use super::types::{SemanticInput, SemanticResult};
use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};

pub fn validate_result(input: &SemanticInput, result: &SemanticResult) -> Result<(), &'static str> {
    if input.context != result.context || result.sections.is_empty() || result.sections.len() > 16 || [&result.provider, &result.model, &result.prompt_version].iter().any(|s| s.trim().is_empty() || s.len() > 256) { return Err("invalid_result"); }
    if input.evidence.is_empty() { return Err("insufficient_evidence"); }
    let nodes = super::snapshot::projected_nodes(&input.events);
    let mut edges: HashMap<String, Vec<String>> = HashMap::new();
    for event in &input.events {
        let kind = event.get("type").and_then(Value::as_str).unwrap_or("");
        let Some(payload) = event.get("payload") else { return Err("invalid_events"); };
        if kind == "call.edge" {
            if let (Some(from), Some(to)) = (payload.get("from").and_then(Value::as_str), payload.get("to").and_then(Value::as_str)) { edges.entry(from.to_owned()).or_default().push(to.to_owned()); }
        }
    }
    let mut reachable = HashSet::from([input.context.node_id.clone()]);
    let mut queue = VecDeque::from([input.context.node_id.clone()]);
    while let Some(node) = queue.pop_front() { if let Some(targets) = edges.get(&node) { for target in targets { if nodes.contains(target) && reachable.insert(target.clone()) { queue.push_back(target.clone()); } } } }
    for section in &result.sections {
        if section.title.trim().is_empty() || section.title.len() > 256 || section.text.trim().is_empty() || section.text.len() > 8192 || section.citations.is_empty() || section.citations.len() > 64 || section.related_node_ids.len() > 64 { return Err("invalid_result"); }
        for cite in &section.citations {
            if cite.id.len() > 512 || cite.node_id.len() > 256 || cite.session_id != input.context.session_id || !nodes.contains(&cite.node_id) || !input.evidence.iter().any(|e| e.id == cite.id && e.node_id == cite.node_id && e.seq <= input.context.prefix_seq) { return Err("invalid_citation"); }
        }
        if section.related_node_ids.iter().any(|id| !nodes.contains(id) || !reachable.contains(id)) { return Err("invalid_relation"); }
    }
    if serde_json::to_vec(result).map_err(|_| "invalid_result")?.len() > 256 * 1024 { return Err("invalid_result"); }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::semantic::{snapshot::build_input, types::*};
    use serde_json::json;

    fn fixture() -> (SemanticInput, SemanticResult) {
        let context = SemanticContext { store_id: "store-a".into(), session_id: "s".into(), node_id: "n".into(), prefix_seq: 3 };
        let events = vec![json!({"session":"s","seq":1,"type":"task.start","payload":{"title":"T"}}),
                          json!({"session":"s","seq":2,"type":"capability.intent","payload":{"id":"n"}}),
                          json!({"session":"s","seq":3,"type":"capability.done","payload":{"id":"n","quote":"evidence"}})];
        let input = build_input(context.clone(), "store-a", events).unwrap();
        let result = SemanticResult { context, provider:"mock".into(), model:"mock".into(), prompt_version:"1".into(),
            sections: vec![SemanticSection { title:"T".into(), text:"text".into(), citations:vec![SemanticCitation { kind:QuoteKind::Quote, session_id:"s".into(), node_id:"n".into(), id:"s:3".into() }], related_node_ids:vec![] }] };
        (input, result)
    }

    #[test]
    fn semantic_validation_rejects_untrusted_and_future_references() {
        let (input, result) = fixture();
        assert!(validate_result(&input, &result).is_ok());
        let mut bad = result.clone(); bad.context.prefix_seq = 4; assert!(validate_result(&input, &bad).is_err());
        let mut bad = result.clone(); bad.sections.clear(); assert!(validate_result(&input, &bad).is_err());
        let mut bad = result.clone(); bad.sections[0].citations.clear(); assert!(validate_result(&input, &bad).is_err());
        let mut bad = result.clone(); bad.sections[0].citations[0].id = "s:4".into(); assert!(validate_result(&input, &bad).is_err());
        let mut bad = result.clone(); bad.sections[0].citations[0].session_id = "other".into(); assert!(validate_result(&input, &bad).is_err());
        let mut bad = result.clone(); bad.sections[0].related_node_ids.push("unknown".into()); assert!(validate_result(&input, &bad).is_err());
        let mut disconnected = input.clone(); disconnected.events.push(json!({"type":"api.expose","payload":{"id":"other"}}));
        let mut bad = result.clone(); bad.sections[0].related_node_ids.push("other".into()); assert!(validate_result(&disconnected, &bad).is_err());
        let mut bad = result.clone(); bad.sections[0].text = "x".repeat(8193); assert!(validate_result(&input, &bad).is_err());
        assert!(serde_json::from_str::<SemanticContext>(r#"{"store_id":"a","session_id":"s","node_id":"n","prefix_seq":3,"token":"x"}"#).is_err());
        assert!(serde_json::from_str::<QuoteKind>(r#""src""#).is_err());
        let mut no_evidence = input.clone(); no_evidence.evidence.clear(); assert_eq!(validate_result(&no_evidence, &result), Err("insufficient_evidence"));
    }
}

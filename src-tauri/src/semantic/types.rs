use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(deny_unknown_fields)]
pub struct SemanticContext { pub store_id: String, pub session_id: String, pub node_id: String, pub prefix_seq: u64 }

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticEvidence { pub id: String, pub node_id: String, pub seq: u64, pub text: String }

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticInput { pub context: SemanticContext, pub events: Vec<Value>, pub evidence: Vec<SemanticEvidence> }

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuoteKind { Quote }

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticCitation { pub kind: QuoteKind, pub session_id: String, pub node_id: String, pub id: String }

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticSection { pub title: String, pub text: String, pub citations: Vec<SemanticCitation>, pub related_node_ids: Vec<String> }

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticResult { pub context: SemanticContext, pub provider: String, pub model: String, pub prompt_version: String, pub sections: Vec<SemanticSection> }

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SemanticError { NotConfigured, InvalidConfig, GatewayUnavailable, IdentityChanged, InvalidContext, InputTooLarge, InsufficientEvidence, Busy, Cancelled, Timeout, Transport, Unauthorized, RateLimited, ProviderUnavailable, InvalidOutput, InvalidReference }

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderStatus { pub configured: bool, pub provider: Option<String>, pub model: Option<String>, pub endpoint_origin: Option<String> }

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preview { pub ticket_id: String, pub context: SemanticContext, pub provider: String, pub model: String, pub prompt_version: String, pub event_count: usize, pub evidence_count: usize, pub byte_count: usize, pub events: Vec<Value>, pub evidence: Vec<SemanticEvidence>, pub expires_at: u64 }

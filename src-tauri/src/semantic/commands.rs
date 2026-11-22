use std::sync::Arc;
use crate::gateway::desktop::DesktopState;
use super::{service::SemanticService, types::*};

fn require_main(label: &str) -> Result<(), SemanticError> { if label == "main" { Ok(()) } else { Err(SemanticError::InvalidContext) } }

#[tauri::command]
pub(crate) fn semantic_status(window: tauri::WebviewWindow, service: tauri::State<'_, Arc<SemanticService>>) -> Result<ProviderStatus, SemanticError> {
    require_main(window.label())?; service.status()
}

#[tauri::command]
pub(crate) async fn prepare_insight(window: tauri::WebviewWindow, state: tauri::State<'_, Arc<DesktopState>>, service: tauri::State<'_, Arc<SemanticService>>, context: SemanticContext) -> Result<Preview, SemanticError> {
    require_main(window.label())?;
    if !service.status()?.configured { return Err(SemanticError::NotConfigured); }
    let input = state.semantic_snapshot(context).await.map_err(|error| match error.as_str() {
        "invalid_context" => SemanticError::IdentityChanged,
        "invalid_node" | "invalid_events" => SemanticError::InvalidContext,
        "input_too_large" => SemanticError::InputTooLarge,
        _ => SemanticError::GatewayUnavailable,
    })?;
    service.prepare(input)
}

#[tauri::command]
pub(crate) async fn generate_insight(window: tauri::WebviewWindow, service: tauri::State<'_, Arc<SemanticService>>, ticket_id: String, request_id: String) -> Result<SemanticResult, SemanticError> {
    require_main(window.label())?; service.generate(&ticket_id, &request_id).await
}

#[tauri::command]
pub(crate) fn cancel_insight(window: tauri::WebviewWindow, service: tauri::State<'_, Arc<SemanticService>>, request_id: String) -> Result<(), SemanticError> {
    require_main(window.label())?; service.cancel(&request_id)
}

#[tauri::command]
pub(crate) fn release_insight(window: tauri::WebviewWindow, service: tauri::State<'_, Arc<SemanticService>>, ticket_id: String, context: SemanticContext) -> Result<(), SemanticError> {
    require_main(window.label())?; service.release(&ticket_id, &context)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn semantic_commands_reject_non_main_window() { assert!(require_main("main").is_ok()); assert_eq!(require_main("other"), Err(SemanticError::InvalidContext)); }
}

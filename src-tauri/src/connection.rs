use serde::Serialize;
use std::sync::Arc;
use tauri::Manager;
use crate::{distribution, gateway::desktop::{DesktopState, DesktopStatus}};

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all="snake_case")]
pub(crate) enum ConnectionError { WrongWindow, ProducerNotPackaged, PathUnavailable, GatewayUnavailable }
#[derive(Serialize)]
pub(crate) struct ConnectionStatus { gateway: DesktopStatus, producer_path: Option<String>, producer_available: bool, data_path: String }
#[derive(Serialize)]
#[serde(rename_all="snake_case")]
enum CheckStatus { Passed, Failed }
#[derive(Serialize)]
struct CheckItem { name: String, status: CheckStatus, code: Option<String> }
#[derive(Serialize)]
pub(crate) struct ConnectionCheck { status: CheckStatus, checks: Vec<CheckItem> }
fn require_main(label: &str) -> Result<(), ConnectionError> { if label == "main" { Ok(()) } else { Err(ConnectionError::WrongWindow) } }

#[tauri::command]
pub(crate) fn connection_status(window: tauri::WebviewWindow, state: tauri::State<'_, Arc<DesktopState>>, app: tauri::AppHandle) -> Result<ConnectionStatus, ConnectionError> {
    require_main(window.label())?;
    let path=distribution::producer_path(&app).ok();
    let data_path=app.path().home_dir().map_err(|_|ConnectionError::PathUnavailable)?.join(".path-tracker").to_string_lossy().into_owned();
    let gateway=match state.status() { DesktopStatus::Ready { store_id } => DesktopStatus::Ready { store_id }, _ => DesktopStatus::Unavailable { message:"本地网关不可用".into() } };
    Ok(ConnectionStatus { gateway, producer_available:path.is_some(), producer_path:path.map(|p|p.to_string_lossy().into_owned()), data_path })
}

#[tauri::command]
pub(crate) async fn connection_check(window: tauri::WebviewWindow, state: tauri::State<'_, Arc<DesktopState>>, app: tauri::AppHandle) -> Result<ConnectionCheck, ConnectionError> {
    require_main(window.label())?;
    let producer=distribution::producer_path(&app).is_ok();
    let gateway=state.connection_read_check().await.is_ok();
    Ok(build_check(producer,gateway))
}
fn build_check(producer: bool, gateway: bool) -> ConnectionCheck {
    let checks=vec![CheckItem { name:"producer".into(),status:if producer { CheckStatus::Passed } else { CheckStatus::Failed },code:if producer { None } else { Some("producer_not_packaged".into()) } }, CheckItem { name:"gateway".into(),status:if gateway { CheckStatus::Passed } else { CheckStatus::Failed },code:if gateway { None } else { Some("gateway_unavailable".into()) } }];
    ConnectionCheck { status:if producer && gateway { CheckStatus::Passed } else { CheckStatus::Failed }, checks }
}

#[cfg(test)] mod tests { use super::*; #[test] fn connection_rejects_other_window() { assert_eq!(require_main("other"),Err(ConnectionError::WrongWindow)); assert!(require_main("main").is_ok()); } #[test] fn connection_check_requires_both_conditions() { for (p,g) in [(false,false),(false,true),(true,false)] { assert_eq!(serde_json::to_value(build_check(p,g)).unwrap()["status"],"failed"); } assert_eq!(serde_json::to_value(build_check(true,true)).unwrap()["status"],"passed"); } }

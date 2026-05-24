//! Workspace state management commands.
//!
//! Phase 2 Unit 4 — bodies became thin proxies into the daemon's
//! `/cli/states/*` routes. The `#[tauri::command]` wrappers stay so
//! `invoke_handler!` in lib.rs can keep dispatching.

use k2so_core::db::schema::WorkspaceState;
use serde_json::json;

use crate::daemon_client::DaemonClient;

fn daemon() -> Result<DaemonClient, String> {
    DaemonClient::try_connect()
}

#[tauri::command]
pub fn states_list() -> Result<Vec<WorkspaceState>, String> {
    daemon()?.cli_get_json("/cli/states/list", &[])
}

#[tauri::command]
pub fn states_get(id: String) -> Result<WorkspaceState, String> {
    daemon()?.cli_get_json("/cli/states/get", &[("id", &id)])
}

#[tauri::command]
pub fn states_create(
    name: String,
    description: Option<String>,
    cap_features: String,
    cap_issues: String,
    cap_crashes: String,
    cap_security: String,
    cap_audits: String,
    heartbeat: bool,
) -> Result<WorkspaceState, String> {
    daemon()?.cli_post_json_decode(
        "/cli/states/create",
        &json!({
            "name": name,
            "description": description,
            "capFeatures": cap_features,
            "capIssues": cap_issues,
            "capCrashes": cap_crashes,
            "capSecurity": cap_security,
            "capAudits": cap_audits,
            "heartbeat": heartbeat,
        }),
    )
}

#[tauri::command]
pub fn states_update(
    id: String,
    name: Option<String>,
    description: Option<String>,
    cap_features: Option<String>,
    cap_issues: Option<String>,
    cap_crashes: Option<String>,
    cap_security: Option<String>,
    cap_audits: Option<String>,
    heartbeat: Option<bool>,
) -> Result<WorkspaceState, String> {
    daemon()?.cli_post_json_decode(
        "/cli/states/update",
        &json!({
            "id": id,
            "name": name,
            "description": description,
            "capFeatures": cap_features,
            "capIssues": cap_issues,
            "capCrashes": cap_crashes,
            "capSecurity": cap_security,
            "capAudits": cap_audits,
            "heartbeat": heartbeat,
        }),
    )
}

#[tauri::command]
pub fn states_delete(id: String) -> Result<(), String> {
    daemon()?
        .cli_post_json("/cli/states/delete", &json!({ "id": id }))
        .map(|_| ())
}

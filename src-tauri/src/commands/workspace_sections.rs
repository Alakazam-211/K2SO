//! Workspace section commands.
//!
//! Phase 2 Unit 4 — bodies became thin proxies into the daemon's
//! `/cli/sections/*` routes.

use k2so_core::db::schema::{Workspace, WorkspaceSection};
use serde_json::json;

use crate::daemon_client::DaemonClient;

fn daemon() -> Result<DaemonClient, String> {
    DaemonClient::try_connect()
}

#[tauri::command]
pub fn sections_list(project_id: String) -> Result<Vec<WorkspaceSection>, String> {
    daemon()?.cli_get_json("/cli/sections/list", &[("project_id", &project_id)])
}

#[tauri::command]
pub fn sections_create(
    project_id: String,
    name: String,
    color: Option<String>,
) -> Result<WorkspaceSection, String> {
    daemon()?.cli_post_json_decode(
        "/cli/sections/create",
        &json!({
            "projectId": project_id,
            "name": name,
            "color": color,
        }),
    )
}

#[tauri::command]
pub fn sections_update(
    id: String,
    name: Option<String>,
    color: Option<String>,
    is_collapsed: Option<i64>,
    tab_order: Option<i64>,
) -> Result<WorkspaceSection, String> {
    daemon()?.cli_post_json_decode(
        "/cli/sections/update",
        &json!({
            "id": id,
            "name": name,
            "color": color,
            "isCollapsed": is_collapsed,
            "tabOrder": tab_order,
        }),
    )
}

#[tauri::command]
pub fn sections_delete(id: String) -> Result<(), String> {
    daemon()?
        .cli_post_json("/cli/sections/delete", &json!({ "id": id }))
        .map(|_| ())
}

#[tauri::command]
pub fn sections_reorder(ids: Vec<String>) -> Result<(), String> {
    daemon()?
        .cli_post_json("/cli/sections/reorder", &json!({ "ids": ids }))
        .map(|_| ())
}

#[tauri::command]
pub fn sections_assign_workspace(
    workspace_id: String,
    section_id: Option<String>,
) -> Result<Workspace, String> {
    daemon()?.cli_post_json_decode(
        "/cli/sections/assign",
        &json!({
            "workspaceId": workspace_id,
            "sectionId": section_id,
        }),
    )
}

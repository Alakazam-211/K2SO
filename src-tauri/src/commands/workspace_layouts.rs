//! Workspace layout commands.
//!
//! Phase 2 Unit 4 — bodies became thin proxies into the daemon's
//! `/cli/workspace-layouts/*` routes. The `WorkspaceLayout` struct
//! shape now lives in `k2so_core::db_ops` and is re-exported below
//! for type continuity in the renderer-facing return type.

use serde::Serialize;

use crate::daemon_client::DaemonClient;

fn daemon() -> Result<DaemonClient, String> {
    DaemonClient::try_connect()
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceLayout {
    pub project_id: String,
    pub workspace_id: String,
    pub layout_json: String,
}

impl From<k2so_core::db_ops::WorkspaceLayout> for WorkspaceLayout {
    fn from(l: k2so_core::db_ops::WorkspaceLayout) -> Self {
        Self {
            project_id: l.project_id,
            workspace_id: l.workspace_id,
            layout_json: l.layout_json,
        }
    }
}

#[tauri::command]
pub fn workspace_layout_save(
    project_id: String,
    workspace_id: String,
    layout_json: String,
) -> Result<(), String> {
    daemon()?
        .cli_post_json(
            "/cli/workspace-layouts/save",
            &serde_json::json!({
                "projectId": project_id,
                "workspaceId": workspace_id,
                "layoutJson": layout_json,
            }),
        )
        .map(|_| ())
}

#[tauri::command]
pub fn workspace_layout_load(
    project_id: String,
    workspace_id: String,
) -> Result<Option<String>, String> {
    daemon()?.cli_get_json(
        "/cli/workspace-layouts/load",
        &[
            ("project_id", &project_id),
            ("workspace_id", &workspace_id),
        ],
    )
}

#[tauri::command]
pub fn workspace_layout_load_all() -> Result<Vec<WorkspaceLayout>, String> {
    let layouts: Vec<k2so_core::db_ops::WorkspaceLayout> = daemon()?
        .cli_get_json("/cli/workspace-layouts/load-all", &[])?;
    Ok(layouts.into_iter().map(Into::into).collect())
}

#[tauri::command]
pub fn workspace_layout_delete(
    project_id: String,
    workspace_id: Option<String>,
) -> Result<(), String> {
    daemon()?
        .cli_post_json(
            "/cli/workspace-layouts/delete",
            &serde_json::json!({
                "projectId": project_id,
                "workspaceId": workspace_id,
            }),
        )
        .map(|_| ())
}

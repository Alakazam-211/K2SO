//! Workspace CRUD commands.
//!
//! Phase 2 Unit 4 — bodies became thin proxies into the daemon's
//! `/cli/workspaces/*` routes. The `#[tauri::command]` wrappers stay
//! so `invoke_handler!` in lib.rs can keep dispatching.

use k2so_core::db::schema::Workspace;
use serde_json::json;

use crate::daemon_client::DaemonClient;

fn daemon() -> Result<DaemonClient, String> {
    DaemonClient::try_connect()
}

#[tauri::command]
pub fn workspaces_list(project_id: String) -> Result<Vec<Workspace>, String> {
    daemon()?.cli_get_json("/cli/workspaces/list", &[("project_id", &project_id)])
}

#[tauri::command]
pub fn workspaces_create(
    project_id: String,
    name: String,
    type_: Option<String>,
    branch: Option<String>,
    worktree_path: Option<String>,
) -> Result<Workspace, String> {
    daemon()?.cli_post_json_decode(
        "/cli/workspaces/create",
        &json!({
            "projectId": project_id,
            "name": name,
            "type": type_,
            "branch": branch,
            "worktreePath": worktree_path,
        }),
    )
}

#[tauri::command]
pub fn workspaces_delete(id: String) -> Result<(), String> {
    daemon()?
        .cli_post_json("/cli/workspaces/delete", &json!({ "id": id }))
        .map(|_| ())
}

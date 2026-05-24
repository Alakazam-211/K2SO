//! Focus group commands.
//!
//! Phase 2 Unit 4 — bodies became thin proxies into the daemon's
//! `/cli/focus-groups/*` routes. `sync:focus-groups` / `sync:projects`
//! events are still emitted from the Tauri shim so existing renderer
//! listeners keep working; once a renderer-side `daemon_events`
//! subscriber replaces them, these emits can come out.

use k2so_core::db::schema::{FocusGroup, Project};
use serde_json::json;
use tauri::{AppHandle, Emitter};

use crate::daemon_client::DaemonClient;

fn daemon() -> Result<DaemonClient, String> {
    DaemonClient::try_connect()
}

#[tauri::command]
pub fn focus_groups_list() -> Result<Vec<FocusGroup>, String> {
    daemon()?.cli_get_json("/cli/focus-groups/list", &[])
}

#[tauri::command]
pub fn focus_groups_create(
    app: AppHandle,
    name: String,
    color: Option<String>,
) -> Result<FocusGroup, String> {
    let r = daemon()?.cli_post_json_decode(
        "/cli/focus-groups/create",
        &json!({ "name": name, "color": color }),
    )?;
    let _ = app.emit("sync:focus-groups", ());
    Ok(r)
}

#[tauri::command]
pub fn focus_groups_update(
    app: AppHandle,
    id: String,
    name: Option<String>,
    color: Option<String>,
    tab_order: Option<i64>,
) -> Result<FocusGroup, String> {
    let r = daemon()?.cli_post_json_decode(
        "/cli/focus-groups/update",
        &json!({
            "id": id,
            "name": name,
            "color": color,
            "tabOrder": tab_order,
        }),
    )?;
    let _ = app.emit("sync:focus-groups", ());
    Ok(r)
}

#[tauri::command]
pub fn focus_groups_delete(app: AppHandle, id: String) -> Result<(), String> {
    daemon()?
        .cli_post_json("/cli/focus-groups/delete", &json!({ "id": id }))?;
    let _ = app.emit("sync:focus-groups", ());
    Ok(())
}

#[tauri::command]
pub fn focus_groups_assign_project(
    app: AppHandle,
    project_id: String,
    focus_group_id: Option<String>,
) -> Result<Project, String> {
    let r: Project = daemon()?.cli_post_json_decode(
        "/cli/focus-groups/assign",
        &json!({
            "projectId": project_id,
            "focusGroupId": focus_group_id,
        }),
    )?;
    let _ = app.emit("sync:focus-groups", ());
    let _ = app.emit("sync:projects", ());
    Ok(r)
}

#[tauri::command]
pub fn focus_groups_reconcile_project(
    app: AppHandle,
    project_id: String,
) -> Result<Project, String> {
    let r: Project = daemon()?.cli_post_json_decode(
        "/cli/focus-groups/reconcile",
        &json!({ "projectId": project_id }),
    )?;
    let _ = app.emit("sync:focus-groups", ());
    let _ = app.emit("sync:projects", ());
    Ok(r)
}

//! Agent preset commands.
//!
//! Phase 2 Unit 4 — bodies became thin proxies into the daemon's
//! `/cli/presets/*` routes. Built-in preset catalog lives in
//! `k2so_core::db_ops::BUILT_IN_PRESETS` so the daemon's
//! `/cli/presets/reset` handler is the single source of truth.

use k2so_core::db::schema::AgentPreset;
use serde_json::json;
use tauri::{AppHandle, Emitter};

use crate::daemon_client::DaemonClient;

fn daemon() -> Result<DaemonClient, String> {
    DaemonClient::try_connect()
}

#[tauri::command]
pub fn presets_list() -> Result<Vec<AgentPreset>, String> {
    daemon()?.cli_get_json("/cli/presets/list", &[])
}

#[tauri::command]
pub fn presets_create(
    app: AppHandle,
    label: String,
    command: String,
    icon: Option<String>,
) -> Result<AgentPreset, String> {
    let r = daemon()?.cli_post_json_decode(
        "/cli/presets/create",
        &json!({ "label": label, "command": command, "icon": icon }),
    )?;
    let _ = app.emit("sync:presets", ());
    Ok(r)
}

#[tauri::command]
pub fn presets_update(
    app: AppHandle,
    id: String,
    label: Option<String>,
    command: Option<String>,
    icon: Option<String>,
    enabled: Option<i64>,
    sort_order: Option<i64>,
) -> Result<AgentPreset, String> {
    let r = daemon()?.cli_post_json_decode(
        "/cli/presets/update",
        &json!({
            "id": id,
            "label": label,
            "command": command,
            "icon": icon,
            "enabled": enabled,
            "sortOrder": sort_order,
        }),
    )?;
    let _ = app.emit("sync:presets", ());
    Ok(r)
}

#[tauri::command]
pub fn presets_delete(app: AppHandle, id: String) -> Result<(), String> {
    daemon()?
        .cli_post_json("/cli/presets/delete", &json!({ "id": id }))?;
    let _ = app.emit("sync:presets", ());
    Ok(())
}

#[tauri::command]
pub fn presets_reorder(app: AppHandle, ids: Vec<String>) -> Result<(), String> {
    daemon()?
        .cli_post_json("/cli/presets/reorder", &json!({ "ids": ids }))?;
    let _ = app.emit("sync:presets", ());
    Ok(())
}

#[tauri::command]
pub fn presets_reset_built_ins(app: AppHandle) -> Result<Vec<AgentPreset>, String> {
    let r = daemon()?
        .cli_post_json_decode("/cli/presets/reset", &json!({}))?;
    let _ = app.emit("sync:presets", ());
    Ok(r)
}

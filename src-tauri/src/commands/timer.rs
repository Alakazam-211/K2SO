//! Timer commands.
//!
//! Phase 2 Unit 4 — bodies became thin proxies into the daemon's
//! `/cli/timer/*` routes.

use k2so_core::db::schema::TimeEntry;
use serde_json::json;
use tauri::{AppHandle, Emitter};

use crate::daemon_client::DaemonClient;

fn daemon() -> Result<DaemonClient, String> {
    DaemonClient::try_connect()
}

fn opt(v: Option<i64>) -> String {
    v.map(|n| n.to_string()).unwrap_or_default()
}

#[tauri::command]
pub fn timer_entries_list(
    start: Option<i64>,
    end: Option<i64>,
    project_id: Option<String>,
) -> Result<Vec<TimeEntry>, String> {
    let s = opt(start);
    let e = opt(end);
    let pid = project_id.unwrap_or_default();
    let mut params: Vec<(&str, &str)> = Vec::new();
    if !s.is_empty() {
        params.push(("start", &s));
    }
    if !e.is_empty() {
        params.push(("end", &e));
    }
    if !pid.is_empty() {
        params.push(("project_id", &pid));
    }
    daemon()?.cli_get_json("/cli/timer/entries-list", &params)
}

#[tauri::command]
pub fn timer_entry_create(
    app: AppHandle,
    id: String,
    project_id: Option<String>,
    start_time: i64,
    end_time: i64,
    duration_seconds: i64,
    memo: Option<String>,
) -> Result<(), String> {
    daemon()?.cli_post_json(
        "/cli/timer/create",
        &json!({
            "id": id,
            "projectId": project_id,
            "startTime": start_time,
            "endTime": end_time,
            "durationSeconds": duration_seconds,
            "memo": memo,
        }),
    )?;
    let _ = app.emit("sync:timer-entries", ());
    Ok(())
}

#[tauri::command]
pub fn timer_entry_delete(app: AppHandle, id: String) -> Result<(), String> {
    daemon()?
        .cli_post_json("/cli/timer/delete", &json!({ "id": id }))?;
    let _ = app.emit("sync:timer-entries", ());
    Ok(())
}

#[tauri::command]
pub fn timer_entries_export(
    format: String,
    start: Option<i64>,
    end: Option<i64>,
    project_id: Option<String>,
) -> Result<String, String> {
    let s = opt(start);
    let e = opt(end);
    let pid = project_id.unwrap_or_default();
    let mut params: Vec<(&str, &str)> = vec![("format", &format)];
    if !s.is_empty() {
        params.push(("start", &s));
    }
    if !e.is_empty() {
        params.push(("end", &e));
    }
    if !pid.is_empty() {
        params.push(("project_id", &pid));
    }
    // Returns raw text (csv) or json string — both are text/plain or
    // application/json bodies; treat as raw string.
    daemon()?.cli_get("/cli/timer/entries-export", &params)
}

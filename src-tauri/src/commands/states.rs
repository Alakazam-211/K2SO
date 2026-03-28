//! Workspace state management commands.

use tauri::State;
use crate::state::AppState;
use crate::db::schema::WorkspaceState;

#[tauri::command]
pub fn states_list(state: State<'_, AppState>) -> Result<Vec<WorkspaceState>, String> {
    let conn = state.db.lock();
    WorkspaceState::list(&conn).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn states_get(state: State<'_, AppState>, id: String) -> Result<WorkspaceState, String> {
    let conn = state.db.lock();
    WorkspaceState::get(&conn, &id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn states_create(
    state: State<'_, AppState>,
    name: String,
    description: Option<String>,
    cap_features: String,
    cap_issues: String,
    cap_crashes: String,
    cap_security: String,
    cap_audits: String,
    heartbeat: bool,
) -> Result<WorkspaceState, String> {
    let conn = state.db.lock();
    let id = format!("state-{}", uuid::Uuid::new_v4().to_string().split('-').next().unwrap_or("custom"));
    WorkspaceState::create(
        &conn, &id, &name, description.as_deref(),
        &cap_features, &cap_issues, &cap_crashes, &cap_security, &cap_audits, heartbeat,
    ).map_err(|e| e.to_string())?;
    WorkspaceState::get(&conn, &id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn states_update(
    state: State<'_, AppState>,
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
    let conn = state.db.lock();
    WorkspaceState::update(
        &conn, &id,
        name.as_deref(), description.as_deref(),
        cap_features.as_deref(), cap_issues.as_deref(),
        cap_crashes.as_deref(), cap_security.as_deref(),
        cap_audits.as_deref(), heartbeat,
    ).map_err(|e| e.to_string())?;
    WorkspaceState::get(&conn, &id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn states_delete(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let conn = state.db.lock();
    WorkspaceState::delete(&conn, &id).map_err(|e| e.to_string())
}

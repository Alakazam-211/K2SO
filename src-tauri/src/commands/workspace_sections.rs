use tauri::State;
use crate::db::schema::{WorkspaceSection, Workspace};
use crate::state::AppState;

#[tauri::command]
pub fn sections_list(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<Vec<WorkspaceSection>, String> {
    let conn = state.db.lock();
    WorkspaceSection::list(&conn, &project_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn sections_create(
    state: State<'_, AppState>,
    project_id: String,
    name: String,
    color: Option<String>,
) -> Result<WorkspaceSection, String> {
    let conn = state.db.lock();
    let id = uuid::Uuid::new_v4().to_string();

    let existing = WorkspaceSection::list(&conn, &project_id).unwrap_or_default();
    let max_order = existing.iter().map(|s| s.tab_order).max().unwrap_or(-1) + 1;

    WorkspaceSection::create(&conn, &id, &project_id, &name, color.as_deref(), max_order)
        .map_err(|e| e.to_string())?;

    WorkspaceSection::get(&conn, &id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn sections_update(
    state: State<'_, AppState>,
    id: String,
    name: Option<String>,
    color: Option<String>,
    is_collapsed: Option<i64>,
    tab_order: Option<i64>,
) -> Result<WorkspaceSection, String> {
    let conn = state.db.lock();
    WorkspaceSection::update(
        &conn,
        &id,
        name.as_deref(),
        color.as_deref(),
        is_collapsed,
        tab_order,
    )
    .map_err(|e| e.to_string())?;
    WorkspaceSection::get(&conn, &id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn sections_delete(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let conn = state.db.lock();
    WorkspaceSection::delete(&conn, &id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn sections_reorder(state: State<'_, AppState>, ids: Vec<String>) -> Result<(), String> {
    let conn = state.db.lock();
    for (i, id) in ids.iter().enumerate() {
        WorkspaceSection::update(&conn, id, None, None, None, Some(i as i64))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub fn sections_assign_workspace(
    state: State<'_, AppState>,
    workspace_id: String,
    section_id: Option<String>,
) -> Result<Workspace, String> {
    let conn = state.db.lock();
    Workspace::update(
        &conn,
        &workspace_id,
        Some(section_id.as_deref()),
        None,
        None,
        None,
        None,
        None,
    )
    .map_err(|e| e.to_string())?;
    Workspace::get(&conn, &workspace_id).map_err(|e| e.to_string())
}

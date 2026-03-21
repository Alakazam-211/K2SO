use tauri::State;
use crate::db::schema::Workspace;
use crate::state::AppState;

#[tauri::command]
pub fn workspaces_list(state: State<'_, AppState>, project_id: String) -> Result<Vec<Workspace>, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    Workspace::list(&conn, &project_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn workspaces_create(
    state: State<'_, AppState>,
    project_id: String,
    name: String,
    type_: Option<String>,
    branch: Option<String>,
    worktree_path: Option<String>,
) -> Result<Workspace, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let id = uuid::Uuid::new_v4().to_string();
    let type_val = type_.unwrap_or_else(|| "branch".to_string());

    // Get max tab_order for this project
    let existing = Workspace::list(&conn, &project_id).unwrap_or_default();
    let max_order = existing.iter().map(|w| w.tab_order).max().unwrap_or(-1) + 1;

    Workspace::create(
        &conn,
        &id,
        &project_id,
        None,
        &type_val,
        branch.as_deref(),
        &name,
        max_order,
        worktree_path.as_deref(),
    )
    .map_err(|e| e.to_string())?;

    Workspace::get(&conn, &id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn workspaces_delete(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    Workspace::delete(&conn, &id).map_err(|e| e.to_string())
}

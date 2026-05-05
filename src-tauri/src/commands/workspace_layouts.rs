use serde::Serialize;
use tauri::State;

use crate::state::AppState;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceLayout {
    pub project_id: String,
    pub workspace_id: String,
    pub layout_json: String,
}

/// Save (upsert) a workspace layout.
#[tauri::command]
pub fn workspace_layout_save(
    state: State<'_, AppState>,
    project_id: String,
    workspace_id: String,
    layout_json: String,
) -> Result<(), String> {
    let conn = state.db.lock();
    let id = format!("{}:{}", project_id, workspace_id);

    conn.execute(
        "INSERT INTO workspace_layouts (id, project_id, workspace_id, layout_json, updated_at)
         VALUES (?1, ?2, ?3, ?4, unixepoch())
         ON CONFLICT(project_id, workspace_id)
         DO UPDATE SET layout_json = excluded.layout_json, updated_at = unixepoch()",
        rusqlite::params![id, project_id, workspace_id, layout_json],
    )
    .map_err(|e| e.to_string())?;

    Ok(())
}

/// Load a single workspace layout.
#[tauri::command]
pub fn workspace_layout_load(
    state: State<'_, AppState>,
    project_id: String,
    workspace_id: String,
) -> Result<Option<String>, String> {
    let conn = state.db.lock();

    let result = conn.query_row(
        "SELECT layout_json FROM workspace_layouts WHERE project_id = ?1 AND workspace_id = ?2",
        rusqlite::params![project_id, workspace_id],
        |row| row.get::<_, String>(0),
    );

    match result {
        Ok(json) => Ok(Some(json)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// Load all workspace layouts (used on app startup).
#[tauri::command]
pub fn workspace_layout_load_all(
    state: State<'_, AppState>,
) -> Result<Vec<WorkspaceLayout>, String> {
    let conn = state.db.lock();

    let mut stmt = conn
        .prepare("SELECT project_id, workspace_id, layout_json FROM workspace_layouts")
        .map_err(|e| e.to_string())?;

    let layouts = stmt
        .query_map([], |row| {
            Ok(WorkspaceLayout {
                project_id: row.get(0)?,
                workspace_id: row.get(1)?,
                layout_json: row.get(2)?,
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;

    Ok(layouts)
}

/// Delete workspace layout(s) for a project (used when removing a project).
#[tauri::command]
pub fn workspace_layout_delete(
    state: State<'_, AppState>,
    project_id: String,
    workspace_id: Option<String>,
) -> Result<(), String> {
    let conn = state.db.lock();

    if let Some(ws_id) = workspace_id {
        conn.execute(
            "DELETE FROM workspace_layouts WHERE project_id = ?1 AND workspace_id = ?2",
            rusqlite::params![project_id, ws_id],
        )
        .map_err(|e| e.to_string())?;
    } else {
        // Delete all layouts for this project
        conn.execute(
            "DELETE FROM workspace_layouts WHERE project_id = ?1",
            rusqlite::params![project_id],
        )
        .map_err(|e| e.to_string())?;
    }

    Ok(())
}

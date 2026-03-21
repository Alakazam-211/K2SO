use tauri::State;
use crate::git;
use crate::db::schema::Workspace;
use crate::state::AppState;

#[tauri::command]
pub fn git_info(path: String) -> Result<git::GitInfo, String> {
    Ok(git::get_git_info(&path))
}

#[tauri::command]
pub fn git_branches(path: String) -> Result<git::BranchList, String> {
    Ok(git::list_branches(&path))
}

#[tauri::command]
pub fn git_worktrees(path: String) -> Result<Vec<git::WorktreeInfo>, String> {
    Ok(git::list_worktrees(&path))
}

#[tauri::command]
pub fn git_create_worktree(
    state: State<'_, AppState>,
    project_path: String,
    branch: String,
    project_id: String,
    existing_branch: Option<bool>,
) -> Result<serde_json::Value, String> {
    // Create or checkout the git worktree
    let result = if existing_branch.unwrap_or(false) {
        git::checkout_worktree(&project_path, &branch)
    } else {
        git::create_worktree(&project_path, &branch)
    }?;

    // Create workspace record in DB
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let ws_id = uuid::Uuid::new_v4().to_string();
    let existing = Workspace::list(&conn, &project_id).unwrap_or_default();
    let max_order = existing.iter().map(|w| w.tab_order).max().unwrap_or(-1) + 1;

    Workspace::create(
        &conn,
        &ws_id,
        &project_id,
        None,
        "worktree",
        Some(&result.branch),
        &result.branch,
        max_order,
        Some(&result.path),
    )
    .map_err(|e| e.to_string())?;

    Ok(serde_json::json!({
        "workspaceId": ws_id,
        "path": result.path,
        "branch": result.branch,
    }))
}

#[tauri::command]
pub fn git_remove_worktree(
    state: State<'_, AppState>,
    project_path: String,
    worktree_path: String,
    workspace_id: Option<String>,
    force: Option<bool>,
) -> Result<(), String> {
    git::remove_worktree(&project_path, &worktree_path, force.unwrap_or(false))?;

    // Clean up workspace DB record if provided
    if let Some(ws_id) = workspace_id {
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        Workspace::delete(&conn, &ws_id).map_err(|e| e.to_string())?;
    }

    Ok(())
}

#[tauri::command]
pub fn git_reopen_worktree(
    project_path: String,
    worktree_path: String,
    branch: String,
) -> Result<serde_json::Value, String> {
    // Verify the worktree exists
    if !std::path::Path::new(&worktree_path).exists() {
        return Err(format!("Worktree path does not exist: {}", worktree_path));
    }

    Ok(serde_json::json!({
        "path": worktree_path,
        "branch": branch,
        "projectPath": project_path
    }))
}

#[tauri::command]
pub fn git_changes(path: String) -> Result<Vec<git::ChangedFile>, String> {
    Ok(git::get_changed_files(&path))
}

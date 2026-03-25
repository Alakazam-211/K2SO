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
    let conn = state.db.lock();
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
        let conn = state.db.lock();
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

// ── Diff Commands ────────────────────────────────────────────────────────

#[tauri::command]
pub fn git_diff_file(path: String, file_path: String) -> Result<Vec<git::DiffHunk>, String> {
    git::diff_file(&path, &file_path)
}

#[tauri::command]
pub fn git_diff_summary(path: String) -> Result<Vec<git::FileDiffSummary>, String> {
    git::diff_summary(&path)
}

#[tauri::command]
pub fn git_diff_between_branches(
    path: String,
    base_branch: String,
    head_branch: String,
) -> Result<Vec<git::FileDiffSummary>, String> {
    git::diff_between_branches(&path, &base_branch, &head_branch)
}

#[tauri::command]
pub fn git_file_content_at_ref(
    path: String,
    file_path: String,
    git_ref: String,
) -> Result<String, String> {
    git::file_content_at_ref(&path, &file_path, &git_ref)
}

// ── Staging Commands ─────────────────────────────────────────────────────

#[tauri::command]
pub fn git_stage_file(path: String, file_path: String) -> Result<(), String> {
    git::stage_file(&path, &file_path)
}

#[tauri::command]
pub fn git_unstage_file(path: String, file_path: String) -> Result<(), String> {
    git::unstage_file(&path, &file_path)
}

#[tauri::command]
pub fn git_stage_all(path: String) -> Result<(), String> {
    git::stage_all(&path)
}

// ── Commit Command ───────────────────────────────────────────────────────

#[tauri::command]
pub fn git_commit(path: String, message: String) -> Result<git::CommitResult, String> {
    git::commit(&path, &message)
}

// ── Merge Commands ───────────────────────────────────────────────────────

#[tauri::command]
pub fn git_merge_branch(path: String, branch: String) -> Result<git::MergeResult, String> {
    git::merge_branch(&path, &branch)
}

#[tauri::command]
pub fn git_merge_status(path: String) -> Result<git::MergeStatus, String> {
    git::merge_status(&path)
}

#[tauri::command]
pub fn git_abort_merge(path: String) -> Result<(), String> {
    git::abort_merge(&path)
}

#[tauri::command]
pub fn git_resolve_conflict(
    path: String,
    file_path: String,
    resolution: String,
) -> Result<(), String> {
    git::resolve_conflict(&path, &file_path, &resolution)
}

#[tauri::command]
pub fn git_delete_branch(path: String, branch: String) -> Result<(), String> {
    git::delete_branch(&path, &branch)
}

/// Prune stale worktree references and clean up DB records for missing worktrees.
#[tauri::command]
pub fn git_prune_worktrees(
    state: State<'_, AppState>,
    project_path: String,
    project_id: String,
) -> Result<(), String> {
    // Run git worktree prune to clean stale refs
    let _ = std::process::Command::new("git")
        .args(["worktree", "prune"])
        .current_dir(&project_path)
        .output();

    // Check DB workspaces against actual worktree paths
    let conn = state.db.lock();
    let workspaces = Workspace::list(&conn, &project_id).unwrap_or_default();

    for ws in &workspaces {
        if ws.type_ == "worktree" {
            if let Some(ref wt_path) = ws.worktree_path {
                if !std::path::Path::new(wt_path).exists() {
                    // Worktree dir is gone — remove DB record
                    log_debug!("[git] Pruning stale workspace '{}' (path missing: {})", ws.name, wt_path);
                    let _ = Workspace::delete(&conn, &ws.id);
                }
            }
        }
    }

    Ok(())
}

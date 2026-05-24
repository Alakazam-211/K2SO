//! Git commands.
//!
//! Phase 2 Unit 4 — bodies became thin proxies into the daemon's
//! `/cli/git/*` routes. libgit2 operations now run inside the daemon
//! (F5 spawn_blocking on POSTs), so Tauri's own `tokio::task::spawn_blocking`
//! wrappers are no longer needed — the daemon HTTP call is itself blocking
//! (`reqwest::blocking`) and the renderer awaits the IPC anyway.

use k2so_core::git;
use serde_json::json;

use crate::daemon_client::DaemonClient;

fn daemon() -> Result<DaemonClient, String> {
    DaemonClient::try_connect()
}

#[tauri::command]
pub fn git_info(path: String) -> Result<git::GitInfo, String> {
    daemon()?.cli_get_json("/cli/git/info", &[("path", &path)])
}

#[tauri::command]
pub fn git_branches(path: String) -> Result<git::BranchList, String> {
    daemon()?.cli_get_json("/cli/git/branches", &[("path", &path)])
}

#[tauri::command]
pub fn git_worktrees(path: String) -> Result<Vec<git::WorktreeInfo>, String> {
    daemon()?.cli_get_json("/cli/git/worktrees", &[("path", &path)])
}

#[tauri::command]
pub fn git_create_worktree(
    project_path: String,
    branch: String,
    project_id: String,
    existing_branch: Option<bool>,
) -> Result<serde_json::Value, String> {
    daemon()?.cli_post_json_decode(
        "/cli/git/create-worktree",
        &json!({
            "projectPath": project_path,
            "branch": branch,
            "projectId": project_id,
            "existingBranch": existing_branch,
        }),
    )
}

#[tauri::command]
pub fn git_remove_worktree(
    project_path: String,
    worktree_path: String,
    workspace_id: Option<String>,
    force: Option<bool>,
) -> Result<(), String> {
    daemon()?
        .cli_post_json(
            "/cli/git/remove-worktree",
            &json!({
                "projectPath": project_path,
                "worktreePath": worktree_path,
                "workspaceId": workspace_id,
                "force": force,
            }),
        )
        .map(|_| ())
}

#[tauri::command]
pub fn git_reopen_worktree(
    project_path: String,
    worktree_path: String,
    branch: String,
) -> Result<serde_json::Value, String> {
    daemon()?.cli_post_json_decode(
        "/cli/git/reopen-worktree",
        &json!({
            "projectPath": project_path,
            "worktreePath": worktree_path,
            "branch": branch,
        }),
    )
}

#[tauri::command]
pub fn git_changes(path: String) -> Result<Vec<git::ChangedFile>, String> {
    daemon()?.cli_get_json("/cli/git/changes", &[("path", &path)])
}

#[tauri::command]
pub fn git_diff_file(path: String, file_path: String) -> Result<Vec<git::DiffHunk>, String> {
    daemon()?.cli_get_json(
        "/cli/git/diff-file",
        &[("path", &path), ("file_path", &file_path)],
    )
}

#[tauri::command]
pub fn git_diff_summary(path: String) -> Result<Vec<git::FileDiffSummary>, String> {
    daemon()?.cli_get_json("/cli/git/diff-summary", &[("path", &path)])
}

#[tauri::command]
pub fn git_diff_between_branches(
    path: String,
    base_branch: String,
    head_branch: String,
) -> Result<Vec<git::FileDiffSummary>, String> {
    daemon()?.cli_get_json(
        "/cli/git/diff-between",
        &[
            ("path", &path),
            ("base_branch", &base_branch),
            ("head_branch", &head_branch),
        ],
    )
}

#[tauri::command]
pub fn git_file_content_at_ref(
    path: String,
    file_path: String,
    git_ref: String,
) -> Result<String, String> {
    daemon()?.cli_get_json(
        "/cli/git/file-at-ref",
        &[("path", &path), ("file_path", &file_path), ("git_ref", &git_ref)],
    )
}

#[tauri::command]
pub fn git_stage_file(path: String, file_path: String) -> Result<(), String> {
    daemon()?
        .cli_post_json(
            "/cli/git/stage",
            &json!({ "path": path, "filePath": file_path }),
        )
        .map(|_| ())
}

#[tauri::command]
pub fn git_unstage_file(path: String, file_path: String) -> Result<(), String> {
    daemon()?
        .cli_post_json(
            "/cli/git/unstage",
            &json!({ "path": path, "filePath": file_path }),
        )
        .map(|_| ())
}

#[tauri::command]
pub fn git_stage_all(path: String) -> Result<(), String> {
    daemon()?
        .cli_post_json("/cli/git/stage-all", &json!({ "path": path }))
        .map(|_| ())
}

#[tauri::command]
pub fn git_commit(path: String, message: String) -> Result<git::CommitResult, String> {
    daemon()?.cli_post_json_decode(
        "/cli/git/commit",
        &json!({ "path": path, "message": message }),
    )
}

#[tauri::command]
pub fn git_merge_branch(path: String, branch: String) -> Result<git::MergeResult, String> {
    daemon()?.cli_post_json_decode(
        "/cli/git/merge-branch",
        &json!({ "path": path, "branch": branch }),
    )
}

#[tauri::command]
pub fn git_merge_status(path: String) -> Result<git::MergeStatus, String> {
    daemon()?.cli_get_json("/cli/git/merge-status", &[("path", &path)])
}

#[tauri::command]
pub fn git_abort_merge(path: String) -> Result<(), String> {
    daemon()?
        .cli_post_json("/cli/git/abort-merge", &json!({ "path": path }))
        .map(|_| ())
}

#[tauri::command]
pub fn git_resolve_conflict(
    path: String,
    file_path: String,
    resolution: String,
) -> Result<(), String> {
    daemon()?
        .cli_post_json(
            "/cli/git/resolve",
            &json!({
                "path": path,
                "filePath": file_path,
                "resolution": resolution,
            }),
        )
        .map(|_| ())
}

#[tauri::command]
pub fn git_delete_branch(path: String, branch: String) -> Result<(), String> {
    daemon()?
        .cli_post_json(
            "/cli/git/delete-branch",
            &json!({ "path": path, "branch": branch }),
        )
        .map(|_| ())
}

#[tauri::command]
pub fn git_prune_worktrees(
    project_path: String,
    project_id: String,
) -> Result<(), String> {
    daemon()?
        .cli_post_json(
            "/cli/git/prune-worktrees",
            &json!({
                "projectPath": project_path,
                "projectId": project_id,
            }),
        )
        .map(|_| ())
}

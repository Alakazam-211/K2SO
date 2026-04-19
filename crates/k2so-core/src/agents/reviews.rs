//! Review queue: workspace-manager approval path for agent work.
//!
//! When an agent moves an item to `done/`, its branch lands on the
//! review queue. The workspace manager (or the CLI) then:
//!
//! - [`review_queue`] — list of per-agent `ReviewItem`s with diff
//!   summary + worktree path, what the UI renders in the Review panel.
//! - [`review_approve`] — merge agent branch → main, remove worktree,
//!   delete branch, archive done items, unlock the agent.
//! - [`review_reject`] — discard worktree + branch, move done items
//!   back to `inbox/` (stripped of worktree frontmatter), optionally
//!   drop a feedback file so the next attempt has context. Unlock.
//! - [`review_request_changes`] — just a feedback file in inbox. No
//!   worktree teardown; the agent keeps their working branch.
//!
//! Moved to core so the daemon can serve `/cli/reviews` +
//! `/cli/review/{approve,reject,feedback}` headlessly.

use serde::{Deserialize, Serialize};
use std::fs;

use crate::agents::delegate::strip_worktree_from_frontmatter;
use crate::agents::parse_frontmatter;
use crate::agents::scheduler::{agent_work_dir, get_workspace_state};
use crate::agents::session::{k2so_agents_unlock, simple_date};
use crate::agents::work_item::{atomic_write, read_work_item, WorkItem};
use crate::agents::agents_dir;

/// One file in the branch diff between main and the agent's worktree.
/// Mirrors `crate::git::FileDiffSummary` but drops the `old_path` field
/// since the review UI doesn't surface renames yet.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewDiffFile {
    pub path: String,
    pub status: String,
    pub additions: u32,
    pub deletions: u32,
}

/// One entry on the review queue — an agent with done items plus its
/// associated worktree/branch and the diff summary vs main.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewItem {
    pub agent_name: String,
    pub branch: String,
    pub worktree_path: Option<String>,
    pub work_items: Vec<WorkItem>,
    pub diff_summary: Vec<ReviewDiffFile>,
}

/// Enumerate the review queue. Agents without a `done/` directory OR
/// with an empty `done/` are skipped. Branch-to-agent matching is by
/// convention: the agent's name appears in the branch, or the branch
/// starts with `agent/`.
pub fn review_queue(project_path: &str) -> Result<Vec<ReviewItem>, String> {
    let dir = agents_dir(project_path);
    if !dir.exists() {
        return Ok(vec![]);
    }

    let worktrees = crate::git::list_worktrees(project_path);

    let mut reviews = Vec::new();
    let entries = fs::read_dir(&dir).map_err(|e| e.to_string())?;

    for entry in entries.flatten() {
        if !entry.file_type().map_or(false, |ft| ft.is_dir()) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        let done_dir = agent_work_dir(project_path, &name, "done");

        if !done_dir.exists() {
            continue;
        }

        let done_items: Vec<WorkItem> = fs::read_dir(&done_dir)
            .ok()
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|e| e.path().extension().map_or(false, |ext| ext == "md"))
                    .filter_map(|e| read_work_item(&e.path(), "done"))
                    .collect()
            })
            .unwrap_or_default();

        if done_items.is_empty() {
            continue;
        }

        let matching_worktree = worktrees.iter().find(|wt| {
            !wt.is_main && (wt.branch.contains(&name) || wt.branch.starts_with("agent/"))
        });

        let diff_summary: Vec<ReviewDiffFile> = if let Some(wt) = matching_worktree {
            crate::git::diff_between_branches(project_path, "main", &wt.branch)
                .unwrap_or_default()
                .into_iter()
                .map(|f| ReviewDiffFile {
                    path: f.path,
                    status: f.status,
                    additions: f.additions,
                    deletions: f.deletions,
                })
                .collect()
        } else {
            vec![]
        };

        reviews.push(ReviewItem {
            agent_name: name,
            branch: matching_worktree.map(|wt| wt.branch.clone()).unwrap_or_default(),
            worktree_path: matching_worktree.map(|wt| wt.path.clone()),
            work_items: done_items,
            diff_summary,
        });
    }

    Ok(reviews)
}

/// Approve + merge + tear down. Returns a short human-readable status
/// string (e.g., `"Approved and merged: 7 files"`).
pub fn review_approve(
    project_path: String,
    branch: String,
    agent_name: String,
) -> Result<String, String> {
    let result = crate::git::merge_branch(&project_path, &branch)?;

    if !result.success {
        return Err(format!("Merge conflicts: {}", result.conflicts.join(", ")));
    }

    // Find the agent's worktree by branch, remove it, drop DB row.
    let worktrees = crate::git::list_worktrees(&project_path);
    if let Some(wt) = worktrees.iter().find(|wt| wt.branch == branch) {
        let wt_path = wt.path.clone();
        let _ = crate::git::remove_worktree(&project_path, &wt_path, true);

        {
            let db = crate::db::shared();
            let conn = db.lock();
            let _ = conn.execute(
                "DELETE FROM workspaces WHERE worktree_path = ?1",
                rusqlite::params![wt_path],
            );
        }
    }

    let _ = crate::git::delete_branch(&project_path, &branch);

    // Archive done items for this agent (they live in git now).
    let done_dir = agent_work_dir(&project_path, &agent_name, "done");
    if done_dir.exists() {
        if let Ok(entries) = fs::read_dir(&done_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().map_or(false, |ext| ext == "md") {
                    let _ = fs::remove_file(&path);
                }
            }
        }
    }

    let _ = k2so_agents_unlock(project_path, agent_name);

    Ok(format!("Approved and merged: {} files", result.merged_files))
}

/// Reject the branch. Tears down the worktree, moves done items back
/// to `inbox/` (stripped of worktree frontmatter so a fresh worktree
/// is created on retry), optionally writes a `review-feedback-*.md`
/// with the rejection reason, unlocks the agent.
pub fn review_reject(
    project_path: String,
    agent_name: String,
    reason: Option<String>,
) -> Result<(), String> {
    let done_dir = agent_work_dir(&project_path, &agent_name, "done");
    let inbox_dir = agent_work_dir(&project_path, &agent_name, "inbox");

    if !done_dir.exists() {
        return Ok(());
    }

    // Nuke the worktree + branch + DB workspace row for this agent.
    let worktrees = crate::git::list_worktrees(&project_path);
    let agent_prefix = format!("agent/{}/", agent_name);
    for wt in worktrees.iter().filter(|wt| wt.branch.starts_with(&agent_prefix)) {
        let wt_path = wt.path.clone();
        if let Err(e) = crate::git::remove_worktree(&project_path, &wt_path, true) {
            crate::log_debug!("[review-reject] Failed to remove worktree {}: {}", wt_path, e);
        }
        if let Err(e) = crate::git::delete_branch(&project_path, &wt.branch) {
            crate::log_debug!("[review-reject] Failed to delete branch {}: {}", wt.branch, e);
        }
        {
            let db = crate::db::shared();
            let conn = db.lock();
            let _ = conn.execute(
                "DELETE FROM workspaces WHERE worktree_path = ?1",
                rusqlite::params![wt_path],
            );
        }
    }

    fs::create_dir_all(&inbox_dir).map_err(|e| format!("Failed to create inbox dir: {}", e))?;
    if let Ok(entries) = fs::read_dir(&done_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map_or(false, |ext| ext == "md") {
                let filename = match path.file_name() {
                    Some(f) => f.to_owned(),
                    None => continue,
                };
                let target = inbox_dir.join(&filename);
                if let Ok(content) = fs::read_to_string(&path) {
                    let cleaned = strip_worktree_from_frontmatter(&content);
                    if let Err(e) = atomic_write(&target, &cleaned) {
                        crate::log_debug!("[review-reject] Failed to write cleaned work item: {}", e);
                    }
                } else if let Err(e) = fs::rename(&path, &target) {
                    crate::log_debug!("[review-reject] Failed to move work item: {}", e);
                }
                let _ = fs::remove_file(&path);
            }
        }
    }

    if let Some(reason) = reason {
        let now = simple_date();
        let content = format!(
            "---\ntitle: Review Feedback — Work Rejected\npriority: high\nassigned_by: reviewer\ncreated: {}\ntype: feedback\n---\n\n## Rejection Reason\n\n{}\n\n## Action Required\n\nReview the feedback above and address the issues in your next attempt.\nA fresh worktree will be created when you are relaunched.\n",
            now, reason
        );
        let filename = format!("review-feedback-{}.md", now);
        let path = inbox_dir.join(&filename);
        atomic_write(&path, &content)?;
    }

    let _ = k2so_agents_unlock(project_path, agent_name);

    Ok(())
}

/// Drop a feedback file in the agent's inbox without tearing down
/// their worktree. The agent keeps their branch; they address the
/// feedback and move the item to `done/` again.
pub fn review_request_changes(
    project_path: String,
    agent_name: String,
    feedback: String,
) -> Result<(), String> {
    let inbox_dir = agent_work_dir(&project_path, &agent_name, "inbox");
    if !inbox_dir.exists() {
        fs::create_dir_all(&inbox_dir).map_err(|e| e.to_string())?;
    }

    let now = simple_date();
    let content = format!(
        "---\ntitle: Review Feedback — Changes Requested\npriority: high\nassigned_by: reviewer\ncreated: {}\ntype: feedback\n---\n\n## Requested Changes\n\n{}\n\n## Action Required\n\nAddress the feedback above, then move this item to done/ when complete.\n",
        now, feedback
    );
    let filename = format!("review-feedback-{}.md", now);
    let path = inbox_dir.join(&filename);
    atomic_write(&path, &content)?;

    Ok(())
}

/// Sub-agent completion. Reads the work item's frontmatter, consults
/// the workspace state's capability for the item's `source`, and
/// either auto-merges (`auto` mode — delegates to [`review_approve`])
/// or moves the file from `active/` to `done/` for human review
/// (`gated` mode). Returns JSON the CLI echoes back.
pub fn agent_complete(
    project_path: String,
    agent_name: String,
    filename: String,
) -> Result<String, String> {
    let active_dir = agent_work_dir(&project_path, &agent_name, "active");
    let item_path = active_dir.join(&filename);
    if !item_path.exists() {
        return Err(format!("Work item not found: {}", filename));
    }
    let content = fs::read_to_string(&item_path).unwrap_or_default();
    let fm = parse_frontmatter(&content);
    let source = fm
        .get("source")
        .cloned()
        .unwrap_or_else(|| "manual".to_string());

    let capability = if let Some(ws_state) = get_workspace_state(&project_path) {
        ws_state.capability_for_source(&source).to_string()
    } else {
        "gated".to_string()
    };

    let branch = fm.get("branch").cloned().unwrap_or_default();

    if capability == "auto" && !branch.is_empty() {
        match review_approve(project_path.clone(), branch.clone(), agent_name.clone()) {
            Ok(_) => Ok(serde_json::json!({
                "mode": "auto",
                "action": "merged",
                "branch": branch,
                "agent": agent_name,
            })
            .to_string()),
            Err(e) => Err(format!("Auto-merge failed: {}", e)),
        }
    } else {
        let done_dir = agent_work_dir(&project_path, &agent_name, "done");
        fs::create_dir_all(&done_dir).ok();
        let dest = done_dir.join(&filename);
        fs::rename(&item_path, &dest).map_err(|e| format!("Failed to move to done: {}", e))?;

        Ok(serde_json::json!({
            "mode": "gated",
            "action": "moved_to_done",
            "branch": branch,
            "agent": agent_name,
            "file": filename,
        })
        .to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn review_item_serializes_camel_case() {
        let item = ReviewItem {
            agent_name: "backend".to_string(),
            branch: "agent/backend/task".to_string(),
            worktree_path: Some("/tmp/wt".to_string()),
            work_items: vec![],
            diff_summary: vec![],
        };
        let json = serde_json::to_string(&item).unwrap();
        assert!(json.contains("\"agentName\":\"backend\""));
        assert!(json.contains("\"worktreePath\":\"/tmp/wt\""));
        assert!(json.contains("\"workItems\":[]"));
        assert!(json.contains("\"diffSummary\":[]"));
    }

    #[test]
    fn review_diff_file_serializes_camel_case() {
        let f = ReviewDiffFile {
            path: "src/main.rs".to_string(),
            status: "M".to_string(),
            additions: 3,
            deletions: 1,
        };
        let json = serde_json::to_string(&f).unwrap();
        assert!(json.contains("\"path\":\"src/main.rs\""));
        assert!(json.contains("\"additions\":3"));
    }
}

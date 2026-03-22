use git2::{Repository, StatusOptions, BranchType};
use serde::Serialize;
use std::path::Path;
use std::process::Command;

// ── Types ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitInfo {
    pub is_repo: bool,
    pub current_branch: String,
    pub ahead: i32,
    pub behind: i32,
    pub changed_files: i32,
    pub untracked_files: i32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchList {
    pub current: String,
    pub local: Vec<String>,
    pub remote: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeInfo {
    pub path: String,
    pub branch: String,
    pub is_main: bool,
    pub is_bare: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChangedFile {
    pub path: String,
    pub status: String,
}

// ── Git Info ──────────────────────────────────────────────────────────────

pub fn get_git_info(path: &str) -> GitInfo {
    let default = GitInfo {
        is_repo: false,
        current_branch: String::new(),
        ahead: 0,
        behind: 0,
        changed_files: 0,
        untracked_files: 0,
    };

    let repo = match Repository::discover(path) {
        Ok(r) => r,
        Err(_) => return default,
    };

    // Get current branch
    let head = match repo.head() {
        Ok(h) => h,
        Err(_) => {
            return GitInfo {
                is_repo: true,
                ..default
            };
        }
    };

    let current_branch = head
        .shorthand()
        .unwrap_or("")
        .to_string();

    // Count changed and untracked files
    let mut opts = StatusOptions::new();
    opts.include_untracked(true);
    opts.recurse_untracked_dirs(true);

    let (changed_files, untracked_files) = match repo.statuses(Some(&mut opts)) {
        Ok(statuses) => {
            let mut changed = 0i32;
            let mut untracked = 0i32;
            for entry in statuses.iter() {
                let s = entry.status();
                if s.contains(git2::Status::WT_NEW) || s.contains(git2::Status::INDEX_NEW) {
                    // Check if it's purely untracked (not staged as new)
                    if s.contains(git2::Status::WT_NEW) && !s.contains(git2::Status::INDEX_NEW) {
                        untracked += 1;
                    } else {
                        changed += 1;
                    }
                } else {
                    changed += 1;
                }
            }
            (changed, untracked)
        }
        Err(_) => (0, 0),
    };

    // Get ahead/behind counts
    let (ahead, behind) = get_ahead_behind(&repo, &current_branch);

    GitInfo {
        is_repo: true,
        current_branch,
        ahead,
        behind,
        changed_files,
        untracked_files,
    }
}

/// Calculate ahead/behind relative to upstream tracking branch.
fn get_ahead_behind(repo: &Repository, branch_name: &str) -> (i32, i32) {
    let local_branch = match repo.find_branch(branch_name, BranchType::Local) {
        Ok(b) => b,
        Err(_) => return (0, 0),
    };

    let upstream = match local_branch.upstream() {
        Ok(u) => u,
        Err(_) => return (0, 0),
    };

    let local_oid = match local_branch.get().target() {
        Some(o) => o,
        None => return (0, 0),
    };

    let upstream_oid = match upstream.get().target() {
        Some(o) => o,
        None => return (0, 0),
    };

    match repo.graph_ahead_behind(local_oid, upstream_oid) {
        Ok((ahead, behind)) => (ahead as i32, behind as i32),
        Err(_) => (0, 0),
    }
}

// ── List Branches ─────────────────────────────────────────────────────────

pub fn list_branches(path: &str) -> BranchList {
    let default = BranchList {
        current: String::new(),
        local: Vec::new(),
        remote: Vec::new(),
    };

    let repo = match Repository::discover(path) {
        Ok(r) => r,
        Err(_) => return default,
    };

    let current = repo
        .head()
        .ok()
        .and_then(|h| h.shorthand().map(|s| s.to_string()))
        .unwrap_or_default();

    let mut local = Vec::new();
    let mut remote = Vec::new();

    if let Ok(branches) = repo.branches(None) {
        for branch_result in branches {
            if let Ok((branch, branch_type)) = branch_result {
                let name = branch
                    .name()
                    .ok()
                    .flatten()
                    .unwrap_or("")
                    .to_string();

                if name.is_empty() {
                    continue;
                }

                match branch_type {
                    BranchType::Local => local.push(name),
                    BranchType::Remote => {
                        // Skip HEAD pointers
                        if !name.ends_with("/HEAD") {
                            remote.push(name);
                        }
                    }
                }
            }
        }
    }

    BranchList {
        current,
        local,
        remote,
    }
}

// ── List Worktrees ────────────────────────────────────────────────────────

pub fn list_worktrees(path: &str) -> Vec<WorktreeInfo> {
    // Use git CLI for worktree list since git2 worktree support is limited
    let output = match Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(path)
        .output()
    {
        Ok(o) => o,
        Err(_) => return Vec::new(),
    };

    if !output.status.success() {
        return Vec::new();
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut worktrees = Vec::new();
    let mut current_path: Option<String> = None;
    let mut current_branch: Option<String> = None;
    let mut is_bare = false;

    for line in stdout.lines() {
        if let Some(wt_path) = line.strip_prefix("worktree ") {
            current_path = Some(wt_path.to_string());
        } else if let Some(branch_ref) = line.strip_prefix("branch ") {
            // branch refs/heads/main -> main
            current_branch = Some(
                branch_ref
                    .strip_prefix("refs/heads/")
                    .unwrap_or(branch_ref)
                    .to_string(),
            );
        } else if line == "bare" {
            is_bare = true;
        } else if line.is_empty() {
            if let Some(ref wt_path) = current_path {
                worktrees.push(WorktreeInfo {
                    path: wt_path.clone(),
                    branch: current_branch.take().unwrap_or_else(|| "(detached)".to_string()),
                    is_main: worktrees.is_empty(),
                    is_bare,
                });
            }
            current_path = None;
            current_branch = None;
            is_bare = false;
        }
    }

    // Handle last entry if no trailing newline
    if let Some(wt_path) = current_path {
        worktrees.push(WorktreeInfo {
            path: wt_path,
            branch: current_branch.unwrap_or_else(|| "(detached)".to_string()),
            is_main: worktrees.is_empty(),
            is_bare,
        });
    }

    worktrees
}

// ── Create Worktree ───────────────────────────────────────────────────────

pub fn create_worktree(
    project_path: &str,
    branch_name: &str,
) -> Result<WorktreeCreateResult, String> {
    let worktrees_dir = Path::new(project_path).join(".worktrees");

    // Create worktrees directory if needed
    if !worktrees_dir.exists() {
        std::fs::create_dir_all(&worktrees_dir)
            .map_err(|e| format!("Failed to create worktrees directory: {}", e))?;
    }

    // Auto-add .worktrees/ to .gitignore
    let gitignore_path = Path::new(project_path).join(".gitignore");
    let gitignore_entry = ".worktrees/";

    let needs_gitignore_update = if gitignore_path.exists() {
        let content = std::fs::read_to_string(&gitignore_path).unwrap_or_default();
        !content.lines().any(|line| line.trim() == gitignore_entry)
    } else {
        true
    };

    if needs_gitignore_update {
        use std::fs::OpenOptions;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&gitignore_path)
            .map_err(|e| format!("Failed to update .gitignore: {}", e))?;
        // Add newline before entry if file doesn't end with one
        if gitignore_path.exists() {
            let content = std::fs::read_to_string(&gitignore_path).unwrap_or_default();
            if !content.is_empty() && !content.ends_with('\n') {
                use std::io::Write;
                writeln!(file).ok();
            }
        }
        use std::io::Write;
        writeln!(file, "{}", gitignore_entry)
            .map_err(|e| format!("Failed to write .gitignore: {}", e))?;
    }

    let safe_branch = branch_name.replace('/', "-");
    let worktree_path = worktrees_dir.join(&safe_branch);
    let wt_path_str = worktree_path.to_string_lossy().to_string();

    // Always create a new branch from HEAD
    let output = Command::new("git")
        .args(["worktree", "add", "-b", branch_name, &wt_path_str, "HEAD"])
        .current_dir(project_path)
        .output()
        .map_err(|e| format!("Failed to run git worktree add: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git worktree add failed: {}", stderr));
    }

    Ok(WorktreeCreateResult {
        path: wt_path_str,
        branch: branch_name.to_string(),
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct WorktreeCreateResult {
    pub path: String,
    pub branch: String,
}

// ── Checkout Existing Branch as Worktree ─────────────────────────────

pub fn checkout_worktree(
    project_path: &str,
    branch_name: &str,
) -> Result<WorktreeCreateResult, String> {
    let worktrees_dir = Path::new(project_path).join(".worktrees");

    // Create worktrees directory if needed
    if !worktrees_dir.exists() {
        std::fs::create_dir_all(&worktrees_dir)
            .map_err(|e| format!("Failed to create worktrees directory: {}", e))?;
    }

    // Auto-add .worktrees/ to .gitignore
    let gitignore_path = Path::new(project_path).join(".gitignore");
    let gitignore_entry = ".worktrees/";

    let needs_gitignore_update = if gitignore_path.exists() {
        let content = std::fs::read_to_string(&gitignore_path).unwrap_or_default();
        !content.lines().any(|line| line.trim() == gitignore_entry)
    } else {
        true
    };

    if needs_gitignore_update {
        use std::fs::OpenOptions;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&gitignore_path)
            .map_err(|e| format!("Failed to update .gitignore: {}", e))?;
        if gitignore_path.exists() {
            let content = std::fs::read_to_string(&gitignore_path).unwrap_or_default();
            if !content.is_empty() && !content.ends_with('\n') {
                use std::io::Write;
                writeln!(file).ok();
            }
        }
        use std::io::Write;
        writeln!(file, "{}", gitignore_entry)
            .map_err(|e| format!("Failed to write .gitignore: {}", e))?;
    }

    let safe_branch = branch_name.replace('/', "-");
    let worktree_path = worktrees_dir.join(&safe_branch);
    let wt_path_str = worktree_path.to_string_lossy().to_string();

    // Checkout existing branch (no -b flag)
    let output = Command::new("git")
        .args(["worktree", "add", &wt_path_str, branch_name])
        .current_dir(project_path)
        .output()
        .map_err(|e| format!("Failed to run git worktree add: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git worktree add failed: {}", stderr));
    }

    Ok(WorktreeCreateResult {
        path: wt_path_str,
        branch: branch_name.to_string(),
    })
}

// ── Remove Worktree ───────────────────────────────────────────────────────

pub fn remove_worktree(
    project_path: &str,
    worktree_path: &str,
    force: bool,
) -> Result<(), String> {
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.push(worktree_path);

    let output = Command::new("git")
        .args(&args)
        .current_dir(project_path)
        .output()
        .map_err(|e| format!("Failed to run git worktree remove: {}", e))?;

    if !output.status.success() {
        // Fallback: trash the worktree directory and prune references
        if Path::new(worktree_path).exists() {
            let _ = trash::delete(worktree_path);
        }

        // Prune stale worktree references
        let _ = Command::new("git")
            .args(["worktree", "prune"])
            .current_dir(project_path)
            .output();
    }

    Ok(())
}

// ── Get Changed Files ─────────────────────────────────────────────────────

pub fn get_changed_files(path: &str) -> Vec<ChangedFile> {
    let repo = match Repository::discover(path) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };

    let mut opts = StatusOptions::new();
    opts.include_untracked(true);
    opts.recurse_untracked_dirs(true);

    let statuses = match repo.statuses(Some(&mut opts)) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };

    let mut files = Vec::new();

    for entry in statuses.iter() {
        let file_path = entry.path().unwrap_or("").to_string();
        let s = entry.status();

        let status = if s.contains(git2::Status::WT_NEW) {
            "untracked"
        } else if s.contains(git2::Status::INDEX_NEW) {
            "added"
        } else if s.contains(git2::Status::WT_DELETED) || s.contains(git2::Status::INDEX_DELETED) {
            "deleted"
        } else {
            "modified"
        };

        files.push(ChangedFile {
            path: file_path,
            status: status.to_string(),
        });
    }

    files
}

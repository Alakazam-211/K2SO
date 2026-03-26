//! K2SO Agent system — autonomous AI workers operating within workspaces.
//!
//! Agents have a work queue (inbox/active/done) of markdown files,
//! a profile (agent.md), and interact with K2SO via the CLI bridge.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

// ── Types ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct K2soAgentInfo {
    pub name: String,
    pub role: String,
    pub inbox_count: usize,
    pub active_count: usize,
    pub done_count: usize,
    pub pod_leader: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkItem {
    pub filename: String,
    pub title: String,
    pub priority: String,
    pub assigned_by: String,
    pub created: String,
    pub item_type: String,
    pub folder: String,
}

// ── Path helpers ────────────────────────────────────────────────────────

fn agents_dir(project_path: &str) -> PathBuf {
    PathBuf::from(project_path).join(".k2so").join("agents")
}

fn agent_dir(project_path: &str, agent_name: &str) -> PathBuf {
    agents_dir(project_path).join(agent_name)
}

fn agent_work_dir(project_path: &str, agent_name: &str, folder: &str) -> PathBuf {
    agent_dir(project_path, agent_name).join("work").join(folder)
}

fn workspace_inbox_dir(project_path: &str) -> PathBuf {
    PathBuf::from(project_path).join(".k2so").join("work").join("inbox")
}

// ── Frontmatter parsing ────────────────────────────────────────────────

fn parse_frontmatter(content: &str) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    if !content.starts_with("---") {
        return map;
    }
    if let Some(end) = content[3..].find("---") {
        let frontmatter = &content[3..3 + end];
        for line in frontmatter.lines() {
            if let Some((key, value)) = line.split_once(':') {
                let key = key.trim().to_string();
                let value = value.trim().to_string();
                if !key.is_empty() && !value.is_empty() {
                    map.insert(key, value);
                }
            }
        }
    }
    map
}

fn read_work_item(path: &Path, folder: &str) -> Option<WorkItem> {
    let content = fs::read_to_string(path).ok()?;
    let fm = parse_frontmatter(&content);
    let filename = path.file_name()?.to_string_lossy().to_string();
    Some(WorkItem {
        filename,
        title: fm.get("title").cloned().unwrap_or_default(),
        priority: fm.get("priority").cloned().unwrap_or("normal".to_string()),
        assigned_by: fm.get("assigned_by").cloned().unwrap_or("unknown".to_string()),
        created: fm.get("created").cloned().unwrap_or_default(),
        item_type: fm.get("type").cloned().unwrap_or("task".to_string()),
        folder: folder.to_string(),
    })
}

fn count_md_files(dir: &Path) -> usize {
    fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter(|e| e.path().extension().map_or(false, |ext| ext == "md"))
                .count()
        })
        .unwrap_or(0)
}

// ── Tauri Commands ──────────────────────────────────────────────────────

/// List all K2SO agents in a project.
#[tauri::command]
pub fn k2so_agents_list(project_path: String) -> Result<Vec<K2soAgentInfo>, String> {
    let dir = agents_dir(&project_path);
    if !dir.exists() {
        return Ok(vec![]);
    }

    let mut agents = Vec::new();
    let entries = fs::read_dir(&dir).map_err(|e| e.to_string())?;

    for entry in entries.flatten() {
        if !entry.file_type().map_or(false, |ft| ft.is_dir()) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        let agent_md = entry.path().join("agent.md");

        let (role, is_pod_leader) = if agent_md.exists() {
            let content = fs::read_to_string(&agent_md).unwrap_or_default();
            let fm = parse_frontmatter(&content);
            let role = fm.get("role").cloned().unwrap_or_default();
            let is_leader = fm.get("pod_leader").map(|v| v == "true").unwrap_or(false);
            (role, is_leader)
        } else {
            (String::new(), false)
        };

        let inbox_count = count_md_files(&agent_work_dir(&project_path, &name, "inbox"));
        let active_count = count_md_files(&agent_work_dir(&project_path, &name, "active"));
        let done_count = count_md_files(&agent_work_dir(&project_path, &name, "done"));

        agents.push(K2soAgentInfo {
            name,
            role,
            inbox_count,
            active_count,
            done_count,
            pod_leader: is_pod_leader,
        });
    }

    agents.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(agents)
}

/// Create a new K2SO agent with directory structure.
#[tauri::command]
pub fn k2so_agents_create(
    project_path: String,
    name: String,
    role: String,
    prompt: Option<String>,
) -> Result<K2soAgentInfo, String> {
    if !name.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
        return Err("Agent name must be alphanumeric (hyphens and underscores allowed)".to_string());
    }

    let dir = agent_dir(&project_path, &name);
    if dir.exists() {
        return Err(format!("Agent '{}' already exists", name));
    }

    fs::create_dir_all(agent_work_dir(&project_path, &name, "inbox"))
        .map_err(|e| format!("Failed to create inbox: {}", e))?;
    fs::create_dir_all(agent_work_dir(&project_path, &name, "active"))
        .map_err(|e| format!("Failed to create active: {}", e))?;
    fs::create_dir_all(agent_work_dir(&project_path, &name, "done"))
        .map_err(|e| format!("Failed to create done: {}", e))?;
    let _ = fs::create_dir_all(workspace_inbox_dir(&project_path));

    let agent_md = dir.join("agent.md");
    let content = format!(
        "---\nname: {}\nrole: {}\n---\n\n{}\n",
        name,
        role,
        prompt.unwrap_or_default()
    );
    fs::write(&agent_md, content).map_err(|e| format!("Failed to write agent.md: {}", e))?;

    Ok(K2soAgentInfo {
        name,
        role,
        inbox_count: 0,
        active_count: 0,
        done_count: 0,
        pod_leader: false,
    })
}

/// Delete a K2SO agent and its directory.
#[tauri::command]
pub fn k2so_agents_delete(project_path: String, name: String) -> Result<(), String> {
    let dir = agent_dir(&project_path, &name);
    if !dir.exists() {
        return Err(format!("Agent '{}' does not exist", name));
    }
    fs::remove_dir_all(&dir).map_err(|e| format!("Failed to delete agent: {}", e))?;
    Ok(())
}

/// Get work items for a K2SO agent.
#[tauri::command]
pub fn k2so_agents_work_list(
    project_path: String,
    agent_name: String,
    folder: Option<String>,
) -> Result<Vec<WorkItem>, String> {
    let folders = match folder.as_deref() {
        Some(f) => vec![f.to_string()],
        None => vec!["inbox".to_string(), "active".to_string(), "done".to_string()],
    };

    let mut items = Vec::new();
    for f in &folders {
        let dir = agent_work_dir(&project_path, &agent_name, f);
        if !dir.exists() {
            continue;
        }
        for entry in fs::read_dir(&dir).map_err(|e| e.to_string())?.flatten() {
            let path = entry.path();
            if path.extension().map_or(false, |ext| ext == "md") {
                if let Some(item) = read_work_item(&path, f) {
                    items.push(item);
                }
            }
        }
    }

    Ok(items)
}

/// Create a work item in a K2SO agent's inbox (or unassigned).
#[tauri::command]
pub fn k2so_agents_work_create(
    project_path: String,
    agent_name: Option<String>,
    title: String,
    body: String,
    priority: Option<String>,
    item_type: Option<String>,
) -> Result<WorkItem, String> {
    let target_dir = match &agent_name {
        Some(name) => {
            let dir = agent_work_dir(&project_path, name, "inbox");
            if !dir.exists() {
                return Err(format!("Agent '{}' does not exist", name));
            }
            dir
        }
        None => {
            let dir = workspace_inbox_dir(&project_path);
            fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            dir
        }
    };

    let priority = priority.unwrap_or_else(|| "normal".to_string());
    let item_type = item_type.unwrap_or_else(|| "task".to_string());

    let slug: String = title
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<&str>>()
        .join("-");
    let slug = &slug[..slug.len().min(60)];
    let filename = format!("{}.md", slug);

    let now = simple_date();
    let content = format!(
        "---\ntitle: {}\npriority: {}\nassigned_by: user\ncreated: {}\ntype: {}\n---\n\n{}\n",
        title, priority, now, item_type, body
    );

    let path = target_dir.join(&filename);
    fs::write(&path, &content).map_err(|e| format!("Failed to write work item: {}", e))?;

    Ok(WorkItem {
        filename,
        title,
        priority,
        assigned_by: "user".to_string(),
        created: now,
        item_type,
        folder: if agent_name.is_some() { "inbox".to_string() } else { "workspace-inbox".to_string() },
    })
}

/// Delegate a work item to an agent — the all-in-one command.
///
/// This is the primary way the lead agent assigns work. In one step, K2SO:
/// 1. Moves the work item to the target agent's active/ folder
/// 2. Creates a worktree (branch: `agent/<name>/<task-slug>`)
/// 3. Writes a task-specific CLAUDE.md into the worktree root
/// 4. Updates the work item frontmatter with worktree_path and branch
/// 5. Emits a `cli:agent-launch` event so the frontend opens a Claude terminal
///
/// Returns JSON with { worktreePath, branch, agentName, taskFile } for the frontend.
#[tauri::command]
pub fn k2so_agents_delegate(
    project_path: String,
    target_agent: String,
    source_file: String,
) -> Result<serde_json::Value, String> {
    let source = PathBuf::from(&source_file);
    if !source.exists() {
        return Err(format!("Source file does not exist: {}", source_file));
    }

    let agent_d = agent_dir(&project_path, &target_agent);
    if !agent_d.exists() {
        return Err(format!("Target agent '{}' does not exist", target_agent));
    }

    // Read the work item
    let content = fs::read_to_string(&source).map_err(|e| e.to_string())?;
    let item = read_work_item(&source, "inbox")
        .ok_or_else(|| "Could not parse work item".to_string())?;

    // 1. Create a worktree for this task
    let task_slug = item.filename.trim_end_matches(".md");
    let branch_name = format!("agent/{}/{}", target_agent, task_slug);
    let worktree = crate::git::create_worktree(&project_path, &branch_name)
        .map_err(|e| format!("Failed to create worktree: {}", e))?;

    // 2. Move work item to agent's active/ folder with worktree info
    let active_dir = agent_work_dir(&project_path, &target_agent, "active");
    fs::create_dir_all(&active_dir).ok();
    let updated = update_assigned_by(&content, "delegated");
    let updated = add_worktree_to_frontmatter(&updated, &worktree.path, &worktree.branch);
    let active_file = active_dir.join(&item.filename);
    fs::write(&active_file, &updated)
        .map_err(|e| format!("Failed to write active work item: {}", e))?;
    fs::remove_file(&source).map_err(|e| format!("Failed to remove source: {}", e))?;

    // 3. Generate a task-specific CLAUDE.md and write it to the worktree root
    let claude_md = generate_agent_claude_md_content(&project_path, &target_agent, Some(&item))?;
    let claude_md_path = PathBuf::from(&worktree.path).join("CLAUDE.md");
    fs::write(&claude_md_path, &claude_md)
        .map_err(|e| format!("Failed to write CLAUDE.md to worktree: {}", e))?;

    // 4. Build the launch command for the frontend
    let initial_prompt = format!(
        "You are the K2SO agent \"{agent}\". You are working in a dedicated worktree at `{wt_path}` on branch `{branch}`.\n\n\
        Your current task:\n**{title}** (priority: {priority})\n\n\
        The full task description is in `.k2so/agents/{agent}/work/active/{filename}`.\n\n\
        Instructions:\n\
        1. Read the task file for full details and acceptance criteria\n\
        2. Implement the changes — all your work happens in this worktree\n\
        3. Commit your work to branch `{branch}`\n\
        4. When done, run: `k2so work move --agent {agent} --file {filename} --from active --to done`\n\
        5. Your work will be reviewed and either approved (merged to main) or sent back with feedback",
        agent = target_agent,
        wt_path = worktree.path,
        branch = worktree.branch,
        title = item.title,
        priority = item.priority,
        filename = item.filename,
    );

    Ok(serde_json::json!({
        "command": "claude",
        "args": ["--append-system-prompt", claude_md, "-p", initial_prompt],
        "cwd": worktree.path,
        "claudeMdPath": claude_md_path.to_string_lossy(),
        "agentName": target_agent,
        "worktreePath": worktree.path,
        "branch": worktree.branch,
        "taskFile": item.filename,
    }))
}

/// Move a work item between folders (inbox → active, active → done, etc.)
#[tauri::command]
pub fn k2so_agents_work_move(
    project_path: String,
    agent_name: String,
    filename: String,
    from_folder: String,
    to_folder: String,
) -> Result<(), String> {
    let source = agent_work_dir(&project_path, &agent_name, &from_folder).join(&filename);
    let target_dir = agent_work_dir(&project_path, &agent_name, &to_folder);
    let target = target_dir.join(&filename);

    if !source.exists() {
        return Err(format!("Work item not found: {}/{}", from_folder, filename));
    }
    if !target_dir.exists() {
        fs::create_dir_all(&target_dir).map_err(|e| e.to_string())?;
    }

    fs::rename(&source, &target).map_err(|e| format!("Failed to move work item: {}", e))?;
    Ok(())
}

/// Read an agent's agent.md content.
#[tauri::command]
pub fn k2so_agents_get_profile(project_path: String, agent_name: String) -> Result<String, String> {
    let path = agent_dir(&project_path, &agent_name).join("agent.md");
    if !path.exists() {
        return Err(format!("Agent '{}' does not exist", agent_name));
    }
    fs::read_to_string(&path).map_err(|e| e.to_string())
}

/// Update an agent's agent.md content.
#[tauri::command]
pub fn k2so_agents_update_profile(
    project_path: String,
    agent_name: String,
    content: String,
) -> Result<(), String> {
    let dir = agent_dir(&project_path, &agent_name);
    if !dir.exists() {
        return Err(format!("Agent '{}' does not exist", agent_name));
    }
    let path = dir.join("agent.md");
    fs::write(&path, content).map_err(|e| e.to_string())
}

// ── Workspace Inbox ─────────────────────────────────────────────────────

/// List items in the workspace-level inbox.
#[tauri::command]
pub fn k2so_agents_workspace_inbox_list(project_path: String) -> Result<Vec<WorkItem>, String> {
    let dir = workspace_inbox_dir(&project_path);
    if !dir.exists() {
        return Ok(vec![]);
    }
    let mut items = Vec::new();
    for entry in fs::read_dir(&dir).map_err(|e| e.to_string())?.flatten() {
        let path = entry.path();
        if path.extension().map_or(false, |ext| ext == "md") {
            if let Some(item) = read_work_item(&path, "workspace-inbox") {
                items.push(item);
            }
        }
    }
    Ok(items)
}

/// Create a work item in a workspace inbox (for cross-workspace delegation).
#[tauri::command]
pub fn k2so_agents_workspace_inbox_create(
    workspace_path: String,
    title: String,
    body: String,
    priority: Option<String>,
    item_type: Option<String>,
    assigned_by: Option<String>,
) -> Result<WorkItem, String> {
    let dir = workspace_inbox_dir(&workspace_path);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    let priority = priority.unwrap_or_else(|| "normal".to_string());
    let item_type = item_type.unwrap_or_else(|| "task".to_string());
    let assigned_by = assigned_by.unwrap_or_else(|| "external".to_string());

    let slug: String = title
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<&str>>()
        .join("-");
    let slug = &slug[..slug.len().min(60)];
    let filename = format!("{}.md", slug);

    let now = simple_date();
    let content = format!(
        "---\ntitle: {}\npriority: {}\nassigned_by: {}\ncreated: {}\ntype: {}\n---\n\n{}\n",
        title, priority, assigned_by, now, item_type, body
    );

    let path = dir.join(&filename);
    fs::write(&path, &content).map_err(|e| e.to_string())?;

    Ok(WorkItem {
        filename,
        title,
        priority,
        assigned_by,
        created: now,
        item_type,
        folder: "workspace-inbox".to_string(),
    })
}

// ── Lock Files ──────────────────────────────────────────────────────────

/// Create a lock file for an agent (called when a Claude session starts).
#[tauri::command]
pub fn k2so_agents_lock(project_path: String, agent_name: String) -> Result<(), String> {
    let lock_path = agent_work_dir(&project_path, &agent_name, "").join(".lock");
    if let Some(parent) = lock_path.parent() {
        fs::create_dir_all(parent).ok();
    }
    fs::write(&lock_path, simple_date()).map_err(|e| e.to_string())
}

/// Remove a lock file for an agent (called when a Claude session ends).
#[tauri::command]
pub fn k2so_agents_unlock(project_path: String, agent_name: String) -> Result<(), String> {
    let lock_path = agent_work_dir(&project_path, &agent_name, "").join(".lock");
    if lock_path.exists() {
        fs::remove_file(&lock_path).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Check if an agent is locked (has an active session).
pub fn is_agent_locked(project_path: &str, agent_name: &str) -> bool {
    let lock_path = agent_work_dir(project_path, agent_name, "").join(".lock");
    lock_path.exists()
}

// ── CLAUDE.md Generator ─────────────────────────────────────────────────

/// Generate a CLAUDE.md for an agent and write it to the agent's directory.
/// Returns the generated content.
#[tauri::command]
pub fn k2so_agents_generate_claude_md(
    project_path: String,
    agent_name: String,
) -> Result<String, String> {
    let md = generate_agent_claude_md_content(&project_path, &agent_name, None)?;

    let claude_md_path = agent_dir(&project_path, &agent_name).join("CLAUDE.md");
    fs::write(&claude_md_path, &md)
        .map_err(|e| format!("Failed to write CLAUDE.md: {}", e))?;

    Ok(md)
}

/// Build the launch command for an agent's Claude session.
///
/// This handles three cases:
/// 1. Agent has active work with a worktree → resume in that worktree
/// 2. Agent has inbox work → internally delegates (creates worktree, moves to active)
/// 3. Agent has no work → launches in project root with empty-inbox prompt
///
/// Used by the UI "Launch" button and the heartbeat auto-launch.
#[tauri::command]
pub fn k2so_agents_build_launch(
    project_path: String,
    agent_name: String,
    agent_cli_command: Option<String>,
) -> Result<serde_json::Value, String> {
    let command = agent_cli_command.unwrap_or_else(|| "claude".to_string());

    // Case 1: Check for active work with a worktree path (resume)
    let active_dir = agent_work_dir(&project_path, &agent_name, "active");
    if active_dir.exists() {
        if let Ok(entries) = fs::read_dir(&active_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().map_or(false, |ext| ext == "md") {
                    if let Some(item) = read_work_item(&path, "active") {
                        let content = fs::read_to_string(&path).unwrap_or_default();
                        let fm = parse_frontmatter(&content);
                        if let Some(wt_path) = fm.get("worktree_path") {
                            let branch = fm.get("branch").cloned().unwrap_or_default();
                            // Resume in the existing worktree
                            let claude_md = generate_agent_claude_md_content(&project_path, &agent_name, Some(&item))?;
                            let claude_md_path = PathBuf::from(wt_path).join("CLAUDE.md");
                            fs::write(&claude_md_path, &claude_md).ok();

                            let prompt = format!(
                                "You are the K2SO agent \"{agent}\". Resuming work in worktree `{wt_path}` on branch `{branch}`.\n\n\
                                Your current task: **{title}** (priority: {priority})\n\
                                Task file: `.k2so/agents/{agent}/work/active/{filename}`\n\n\
                                Continue where you left off. When done: `k2so work move --agent {agent} --file {filename} --from active --to done`",
                                agent = agent_name, wt_path = wt_path, branch = branch,
                                title = item.title, priority = item.priority, filename = item.filename,
                            );

                            return Ok(serde_json::json!({
                                "command": command,
                                "args": ["--append-system-prompt", claude_md, "-p", prompt],
                                "cwd": wt_path,
                                "claudeMdPath": claude_md_path.to_string_lossy(),
                                "agentName": agent_name,
                                "worktreePath": wt_path,
                                "branch": branch,
                            }));
                        }
                    }
                }
            }
        }
    }

    // Case 2: Check for inbox work → delegate (creates worktree + moves to active)
    let inbox_dir = agent_work_dir(&project_path, &agent_name, "inbox");
    if inbox_dir.exists() {
        let mut items: Vec<(PathBuf, WorkItem)> = Vec::new();
        if let Ok(entries) = fs::read_dir(&inbox_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().map_or(false, |ext| ext == "md") {
                    if let Some(item) = read_work_item(&path, "inbox") {
                        items.push((path, item));
                    }
                }
            }
        }
        items.sort_by(|a, b| priority_rank(&a.1.priority).cmp(&priority_rank(&b.1.priority)));

        if let Some((top_path, _)) = items.into_iter().next() {
            // Use the delegate function — it does everything in one step
            let source_file = top_path.to_string_lossy().to_string();
            return k2so_agents_delegate(project_path, agent_name, source_file);
        }
    }

    // Case 3: No work — launch in project root with general context
    let claude_md = generate_agent_claude_md_content(&project_path, &agent_name, None)?;
    let claude_md_path = agent_dir(&project_path, &agent_name).join("CLAUDE.md");
    fs::write(&claude_md_path, &claude_md).ok();

    let prompt = format!(
        "You are the K2SO agent \"{}\". Your inbox is empty. Report your status and wait for work to be assigned.",
        agent_name
    );

    Ok(serde_json::json!({
        "command": command,
        "args": ["--append-system-prompt", claude_md, "-p", prompt],
        "cwd": project_path,
        "claudeMdPath": claude_md_path.to_string_lossy(),
        "agentName": agent_name,
        "worktreePath": null,
        "branch": null,
    }))
}

/// Add worktree_path and branch to a work item's frontmatter.
fn add_worktree_to_frontmatter(content: &str, worktree_path: &str, branch: &str) -> String {
    if content.starts_with("---") {
        if let Some(end_idx) = content[3..].find("---") {
            let frontmatter = &content[3..3 + end_idx];
            let body = &content[3 + end_idx + 3..];
            return format!(
                "---\n{}worktree_path: {}\nbranch: {}\n---{}",
                frontmatter, worktree_path, branch, body
            );
        }
    }
    content.to_string()
}

/// Strip worktree_path and branch from a work item's frontmatter (used on rejection/retry).
fn strip_worktree_from_frontmatter(content: &str) -> String {
    if content.starts_with("---") {
        if let Some(end_idx) = content[3..].find("---") {
            let frontmatter = &content[3..3 + end_idx];
            let body = &content[3 + end_idx + 3..];
            let cleaned: String = frontmatter
                .lines()
                .filter(|line| !line.starts_with("worktree_path:") && !line.starts_with("branch:"))
                .collect::<Vec<_>>()
                .join("\n");
            return format!("---\n{}\n---{}", cleaned.trim(), body);
        }
    }
    content.to_string()
}

/// Log a warning for an agent (appends to .k2so/agents/<name>/agent.log).
fn log_agent_warning(project_path: &str, agent_name: &str, message: &str) {
    let log_path = agent_dir(project_path, agent_name).join("agent.log");
    let entry = format!("[{}] WARN: {}\n", simple_date(), message);
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&log_path) {
        use std::io::Write;
        let _ = file.write_all(entry.as_bytes());
    }
}

/// Generate the CLAUDE.md content for an agent, optionally focused on a specific task.
fn generate_agent_claude_md_content(
    project_path: &str,
    agent_name: &str,
    current_task: Option<&WorkItem>,
) -> Result<String, String> {
    let dir = agent_dir(project_path, agent_name);
    if !dir.exists() {
        return Err(format!("Agent '{}' does not exist", agent_name));
    }

    // Read agent identity
    let agent_md_path = dir.join("agent.md");
    let agent_md = fs::read_to_string(&agent_md_path).unwrap_or_default();
    let fm = parse_frontmatter(&agent_md);
    let role = fm.get("role").cloned().unwrap_or("AI Agent".to_string());

    let agent_body = if agent_md.starts_with("---") {
        if let Some(end) = agent_md[3..].find("---") {
            agent_md[3 + end + 3..].trim().to_string()
        } else {
            agent_md.clone()
        }
    } else {
        agent_md.clone()
    };

    // List other agents for delegation awareness
    let mut other_agents = Vec::new();
    let agents_root = agents_dir(project_path);
    if agents_root.exists() {
        if let Ok(entries) = fs::read_dir(&agents_root) {
            for entry in entries.flatten() {
                if entry.file_type().map_or(false, |ft| ft.is_dir()) {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name != agent_name {
                        let their_md = entry.path().join("agent.md");
                        let their_role = if their_md.exists() {
                            let content = fs::read_to_string(&their_md).unwrap_or_default();
                            let fm = parse_frontmatter(&content);
                            fm.get("role").cloned().unwrap_or_default()
                        } else {
                            String::new()
                        };
                        other_agents.push((name, their_role));
                    }
                }
            }
        }
    }

    let mut md = String::new();
    md.push_str(&format!("# K2SO Agent: {}\n\n", agent_name));
    md.push_str(&format!("## Identity\n**Role:** {}\n\n", role));
    if !agent_body.is_empty() {
        md.push_str(&format!("{}\n\n", agent_body));
    }

    // Current task (if launching with specific work)
    if let Some(task) = current_task {
        md.push_str("## Current Task\n\n");
        md.push_str(&format!("**{}** (priority: {}, type: {})\n\n", task.title, task.priority, task.item_type));
        md.push_str(&format!("Task file: `.k2so/agents/{}/work/active/{}`\n\n", agent_name, task.filename));
        md.push_str("Read the full task file for acceptance criteria and details.\n\n");
    }

    // Work queue info
    md.push_str("## Work Queue\n\n");
    md.push_str(&format!(
        "Your work items are at: `.k2so/agents/{}/work/`\n",
        agent_name
    ));
    md.push_str("- `inbox/` — assigned to you, pick the highest priority\n");
    md.push_str("- `active/` — items you're currently working on\n");
    md.push_str("- `done/` — move items here when complete\n\n");

    // Other agents
    if !other_agents.is_empty() {
        md.push_str("## Other Agents\n");
        md.push_str("You can delegate work to these agents:\n\n");
        for (name, their_role) in &other_agents {
            md.push_str(&format!("- **{}** — {}\n", name, their_role));
        }
        md.push_str("\n");
    }

    md.push_str(CLI_TOOLS_DOCS);
    md.push_str(WORKFLOW_DOCS);

    Ok(md)
}

/// Priority rank for sorting (lower = higher priority).
fn priority_rank(priority: &str) -> u8 {
    match priority {
        "critical" => 0,
        "high" => 1,
        "normal" => 2,
        "low" => 3,
        _ => 2,
    }
}

/// Generate a comprehensive CLAUDE.md for the workspace root.
/// This is the lead agent's complete operating manual for K2SO.
/// Written to `<project-root>/CLAUDE.md` so Claude Code auto-discovers it.
#[tauri::command]
pub fn k2so_agents_generate_workspace_claude_md(
    project_path: String,
) -> Result<String, String> {
    let project_name = std::path::Path::new(&project_path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "workspace".to_string());

    // Scaffold .k2so/ structure if it doesn't exist
    let k2so_dir = PathBuf::from(&project_path).join(".k2so");
    let _ = fs::create_dir_all(k2so_dir.join("agents"));
    let _ = fs::create_dir_all(k2so_dir.join("work").join("inbox"));
    let _ = fs::create_dir_all(k2so_dir.join("prds"));

    // Auto-create pod-leader agent if it doesn't exist (for pod mode)
    let pod_leader_dir = k2so_dir.join("agents").join("pod-leader");
    if !pod_leader_dir.exists() {
        let _ = fs::create_dir_all(pod_leader_dir.join("work").join("inbox"));
        let _ = fs::create_dir_all(pod_leader_dir.join("work").join("active"));
        let _ = fs::create_dir_all(pod_leader_dir.join("work").join("done"));
        let pod_leader_md = format!(
            "---\nname: pod-leader\nrole: Pod orchestrator — delegates work to agents, reviews completed branches, drives milestones\npod_leader: true\n---\n\nYou are the pod leader for the {} workspace.\n",
            project_name
        );
        let _ = fs::write(pod_leader_dir.join("agent.md"), &pod_leader_md);
    }

    // List existing agents
    let mut agent_list = String::new();
    let agents_root = agents_dir(&project_path);
    if agents_root.exists() {
        if let Ok(entries) = fs::read_dir(&agents_root) {
            for entry in entries.flatten() {
                if entry.file_type().map_or(false, |ft| ft.is_dir()) {
                    let name = entry.file_name().to_string_lossy().to_string();
                    let agent_md = entry.path().join("agent.md");
                    let role = if agent_md.exists() {
                        let content = fs::read_to_string(&agent_md).unwrap_or_default();
                        let fm = parse_frontmatter(&content);
                        fm.get("role").cloned().unwrap_or_default()
                    } else {
                        String::new()
                    };
                    agent_list.push_str(&format!("- **{}** — {}\n", name, role));
                }
            }
        }
    }

    // List workspace inbox items
    let mut inbox_summary = String::new();
    let ws_inbox = workspace_inbox_dir(&project_path);
    if ws_inbox.exists() {
        if let Ok(entries) = fs::read_dir(&ws_inbox) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().map_or(false, |ext| ext == "md") {
                    if let Some(item) = read_work_item(&path, "inbox") {
                        inbox_summary.push_str(&format!(
                            "- **{}** (priority: {}, type: {})\n",
                            item.title, item.priority, item.item_type
                        ));
                    }
                }
            }
        }
    }

    // Detect mode — read from DB, fall back to filesystem
    let is_pod_mode = {
        // Try reading from DB first
        let db_mode = dirs::home_dir()
            .and_then(|h| {
                let db_path = h.join(".k2so").join("k2so.db");
                rusqlite::Connection::open(&db_path).ok()
            })
            .and_then(|conn| {
                conn.query_row(
                    "SELECT agent_mode FROM projects WHERE path = ?1",
                    rusqlite::params![project_path],
                    |row| row.get::<_, String>(0),
                ).ok()
            });

        match db_mode.as_deref() {
            Some("pod") => true,
            Some("agent") => false,
            _ => {
                // Fallback: if agents dir has sub-agents, assume pod
                let agents_root = agents_dir(&project_path);
                agents_root.exists() && fs::read_dir(&agents_root)
                    .map(|e| e.flatten().any(|e| e.file_type().map_or(false, |ft| ft.is_dir())))
                    .unwrap_or(false)
            }
        }
    };

    let md = if is_pod_mode {
        // ── Agent 2: Pod Leader CLAUDE.md ──────────────────────────────
        format!(
            r#"# K2SO Pod Leader: {project_name}

You are the **pod leader** for the {project_name} workspace, operating inside K2SO.

## Your Role

You manage a team of AI agents that build this project. You:
- **Read PRDs and milestones** in `.k2so/prds/` and `.k2so/milestones/` to understand the plan
- **Delegate work** to sub-agents — K2SO automatically creates a worktree, writes a CLAUDE.md, and launches the agent
- **Manage your team** — create new agents when you need new skills, assign multiple tasks to the same agent type across parallel worktrees
- **Review completed work** — when agents finish, review their diffs and either approve (merge to main) or reject with feedback
- **Drive milestones forward** — after merging one batch, assign the next batch of tasks

**Important:** An agent is a role template, not a person. `backend-eng` can run in 5 worktrees simultaneously — each gets its own branch, its own CLAUDE.md, and its own Claude session. Don't wait for one task to finish before assigning the next.

## Workspace Inbox

{inbox_section}

## Your Agents

{agent_section}

## Delegation (one command does everything)

```bash
# Create a task and assign it
k2so work create --agent backend-eng --title "Build OAuth endpoints" \
  --body "Implement /auth/login and /auth/callback. See PRD: .k2so/prds/auth.md" \
  --priority high --type task

# Delegate — creates worktree, writes CLAUDE.md, launches the agent:
k2so delegate backend-eng .k2so/agents/backend-eng/work/inbox/build-oauth-endpoints.md
```

You can delegate multiple tasks to the same agent simultaneously:
```bash
k2so delegate backend-eng .k2so/agents/backend-eng/work/inbox/task-1.md
k2so delegate backend-eng .k2so/agents/backend-eng/work/inbox/task-2.md
k2so delegate backend-eng .k2so/agents/backend-eng/work/inbox/task-3.md
```
Each gets its own worktree and runs in parallel.

## Reviewing and Merging

When agents move their work to done/, it appears in the review queue:
```bash
k2so reviews                                    # See all pending reviews with diffs
k2so review approve backend-eng <branch>        # Merge to main + cleanup worktree
k2so review reject backend-eng --reason "..."   # Discard worktree + send back to inbox
k2so review feedback backend-eng -m "..."       # Send feedback without rejecting
```

**Your review responsibility:** You are the first reviewer. Check the diff, verify it meets the task's acceptance criteria, and approve or reject. Only escalate to the user when a milestone is complete or if you're unsure about a design decision.

## Creating New Agents

When you need a skill your team doesn't have:
```bash
k2so agents create devops-eng --role "DevOps — CI/CD, Docker, deployment, infrastructure"
k2so agents create docs-writer --role "Documentation — README, API docs, user guides"
```

## Planning

Store plans as markdown files:
- `.k2so/prds/` — Product requirement documents
- `.k2so/milestones/` — Milestone breakdowns with task lists
- `.k2so/specs/` — Technical specifications

{cli_section}

{workflow_section}
"#,
            project_name = project_name,
            inbox_section = if inbox_summary.is_empty() {
                "*Workspace inbox is empty. Waiting for tasks from the AI Planner or user.*".to_string()
            } else {
                format!("### Current Inbox\n{}", inbox_summary)
            },
            agent_section = if agent_list.is_empty() {
                "*No agents yet. Create agents based on the skills this project needs.*".to_string()
            } else {
                agent_list
            },
            cli_section = CLI_TOOLS_DOCS,
            workflow_section = WORKFLOW_DOCS,
        )
    } else {
        // ── Agent 1: AI Planner CLAUDE.md ──────────────────────────────
        format!(
            r#"# K2SO AI Planner: {project_name}

You are the **AI Planner** for the {project_name} workspace, operating inside K2SO.

## Your Role

You collaborate with the user to plan and orchestrate software projects. You:
- **Talk with the user** to understand what they want to build
- **Create PRDs** (product requirement documents), milestones, and technical specifications
- **Set up workspaces** for each project — enable worktrees, pod mode, create agent teams
- **Coordinate across workspaces** — send work to different projects, check on progress
- **You do NOT write code** — you plan, then hand off execution to pod leaders and their agent teams

## Setting Up a Project Workspace

When the user has a project they want to build or maintain with agents:

```bash
# 1. Enable the workspace for autonomous work
k2so worktree on                    # Agents work in isolated git branches
k2so mode pod                       # Enable multi-agent orchestration
k2so heartbeat on                   # Agents wake up automatically on schedule

# 2. Create the agent team based on the project's tech stack
k2so agents create backend-eng --role "Backend engineer — APIs, databases, server logic"
k2so agents create frontend-eng --role "Frontend engineer — React, UI, styling, UX"
k2so agents create qa-tester --role "QA — testing, test automation, quality assurance"

# 3. Verify setup
k2so settings                       # Shows mode, worktrees, heartbeat status
k2so agents list                    # Shows agents with work counts
```

## Planning Workflow

1. **Discuss with the user** what they want built — goals, constraints, timeline
2. **Create a PRD** that captures the full scope:
   ```
   mkdir -p .k2so/prds
   # Write the PRD as a markdown file
   ```
3. **Break the PRD into milestones** — each milestone should be shippable
4. **Break milestones into tasks** with clear acceptance criteria
5. **Send tasks to the project workspace** for the pod leader to execute:
   ```bash
   k2so work send --workspace /path/to/project \
     --title "Milestone 1: User Authentication" \
     --body "See PRD at .k2so/prds/auth.md. Tasks: ..."
   ```
   The pod leader in that workspace picks it up and delegates to its agents.

## Cross-Workspace Coordination

You can see and manage multiple workspaces:
```bash
# Send work to any workspace
k2so work send --workspace /path/to/frontend-app --title "..." --body "..."
k2so work send --workspace /path/to/api-server --title "..." --body "..."

# Set up a new workspace from scratch
K2SO_PROJECT_PATH="/path/to/new-project" k2so mode pod
K2SO_PROJECT_PATH="/path/to/new-project" k2so worktree on
K2SO_PROJECT_PATH="/path/to/new-project" k2so agents create backend-eng --role "..."
```

## Current Context

{inbox_section}

{cli_section}
"#,
            project_name = project_name,
            inbox_section = if inbox_summary.is_empty() {
                "No items in the workspace inbox.".to_string()
            } else {
                format!("### Workspace Inbox\n{}", inbox_summary)
            },
            cli_section = CLI_TOOLS_DOCS,
        )
    };

    let claude_md_path = PathBuf::from(&project_path).join("CLAUDE.md");
    let disabled_path = PathBuf::from(&project_path).join(".k2so").join("CLAUDE.md.disabled");
    let generated_path = PathBuf::from(&project_path).join(".k2so").join("CLAUDE.md.generated");

    // Detect if the existing CLAUDE.md was generated by K2SO (vs user-written)
    // by checking for our header pattern. If it was ours, regenerate it.
    let is_k2so_generated = if claude_md_path.exists() {
        let existing = fs::read_to_string(&claude_md_path).unwrap_or_default();
        existing.starts_with("# K2SO ") // Our headers: "# K2SO Pod Leader:" or "# K2SO AI Planner:" or "# K2SO Lead Agent:"
    } else {
        false
    };

    if disabled_path.exists() {
        // Was disabled — check if the disabled content matches our pattern
        let disabled_content = fs::read_to_string(&disabled_path).unwrap_or_default();
        if disabled_content.starts_with("# K2SO ") {
            // It was our generated content — write fresh for the new mode
            let _ = fs::remove_file(&disabled_path);
            fs::write(&claude_md_path, &md)
                .map_err(|e| format!("Failed to write CLAUDE.md: {}", e))?;
        } else {
            // It was user-written — restore it, save ours for reference
            fs::rename(&disabled_path, &claude_md_path)
                .map_err(|e| format!("Failed to restore CLAUDE.md: {}", e))?;
            let _ = fs::write(&generated_path, &md);
            return fs::read_to_string(&claude_md_path).map_err(|e| e.to_string());
        }
    } else if is_k2so_generated {
        // Existing CLAUDE.md was generated by K2SO — regenerate for current mode
        fs::write(&claude_md_path, &md)
            .map_err(|e| format!("Failed to write CLAUDE.md: {}", e))?;
    } else if claude_md_path.exists() {
        // Existing user-written CLAUDE.md — don't overwrite, save ours for reference
        let _ = fs::write(&generated_path, &md);
        return fs::read_to_string(&claude_md_path).map_err(|e| e.to_string());
    } else {
        // No CLAUDE.md exists — write fresh
        fs::write(&claude_md_path, &md)
            .map_err(|e| format!("Failed to write CLAUDE.md: {}", e))?;
    }

    Ok(md)
}

/// Remove or disable the workspace CLAUDE.md (when Agent toggle is turned off).
#[tauri::command]
pub fn k2so_agents_disable_workspace_claude_md(project_path: String) -> Result<(), String> {
    let claude_md = PathBuf::from(&project_path).join("CLAUDE.md");
    let disabled = PathBuf::from(&project_path).join(".k2so").join("CLAUDE.md.disabled");

    if claude_md.exists() {
        // Move to .k2so/ rather than delete — preserves any user edits
        fs::rename(&claude_md, &disabled)
            .map_err(|e| format!("Failed to disable CLAUDE.md: {}", e))?;
    }
    Ok(())
}

const CLI_TOOLS_DOCS: &str = r#"## K2SO CLI Tools

You are operating inside K2SO. The `k2so` command is available in your terminal.
K2SO does the heavy lifting — each command is a single atomic operation.

### Assign Work to an Agent (one step)
```
k2so delegate <agent> <work-file>
```
This single command does everything:
- Creates a git worktree (branch: `agent/<name>/<task>`)
- Writes a CLAUDE.md into the worktree with the agent's identity + task context
- Moves the work item from inbox → active with worktree metadata
- Opens a Claude terminal session in the worktree for the agent to start working

### Create Work Items
```
k2so work create --title "..." --body "..." --agent <name> --priority high --type task
k2so work create --title "..." --body "..."   # Goes to workspace inbox (no agent)
```

### Check Status
```
k2so agents list                     # All agents with inbox/active/done counts
k2so agents work <name>              # Agent's work items
k2so work inbox                      # Workspace-level inbox
k2so reviews                         # Pending reviews (completed work)
```

### Reviews (one step each)
```
k2so review approve <agent> <branch>   # Merges branch + removes worktree + cleans up
k2so review reject <agent>             # Removes worktree + moves work back to inbox
k2so review reject <agent> --reason "..." # Same + creates feedback file
k2so review feedback <agent> -m "..."  # Send feedback without rejecting
```

### Git
```
k2so commit                          # AI-assisted commit review
k2so commit-merge                    # AI commit then merge into main
```

### Other
```
k2so agents create <name> --role "..."   # Create a new agent
k2so agents profile <name>              # Read agent's identity
k2so work move --agent <name> --file <f> --from inbox --to active
k2so work send --workspace <path> --title "..." --body "..."
k2so heartbeat                          # Trigger triage manually
```

"#;

const WORKFLOW_DOCS: &str = r#"## Workflow

### If you are the Lead Agent (orchestrator):
1. Check for work: `k2so work inbox`
2. Read each request and decide which agent should handle it
3. Assign work with a single command — K2SO handles everything else:
   ```
   k2so delegate backend-eng .k2so/work/inbox/add-oauth-support.md
   ```
   This creates a worktree, writes a CLAUDE.md, and launches the agent automatically.
4. To break a large request into sub-tasks first:
   ```
   k2so work create --agent backend-eng --title "Build API endpoints" --body "..." --priority high
   k2so work create --agent frontend-eng --title "Build login UI" --body "..." --priority high
   ```
   Then delegate each: `k2so delegate backend-eng .k2so/agents/backend-eng/work/inbox/build-api-endpoints.md`
5. If a request is blocked or needs user input, leave it in the workspace inbox
6. You orchestrate — you do NOT implement code yourself

### If you are a Sub-Agent (executor):
You are launched into a dedicated worktree with your task already set up.
1. Read your task file (path is in your launch prompt)
2. Implement the changes — all work happens in your worktree
3. Commit to your branch as you go
4. When done: `k2so work move --agent <your-name> --file <task>.md --from active --to done`
5. Your work appears in the review queue — the user will approve, reject, or request changes

### Review lifecycle (handled by user or lead agent):
- **Approve**: `k2so review approve <agent> <branch>` — merges to main, cleans up worktree
- **Reject**: `k2so review reject <agent> --reason "..."` — cleans up worktree, puts task back in inbox with feedback, agent retries with a fresh worktree on next launch
- **Feedback**: `k2so review feedback <agent> -m "..."` — sends feedback without rejecting

## Important Rules
- Each agent works in its own worktree — never edit main directly
- K2SO creates worktrees, branches, and CLAUDE.md files for you automatically
- Commit often with clear messages referencing your task
- If blocked, move your task back to inbox and document the blocker
"#;

// ── Review Queue ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewDiffFile {
    pub path: String,
    pub status: String,
    pub additions: u32,
    pub deletions: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewItem {
    pub agent_name: String,
    pub branch: String,
    pub worktree_path: Option<String>,
    pub work_items: Vec<WorkItem>,
    pub diff_summary: Vec<ReviewDiffFile>,
}

/// Get the review queue — agents with completed work in worktree branches.
#[tauri::command]
pub fn k2so_agents_review_queue(project_path: String) -> Result<Vec<ReviewItem>, String> {
    let dir = agents_dir(&project_path);
    if !dir.exists() {
        return Ok(vec![]);
    }

    // Get worktrees for this project
    let worktrees = crate::git::list_worktrees(&project_path);

    let mut reviews = Vec::new();
    let entries = fs::read_dir(&dir).map_err(|e| e.to_string())?;

    for entry in entries.flatten() {
        if !entry.file_type().map_or(false, |ft| ft.is_dir()) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        let done_dir = agent_work_dir(&project_path, &name, "done");

        if !done_dir.exists() {
            continue;
        }

        // Collect done items
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

        // Find worktree branch for this agent (convention: branch name contains agent name)
        let matching_worktree = worktrees.iter().find(|wt| {
            !wt.is_main && (wt.branch.contains(&name) || wt.branch.starts_with("agent/"))
        });

        // Get diff summary if we have a branch
        let diff_summary: Vec<ReviewDiffFile> = if let Some(wt) = matching_worktree {
            crate::git::diff_between_branches(&project_path, "main", &wt.branch)
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

/// Approve an agent's work — merge branch, clean up worktree, archive done items.
///
/// This is the all-in-one approve command. In one step, K2SO:
/// 1. Merges the agent's branch into main
/// 2. Removes the worktree directory
/// 3. Deletes the branch (it's now merged)
/// 4. Archives done items (deletes them — the work is in git history now)
/// 5. Unlocks the agent
#[tauri::command]
pub fn k2so_agents_review_approve(
    project_path: String,
    branch: String,
    agent_name: String,
) -> Result<String, String> {
    // 1. Merge the branch into main
    let result = crate::git::merge_branch(&project_path, &branch)?;

    if !result.success {
        return Err(format!("Merge conflicts: {}", result.conflicts.join(", ")));
    }

    // 2. Remove the worktree (find it by branch name)
    let worktrees = crate::git::list_worktrees(&project_path);
    if let Some(wt) = worktrees.iter().find(|wt| wt.branch == branch) {
        let _ = crate::git::remove_worktree(&project_path, &wt.path, true);
    }

    // 3. Delete the branch (now merged)
    let _ = crate::git::delete_branch(&project_path, &branch);

    // 4. Archive done items for this agent
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

    // 5. Unlock the agent so it can pick up new work
    let _ = k2so_agents_unlock(project_path, agent_name);

    Ok(format!("Approved and merged: {} files", result.merged_files))
}

/// Reject an agent's work — clean up worktree, move done items back to inbox.
///
/// This is the all-in-one reject command. In one step, K2SO:
/// 1. Removes the worktree directory (discards the code)
/// 2. Deletes the branch
/// 3. Moves done items back to inbox (so the agent retries on next launch)
/// 4. Creates a high-priority feedback file explaining what went wrong
/// 5. Unlocks the agent
#[tauri::command]
pub fn k2so_agents_review_reject(
    project_path: String,
    agent_name: String,
    reason: Option<String>,
) -> Result<(), String> {
    let done_dir = agent_work_dir(&project_path, &agent_name, "done");
    let inbox_dir = agent_work_dir(&project_path, &agent_name, "inbox");

    if !done_dir.exists() {
        return Ok(());
    }

    // 1. Find and remove the worktree + branch for this agent
    let worktrees = crate::git::list_worktrees(&project_path);
    for wt in worktrees.iter().filter(|wt| wt.branch.starts_with(&format!("agent/{}/", agent_name))) {
        let _ = crate::git::remove_worktree(&project_path, &wt.path, true);
        let _ = crate::git::delete_branch(&project_path, &wt.branch);
    }

    // 2. Move all done items back to inbox (strip worktree info from frontmatter)
    fs::create_dir_all(&inbox_dir).ok();
    if let Ok(entries) = fs::read_dir(&done_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map_or(false, |ext| ext == "md") {
                let filename = path.file_name().unwrap();
                let target = inbox_dir.join(filename);
                // Strip old worktree info so a fresh worktree gets created on retry
                if let Ok(content) = fs::read_to_string(&path) {
                    let cleaned = strip_worktree_from_frontmatter(&content);
                    let _ = fs::write(&target, &cleaned);
                } else {
                    let _ = fs::rename(&path, &target);
                }
                let _ = fs::remove_file(&path);
            }
        }
    }

    // 3. Create a feedback file in inbox if reason provided
    if let Some(reason) = reason {
        let now = simple_date();
        let content = format!(
            "---\ntitle: Review Feedback — Work Rejected\npriority: high\nassigned_by: reviewer\ncreated: {}\ntype: feedback\n---\n\n## Rejection Reason\n\n{}\n\n## Action Required\n\nReview the feedback above and address the issues in your next attempt.\nA fresh worktree will be created when you are relaunched.\n",
            now, reason
        );
        let filename = format!("review-feedback-{}.md", now);
        let path = inbox_dir.join(&filename);
        fs::write(&path, &content).map_err(|e| e.to_string())?;
    }

    // 4. Unlock the agent
    let _ = k2so_agents_unlock(project_path, agent_name);

    Ok(())
}

/// Request changes on an agent's work — create feedback file in inbox.
#[tauri::command]
pub fn k2so_agents_review_request_changes(
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
    fs::write(&path, &content).map_err(|e| e.to_string())?;

    Ok(())
}

// ── Heartbeat Triage ────────────────────────────────────────────────────

/// Build a triage summary for the local LLM to evaluate.
/// Returns a plain-text summary of all agents with pending work in a project.
/// The local LLM reads this and decides which agents (if any) should be launched.
#[tauri::command]
pub fn k2so_agents_triage_summary(project_path: String) -> Result<String, String> {
    let dir = agents_dir(&project_path);
    if !dir.exists() {
        return Ok("No agents configured.".to_string());
    }

    let mut summary = String::new();
    let entries = fs::read_dir(&dir).map_err(|e| e.to_string())?;

    for entry in entries.flatten() {
        if !entry.file_type().map_or(false, |ft| ft.is_dir()) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();

        // Check inbox
        let inbox = agent_work_dir(&project_path, &name, "inbox");
        let active = agent_work_dir(&project_path, &name, "active");

        let inbox_items: Vec<WorkItem> = if inbox.exists() {
            fs::read_dir(&inbox)
                .ok()
                .map(|entries| {
                    entries
                        .flatten()
                        .filter(|e| e.path().extension().map_or(false, |ext| ext == "md"))
                        .filter_map(|e| read_work_item(&e.path(), "inbox"))
                        .collect()
                })
                .unwrap_or_default()
        } else {
            vec![]
        };

        let active_count = if active.exists() {
            fs::read_dir(&active)
                .map(|e| e.flatten().filter(|e| e.path().extension().map_or(false, |ext| ext == "md")).count())
                .unwrap_or(0)
        } else {
            0
        };

        if inbox_items.is_empty() && active_count == 0 {
            continue;
        }

        summary.push_str(&format!("Agent: {}\n", name));
        if active_count > 0 {
            summary.push_str(&format!("  Currently active: {} items in progress\n", active_count));
        }
        for item in &inbox_items {
            summary.push_str(&format!(
                "  Inbox: \"{}\" (priority: {}, type: {})\n",
                item.title, item.priority, item.item_type
            ));
        }
        summary.push('\n');
    }

    if summary.is_empty() {
        Ok("No agents have pending work.".to_string())
    } else {
        Ok(summary)
    }
}

/// Determine what should be launched based on triage.
///
/// Agents are templates — the same agent (e.g., "backend-eng") can run in multiple
/// worktrees simultaneously. Each inbox item gets its own worktree when delegated.
///
/// Triage order:
/// 1. Workspace inbox has items → wake lead agent ("__lead__")
/// 2. Sub-agent inboxes have items → wake those agents (one launch per inbox item)
#[tauri::command]
pub fn k2so_agents_triage_decide(project_path: String) -> Result<Vec<String>, String> {
    let mut launchable = Vec::new();

    // Step 1: Check workspace inbox
    let ws_inbox = workspace_inbox_dir(&project_path);
    let has_workspace_inbox = ws_inbox.exists() && fs::read_dir(&ws_inbox)
        .map(|e| e.flatten().any(|e| e.path().extension().map_or(false, |ext| ext == "md")))
        .unwrap_or(false);

    if has_workspace_inbox {
        launchable.push("__lead__".to_string());
    }

    // Step 2: Check sub-agent inboxes
    // An agent is a template/role — it can have multiple items in its inbox and
    // each one gets its own worktree. We launch once per agent that has inbox items.
    // The delegate/build_launch function handles picking the top-priority item.
    let dir = agents_dir(&project_path);
    if dir.exists() {
        if let Ok(entries) = fs::read_dir(&dir) {
            for entry in entries.flatten() {
                if !entry.file_type().map_or(false, |ft| ft.is_dir()) {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().to_string();

                let inbox = agent_work_dir(&project_path, &name, "inbox");
                let has_inbox = inbox.exists() && fs::read_dir(&inbox)
                    .map(|e| e.flatten().any(|e| e.path().extension().map_or(false, |ext| ext == "md")))
                    .unwrap_or(false);

                if has_inbox {
                    launchable.push(name);
                }
            }
        }
    }

    Ok(launchable)
}

// ── Heartbeat Scheduler ─────────────────────────────────────────────────

/// Install the heartbeat scheduler (launchd on macOS, cron on Linux).
/// The heartbeat script reads ~/.k2so/heartbeat.port, checks if K2SO is alive,
/// and triggers triage for projects that have heartbeat enabled.
#[tauri::command]
pub fn k2so_agents_install_heartbeat(
    state: tauri::State<'_, crate::state::AppState>,
) -> Result<(), String> {
    let k2so_home = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".k2so");
    fs::create_dir_all(&k2so_home).map_err(|e| e.to_string())?;

    // Collect heartbeat-enabled project paths from DB
    let conn = state.db.lock();
    let projects = crate::db::schema::Project::list(&conn).map_err(|e| e.to_string())?;
    let heartbeat_paths: Vec<String> = projects
        .iter()
        .filter(|p| p.heartbeat_enabled != 0)
        .map(|p| p.path.clone())
        .collect();
    drop(conn);

    // Write the project paths list for the heartbeat script
    let paths_file = k2so_home.join("heartbeat-projects.txt");
    fs::write(&paths_file, heartbeat_paths.join("\n"))
        .map_err(|e| format!("Failed to write heartbeat projects: {}", e))?;

    // Generate heartbeat script
    let script_path = k2so_home.join("heartbeat.sh");
    let script = generate_heartbeat_script();
    fs::write(&script_path, &script)
        .map_err(|e| format!("Failed to write heartbeat script: {}", e))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755))
            .map_err(|e| e.to_string())?;
    }

    // Install platform scheduler
    #[cfg(target_os = "macos")]
    install_heartbeat_launchd(&script_path)?;

    #[cfg(target_os = "linux")]
    install_heartbeat_cron(&script_path)?;

    Ok(())
}

/// Uninstall the heartbeat scheduler.
#[tauri::command]
pub fn k2so_agents_uninstall_heartbeat() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    uninstall_heartbeat_launchd()?;

    #[cfg(target_os = "linux")]
    uninstall_heartbeat_cron()?;

    // Clean up script
    let k2so_home = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".k2so");
    let _ = fs::remove_file(k2so_home.join("heartbeat.sh"));
    let _ = fs::remove_file(k2so_home.join("heartbeat-projects.txt"));

    Ok(())
}

/// Update the heartbeat project list (called when heartbeat toggle changes).
#[tauri::command]
pub fn k2so_agents_update_heartbeat_projects(
    state: tauri::State<'_, crate::state::AppState>,
) -> Result<(), String> {
    let conn = state.db.lock();
    let projects = crate::db::schema::Project::list(&conn).map_err(|e| e.to_string())?;
    let heartbeat_paths: Vec<String> = projects
        .iter()
        .filter(|p| p.heartbeat_enabled != 0)
        .map(|p| p.path.clone())
        .collect();
    drop(conn);

    let k2so_home = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".k2so");
    let paths_file = k2so_home.join("heartbeat-projects.txt");
    fs::write(&paths_file, heartbeat_paths.join("\n"))
        .map_err(|e| format!("Failed to write heartbeat projects: {}", e))?;

    Ok(())
}

fn generate_heartbeat_script() -> String {
    let home = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("~"))
        .to_string_lossy()
        .to_string();

    format!(r##"#!/bin/bash
# K2SO Agent Heartbeat — DO NOT EDIT (managed by K2SO)
# Checks if K2SO is running, then triggers triage for heartbeat-enabled projects.

PORT_FILE="{home}/.k2so/heartbeat.port"
PROJECTS_FILE="{home}/.k2so/heartbeat-projects.txt"
LOG_FILE="{home}/.k2so/heartbeat.log"
TOKEN_FILE="{home}/.k2so/heartbeat.token"

ts() {{ date '+%Y-%m-%d %H:%M:%S'; }}

# Read K2SO port
if [ ! -f "$PORT_FILE" ]; then
    exit 0
fi
PORT=$(cat "$PORT_FILE" 2>/dev/null)
[ -z "$PORT" ] && exit 0

# Check if K2SO is alive
if ! curl -s --connect-timeout 2 "http://127.0.0.1:$PORT/health" | grep -q "ok"; then
    exit 0
fi

# Read project paths
if [ ! -f "$PROJECTS_FILE" ]; then
    exit 0
fi

# Read auth token
TOKEN=""
if [ -f "$TOKEN_FILE" ]; then
    TOKEN=$(cat "$TOKEN_FILE" 2>/dev/null)
fi

if [ -z "$TOKEN" ]; then
    echo "$(ts) No auth token available — skipping heartbeat" >> "$LOG_FILE"
    exit 0
fi

# Trigger triage for each heartbeat-enabled project
while IFS= read -r project_path; do
    [ -z "$project_path" ] && continue
    ENCODED_PATH=$(python3 -c "import urllib.parse; print(urllib.parse.quote('$project_path', safe=''))" 2>/dev/null)
    RESULT=$(curl -sG "http://127.0.0.1:$PORT/cli/heartbeat?token=$TOKEN&project=$ENCODED_PATH" --connect-timeout 5 --max-time 30 2>/dev/null)
    COUNT=$(echo "$RESULT" | python3 -c "import sys,json; print(json.load(sys.stdin).get('count',0))" 2>/dev/null || echo 0)
    if [ "$COUNT" -gt 0 ]; then
        echo "$(ts) Heartbeat: launched $COUNT agents for $project_path" >> "$LOG_FILE"
    fi
done < "$PROJECTS_FILE"

# Trim log
tail -200 "$LOG_FILE" > "$LOG_FILE.tmp" 2>/dev/null && mv "$LOG_FILE.tmp" "$LOG_FILE"
"##, home = home)
}

#[cfg(target_os = "macos")]
fn install_heartbeat_launchd(script_path: &Path) -> Result<(), String> {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
    let plist_path = home.join("Library/LaunchAgents/com.k2so.agent-heartbeat.plist");

    // Ensure dir exists
    if let Some(parent) = plist_path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    // Unload existing
    if plist_path.exists() {
        let _ = std::process::Command::new("launchctl")
            .args(["unload", &plist_path.to_string_lossy()])
            .output();
    }

    let plist = format!(r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.k2so.agent-heartbeat</string>
    <key>ProgramArguments</key>
    <array>
        <string>/bin/bash</string>
        <string>{script}</string>
    </array>
    <key>StartInterval</key>
    <integer>300</integer>
    <key>RunAtLoad</key>
    <false/>
    <key>StandardErrorPath</key>
    <string>{home}/.k2so/heartbeat-stderr.log</string>
</dict>
</plist>"#,
        script = script_path.to_string_lossy(),
        home = home.to_string_lossy(),
    );

    fs::write(&plist_path, &plist).map_err(|e| format!("Failed to write plist: {}", e))?;

    let output = std::process::Command::new("launchctl")
        .args(["load", &plist_path.to_string_lossy()])
        .output()
        .map_err(|e| format!("launchctl failed: {}", e))?;

    if !output.status.success() {
        return Err(format!("launchctl load failed: {}", String::from_utf8_lossy(&output.stderr)));
    }

    Ok(())
}

#[cfg(target_os = "macos")]
fn uninstall_heartbeat_launchd() -> Result<(), String> {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
    let plist_path = home.join("Library/LaunchAgents/com.k2so.agent-heartbeat.plist");
    if plist_path.exists() {
        let _ = std::process::Command::new("launchctl")
            .args(["unload", &plist_path.to_string_lossy()])
            .output();
        fs::remove_file(&plist_path).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn install_heartbeat_cron(script_path: &Path) -> Result<(), String> {
    let marker = "# k2so-agent-heartbeat";
    let entry = format!("*/5 * * * * {} {}", script_path.to_string_lossy(), marker);

    let existing = std::process::Command::new("crontab")
        .args(["-l"])
        .output()
        .ok()
        .and_then(|o| if o.status.success() { String::from_utf8(o.stdout).ok() } else { None })
        .unwrap_or_default();

    let mut lines: Vec<&str> = existing.lines().filter(|l| !l.contains("k2so-agent-heartbeat")).collect();
    lines.push(&entry);
    let new_crontab = lines.join("\n") + "\n";

    let mut child = std::process::Command::new("crontab")
        .args(["-"])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;

    use std::io::Write;
    child.stdin.as_mut().ok_or("stdin")?.write_all(new_crontab.as_bytes()).map_err(|e| e.to_string())?;
    child.wait().map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn uninstall_heartbeat_cron() -> Result<(), String> {
    let existing = std::process::Command::new("crontab")
        .args(["-l"])
        .output()
        .ok()
        .and_then(|o| if o.status.success() { String::from_utf8(o.stdout).ok() } else { None })
        .unwrap_or_default();

    let new_crontab: String = existing.lines()
        .filter(|l| !l.contains("k2so-agent-heartbeat"))
        .collect::<Vec<&str>>()
        .join("\n") + "\n";

    let mut child = std::process::Command::new("crontab")
        .args(["-"])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;

    use std::io::Write;
    child.stdin.as_mut().ok_or("stdin")?.write_all(new_crontab.as_bytes()).map_err(|e| e.to_string())?;
    child.wait().map_err(|e| e.to_string())?;
    Ok(())
}

// ── Utility ─────────────────────────────────────────────────────────────

fn simple_date() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let days = now / 86400;
    let mut y = 1970i64;
    let mut remaining = days as i64;
    loop {
        let days_in_year = if is_leap(y) { 366 } else { 365 };
        if remaining < days_in_year { break; }
        remaining -= days_in_year;
        y += 1;
    }
    let months = [31, if is_leap(y) { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut m = 1;
    for &dim in &months {
        if remaining < dim { break; }
        remaining -= dim;
        m += 1;
    }
    format!("{:04}-{:02}-{:02}", y, m, remaining + 1)
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn update_assigned_by(content: &str, new_value: &str) -> String {
    if content.starts_with("---") {
        if let Some(end) = content[3..].find("---") {
            let frontmatter = &content[3..3 + end];
            let rest = &content[3 + end..];
            let updated_fm: String = frontmatter
                .lines()
                .map(|line| {
                    if line.starts_with("assigned_by:") {
                        format!("assigned_by: {}", new_value)
                    } else {
                        line.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            return format!("---{}{}", updated_fm, rest);
        }
    }
    content.to_string()
}

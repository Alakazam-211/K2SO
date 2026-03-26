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

        let role = if agent_md.exists() {
            let content = fs::read_to_string(&agent_md).unwrap_or_default();
            let fm = parse_frontmatter(&content);
            fm.get("role").cloned().unwrap_or_default()
        } else {
            String::new()
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

/// Delegate a work item to another agent's inbox.
#[tauri::command]
pub fn k2so_agents_delegate(
    project_path: String,
    target_agent: String,
    source_file: String,
) -> Result<(), String> {
    let source = PathBuf::from(&source_file);
    if !source.exists() {
        return Err(format!("Source file does not exist: {}", source_file));
    }

    let target_dir = agent_work_dir(&project_path, &target_agent, "inbox");
    if !target_dir.exists() {
        return Err(format!("Target agent '{}' does not exist", target_agent));
    }

    let filename = source.file_name().ok_or("Invalid source filename")?;
    let target = target_dir.join(filename);

    let content = fs::read_to_string(&source).map_err(|e| e.to_string())?;
    let updated = update_assigned_by(&content, "delegated");

    fs::write(&target, &updated).map_err(|e| format!("Failed to write to target: {}", e))?;
    fs::remove_file(&source).map_err(|e| format!("Failed to remove source: {}", e))?;

    Ok(())
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

/// Generate a CLAUDE.md for an agent and write it to the project root.
/// Returns the generated content and the path it was written to.
#[tauri::command]
pub fn k2so_agents_generate_claude_md(
    project_path: String,
    agent_name: String,
) -> Result<String, String> {
    let dir = agent_dir(&project_path, &agent_name);
    if !dir.exists() {
        return Err(format!("Agent '{}' does not exist", agent_name));
    }

    // Read agent identity
    let agent_md_path = dir.join("agent.md");
    let agent_md = fs::read_to_string(&agent_md_path).unwrap_or_default();
    let fm = parse_frontmatter(&agent_md);
    let role = fm.get("role").cloned().unwrap_or("AI Agent".to_string());

    // Strip frontmatter to get body
    let agent_body = if agent_md.starts_with("---") {
        if let Some(end) = agent_md[3..].find("---") {
            agent_md[3 + end + 3..].trim().to_string()
        } else {
            agent_md.clone()
        }
    } else {
        agent_md.clone()
    };

    // Read inbox items
    let inbox_dir = agent_work_dir(&project_path, &agent_name, "inbox");
    let mut inbox_items = Vec::new();
    if inbox_dir.exists() {
        if let Ok(entries) = fs::read_dir(&inbox_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().map_or(false, |ext| ext == "md") {
                    if let Some(item) = read_work_item(&path, "inbox") {
                        inbox_items.push(item);
                    }
                }
            }
        }
    }

    // Read active items
    let active_dir = agent_work_dir(&project_path, &agent_name, "active");
    let mut active_items = Vec::new();
    if active_dir.exists() {
        if let Ok(entries) = fs::read_dir(&active_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().map_or(false, |ext| ext == "md") {
                    if let Some(item) = read_work_item(&path, "active") {
                        active_items.push(item);
                    }
                }
            }
        }
    }

    // List other agents for delegation awareness
    let mut other_agents = Vec::new();
    let agents_root = agents_dir(&project_path);
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

    // Build the CLAUDE.md
    let mut md = String::new();

    md.push_str(&format!("# K2SO Agent: {}\n\n", agent_name));
    md.push_str(&format!("## Identity\n**Role:** {}\n\n", role));
    if !agent_body.is_empty() {
        md.push_str(&format!("{}\n\n", agent_body));
    }

    // Current work
    md.push_str("## Your Work\n\n");
    md.push_str(&format!(
        "Your work items are at: `.k2so/agents/{}/work/`\n",
        agent_name
    ));
    md.push_str("- `inbox/` — assigned to you, pick the highest priority\n");
    md.push_str("- `active/` — move items here when you start working\n");
    md.push_str("- `done/` — move items here when complete\n\n");

    if !active_items.is_empty() {
        md.push_str("### Currently Active\n");
        for item in &active_items {
            md.push_str(&format!(
                "- **{}** (priority: {}, file: `{}`)\n",
                item.title, item.priority, item.filename
            ));
        }
        md.push_str("\n");
    }

    if !inbox_items.is_empty() {
        md.push_str("### Inbox (Pending)\n");
        for item in &inbox_items {
            md.push_str(&format!(
                "- **{}** (priority: {}, file: `{}`)\n",
                item.title, item.priority, item.filename
            ));
        }
        md.push_str("\n");
    }

    if active_items.is_empty() && inbox_items.is_empty() {
        md.push_str("*No work items currently assigned.*\n\n");
    }

    // Other agents
    if !other_agents.is_empty() {
        md.push_str("## Other Agents\n");
        md.push_str("You can delegate work to these agents:\n\n");
        for (name, their_role) in &other_agents {
            md.push_str(&format!("- **{}** — {}\n", name, their_role));
        }
        md.push_str("\n");
    }

    // CLI tools documentation
    md.push_str(CLI_TOOLS_DOCS);

    // Workflow
    md.push_str(WORKFLOW_DOCS);

    // Write to the agent's directory as CLAUDE.md
    let claude_md_path = dir.join("CLAUDE.md");
    fs::write(&claude_md_path, &md)
        .map_err(|e| format!("Failed to write CLAUDE.md: {}", e))?;

    Ok(md)
}

/// Build the launch command and args for starting an agent's Claude session.
/// Returns { command, args, cwd, claudeMdPath } for the frontend to open a terminal.
#[tauri::command]
pub fn k2so_agents_build_launch(
    project_path: String,
    agent_name: String,
    agent_cli_command: Option<String>,
) -> Result<serde_json::Value, String> {
    // Generate CLAUDE.md first
    k2so_agents_generate_claude_md(project_path.clone(), agent_name.clone())?;

    let claude_md_path = agent_dir(&project_path, &agent_name).join("CLAUDE.md");

    // Read the generated CLAUDE.md to use as system prompt
    let claude_md = fs::read_to_string(&claude_md_path).unwrap_or_default();

    // Build the prompt — the initial instruction for the agent
    let inbox_dir = agent_work_dir(&project_path, &agent_name, "inbox");
    let has_inbox = inbox_dir.exists() && fs::read_dir(&inbox_dir)
        .map(|e| e.count() > 0)
        .unwrap_or(false);

    let initial_prompt = if has_inbox {
        format!(
            "You are the K2SO agent \"{}\". Check your work queue at .k2so/agents/{}/work/inbox/ and start on the highest priority item. Move it to active/ when you begin. When done, move it to done/ and check if there's more work.",
            agent_name, agent_name
        )
    } else {
        format!(
            "You are the K2SO agent \"{}\". Your inbox is empty. Report your status and wait for work to be assigned.",
            agent_name
        )
    };

    let command = agent_cli_command.unwrap_or_else(|| "claude".to_string());

    Ok(serde_json::json!({
        "command": command,
        "args": [
            "--append-system-prompt",
            claude_md,
            "-p",
            initial_prompt
        ],
        "cwd": project_path,
        "claudeMdPath": claude_md_path.to_string_lossy(),
        "agentName": agent_name,
    }))
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

    let md = format!(
        r#"# K2SO Lead Agent: {project_name}

You are the **lead agent** for the {project_name} workspace, operating inside K2SO.

## Your Role

You are an orchestrator. You:
- **Triage** incoming requests from the workspace inbox (`.k2so/work/inbox/`)
- **Plan** work by creating PRDs, milestones, and technical specifications
- **Create sub-agents** to handle specialized work (backend, frontend, testing, etc.)
- **Delegate** tasks to the right sub-agents based on their skills
- **Coordinate** across the pod of agents in this workspace
- **You do NOT write code yourself** — you break work down and delegate to sub-agents who execute in worktrees

## Workspace Inbox

Incoming requests land at `.k2so/work/inbox/`. Check this first on every session.

{inbox_section}

## Your Pod

Sub-agents in this workspace:

{agent_section}

To create a new sub-agent:
```bash
k2so agents create <name> --role "Description of what this agent does"
```

## How to Plan Work

1. When a request arrives in the workspace inbox, read it carefully
2. Check existing PRDs and milestones in `.k2so/` for context
3. Break the request into actionable tasks with clear acceptance criteria
4. Create work items and assign to the right sub-agents:
   ```bash
   k2so work create --agent backend-eng --title "Implement OAuth endpoints" --body "..." --priority high --type technical-spec
   ```
5. For large features, create a PRD first:
   ```bash
   cat > .k2so/prds/oauth-support.md << 'EOF'
   ---
   title: OAuth Support
   status: planning
   ---
   ## Goal
   ...
   ## Milestones
   ...
   EOF
   ```

## How to Delegate

- Move workspace inbox items to agent inboxes: `k2so delegate <agent> <file>`
- Create new tasks directly: `k2so work create --agent <name> --title "..."`
- Check agent status: `k2so agents list` and `k2so agents work <name>`
- Send work to other workspaces: `k2so work send --workspace /path/to/project --title "..."`

{cli_section}

{workflow_section}
"#,
        project_name = project_name,
        inbox_section = if inbox_summary.is_empty() {
            "*Workspace inbox is empty.*".to_string()
        } else {
            format!("### Current Inbox\n{}", inbox_summary)
        },
        agent_section = if agent_list.is_empty() {
            "*No sub-agents created yet. Create agents as you identify the skills needed for this project.*".to_string()
        } else {
            agent_list
        },
        cli_section = CLI_TOOLS_DOCS,
        workflow_section = WORKFLOW_DOCS,
    );

    let claude_md_path = PathBuf::from(&project_path).join("CLAUDE.md");
    let disabled_path = PathBuf::from(&project_path).join(".k2so").join("CLAUDE.md.disabled");

    if disabled_path.exists() {
        // Restore previously disabled CLAUDE.md (preserves user edits)
        fs::rename(&disabled_path, &claude_md_path)
            .map_err(|e| format!("Failed to restore CLAUDE.md: {}", e))?;
        // Also save the freshly generated version for reference
        let generated_path = PathBuf::from(&project_path).join(".k2so").join("CLAUDE.md.generated");
        let _ = fs::write(&generated_path, &md);
        // Read back what was restored
        return fs::read_to_string(&claude_md_path).map_err(|e| e.to_string());
    }

    if claude_md_path.exists() {
        // CLAUDE.md already exists — don't overwrite user edits
        // Save generated version for reference so agents can diff if needed
        let generated_path = PathBuf::from(&project_path).join(".k2so").join("CLAUDE.md.generated");
        let _ = fs::write(&generated_path, &md);
        return fs::read_to_string(&claude_md_path).map_err(|e| e.to_string());
    }

    // First time — write fresh CLAUDE.md
    fs::write(&claude_md_path, &md)
        .map_err(|e| format!("Failed to write CLAUDE.md: {}", e))?;

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

You are operating inside K2SO. The `k2so` command is available in your terminal. Use it to interact with K2SO's workspace orchestration.

### Query
```
k2so agents list                     # List all agents in this workspace
k2so agents work <name>              # Show an agent's work items
k2so agents status <name>            # Show agent status
```

### Work Management
```
k2so work create --title "..." --body "..."  # Create a work item
  --agent <name>                              # Assign to agent (omit for unassigned)
  --priority high|normal|low                  # Set priority
  --type prd|milestone|task                   # Set type
```

### Workspace Inbox
```
k2so work inbox                      # Show items in workspace-level inbox
k2so work send --workspace <path>    # Send work to another workspace's inbox
  --title "..." --body "..."
```

### Delegation
```
k2so delegate <agent> <work-file>    # Move work item to another agent's inbox
```

### Git Operations
```
k2so commit                          # AI Commit: launch Claude to review & commit
k2so commit-merge                    # AI Commit & Merge: commit then merge into main
```

"#;

const WORKFLOW_DOCS: &str = r#"## Workflow

### If you are the Lead Agent (orchestrator):
1. Check the workspace inbox: `ls .k2so/work/inbox/` or `k2so work inbox`
2. Read each request and assess what needs to happen
3. Check existing PRDs/milestones in `.k2so/` for context
4. Decide which sub-agent should handle each request:
   - `k2so agents list` to see available agents and their roles
   - `k2so delegate <agent> <file>` to assign work to a sub-agent
   - Or break the request into sub-tasks and create work items: `k2so work create --agent <name> --title "..."`
5. If a request is blocked or needs user input, leave it in the workspace inbox and add a note
6. You do NOT implement code yourself — you orchestrate and plan

### If you are a Sub-Agent (executor):
1. Check your inbox: `ls .k2so/agents/<your-name>/work/inbox/`
2. Pick the highest priority item
3. Move it to `active/`: `mv .k2so/agents/<name>/work/inbox/<file> .k2so/agents/<name>/work/active/`
4. Create a worktree if needed for isolated work
5. Implement the changes described in the work item
6. Commit your changes
7. Move the item to `done/`: `mv .k2so/agents/<name>/work/active/<file> .k2so/agents/<name>/work/done/`
8. If sub-tasks need other skills, delegate to appropriate agents using `k2so delegate`
9. Use `k2so commit` when ready to finalize your changes

## Cross-Workspace Coordination
- Send work to another workspace: `k2so work send --workspace /path/to/project --title "..." --body "..."`
- This drops a work item in that workspace's inbox for its lead agent to triage

## Important Rules

- Always read your work item fully before starting
- Move items between folders to track your progress
- Create clear commit messages that reference the work item
- If blocked, document the blocker in the work item and move it back to inbox
- You can create new work items for other agents using `k2so work create --agent <name>`
- Never grab a work item that another agent has in their active/ folder
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

/// Approve an agent's work — merge the branch into main.
#[tauri::command]
pub fn k2so_agents_review_approve(
    project_path: String,
    branch: String,
    agent_name: String,
) -> Result<String, String> {
    // Merge the branch
    let result = crate::git::merge_branch(&project_path, &branch)?;

    // Clear done items for this agent
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

    if result.success {
        Ok(format!("Merged {} files", result.merged_files))
    } else {
        Err(format!("Merge conflicts: {}", result.conflicts.join(", ")))
    }
}

/// Reject an agent's work — move done items back to inbox with rejection feedback.
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

    // Move all done items back to inbox
    if let Ok(entries) = fs::read_dir(&done_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map_or(false, |ext| ext == "md") {
                let filename = path.file_name().unwrap();
                let target = inbox_dir.join(filename);
                let _ = fs::rename(&path, &target);
            }
        }
    }

    // Create a feedback file in inbox if reason provided
    if let Some(reason) = reason {
        let now = simple_date();
        let content = format!(
            "---\ntitle: Review Feedback — Work Rejected\npriority: high\nassigned_by: reviewer\ncreated: {}\ntype: feedback\n---\n\n## Rejection Reason\n\n{}\n\n## Action Required\n\nReview the feedback above and address the issues before resubmitting.\n",
            now, reason
        );
        let filename = format!("review-feedback-{}.md", now);
        let path = inbox_dir.join(&filename);
        fs::write(&path, &content).map_err(|e| e.to_string())?;
    }

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
/// Returns a structured result: whether the lead agent should wake (workspace inbox has items),
/// and which sub-agents have actionable inbox items.
///
/// Triage order:
/// 1. Workspace inbox has items → wake lead agent (returned as "__lead__")
/// 2. Sub-agent inboxes have items + no active work + no lock → wake those sub-agents
#[tauri::command]
pub fn k2so_agents_triage_decide(project_path: String) -> Result<Vec<String>, String> {
    let mut launchable = Vec::new();

    // Step 1: Check workspace inbox
    let ws_inbox = workspace_inbox_dir(&project_path);
    let has_workspace_inbox = ws_inbox.exists() && fs::read_dir(&ws_inbox)
        .map(|e| e.flatten().any(|e| e.path().extension().map_or(false, |ext| ext == "md")))
        .unwrap_or(false);

    if has_workspace_inbox {
        // Wake the lead agent to triage workspace inbox
        launchable.push("__lead__".to_string());
    }

    // Step 2: Check sub-agent inboxes
    let dir = agents_dir(&project_path);
    if dir.exists() {
        if let Ok(entries) = fs::read_dir(&dir) {
            for entry in entries.flatten() {
                if !entry.file_type().map_or(false, |ft| ft.is_dir()) {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().to_string();

                // Skip if locked (active Claude session)
                if is_agent_locked(&project_path, &name) {
                    continue;
                }

                let inbox = agent_work_dir(&project_path, &name, "inbox");
                let active = agent_work_dir(&project_path, &name, "active");

                let has_inbox = inbox.exists() && fs::read_dir(&inbox)
                    .map(|e| e.flatten().any(|e| e.path().extension().map_or(false, |ext| ext == "md")))
                    .unwrap_or(false);

                let has_active = active.exists() && fs::read_dir(&active)
                    .map(|e| e.flatten().any(|e| e.path().extension().map_or(false, |ext| ext == "md")))
                    .unwrap_or(false);

                if has_inbox && !has_active {
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
TOKEN_FILE="{home}/.k2so/hooks/notify.sh"

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

# We need the token. Extract from hook script if available.
TOKEN=""
if [ -f "$TOKEN_FILE" ]; then
    # The hook script doesn't embed the token — it reads K2SO_HOOK_TOKEN from env.
    # For the heartbeat, we need to get it differently.
    # Fallback: read from any K2SO terminal process environment
    TOKEN=$(ps -Eww -ax 2>/dev/null | grep K2SO_HOOK_TOKEN | grep -oE 'K2SO_HOOK_TOKEN=[^ ]+' | head -1 | cut -d= -f2)
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

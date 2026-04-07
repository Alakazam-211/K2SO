//! K2SO Agent system — autonomous AI workers operating within workspaces.
//!
//! Agents have a work queue (inbox/active/done) of markdown files,
//! a profile (agent.md), and interact with K2SO via the CLI bridge.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

use crate::db::schema::{AgentSession, WorkspaceRelation};

// ── DB helpers (standalone connection, no AppState needed) ──────────────

fn k2so_db_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".k2so/k2so.db")
}

fn resolve_project_id(conn: &rusqlite::Connection, path: &str) -> Option<String> {
    conn.query_row(
        "SELECT id FROM projects WHERE path = ?1",
        rusqlite::params![path],
        |r| r.get(0),
    )
    .ok()
}

// ── Types ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct K2soAgentInfo {
    pub name: String,
    pub role: String,
    pub inbox_count: usize,
    pub active_count: usize,
    pub done_count: usize,
    pub is_manager: bool,
    /// Agent type: "k2so", "custom", "manager", "agent-template"
    pub agent_type: String,
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
    pub body_preview: String,
    /// Work source: "feature", "issue", "crash", "security", "audit", "manual"
    pub source: String,
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
    let content = safe_read_to_string(path).ok()?;
    let fm = parse_frontmatter(&content);
    let filename = path.file_name()?.to_string_lossy().to_string();

    // Extract body preview (first ~120 chars after frontmatter)
    let body_preview = if content.starts_with("---") {
        if let Some(end) = content[3..].find("---") {
            let body = content[3 + end + 3..].trim();
            let preview: String = body.chars().take(120).collect();
            if body.len() > 120 { format!("{}...", preview.trim()) } else { preview.trim().to_string() }
        } else {
            String::new()
        }
    } else {
        let preview: String = content.chars().take(120).collect();
        if content.len() > 120 { format!("{}...", preview.trim()) } else { preview.trim().to_string() }
    };

    Some(WorkItem {
        filename,
        title: fm.get("title").cloned().unwrap_or_default(),
        priority: fm.get("priority").cloned().unwrap_or("normal".to_string()),
        assigned_by: fm.get("assigned_by").cloned().unwrap_or("unknown".to_string()),
        created: fm.get("created").cloned().unwrap_or_default(),
        item_type: fm.get("type").cloned().unwrap_or("task".to_string()),
        folder: folder.to_string(),
        body_preview,
        source: fm.get("source").cloned().unwrap_or("manual".to_string()),
    })
}

/// Bounded count of .md files in a directory (max 10,000 to prevent memory exhaustion
/// from corrupted or adversarial directories with millions of entries).
fn count_md_files(dir: &Path) -> usize {
    const MAX_COUNT: usize = 10_000;
    fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter(|e| e.path().extension().map_or(false, |ext| ext == "md"))
                .take(MAX_COUNT)
                .count()
        })
        .unwrap_or(0)
}

/// Maximum file size for reading work items and agent profiles (1MB).
/// Prevents memory exhaustion from malicious or corrupted files.
const MAX_FILE_SIZE: u64 = 1_048_576;

/// Atomic file write: write to a temp file in the same directory, then rename.
/// This prevents partial/corrupted files if the process crashes during write.
/// (Zed pattern: NamedTempFile + persist for atomic rename)
fn atomic_write(path: &Path, content: &str) -> Result<(), String> {
    let parent = path.parent().unwrap_or(Path::new("."));
    let tmp_path = parent.join(format!(".{}.tmp", path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "k2so".to_string())));
    fs::write(&tmp_path, content)
        .map_err(|e| format!("Failed to write temp file: {}", e))?;
    fs::rename(&tmp_path, path)
        .map_err(|e| {
            // Clean up temp file if rename fails
            let _ = fs::remove_file(&tmp_path);
            format!("Failed to rename temp file: {}", e)
        })
}

/// Read a file with size limit check to prevent OOM from large/malicious files.
fn safe_read_to_string(path: &Path) -> Result<String, String> {
    let metadata = fs::metadata(path).map_err(|e| format!("Cannot read {}: {}", path.display(), e))?;
    if metadata.len() > MAX_FILE_SIZE {
        return Err(format!("File too large ({} bytes, max {}): {}", metadata.len(), MAX_FILE_SIZE, path.display()));
    }
    fs::read_to_string(path).map_err(|e| format!("Failed to read {}: {}", path.display(), e))
}

// ── Heartbeat Configuration ─────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentHeartbeatConfig {
    /// Execution mode: "heartbeat", "persistent", or "hybrid"
    #[serde(default = "default_heartbeat_mode")]
    pub mode: String,
    /// Current check-in interval in seconds
    #[serde(default = "default_interval")]
    pub interval_seconds: u64,
    /// Current work phase (freeform, but well-known: setup, active, monitoring, idle, blocked)
    #[serde(default = "default_phase")]
    pub phase: String,
    /// Active hours window (optional)
    #[serde(default)]
    pub active_hours: Option<ActiveHours>,
    /// Maximum interval (auto-backoff ceiling)
    #[serde(default = "default_max_interval")]
    pub max_interval_seconds: u64,
    /// Minimum interval (floor)
    #[serde(default = "default_min_interval")]
    pub min_interval_seconds: u64,
    /// Cost budget: "low", "medium", "high"
    #[serde(default = "default_cost_budget")]
    pub cost_budget: String,
    /// Consecutive no-ops (for auto-backoff)
    #[serde(default)]
    pub consecutive_no_ops: u32,
    /// Enable auto-backoff on idle
    #[serde(default = "default_true")]
    pub auto_backoff: bool,
    /// ISO timestamp of last wake
    #[serde(default)]
    pub last_wake: Option<String>,
    /// ISO timestamp of next scheduled wake
    #[serde(default)]
    pub next_wake: Option<String>,
    /// Who last updated: "agent" or "user"
    #[serde(default = "default_updated_by")]
    pub updated_by: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveHours {
    pub start: String,
    pub end: String,
    pub timezone: String,
}

fn default_heartbeat_mode() -> String { "heartbeat".to_string() }
fn default_interval() -> u64 { 300 }
fn default_phase() -> String { "monitoring".to_string() }
fn default_max_interval() -> u64 { 3600 }
fn default_min_interval() -> u64 { 60 }
fn default_cost_budget() -> String { "low".to_string() }
fn default_true() -> bool { true }
fn default_updated_by() -> String { "user".to_string() }

impl Default for AgentHeartbeatConfig {
    fn default() -> Self {
        Self {
            mode: default_heartbeat_mode(),
            interval_seconds: default_interval(),
            phase: default_phase(),
            active_hours: None,
            max_interval_seconds: default_max_interval(),
            min_interval_seconds: default_min_interval(),
            cost_budget: default_cost_budget(),
            consecutive_no_ops: 0,
            auto_backoff: true,
            last_wake: None,
            next_wake: None,
            updated_by: default_updated_by(),
        }
    }
}

/// Read an agent's heartbeat configuration from .k2so/agents/<name>/heartbeat.json
fn read_heartbeat_config(project_path: &str, agent_name: &str) -> AgentHeartbeatConfig {
    let path = agent_dir(project_path, agent_name).join("heartbeat.json");
    if path.exists() {
        fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    } else {
        AgentHeartbeatConfig::default()
    }
}

/// Write an agent's heartbeat configuration to .k2so/agents/<name>/heartbeat.json
fn write_heartbeat_config(project_path: &str, agent_name: &str, config: &AgentHeartbeatConfig) -> Result<(), String> {
    let dir = agent_dir(project_path, agent_name);
    if !dir.exists() {
        return Err(format!("Agent '{}' does not exist", agent_name));
    }
    let path = dir.join("heartbeat.json");
    let json = serde_json::to_string_pretty(config)
        .map_err(|e| format!("Failed to serialize heartbeat config: {}", e))?;
    atomic_write(&path, &json)
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

        let (role, is_manager, agent_type) = if agent_md.exists() {
            let content = fs::read_to_string(&agent_md).unwrap_or_default();
            let fm = parse_frontmatter(&content);
            let role = fm.get("role").cloned().unwrap_or_default();
            // Support old ("pod_leader", "coordinator") and new ("manager") frontmatter keys
            let is_mgr = fm.get("pod_leader").map(|v| v == "true").unwrap_or(false)
                || fm.get("coordinator").map(|v| v == "true").unwrap_or(false)
                || fm.get("manager").map(|v| v == "true").unwrap_or(false);
            let agent_type = fm.get("type").cloned().map(|t| {
                // Migrate old type values to new ones
                match t.as_str() {
                    "pod-leader" | "coordinator" => "manager".to_string(),
                    "pod-member" => "agent-template".to_string(),
                    other => other.to_string(),
                }
            }).unwrap_or_else(|| {
                if is_mgr { "manager".to_string() } else { "agent-template".to_string() }
            });
            (role, is_mgr, agent_type)
        } else {
            (String::new(), false, "agent-template".to_string())
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
            is_manager,
            agent_type,
        });
    }

    agents.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(agents)
}

/// Create a new K2SO agent with directory structure.
/// `agent_type` can be: "k2so", "custom", "manager", "agent-template" (defaults to "agent-template").
#[tauri::command]
pub fn k2so_agents_create(
    project_path: String,
    name: String,
    role: String,
    prompt: Option<String>,
    agent_type: Option<String>,
) -> Result<K2soAgentInfo, String> {
    if !name.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
        return Err("Agent name must be alphanumeric (hyphens and underscores allowed)".to_string());
    }

    let dir = agent_dir(&project_path, &name);
    if dir.exists() {
        return Err(format!("Agent '{}' already exists", name));
    }

    let agent_type = agent_type.unwrap_or_else(|| "agent-template".to_string());
    let is_manager = agent_type == "manager" || agent_type == "coordinator";

    fs::create_dir_all(agent_work_dir(&project_path, &name, "inbox"))
        .map_err(|e| format!("Failed to create inbox: {}", e))?;
    fs::create_dir_all(agent_work_dir(&project_path, &name, "active"))
        .map_err(|e| format!("Failed to create active: {}", e))?;
    fs::create_dir_all(agent_work_dir(&project_path, &name, "done"))
        .map_err(|e| format!("Failed to create done: {}", e))?;
    let _ = fs::create_dir_all(workspace_inbox_dir(&project_path));

    let agent_md = dir.join("agent.md");
    let mut frontmatter = format!("name: {}\nrole: {}\ntype: {}", name, role, agent_type);
    if is_manager {
        frontmatter.push_str("\nmanager: true");
    }

    // Build type-appropriate default body if no custom prompt provided
    let body = if let Some(ref p) = prompt {
        if !p.is_empty() { p.clone() } else { generate_default_agent_body(&agent_type, &name, &role, &project_path) }
    } else {
        generate_default_agent_body(&agent_type, &name, &role, &project_path)
    };

    let content = format!("---\n{}\n---\n\n{}\n", frontmatter, body);
    atomic_write(&agent_md, &content)?;

    // Generate SKILL.md for the new agent
    write_agent_skill_file(&project_path, &name, &agent_type);

    Ok(K2soAgentInfo {
        name,
        role,
        inbox_count: 0,
        active_count: 0,
        done_count: 0,
        is_manager,
        agent_type,
    })
}

/// Delete a K2SO agent and its directory.
#[tauri::command]
pub fn k2so_agents_delete(project_path: String, name: String) -> Result<(), String> {
    k2so_agents_delete_inner(&project_path, &name, false)
}

/// Delete an agent with optional force flag.
/// - Refuses to delete manager/coordinator (unless force)
/// - Refuses to delete if agent has active work (unless force)
/// - Removes .k2so/agents/<name>/ directory
pub fn k2so_agents_delete_inner(project_path: &str, name: &str, force: bool) -> Result<(), String> {
    let dir = agent_dir(project_path, name);
    if !dir.exists() {
        return Err(format!("Agent '{}' does not exist", name));
    }

    // Read agent type to check if it's a manager/coordinator
    let agent_md = dir.join("agent.md");
    if agent_md.exists() {
        let content = fs::read_to_string(&agent_md).unwrap_or_default();
        let fm = parse_frontmatter(&content);
        if fm.get("type").map_or(false, |t| t == "manager" || t == "coordinator" || t == "pod-leader") && !force {
            return Err("Cannot delete manager agent. Use --force to override.".to_string());
        }
    }

    // Check for active work items
    if !force {
        let active_dir = agent_work_dir(project_path, name, "active");
        if active_dir.exists() {
            let active_count = fs::read_dir(&active_dir)
                .map_err(|e| format!("Cannot check active work for '{}': {}", name, e))?
                .flatten()
                .count();
            if active_count > 0 {
                return Err(format!(
                    "Agent '{}' has {} active work item(s). Use --force to delete anyway.",
                    name, active_count
                ));
            }
        }
    }

    fs::remove_dir_all(&dir).map_err(|e| format!("Failed to delete agent: {}", e))?;
    Ok(())
}

/// Update a specific field in an agent's frontmatter (or a markdown section in the body).
/// `field` can be a frontmatter key (e.g. "role") or a section name (e.g. "Work Sources").
/// For frontmatter fields, `value` replaces the existing value.
/// For body sections (## heading), `value` replaces everything from ## heading to the next ## heading.
#[tauri::command]
pub fn k2so_agents_update_field(
    project_path: String,
    name: String,
    field: String,
    value: String,
) -> Result<String, String> {
    let dir = agent_dir(&project_path, &name);
    if !dir.exists() {
        return Err(format!("Agent '{}' does not exist", name));
    }

    let md_path = dir.join("agent.md");
    let content = fs::read_to_string(&md_path)
        .map_err(|e| format!("Failed to read agent.md: {}", e))?;

    let updated = if content.starts_with("---") {
        if let Some(end_idx) = content[3..].find("---") {
            let frontmatter = &content[3..3 + end_idx];
            let body = &content[3 + end_idx + 3..];

            // Check if it's a frontmatter field
            let fm_keys: Vec<&str> = frontmatter.lines()
                .filter_map(|l| l.split_once(':').map(|(k, _)| k.trim()))
                .collect();

            if fm_keys.contains(&field.as_str()) {
                // Update frontmatter field
                let updated_fm: String = frontmatter
                    .lines()
                    .map(|line| {
                        if let Some((key, _)) = line.split_once(':') {
                            if key.trim() == field {
                                return format!("{}: {}", field, value);
                            }
                        }
                        line.to_string()
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                format!("---\n{}\n---{}", updated_fm.trim(), body)
            } else {
                // Try to find and replace a markdown section (## heading)
                let section_header = format!("## {}", field);
                if let Some(start) = body.find(&section_header) {
                    let after_header = start + section_header.len();
                    // Find the next ## heading or end of body
                    let end = body[after_header..].find("\n## ")
                        .map(|pos| after_header + pos)
                        .unwrap_or(body.len());
                    let mut new_body = String::new();
                    new_body.push_str(&body[..start]);
                    new_body.push_str(&section_header);
                    new_body.push_str("\n\n");
                    new_body.push_str(&value);
                    new_body.push_str("\n\n");
                    new_body.push_str(body[end..].trim_start());
                    format!("---\n{}\n---{}", frontmatter.trim(), new_body)
                } else {
                    // Section doesn't exist — append it
                    let mut new_body = body.to_string();
                    if !new_body.ends_with('\n') {
                        new_body.push('\n');
                    }
                    new_body.push_str(&format!("\n## {}\n\n{}\n", field, value));
                    format!("---\n{}\n---{}", frontmatter.trim(), new_body)
                }
            }
        } else {
            return Err("Invalid frontmatter in agent.md".to_string());
        }
    } else {
        return Err("agent.md missing frontmatter".to_string());
    };

    // Backup before writing
    let backup_dir = dir.join("agent-backups");
    let _ = fs::create_dir_all(&backup_dir);
    let backup_name = format!("agent-{}.md", simple_date().replace(' ', "_").replace(':', "-"));
    let _ = fs::copy(&md_path, backup_dir.join(&backup_name));
    cleanup_agent_backups(&backup_dir, 20);

    atomic_write(&md_path, &updated)?;

    Ok(updated)
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
    source: Option<String>,
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
    let source = source.unwrap_or_else(|| "manual".to_string());

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
        "---\ntitle: {}\npriority: {}\nassigned_by: user\ncreated: {}\ntype: {}\nsource: {}\n---\n\n{}\n",
        title, priority, now, item_type, source, body
    );

    let path = target_dir.join(&filename);
    atomic_write(&path, &content)?;

    let body_preview = { let trimmed = body.trim(); let preview: String = trimmed.chars().take(120).collect(); if trimmed.chars().count() > 120 { format!("{}...", preview.trim()) } else { preview } };

    // Push channel event for persistent agents
    if let Some(ref agent) = agent_name {
        crate::agent_hooks::push_agent_event(
            &project_path,
            agent,
            "work-item",
            &format!("New work item in your inbox: \"{}\" (priority: {})", title, priority),
            &priority,
        );
    } else {
        // Workspace inbox — notify the lead agent
        crate::agent_hooks::push_agent_event(
            &project_path,
            "__lead__",
            "work-item",
            &format!("New item in workspace inbox: \"{}\" (priority: {})", title, priority),
            &priority,
        );
    }

    Ok(WorkItem {
        filename,
        title,
        priority,
        assigned_by: "user".to_string(),
        created: now,
        item_type,
        folder: if agent_name.is_some() { "inbox".to_string() } else { "workspace-inbox".to_string() },
        body_preview,
        source,
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
    let full_slug = item.filename.trim_end_matches(".md");
    let task_slug = shorten_slug(full_slug, 40);
    let branch_name = format!("agent/{}/{}", target_agent, task_slug);
    let worktree = crate::git::create_worktree(&project_path, &branch_name)
        .map_err(|e| format!("Failed to create worktree: {}", e))?;

    // Register the worktree as a workspace in the DB so it appears in the sidebar.
    // Uses the same schema as git_create_worktree: (id, project_id, name, type, branch, tab_order, worktree_path)
    if let Some(home) = dirs::home_dir() {
        let db_path = home.join(".k2so").join("k2so.db");
        if let Ok(conn) = rusqlite::Connection::open(&db_path) {
            conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;").ok();
            if let Ok(project_id) = conn.query_row(
                "SELECT id FROM projects WHERE path = ?1",
                rusqlite::params![project_path],
                |row| row.get::<_, String>(0),
            ) {
                let ws_id = uuid::Uuid::new_v4().to_string();
                let max_order: i32 = conn.query_row(
                    "SELECT COALESCE(MAX(tab_order), -1) + 1 FROM workspaces WHERE project_id = ?1",
                    rusqlite::params![project_id],
                    |row| row.get(0),
                ).unwrap_or(0);
                if let Err(e) = conn.execute(
                    "INSERT INTO workspaces (id, project_id, name, type, branch, tab_order, worktree_path) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    rusqlite::params![ws_id, project_id, worktree.branch, "worktree", worktree.branch, max_order, worktree.path],
                ) {
                    log_debug!("[delegate] Failed to register worktree in DB: {}", e);
                }
            }
        }
    }

    // 2. Move work item to agent's active/ folder with worktree info
    let active_dir = agent_work_dir(&project_path, &target_agent, "active");
    fs::create_dir_all(&active_dir).ok();
    let updated = update_assigned_by(&content, "delegated");
    let updated = add_worktree_to_frontmatter(&updated, &worktree.path, &worktree.branch);
    let active_file = active_dir.join(&item.filename);
    atomic_write(&active_file, &updated)?;
    fs::remove_file(&source).map_err(|e| format!("Failed to remove source: {}", e))?;

    // 3. Generate a task-specific CLAUDE.md and write it to the worktree root
    let claude_md = generate_agent_claude_md_content(&project_path, &target_agent, Some(&item))?;
    let claude_md_path = PathBuf::from(&worktree.path).join("CLAUDE.md");
    atomic_write(&claude_md_path, &claude_md)?;

    // 4. Build the launch command for the frontend
    // Use --append-system-prompt with task instructions baked in (NOT -p which is non-interactive).
    // The CLAUDE.md already contains agent identity + task context. We append the initial
    // instructions so Claude starts in interactive mode with full context loaded.
    // Determine completion protocol based on workspace state capability
    let source = &item.source;
    let capability = if let Some(ws_state) = get_workspace_state(&project_path) {
        ws_state.capability_for_source(source).to_string()
    } else {
        "gated".to_string()
    };

    let completion_protocol = if capability == "auto" {
        format!(
            "When done:\n\
            1. Commit all your changes to branch `{branch}`\n\
            2. Run: `k2so agent complete --agent {agent} --file {filename}`\n\
            This will automatically merge your branch into main and clean up the worktree.\n\
            3. Notify the workspace manager that you're done:\n\
            Run `k2so agents running` to find the manager's terminal ID (look for `.k2so/agents/manager` in the CWD),\n\
            then run: `k2so terminal write <manager-terminal-id> \"Completed: {title}. Branch {branch} merged.\"`",
            agent = target_agent, branch = worktree.branch, filename = item.filename, title = item.title,
        )
    } else {
        format!(
            "When done:\n\
            1. Commit all your changes to branch `{branch}`\n\
            2. Run: `k2so agent complete --agent {agent} --file {filename}`\n\
            This will move your work to done and flag it for human review.\n\
            3. Notify the workspace manager that your work is ready for review:\n\
            Run `k2so agents running` to find the manager's terminal ID (look for `.k2so/agents/manager` in the CWD),\n\
            then run: `k2so terminal write <manager-terminal-id> \"Ready for review: {title}. Branch: {branch}\"`",
            agent = target_agent, branch = worktree.branch, filename = item.filename, title = item.title,
        )
    };

    let task_instructions = format!(
        "\n\n## Your Current Assignment\n\n\
        You are working in a dedicated worktree at `{wt_path}` on branch `{branch}`.\n\n\
        **{title}** (priority: {priority})\n\n\
        Read the full task file at `.k2so/agents/{agent}/work/active/{filename}` for details and acceptance criteria.\n\n\
        ## Completion Protocol\n\n\
        {completion_protocol}",
        agent = target_agent,
        wt_path = worktree.path,
        branch = worktree.branch,
        title = item.title,
        priority = item.priority,
        filename = item.filename,
        completion_protocol = completion_protocol,
    );

    // Append task instructions to the CLAUDE.md content for the system prompt
    let full_system_prompt = format!("{}\n{}", claude_md, task_instructions);

    // Initial message to kick off work (positional arg, NOT -p which is non-interactive)
    let kickoff = format!(
        "Read your task file at `{}` and begin implementing the fix. \
        Commit your work as you go.",
        agent_work_dir(&project_path, &target_agent, "active").join(&item.filename).to_string_lossy()
    );

    Ok(serde_json::json!({
        "command": "claude",
        "args": ["--dangerously-skip-permissions", "--append-system-prompt", full_system_prompt, kickoff],
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
    atomic_write(&path, &content)
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
            if let Some(item) = read_work_item(&path, "inbox") {
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
    source: Option<String>,
) -> Result<WorkItem, String> {
    let dir = workspace_inbox_dir(&workspace_path);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    let priority = priority.unwrap_or_else(|| "normal".to_string());
    let item_type = item_type.unwrap_or_else(|| "task".to_string());
    let assigned_by = assigned_by.unwrap_or_else(|| "external".to_string());
    let source = source.unwrap_or_else(|| "manual".to_string());

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
        "---\ntitle: {}\npriority: {}\nassigned_by: {}\ncreated: {}\ntype: {}\nsource: {}\n---\n\n{}\n",
        title, priority, assigned_by, now, item_type, source, body
    );

    let path = dir.join(&filename);
    atomic_write(&path, &content)?;

    let body_preview = { let trimmed = body.trim(); let preview: String = trimmed.chars().take(120).collect(); if trimmed.chars().count() > 120 { format!("{}...", preview.trim()) } else { preview } };
    Ok(WorkItem {
        filename,
        title,
        priority,
        assigned_by,
        created: now,
        item_type,
        folder: "workspace-inbox".to_string(),
        body_preview,
        source,
    })
}

// ── Lock Files ──────────────────────────────────────────────────────────

/// Create a lock file for an agent (called when a Claude session starts).
/// Also upserts an AgentSession row in the DB for richer tracking.
#[tauri::command]
pub fn k2so_agents_lock(
    project_path: String,
    agent_name: String,
    terminal_id: Option<String>,
    owner: Option<String>,
) -> Result<(), String> {
    // DB tracking (best-effort)
    if let Ok(conn) = rusqlite::Connection::open(k2so_db_path()) {
        if let Some(project_id) = resolve_project_id(&conn, &project_path) {
            let session_uuid = uuid::Uuid::new_v4().to_string();
            let owner_val = owner.as_deref().unwrap_or("system");
            let _ = AgentSession::upsert(
                &conn,
                &session_uuid,
                &project_id,
                &agent_name,
                terminal_id.as_deref(),
                None,
                "claude",
                owner_val,
                "running",
            );
        }
    }

    // Legacy .lock file (backward compat)
    let lock_path = agent_work_dir(&project_path, &agent_name, "").join(".lock");
    if let Some(parent) = lock_path.parent() {
        fs::create_dir_all(parent).ok();
    }
    fs::write(&lock_path, simple_date()).map_err(|e| e.to_string())
}

/// Remove a lock file for an agent (called when a Claude session ends).
/// Also updates the DB session status to "sleeping".
#[tauri::command]
pub fn k2so_agents_unlock(project_path: String, agent_name: String) -> Result<(), String> {
    // DB tracking (best-effort)
    if let Ok(conn) = rusqlite::Connection::open(k2so_db_path()) {
        if let Some(project_id) = resolve_project_id(&conn, &project_path) {
            let _ = AgentSession::update_status(&conn, &project_id, &agent_name, "sleeping");
        }
    }

    // Legacy .lock file removal
    let lock_path = agent_work_dir(&project_path, &agent_name, "").join(".lock");
    if lock_path.exists() {
        fs::remove_file(&lock_path).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Check if an agent is locked (has an active session).
/// Tries DB first, falls back to .lock file.
pub fn is_agent_locked(project_path: &str, agent_name: &str) -> bool {
    // Try DB first
    if let Ok(conn) = rusqlite::Connection::open(k2so_db_path()) {
        if let Some(project_id) = resolve_project_id(&conn, project_path) {
            if let Ok(Some(session)) = AgentSession::get_by_agent(&conn, &project_id, agent_name) {
                if session.status == "running" {
                    return true;
                }
            }
        }
    }

    // Fall back to .lock file
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
    atomic_write(&claude_md_path, &md)?;

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

                            let resume_context = format!(
                                "{}\n\n## Resuming Work\n\n\
                                You are in worktree `{wt_path}` on branch `{branch}`.\n\
                                Current task: **{title}** (priority: {priority})\n\
                                Task file: `.k2so/agents/{agent}/work/active/{filename}`\n\n\
                                Continue where you left off. When done: `k2so work move --agent {agent} --file {filename} --from active --to done`",
                                claude_md,
                                agent = agent_name, wt_path = wt_path, branch = branch,
                                title = item.title, priority = item.priority, filename = item.filename,
                            );

                            let resume_kickoff = format!(
                                "Continue working on your task: **{}**. Check your progress and pick up where you left off.",
                                item.title
                            );

                            return Ok(serde_json::json!({
                                "command": command,
                                "args": ["--dangerously-skip-permissions", "--append-system-prompt", resume_context, resume_kickoff],
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

    // Also regenerate the workspace root CLAUDE.md so the user's Claude session
    // (which launches from the workspace root) has the latest CLI tools and docs
    let _ = k2so_agents_generate_workspace_claude_md(project_path.clone());

    // Check for previous session to resume (avoids cold-start context reload)
    // Priority: DB → .last_session file → Claude's history.jsonl
    let agent_cwd = agent_dir(&project_path, &agent_name);
    let resume_session = (|| -> Option<String> {
        // Try DB first
        if let Ok(conn) = rusqlite::Connection::open(k2so_db_path()) {
            if let Some(project_id) = resolve_project_id(&conn, &project_path) {
                if let Ok(Some(session)) = AgentSession::get_by_agent(&conn, &project_id, &agent_name) {
                    if let Some(sid) = session.session_id {
                        if !sid.is_empty() {
                            return Some(sid);
                        }
                    }
                }
            }
        }
        None
    })()
    .or_else(|| {
        // Fall back to .last_session file
        let session_file = agent_cwd.join(".last_session");
        fs::read_to_string(&session_file).ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    })
    .or_else(|| {
        // Fall back to detecting from Claude's history (agent dir)
        crate::commands::chat_history::chat_history_detect_active_session(
            "claude".to_string(),
            agent_cwd.to_string_lossy().to_string(),
        ).ok().flatten()
    })
    .or_else(|| {
        // Also try project root — older sessions may be stored under project path
        crate::commands::chat_history::chat_history_detect_active_session(
            "claude".to_string(),
            project_path.clone(),
        ).ok().flatten()
    });

    let mut args = vec!["--dangerously-skip-permissions".to_string(), "--append-system-prompt".to_string(), claude_md];
    if let Some(ref session_id) = resume_session {
        args.push("--resume".to_string());
        args.push(session_id.clone());
    }

    // Use project root as CWD so the agent has access to the codebase
    let launch_cwd = project_path.clone();

    Ok(serde_json::json!({
        "command": command,
        "args": args,
        "cwd": launch_cwd,
        "claudeMdPath": claude_md_path.to_string_lossy(),
        "agentName": agent_name,
        "worktreePath": null,
        "branch": null,
        "resumeSession": resume_session,
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

/// Generate a default agent.md body based on agent type.
/// This gives each agent a rich starting template that users (or AI) can refine via AIFileEditor.
fn generate_default_agent_body(agent_type: &str, name: &str, role: &str, project_path: &str) -> String {
    let project_name = std::path::Path::new(project_path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "workspace".to_string());

    match agent_type {
        "manager" | "coordinator" | "pod-leader" => {
            // List existing agent templates for the "Your Team" section
            let mut team_lines = String::new();
            let agents_root = agents_dir(project_path);
            if agents_root.exists() {
                if let Ok(entries) = fs::read_dir(&agents_root) {
                    for entry in entries.flatten() {
                        if entry.file_type().map_or(false, |ft| ft.is_dir()) {
                            let member_name = entry.file_name().to_string_lossy().to_string();
                            if member_name == name { continue; }
                            let md_path = entry.path().join("agent.md");
                            let member_role = if md_path.exists() {
                                let content = fs::read_to_string(&md_path).unwrap_or_default();
                                let fm = parse_frontmatter(&content);
                                fm.get("role").cloned().unwrap_or_default()
                            } else {
                                String::new()
                            };
                            team_lines.push_str(&format!(
                                "- **{}**: `.k2so/agents/{}/agent.md` — {}\n",
                                member_name, member_name, member_role
                            ));
                        }
                    }
                }
            }
            let team_section = if team_lines.is_empty() {
                "No agent templates yet. Create agents based on the skills this project needs.".to_string()
            } else {
                format!("Read their agent.md profiles when delegating to match tasks to the right specialist.\n\n{}", team_lines)
            };

            format!(
r#"You are the Workspace Manager for the {project_name} workspace.

## Work Sources

Primary (always checked by local LLM triage — near-zero cost):
- Workspace inbox: `.k2so/work/inbox/` (unassigned work items)
- Your inbox: `.k2so/agents/{name}/work/inbox/` (delegated to you)

External (scan these proactively when woken — customize for your project):
- GitHub Issues: `gh issue list --repo OWNER/REPO --label bug,feature --state open`
- Open PRs needing review: `gh pr list --repo OWNER/REPO --review-requested`
- Local PRDs: `.k2so/prds/*.md`

## Your Team

{team_section}

## Tools Available

- `k2so agent create --name "new-agent" --role "Specialization description"` — create a new agent template
- `k2so agent update --name "agent-name" --field role --value "Updated role"` — update a member's profile
- `k2so delegate <agent> <work-file>` — assign work (creates worktree + launches agent)
- `k2so work create --agent <name> --title "..." --body "..."` — create a task for an agent
- `k2so reviews` — see completed work ready for review
- `k2so review approve <agent> <branch>` — merge completed work
- `k2so terminal spawn --title "..." --command "..."` — run parallel tasks

## Standing Orders

<!-- Persistent directives checked every time this agent wakes up. -->
<!-- Unlike work items (which are one-off tasks), standing orders are ongoing. -->
<!-- Examples: -->
<!-- - Check CI status on main branch every wake and report failures -->
<!-- - Review open PRs older than 24 hours -->
<!-- - Monitor .k2so/work/inbox/ for unassigned items and delegate immediately -->

## Operational Notes

- An agent is a role template, not a person — the same agent can run in multiple worktrees simultaneously
- You orchestrate and review — you do NOT implement code yourself
- When you need a new skill, create a new agent with `k2so agent create`
- Read agent templates' agent.md files to understand their strengths before delegating
"#,
                project_name = project_name,
                name = name,
                team_section = team_section,
            )
        }
        "agent-template" | "pod-member" => {
            format!(
r#"## Specialization

{role}

## Capabilities

- Implement changes in isolated git worktrees (one branch per task)
- Commit frequently with clear messages referencing the task
- Follow existing code patterns and conventions in the project
- Run tests before marking work as done

## How You Work

1. You are launched into a dedicated worktree with your task in the CLAUDE.md
2. Read the task file for full requirements and acceptance criteria
3. Implement the changes — all work happens in your worktree
4. Commit to your branch as you go
5. When done: `k2so work move --agent {name} --file <task>.md --from active --to done`
6. Your work appears in the review queue for the Workspace Manager to approve or reject

## Standing Orders

<!-- Persistent directives checked every time this agent wakes up. -->
<!-- Examples: -->
<!-- - Run tests before marking any task as done -->
<!-- - Follow the project's commit message convention -->
<!-- - Never modify files outside your assigned scope -->

## If Blocked

- If you need clarification, move the task back to inbox with a note
- If you need another agent's work first, document the dependency in the task file
- Never edit files outside your worktree
"#,
                role = role,
                name = name,
            )
        }
        "custom" => {
            format!(
r#"## Role

{role}

## Heartbeat Control

You run on an adaptive heartbeat. Adjust your check-in frequency based on what you're doing:

- `k2so heartbeat set --interval 60 --phase "active"` — check every minute (busy periods)
- `k2so heartbeat set --interval 300 --phase "monitoring"` — every 5 minutes (watching)
- `k2so heartbeat set --interval 3600 --phase "idle"` — every hour (dormant)

## Tools Available

- `k2so terminal spawn --title "..." --command "..."` — run parallel tasks
- `k2so heartbeat set --interval N --phase "..."` — adjust your check-in frequency
- Standard CLI tools available in your terminal: `gh`, `git`, `curl`, etc.

## Standing Orders

<!-- Persistent directives checked every time this agent wakes on the heartbeat. -->
<!-- Unlike one-off tasks, standing orders are ongoing responsibilities. -->
<!-- Examples: -->
<!-- - Check GitHub issues every wake: `gh issue list --repo OWNER/REPO --state open` -->
<!-- - Monitor a Slack channel for requests -->
<!-- - Run a health check script and report failures -->

## Operational Notes

- Your agent.md is your complete identity — everything about who you are and what you do lives here
- Customize the sections above to match your specific use case
- Use the AIFileEditor in K2SO Settings to refine your profile with AI assistance
"#,
                role = role,
            )
        }
        "k2so" => {
            format!(
r#"You are the K2SO Agent for the {project_name} workspace — the top-level planner and orchestrator.

## Work Sources

Primary (checked automatically by the heartbeat system at near-zero cost):
- Workspace inbox: `.k2so/work/inbox/` (unassigned work items)
- Your inbox: `.k2so/agents/{name}/work/inbox/` (items delegated to you)

External (add your project-specific sources below — CLI tools only, no MCP):
- GitHub Issues: `gh issue list --repo OWNER/REPO --label bug,feature --state open`
- Open PRs: `gh pr list --repo OWNER/REPO --review-requested`
<!-- Add more work sources here: Linear, Jira, custom APIs, intake directories, etc. -->

## Project Context

<!-- Describe what this project does, key directories, conventions, tech stack -->

## Integration Commands

<!-- CLI tools this agent should use to check for work, report status, or interact with external systems -->
- `gh` — GitHub CLI for issues, PRs, releases
- `git` — Version control operations
- `curl` / `jq` — API calls and JSON processing

## Constraints

<!-- Hours of operation, cost limits, repos off-limits, branches to protect -->

## Standing Orders

<!-- Persistent directives checked every time this agent wakes up. -->
<!-- Unlike work items in the inbox (one-off tasks), standing orders are ongoing. -->
<!-- Examples: -->
<!-- - Scan GitHub issues for new bugs every wake -->
<!-- - Check CI pipeline status on main and report failures -->
<!-- - Review PRs older than 48 hours -->
<!-- - Monitor .k2so/work/inbox/ and delegate unassigned items immediately -->

## Operational Notes

- Editing the sections above is how you customize the K2SO agent for your project
- The default K2SO knowledge (CLI tools, workflow, work queues) is auto-injected at launch
- Modifying the auto-injected defaults in CLAUDE.md is at your own risk
- Use the Manage Persona button in Settings to refine this profile with AI assistance
"#,
                project_name = project_name,
                name = name,
            )
        }
        _ => {
            // Unknown type — empty body
            String::new()
        }
    }
}

/// Format a capability state for display in CLAUDE.md.
fn format_cap(cap: &str) -> &str {
    match cap {
        "auto" => "auto (build + merge)",
        "gated" => "gated (build PR, wait for approval)",
        "off" => "off (do not act)",
        _ => cap,
    }
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

/// Shorten a slug to a maximum length, breaking at word boundaries.
/// Strips common filler prefixes (bug-, feature-) and filler words.
fn shorten_slug(slug: &str, max_len: usize) -> String {
    // Strip common prefixes
    let stripped = slug
        .strip_prefix("bug-").or_else(|| slug.strip_prefix("feature-"))
        .or_else(|| slug.strip_prefix("task-"))
        .unwrap_or(slug);

    if stripped.len() <= max_len {
        return stripped.to_string();
    }

    // Truncate at a word boundary (hyphen)
    let truncated = &stripped[..max_len];
    match truncated.rfind('-') {
        Some(pos) if pos > max_len / 2 => truncated[..pos].to_string(),
        _ => truncated.to_string(),
    }
}

/// Extract a named section from markdown content (## Heading through next ## or end).
/// Returns the body text (without the heading itself), or None if the section is empty/absent.
fn extract_section(content: &str, heading: &str) -> Option<String> {
    let marker = format!("## {}", heading);
    let start = content.find(&marker)?;
    let after_heading = start + marker.len();
    // Skip to the line after the heading (or use remaining content if heading is at EOF)
    let body_start = match content[after_heading..].find('\n') {
        Some(i) => after_heading + i + 1,
        None => return None, // heading at EOF with no body
    };
    // Find the next ## heading or end of content
    let body_end = content[body_start..]
        .find("\n## ")
        .map(|i| body_start + i)
        .unwrap_or(content.len());
    let body = content[body_start..body_end].trim();
    // Check if there's meaningful content (not just pure HTML comments)
    // A line is a "pure comment" only if it starts with <!-- and ends with -->
    // Lines with mixed content (e.g., "real text<!-- note -->") are kept
    let meaningful: Vec<&str> = body.lines()
        .filter(|l| {
            let t = l.trim();
            if t.is_empty() { return false; }
            // Pure comment line: starts with <!-- and ends with -->
            if t.starts_with("<!--") && t.ends_with("-->") { return false; }
            true
        })
        .collect();
    if meaningful.is_empty() {
        None
    } else {
        Some(body.to_string())
    }
}

/// Strip YAML frontmatter (--- delimited) from markdown content, returning just the body.
fn strip_frontmatter(content: &str) -> String {
    if content.starts_with("---") {
        if let Some(end) = content[3..].find("---") {
            return content[3 + end + 3..].trim().to_string();
        }
    }
    content.trim().to_string()
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
    let agent_type = fm.get("type").cloned().map(|t| {
        match t.as_str() {
            "pod-leader" | "coordinator" => "manager".to_string(),
            "pod-member" => "agent-template".to_string(),
            other => other.to_string(),
        }
    }).unwrap_or("agent-template".to_string());
    let is_custom = agent_type == "custom";

    let agent_body = strip_frontmatter(&agent_md);

    // Read shared project context (.k2so/PROJECT.md) — manager mode agents
    let is_manager_type = agent_type == "manager" || agent_type == "agent-template";
    let project_md_path = PathBuf::from(project_path).join(".k2so").join("PROJECT.md");
    let project_context = if is_manager_type && project_md_path.exists() {
        let raw = safe_read_to_string(&project_md_path).unwrap_or_default();
        let stripped = strip_frontmatter(&raw);
        // Only include if it has real content (not just comments/empty sections)
        let has_content = stripped.lines().any(|line| {
            let trimmed = line.trim();
            !trimmed.is_empty() && !trimmed.starts_with('#') && !trimmed.starts_with("<!--")
        });
        if has_content { Some(stripped) } else { None }
    } else {
        None
    };

    // Extract Standing Orders section from agent body (if user filled it in)
    let standing_orders = extract_section(&agent_body, "Standing Orders");

    let mut md = String::new();

    if is_custom {
        // ── Custom Agent: agent.md body + heartbeat control + tools ──
        md.push_str(&format!("# {}\n\n", agent_name));
        md.push_str(&format!("**Role:** {}\n\n", role));

        if !agent_body.is_empty() {
            md.push_str(&format!("{}\n\n", agent_body));
        }

        // Add heartbeat control docs if not already in agent body
        if !agent_body.contains("Heartbeat Control") {
            md.push_str(CUSTOM_AGENT_HEARTBEAT_DOCS);
        }

        return Ok(md);
    }

    // ── K2SO / Coordinator agents: full infrastructure CLAUDE.md ───────

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

    md.push_str(&format!("# K2SO Agent: {}\n\n", agent_name));
    md.push_str(&format!("## Identity\n**Role:** {}\n\n", role));
    // Reference the agent's full profile (absolute path so it resolves from worktrees)
    md.push_str(&format!(
        "**Full profile:** `{}`\n\n",
        agent_md_path.to_string_lossy()
    ));
    if !agent_body.is_empty() {
        md.push_str(&format!("{}\n\n", agent_body));
    }

    // Inject shared project context
    if let Some(ref ctx) = project_context {
        md.push_str("## Project Context (shared)\n\n");
        md.push_str(ctx);
        md.push_str("\n\n");
    }

    // Inject standing orders (persistent directives from agent.md)
    if let Some(ref orders) = standing_orders {
        md.push_str("## Standing Orders\n\n");
        md.push_str(orders);
        md.push_str("\n\n");
    }

    // Current task (if launching with specific work)
    if let Some(task) = current_task {
        // Use absolute path so it resolves from worktrees (where relative .k2so/ doesn't exist)
        let task_file_abs = agent_work_dir(project_path, agent_name, "active").join(&task.filename);
        md.push_str("## Current Task\n\n");
        md.push_str(&format!("**{}** (priority: {}, type: {})\n\n", task.title, task.priority, task.item_type));
        md.push_str(&format!("Task file: `{}`\n\n", task_file_abs.to_string_lossy()));
        md.push_str("Read the full task file for complete details, acceptance criteria, and context.\n\n");
    }

    // Work queue info (absolute paths for worktree compatibility)
    let work_dir_abs = PathBuf::from(project_path).join(".k2so").join("agents").join(agent_name).join("work");
    md.push_str("## Work Queue\n\n");
    md.push_str(&format!(
        "Your work items are at: `{}/`\n",
        work_dir_abs.to_string_lossy()
    ));
    md.push_str(&format!("- `{}/inbox/` — assigned to you, pick the highest priority\n", work_dir_abs.to_string_lossy()));
    md.push_str(&format!("- `{}/active/` — items you're currently working on\n", work_dir_abs.to_string_lossy()));
    md.push_str(&format!("- `{}/done/` — move items here when complete\n\n", work_dir_abs.to_string_lossy()));

    // Other agents — for managers, include profile paths so they can read agent.md files
    let is_manager_lead = agent_type == "manager" || agent_type == "k2so";
    if !other_agents.is_empty() {
        if is_manager_lead {
            md.push_str("## Your Team\n\n");
            md.push_str("These are your agent templates. Read their `agent.md` profiles to understand their strengths before delegating:\n\n");
            for (name, their_role) in &other_agents {
                md.push_str(&format!(
                    "- **{}** — {} (profile: `.k2so/agents/{}/agent.md`)\n",
                    name, their_role, name
                ));
            }
            md.push_str("\nYou can create new agents (`k2so agents create <name> --role \"...\"`) or update existing ones (`k2so agent update --name <name> --field role --value \"...\"`).\n\n");
        } else {
            md.push_str("## Other Agents\n");
            md.push_str("You can delegate work to these agents:\n\n");
            for (name, their_role) in &other_agents {
                md.push_str(&format!("- **{}** — {}\n", name, their_role));
            }
            md.push_str("\n");
        }
    }

    // Add workspace state constraints
    if let Some(ws_state) = get_workspace_state(project_path) {
        md.push_str("## Workspace State Constraints\n\n");
        md.push_str(&format!("This workspace operates under the **{}** state.\n\n", ws_state.name));
        if let Some(ref desc) = ws_state.description {
            md.push_str(&format!("{}\n\n", desc));
        }
        md.push_str("| Source Type | Permission |\n|---|---|\n");
        md.push_str(&format!("| Features | {} |\n", format_cap(&ws_state.cap_features)));
        md.push_str(&format!("| Issues | {} |\n", format_cap(&ws_state.cap_issues)));
        md.push_str(&format!("| Crashes | {} |\n", format_cap(&ws_state.cap_crashes)));
        md.push_str(&format!("| Security | {} |\n", format_cap(&ws_state.cap_security)));
        md.push_str(&format!("| Audits | {} |\n", format_cap(&ws_state.cap_audits)));
        md.push_str("\n**auto** = build and merge automatically. **gated** = build PR but wait for human approval. **off** = do not act.\n\n");
    }

    // Write the SKILL.md file alongside the CLAUDE.md.
    // SKILL.md is harness-agnostic — works with Claude Code, Pi, Aider, etc.
    // CLAUDE.md contains identity + task context only. SKILL.md has the CLI protocol.
    let project_name = std::path::Path::new(project_path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "workspace".to_string());

    let skill_content = if is_manager_lead {
        generate_manager_skill_content(project_path, &project_name)
    } else if agent_type == "custom" {
        generate_custom_agent_skill_content(&project_name, agent_name)
    } else {
        generate_template_skill_content(&project_name, agent_name)
    };

    // Write SKILL.md to agent directory
    let skill_path = agent_dir(project_path, agent_name).join("SKILL.md");
    let _ = fs::write(&skill_path, &skill_content);

    // Reference the SKILL.md from CLAUDE.md so Claude Code picks it up
    md.push_str("\n## Tools\n\nSee `SKILL.md` in this directory for your available CLI commands.\n");

    Ok(md)
}

/// Generate the universal skill protocol for the Workspace Manager.
/// Includes delegation, cross-workspace messaging, and full orchestration commands.
fn generate_manager_skill_content(project_path: &str, project_name: &str) -> String {
    format!(
r#"# K2SO Workspace Manager Skill

You are the Workspace Manager for {project_name}. Check in first, then triage work.

### Check In (do this first on every wake)

```
k2so checkin
```

This returns your current task, inbox messages, peer agent status, file reservations, and recent activity. Always start here.

### Decision Framework

When a task arrives in the workspace inbox:
1. **Simple tasks** (typo fixes, config changes, single-file edits): Work directly in the main branch. No delegation needed.
2. **Complex tasks** (multi-file features, refactors, new systems): Delegate to a specialist agent template using `k2so delegate <agent-name> <work-item-file>`.

### Report Status

```
k2so status "triaging inbox"
k2so status "working on config update"
```

### Complete Task

```
k2so done
k2so done --blocked "waiting for API spec from upstream"
```

### Send Message

Send work to another workspace's inbox (cross-workspace, requires a connection):
```
k2so msg <workspace-name>:inbox "description of the work needed"
```

### Claim Files (prevent conflicts with other agents)

```
k2so reserve src/auth/ src/middleware/jwt.ts
k2so release
```

"#,
        project_name = project_name,
    )
}

/// Generate the skill protocol for custom agents.
/// Has checkin, status, done, msg (to connected workspaces), reserve/release.
/// No delegation — custom agents send work to workspace inboxes.
fn generate_custom_agent_skill_content(project_name: &str, agent_name: &str) -> String {
    format!(
r#"# K2SO Agent Skill

You are {agent_name}, a custom agent for {project_name}.

## Check In (do this first on every wake)

```
k2so checkin
```

Returns your current task, inbox messages, peer status, file reservations, and recent activity.

## Report Status

```
k2so status "reviewing security audit"
```

## Complete Task

```
k2so done
k2so done --blocked "waiting for API access"
```

## Send Work to a Connected Workspace

```
k2so msg <workspace-name>:inbox "description of work needed"
```

Only works for workspaces connected via `k2so connections`.

## Claim Files

```
k2so reserve src/auth/ src/config.ts
k2so release
```
"#,
        agent_name = agent_name,
        project_name = project_name,
    )
}

/// Generate the universal skill protocol for agent templates (delegates).
/// Focused protocol — NO delegate, NO cross-workspace messaging.
fn generate_template_skill_content(project_name: &str, agent_name: &str) -> String {
    format!(
r#"# K2SO Agent Skill

You are {agent_name}, a specialist agent working in a dedicated worktree for {project_name}.

### Check In (do this first)

```
k2so checkin
```

This returns your assigned task and any file reservations from other active agents.

### Report Status

```
k2so status "implementing JWT validation"
```

### Complete Task

When you have finished your assigned work:
```
k2so done
```

If you are blocked and cannot proceed:
```
k2so done --blocked "need clarification on auth flow"
```

### Claim Files (coordinate with other active agents)

Before editing shared paths, check reservations and claim what you need:
```
k2so reserve src/auth/ src/middleware/jwt.ts
k2so release
```

"#,
        agent_name = agent_name,
        project_name = project_name,
    )
}

/// Generate the workspace-level skill for users working directly with an LLM.
/// Lightweight — just the commands a human user would need when working alongside K2SO agents.
fn generate_workspace_skill_content(project_name: &str) -> String {
    format!(
r#"# K2SO Skill

This workspace ({project_name}) is managed by K2SO. You can use these commands to interact with the agent system.

## Send Work to a Workspace

Send a task to a workspace's manager for triage and execution:
```
k2so msg <workspace-name>:inbox "description of work needed"
```

## View Activity Feed

See recent agent activity in this workspace:
```
k2so feed
```

## View Connections

See which workspaces are connected:
```
k2so connections list
```

## Create a Work Item

Add work to this workspace's inbox for the manager to triage:
```
k2so work create --title "Fix login bug" --body "Users can't log in after password reset" --source issue
```
"#,
        project_name = project_name,
    )
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

    // Auto-create manager agent if it doesn't exist
    // Check for old "pod-leader" and "coordinator" directory names as fallback
    let manager_dir = k2so_dir.join("agents").join("manager");
    let legacy_coordinator_dir = k2so_dir.join("agents").join("coordinator");
    let legacy_pod_leader_dir = k2so_dir.join("agents").join("pod-leader");
    if !manager_dir.exists() && !legacy_coordinator_dir.exists() && !legacy_pod_leader_dir.exists() {
        let _ = fs::create_dir_all(manager_dir.join("work").join("inbox"));
        let _ = fs::create_dir_all(manager_dir.join("work").join("active"));
        let _ = fs::create_dir_all(manager_dir.join("work").join("done"));
        let manager_role = "Workspace Manager — delegates work to agents, reviews completed branches, drives milestones";
        let manager_body = generate_default_agent_body("manager", "manager", &manager_role, &project_path);
        let manager_md = format!(
            "---\nname: manager\nrole: {}\ntype: manager\nmanager: true\n---\n\n{}\n",
            manager_role, manager_body
        );
        let _ = fs::write(manager_dir.join("agent.md"), &manager_md);
        write_agent_skill_file(&project_path, "manager", "manager");
    }

    // Auto-create K2SO agent if it doesn't exist (for agent mode)
    let k2so_agent_dir = k2so_dir.join("agents").join("k2so-agent");
    if !k2so_agent_dir.exists() {
        let _ = fs::create_dir_all(k2so_agent_dir.join("work").join("inbox"));
        let _ = fs::create_dir_all(k2so_agent_dir.join("work").join("active"));
        let _ = fs::create_dir_all(k2so_agent_dir.join("work").join("done"));
        let k2so_role = "K2SO planner — builds PRDs, milestones, and technical plans";
        let k2so_body = generate_default_agent_body("k2so", "k2so-agent", k2so_role, &project_path);
        let k2so_md = format!(
            "---\nname: k2so-agent\nrole: {}\ntype: k2so\n---\n\n{}\n",
            k2so_role, k2so_body
        );
        let _ = fs::write(k2so_agent_dir.join("agent.md"), &k2so_md);
        write_agent_skill_file(&project_path, "k2so-agent", "k2so");
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
    let is_manager_mode = {
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
            Some("manager") | Some("coordinator") | Some("pod") => true,
            Some("agent") => false,
            _ => {
                // Fallback: if agents dir has sub-agents, assume manager mode
                let agents_root = agents_dir(&project_path);
                agents_root.exists() && fs::read_dir(&agents_root)
                    .map(|e| e.flatten().any(|e| e.file_type().map_or(false, |ft| ft.is_dir())))
                    .unwrap_or(false)
            }
        }
    };

    // Scaffold PROJECT.md for manager mode — shared context across all agents
    if is_manager_mode {
        let project_md_path = k2so_dir.join("PROJECT.md");
        if !project_md_path.exists() {
            let project_md_content = format!(
r#"# {project_name}

<!-- This file is shared context injected into every agent's CLAUDE.md at launch. -->
<!-- Use it for project-wide knowledge that all agents need, regardless of their role. -->
<!-- Edit this file via Settings > Manage Project Context, or directly. -->

## About This Project

<!-- What does this codebase do? What problem does it solve? -->

## Tech Stack

<!-- Languages, frameworks, databases, infrastructure -->

## Key Directories

<!-- Important paths and what lives in them -->

## Conventions

<!-- Code style, commit message format, PR process, branch naming -->

## External Systems

<!-- Links to issue trackers, CI dashboards, staging environments, docs -->
"#,
                project_name = project_name,
            );
            let _ = atomic_write(&project_md_path, &project_md_content);
        }
    }

    let md = if is_manager_mode {
        // ── Workspace Manager CLAUDE.md ──────────────────────────────────────
        format!(
            r#"# K2SO Workspace Manager: {project_name}

You are the **workspace manager** for the {project_name} workspace, operating inside K2SO.

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

## Communicating with Running Agents

You can see and message any running agent session:
```bash
k2so agents running                            # List all active sessions with terminal IDs
k2so terminal read <terminal-id> --lines 30    # See what an agent is doing
k2so terminal write <terminal-id> "message"    # Send instructions to a running agent
```

**Auto-merge (Build state):** When all capabilities are "auto", tell the sub-agent to self-merge:
```bash
k2so terminal write <id> "Your work is approved. Run: k2so agent complete --agent <name> --file <filename>"
```

**Gated (Managed Service state):** The agent moves work to done and you review:
```bash
k2so reviews                                   # Check pending reviews
k2so review approve <agent> <branch>           # Merge after reviewing
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
                format!("{}\n\nRead each agent's profile at `.k2so/agents/<name>/agent.md` to understand their strengths before delegating. You can also update their profiles with `k2so agent update --name <name> --field role --value \"...\"`.", agent_list)
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
- **Set up workspaces** for each project — enable worktrees, manager mode, create agent teams
- **Coordinate across workspaces** — send work to different projects, check on progress
- **You do NOT write code** — you plan, then hand off execution to workspace managers and their agent teams

## Setting Up a Project Workspace

When the user has a project they want to build or maintain with agents:

```bash
# 1. Enable the workspace for autonomous work
k2so mode manager                    # Enable multi-agent orchestration
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
5. **Send tasks to the project workspace** for the workspace manager to execute:
   ```bash
   k2so work send --workspace /path/to/project \
     --title "Milestone 1: User Authentication" \
     --body "See PRD at .k2so/prds/auth.md. Tasks: ..."
   ```
   The workspace manager picks it up and delegates to its agents.

## Cross-Workspace Coordination

You can see and manage multiple workspaces:
```bash
# Send work to any workspace
k2so work send --workspace /path/to/frontend-app --title "..." --body "..."
k2so work send --workspace /path/to/api-server --title "..." --body "..."

# Set up a new workspace from scratch
K2SO_PROJECT_PATH="/path/to/new-project" k2so mode manager
K2SO_PROJECT_PATH="/path/to/new-project" k2so heartbeat on
K2SO_PROJECT_PATH="/path/to/new-project" k2so agents create backend-eng --role "..."

# Register a new workspace via CLI
k2so workspace create /path/to/new-project   # Create folder + register
k2so workspace open /path/to/existing        # Register existing folder
```

## Testing Workspace Manager Workflows

To wake the workspace manager and have it process inbox work:
```bash
# Add work to the workspace inbox
k2so work create --title "..." --body "..." --priority high --type task --source feature

# Wake the workspace manager (resumes previous session, sends triage message)
k2so heartbeat wake
```

The workspace manager will check inbox, delegate to agents, and track progress.

## Monitoring Running Agents

```bash
# See all active CLI LLM sessions across workspaces
k2so agents running

# Read what an agent is doing
k2so terminal read <terminal-id> --lines 30

# Send a message to a running agent
k2so terminal write <terminal-id> "message"

# Check agent work status
k2so agents list
k2so reviews                    # See pending reviews
```

## Workspace States

Workspaces operate under states that control agent autonomy:
- **Build** — agents auto-merge everything
- **Managed Service** — features are gated (need human approval), bugs/security auto-merge
- **Maintenance** — everything gated
- **Locked** — no agent activity

The workspace manager and sub-agents adapt their completion behavior based on the state.
Sub-agents use `k2so agent complete` which auto-merges or submits for review accordingly.

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
        existing.starts_with("# K2SO ") // Our headers: "# K2SO Coordinator:" or "# K2SO AI Planner:" or "# K2SO Lead Agent:"
    } else {
        false
    };

    if disabled_path.exists() {
        // Was disabled — check if the disabled content matches our pattern
        let disabled_content = fs::read_to_string(&disabled_path).unwrap_or_default();
        if disabled_content.starts_with("# K2SO ") {
            // It was our generated content — write fresh for the new mode
            let _ = fs::remove_file(&disabled_path);
            atomic_write(&claude_md_path, &md)?;
        } else {
            // It was user-written — restore it, save ours for reference
            fs::rename(&disabled_path, &claude_md_path)
                .map_err(|e| format!("Failed to restore CLAUDE.md: {}", e))?;
            let _ = atomic_write(&generated_path, &md);
            return fs::read_to_string(&claude_md_path).map_err(|e| e.to_string());
        }
    } else if is_k2so_generated {
        // Existing CLAUDE.md was generated by K2SO — regenerate for current mode
        atomic_write(&claude_md_path, &md)?;
    } else if claude_md_path.exists() {
        // Existing user-written CLAUDE.md — don't overwrite, save ours for reference
        let _ = atomic_write(&generated_path, &md);
        return fs::read_to_string(&claude_md_path).map_err(|e| e.to_string());
    } else {
        // No CLAUDE.md exists — write fresh
        atomic_write(&claude_md_path, &md)?;
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

### Waking the Workspace Manager (USE THIS — not `k2so heartbeat`)
```
k2so heartbeat wake                     # THE RIGHT WAY: resumes manager session, sends triage message
```
**IMPORTANT:** Always use `k2so heartbeat wake` to wake the workspace manager, NOT `k2so heartbeat`.
- `heartbeat wake` → resumes the manager's previous session, detects inbox work, sends delegation instructions
- `heartbeat` (without "wake") → raw triage that launches `__lead__`, does NOT resume sessions or send messages

### Workspace Setup
```
k2so mode                               # Show current settings
k2so mode <off|agent|manager>            # Set workspace agent mode
k2so heartbeat <on|off>                 # Enable/disable automatic heartbeat
k2so settings                           # Show all workspace settings
```

### Agent Management
```
k2so agent create <name> --role "..."   # Create a new agent
k2so agent update --name <n> --field <f> --value "..."  # Update agent profile
k2so agent list                         # List all agents with work counts
k2so agent profile <name>              # Read agent's identity (agent.md)
k2so agents work <name>                 # Show agent's work items
k2so agents launch <name>              # Launch agent's Claude session
```

### Cross-Workspace (use K2SO_PROJECT_PATH, not cd)
```
K2SO_PROJECT_PATH=/path/to/workspace k2so work send --title "..." --body "..."
K2SO_PROJECT_PATH=/path/to/workspace k2so heartbeat wake
k2so work move --agent <name> --file <f> --from inbox --to active
```
**IMPORTANT:** When targeting a different workspace, use `K2SO_PROJECT_PATH=/path k2so ...`
Do NOT use `cd /path && k2so ...` — the cd resets your shell and may cause path resolution issues.

### Running Agents & Terminal I/O
```
k2so agents running                 # List all active CLI LLM sessions
k2so terminal write <id> "message"  # Send text to a running terminal
k2so terminal read <id> --lines 50  # Read last N lines from terminal buffer
```

### Completion
```
k2so agent complete --agent <n> --file <f>  # Complete work (auto-merge or submit for review)
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

const CUSTOM_AGENT_HEARTBEAT_DOCS: &str = r#"## Heartbeat Control

You run on an adaptive heartbeat. Adjust your check-in frequency based on your current work phase:

```
k2so heartbeat set --agent <your-name> --interval 60 --phase "active"       # Every minute — actively building
k2so heartbeat set --agent <your-name> --interval 300 --phase "monitoring"   # Every 5 min — watching
k2so heartbeat set --agent <your-name> --interval 3600 --phase "idle"        # Every hour — dormant
```

**Important — report your status after each wake:**
- If you checked your inbox and had nothing to do: `k2so heartbeat noop --agent <your-name>`
  (This triggers auto-backoff and saves money by not waking you unnecessarily)
- If you took action (delegated, built, reviewed): `k2so heartbeat action --agent <your-name>`
  (This resets the backoff counter so you stay responsive)

The system auto-backs off after 3 consecutive no-ops, increasing your interval by 1.5x each time.

## Available Tools

Standard CLI tools are available in your terminal (`gh`, `git`, `curl`, etc.).
K2SO tools:
```
k2so terminal spawn --title "..." --command "..."   # Run parallel tasks
k2so heartbeat set --agent <name> --interval N      # Adjust check-in frequency
k2so heartbeat noop --agent <name>                  # Report no work found (saves cost)
k2so heartbeat action --agent <name>                # Report action taken (stay responsive)
```
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
pub async fn k2so_agents_review_queue(project_path: String) -> Result<Vec<ReviewItem>, String> {
    tokio::task::spawn_blocking(move || k2so_agents_review_queue_inner(&project_path))
        .await
        .map_err(|e| format!("review_queue task failed: {}", e))?
}

pub fn k2so_agents_review_queue_inner(project_path: &str) -> Result<Vec<ReviewItem>, String> {
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
/// Sub-agent completion handler. Reads workspace state capability for the work
/// item's source type, then either auto-merges (auto mode) or moves to done (gated mode).
/// Returns JSON describing what was done.
pub fn k2so_agent_complete(
    project_path: String,
    agent_name: String,
    filename: String,
) -> Result<String, String> {
    // Read the work item to get its source type
    let active_dir = agent_work_dir(&project_path, &agent_name, "active");
    let item_path = active_dir.join(&filename);
    if !item_path.exists() {
        return Err(format!("Work item not found: {}", filename));
    }
    let content = fs::read_to_string(&item_path).unwrap_or_default();
    let fm = parse_frontmatter(&content);
    let source = fm.get("source").cloned().unwrap_or_else(|| "manual".to_string());

    // Get workspace state and determine capability for this source
    let capability = if let Some(ws_state) = get_workspace_state(&project_path) {
        ws_state.capability_for_source(&source).to_string()
    } else {
        "gated".to_string()
    };

    // Get branch from work item frontmatter
    let branch = fm.get("branch").cloned().unwrap_or_default();

    if capability == "auto" && !branch.is_empty() {
        // AUTO MODE: merge branch, clean up worktree, archive done items, unlock
        match k2so_agents_review_approve(project_path.clone(), branch.clone(), agent_name.clone()) {
            Ok(_) => Ok(serde_json::json!({
                "mode": "auto",
                "action": "merged",
                "branch": branch,
                "agent": agent_name,
            }).to_string()),
            Err(e) => Err(format!("Auto-merge failed: {}", e)),
        }
    } else {
        // GATED MODE: move work to done, let human review
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
        }).to_string())
    }
}

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

    // 2. Remove the worktree (find it by branch name) + cleanup DB workspace record
    let worktrees = crate::git::list_worktrees(&project_path);
    if let Some(wt) = worktrees.iter().find(|wt| wt.branch == branch) {
        let wt_path = wt.path.clone();
        let _ = crate::git::remove_worktree(&project_path, &wt_path, true);

        // Remove the workspace DB record so it disappears from the UI
        if let Some(home) = dirs::home_dir() {
            let db_path = home.join(".k2so").join("k2so.db");
            if let Ok(conn) = rusqlite::Connection::open(&db_path) {
                let _ = conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;");
                let _ = conn.execute(
                    "DELETE FROM workspaces WHERE worktree_path = ?1",
                    rusqlite::params![wt_path],
                );
            }
        }
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

    // 1. Find and remove the worktree + branch + DB record for this agent
    let worktrees = crate::git::list_worktrees(&project_path);
    for wt in worktrees.iter().filter(|wt| wt.branch.starts_with(&format!("agent/{}/", agent_name))) {
        let wt_path = wt.path.clone();
        if let Err(e) = crate::git::remove_worktree(&project_path, &wt_path, true) {
            log_debug!("[review-reject] Failed to remove worktree {}: {}", wt_path, e);
        }
        if let Err(e) = crate::git::delete_branch(&project_path, &wt.branch) {
            log_debug!("[review-reject] Failed to delete branch {}: {}", wt.branch, e);
        }
        // Remove workspace DB record
        if let Some(home) = dirs::home_dir() {
            let db_path = home.join(".k2so").join("k2so.db");
            if let Ok(conn) = rusqlite::Connection::open(&db_path) {
                let _ = conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;");
                let _ = conn.execute(
                    "DELETE FROM workspaces WHERE worktree_path = ?1",
                    rusqlite::params![wt_path],
                );
            }
        }
    }

    // 2. Move all done items back to inbox (strip worktree info from frontmatter)
    fs::create_dir_all(&inbox_dir).map_err(|e| format!("Failed to create inbox dir: {}", e))?;
    if let Ok(entries) = fs::read_dir(&done_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map_or(false, |ext| ext == "md") {
                let filename = path.file_name().unwrap();
                let target = inbox_dir.join(filename);
                // Strip old worktree info so a fresh worktree gets created on retry
                if let Ok(content) = fs::read_to_string(&path) {
                    let cleaned = strip_worktree_from_frontmatter(&content);
                    if let Err(e) = atomic_write(&target, &cleaned) {
                        log_debug!("[review-reject] Failed to write cleaned work item: {}", e);
                    }
                } else {
                    if let Err(e) = fs::rename(&path, &target) {
                        log_debug!("[review-reject] Failed to move work item: {}", e);
                    }
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
        atomic_write(&path, &content)?;
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
    atomic_write(&path, &content)?;

    Ok(())
}

// ── Heartbeat Triage (Workspace State) ──────────────────────────────────

/// Read the workspace state for a project, returning the state or None if unset.
fn get_workspace_state(project_path: &str) -> Option<crate::db::schema::WorkspaceState> {
    let db_path = dirs::home_dir()?.join(".k2so").join("k2so.db");
    let conn = rusqlite::Connection::open(&db_path).ok()?;
    // Safety: read-only access from background threads — set WAL mode and busy timeout
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000; PRAGMA query_only=ON;").ok()?;
    let state_id: Option<String> = conn.query_row(
        "SELECT tier_id FROM projects WHERE path = ?1",
        rusqlite::params![project_path],
        |row| row.get(0),
    ).ok()?;
    let sid = state_id?;
    crate::db::schema::WorkspaceState::get(&conn, &sid).ok()
}

/// Build a triage summary for the local LLM to evaluate.
/// Returns a plain-text summary of all agents with pending work in a project.
/// The local LLM reads this and decides which agents (if any) should be launched.
/// Respects workspace state capabilities — items with "off" capability are excluded.
#[tauri::command]
pub fn k2so_agents_triage_summary(project_path: String) -> Result<String, String> {
    let dir = agents_dir(&project_path);
    if !dir.exists() {
        return Ok("No agents configured.".to_string());
    }

    // Load workspace state for capability gating
    let ws_state = get_workspace_state(&project_path);
    let state_name = ws_state.as_ref().map(|t| t.name.as_str()).unwrap_or("(no state set)");

    let mut summary = String::new();
    summary.push_str(&format!("Workspace state: {}\n\n", state_name));
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

        let is_locked = is_agent_locked(&project_path, &name);

        if inbox_items.is_empty() && active_count == 0 {
            continue;
        }

        // Read agent type and role for LLM context
        let agent_md_path = entry.path().join("agent.md");
        let (agent_type, agent_role) = if agent_md_path.exists() {
            let content = fs::read_to_string(&agent_md_path).unwrap_or_default();
            let fm = parse_frontmatter(&content);
            (
                fm.get("type").cloned().unwrap_or("agent-template".to_string()),
                fm.get("role").cloned().unwrap_or_default(),
            )
        } else {
            ("agent-template".to_string(), String::new())
        };

        summary.push_str(&format!("Agent: {} (type: {}, role: {})\n", name, agent_type, agent_role));
        if is_locked {
            summary.push_str("  Status: LOCKED (active session running)\n");
        }
        if active_count > 0 {
            summary.push_str(&format!("  Active: {} items in progress\n", active_count));
        }
        for item in &inbox_items {
            let cap_status = ws_state.as_ref()
                .map(|t| t.capability_for_source(&item.source).to_string())
                .unwrap_or_else(|| "auto".to_string()); // No state = allow all
            if cap_status == "off" {
                continue; // State disables this source type — skip entirely
            }
            let gate_label = if cap_status == "gated" { " [NEEDS APPROVAL]" } else { "" };
            summary.push_str(&format!(
                "  Inbox: \"{}\" (priority: {}, type: {}, source: {}{})\n",
                item.title, item.priority, item.item_type, item.source, gate_label
            ));
        }
        summary.push('\n');
    }

    // Add workspace inbox items
    let ws_inbox = workspace_inbox_dir(&project_path);
    if ws_inbox.exists() {
        let ws_items: Vec<WorkItem> = fs::read_dir(&ws_inbox)
            .ok()
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|e| e.path().extension().map_or(false, |ext| ext == "md"))
                    .filter_map(|e| read_work_item(&e.path(), "inbox"))
                    .collect()
            })
            .unwrap_or_default();

        if !ws_items.is_empty() {
            let lead_locked = is_agent_locked(&project_path, "__lead__");
            summary.push_str("Workspace Inbox (unassigned — needs Coordinator):\n");
            if lead_locked {
                summary.push_str("  Coordinator: LOCKED (active session running)\n");
            }
            for item in &ws_items {
                let cap_status = ws_state.as_ref()
                    .map(|t| t.capability_for_source(&item.source).to_string())
                    .unwrap_or_else(|| "auto".to_string());
                if cap_status == "off" { continue; }
                let gate_label = if cap_status == "gated" { " [NEEDS APPROVAL]" } else { "" };
                summary.push_str(&format!(
                    "  \"{}\" (priority: {}, type: {}, source: {}{})\n",
                    item.title, item.priority, item.item_type, item.source, gate_label
                ));
            }
            summary.push('\n');
        }
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

// ── LLM-Powered Triage ──────────────────────────────────────────────────

const TRIAGE_SYSTEM_PROMPT: &str = r#"You are K2SO's agent dispatcher. Your job is to decide which AI agents should be woken up based on their pending work.

Rules:
1. SKIP agents marked as LOCKED (they already have an active session)
2. Wake agents with "critical" or "high" priority inbox items first
3. If an agent already has active items in progress, only wake it for critical new inbox items
4. Consider dependencies: if one agent's work likely depends on another's output, wake the dependency first
5. If the workspace inbox has unassigned items and the Coordinator is not locked, wake __lead__
6. For "low" priority items, only wake if the agent has no other work in progress
7. Items marked [NEEDS APPROVAL] can be worked on but must be built as PRs (not auto-merged)
8. Items with source type "off" in the workspace state are already filtered out — you won't see them
9. The workspace state controls what kinds of work are allowed. Respect it.

Respond with ONLY a JSON object, no other text:
{"wake":["agent-name-1","agent-name-2"],"reasoning":"brief explanation"}

If no agents should be woken, respond:
{"wake":[],"reasoning":"brief explanation"}"#;

/// Run LLM-based triage: feed the triage summary to the local LLM and parse its decision.
/// Returns the list of agent names to launch.
/// Falls back to filesystem-based triage if LLM is unavailable or fails.
pub fn llm_triage_decide(
    project_path: &str,
    llm_manager: &crate::llm::LlmManager,
) -> Result<Vec<String>, String> {
    // Build the triage summary
    let summary = k2so_agents_triage_summary(project_path.to_string())?;

    // Quick exit: nothing to triage
    if summary == "No agents have pending work." {
        return Ok(vec![]);
    }

    // Check if LLM is loaded
    if !llm_manager.is_loaded() {
        log_agent_warning(project_path, "__lead__", "LLM not loaded — falling back to filesystem triage");
        // Direct fallback — no recursion, calls scheduler_tick which is non-recursive
        return k2so_agents_scheduler_tick(project_path.to_string());
    }

    // Call the local LLM via safe subprocess
    let result = crate::commands::assistant::safe_generate_for_triage(
        llm_manager,
        TRIAGE_SYSTEM_PROMPT,
        &summary,
    );

    match result {
        Ok(response) => {
            // Parse the JSON response
            match parse_triage_response(&response) {
                Some(agents) => {
                    // Validate agent names — filter out hallucinated names
                    let agents_root = agents_dir(project_path);
                    let valid_agents: Vec<String> = agents.into_iter().filter(|name| {
                        name == "__lead__" || agents_root.join(name).exists()
                    }).collect();

                    // Log the decision
                    let k2so_dir = PathBuf::from(project_path).join(".k2so");
                    let log_path = k2so_dir.join("triage.log");
                    let entry = format!(
                        "[{}] LLM triage: wake={:?} | summary_len={}\n",
                        simple_date(),
                        valid_agents,
                        summary.len()
                    );
                    if let Ok(mut file) = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&log_path)
                    {
                        use std::io::Write;
                        let _ = file.write_all(entry.as_bytes());
                    }
                    Ok(valid_agents)
                }
                None => {
                    log_agent_warning(project_path, "__lead__",
                        &format!("LLM triage returned unparseable response: {}",
                            &response[..response.len().min(200)]));
                    // Fallback to filesystem triage (non-recursive — calls scheduler_tick directly)
                    k2so_agents_scheduler_tick(project_path.to_string())
                }
            }
        }
        Err(e) => {
            log_agent_warning(project_path, "__lead__",
                &format!("LLM triage failed: {} — falling back to filesystem", e));
            // Fallback to filesystem triage (non-recursive — calls scheduler_tick directly)
            k2so_agents_scheduler_tick(project_path.to_string())
        }
    }
}

/// Parse the LLM's triage JSON response into a list of agent names.
fn parse_triage_response(response: &str) -> Option<Vec<String>> {
    // Try to find JSON in the response (LLM might add extra text/preamble)
    let json_start = response.find('{')?;
    let json_end = response.rfind('}').map(|i| i + 1)?;
    // Bounds check: end must be after start
    if json_end <= json_start {
        return None;
    }
    let json_str = response.get(json_start..json_end)?;

    let parsed: serde_json::Value = serde_json::from_str(json_str).ok()?;
    let wake = parsed.get("wake")?.as_array()?;
    Some(
        wake.iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect()
    )
}

// ── Adaptive Heartbeat Commands ──────────────────────────────────────────

/// Get an agent's heartbeat configuration.
#[tauri::command]
pub fn k2so_agents_get_heartbeat(
    project_path: String,
    agent_name: String,
) -> Result<AgentHeartbeatConfig, String> {
    let dir = agent_dir(&project_path, &agent_name);
    if !dir.exists() {
        return Err(format!("Agent '{}' does not exist", agent_name));
    }
    Ok(read_heartbeat_config(&project_path, &agent_name))
}

/// Update an agent's heartbeat configuration (partial update).
/// Used by both the CLI (`k2so heartbeat set`) and the frontend settings UI.
#[tauri::command]
pub fn k2so_agents_set_heartbeat(
    project_path: String,
    agent_name: String,
    interval: Option<u64>,
    phase: Option<String>,
    mode: Option<String>,
    cost_budget: Option<String>,
    force_wake: Option<bool>,
) -> Result<AgentHeartbeatConfig, String> {
    let dir = agent_dir(&project_path, &agent_name);
    if !dir.exists() {
        return Err(format!("Agent '{}' does not exist", agent_name));
    }

    let mut config = read_heartbeat_config(&project_path, &agent_name);

    if let Some(interval) = interval {
        // Clamp to min/max
        config.interval_seconds = interval
            .max(config.min_interval_seconds)
            .min(config.max_interval_seconds);
    }
    if let Some(phase) = phase {
        config.phase = phase;
    }
    if let Some(mode) = mode {
        config.mode = mode;
    }
    if let Some(budget) = cost_budget {
        config.cost_budget = budget;
    }
    config.updated_by = "agent".to_string();

    // Recalculate next wake (or set to now if force_wake)
    let now = chrono::Utc::now();
    if force_wake.unwrap_or(false) {
        config.next_wake = Some(now.to_rfc3339()); // Wake immediately on next tick
        config.updated_by = "user".to_string();
    } else {
        config.next_wake = Some((now + chrono::Duration::seconds(config.interval_seconds as i64)).to_rfc3339());
    }

    write_heartbeat_config(&project_path, &agent_name, &config)?;
    Ok(config)
}

/// Scheduler tick: check all agents in a project and return those ready to wake.
/// Called by the heartbeat script (via /cli/scheduler-tick).
/// Differentiates between manager agents (inbox-based) and custom agents (timing-based).
#[tauri::command]
pub fn k2so_agents_scheduler_tick(project_path: String) -> Result<Vec<String>, String> {
    // Check workspace state — if heartbeat is disabled (Locked state), skip entirely
    if let Some(ws_state) = get_workspace_state(&project_path) {
        if ws_state.heartbeat == 0 {
            return Ok(vec![]); // Locked state — no agent activity
        }
    }

    // Project-level schedule gate: check if the schedule says "fire now"
    // Look up project from DB by path
    let db_path = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".k2so/k2so.db");
    let mut resolved_project_id: Option<String> = None;
    if let Ok(conn) = rusqlite::Connection::open(&db_path) {
        let project_row: Option<(String, String, Option<String>, Option<String>)> = conn.query_row(
            "SELECT id, heartbeat_mode, heartbeat_schedule, heartbeat_last_fire FROM projects WHERE path = ?1",
            rusqlite::params![project_path],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        ).ok();

        if let Some((project_id, mode, schedule, last_fire)) = project_row {
            resolved_project_id = Some(project_id.clone());
            if mode == "off" {
                return Ok(vec![]);
            }
            if mode == "scheduled" || mode == "hourly" {
                if !should_project_fire(&mode, schedule.as_deref(), last_fire.as_deref()) {
                    return Ok(vec![]); // Schedule says "not now"
                }
                // Update last_fire timestamp
                let _ = conn.execute(
                    "UPDATE projects SET heartbeat_last_fire = ?1 WHERE id = ?2",
                    rusqlite::params![chrono::Local::now().to_rfc3339(), project_id],
                );
            }
        }
    }

    let mut launchable = Vec::new();
    let now = chrono::Utc::now();

    // Step 1: Check workspace inbox (always — same as before)
    let ws_inbox = workspace_inbox_dir(&project_path);
    let has_workspace_inbox = ws_inbox.exists() && fs::read_dir(&ws_inbox)
        .map(|e| e.flatten().any(|e| e.path().extension().map_or(false, |ext| ext == "md")))
        .unwrap_or(false);

    if has_workspace_inbox && !is_agent_locked(&project_path, "__lead__") {
        launchable.push("__lead__".to_string());
    }

    // Step 2: Check each agent
    let dir = agents_dir(&project_path);
    if !dir.exists() {
        return Ok(launchable);
    }

    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            if !entry.file_type().map_or(false, |ft| ft.is_dir()) {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();

            // Skip agents that already have an active session (lock file exists)
            if is_agent_locked(&project_path, &name) {
                continue;
            }

            // Safety: skip agents whose terminal is being used interactively by the user
            if let Some(ref pid) = resolved_project_id {
                if let Ok(conn) = rusqlite::Connection::open(&db_path) {
                    if let Ok(Some(session)) = AgentSession::get_by_agent(&conn, pid, &name) {
                        if session.owner == "user" && session.status == "running" {
                            continue;
                        }
                    }
                }
            }

            // Read agent type
            let agent_md = entry.path().join("agent.md");
            let agent_type = if agent_md.exists() {
                let content = fs::read_to_string(&agent_md).unwrap_or_default();
                let fm = parse_frontmatter(&content);
                fm.get("type").cloned().unwrap_or("agent-template".to_string())
            } else {
                "agent-template".to_string()
            };

            if agent_type == "custom" {
                // Custom agents: check heartbeat timing
                let config = read_heartbeat_config(&project_path, &name);

                // Skip persistent mode agents (they're always running)
                if config.mode == "persistent" {
                    continue;
                }

                // Check active hours
                if let Some(ref hours) = config.active_hours {
                    if !is_within_active_hours(hours, &now) {
                        continue;
                    }
                }

                // Check if it's time to wake
                let should_wake = match &config.next_wake {
                    Some(next) => {
                        chrono::DateTime::parse_from_rfc3339(next)
                            .map(|t| now >= t)
                            .unwrap_or(true) // If parse fails, wake anyway
                    }
                    None => true, // No next_wake set — wake now
                };

                if should_wake {
                    // Update last_wake and schedule next
                    let mut updated = config.clone();
                    updated.last_wake = Some(now.to_rfc3339());
                    updated.next_wake = Some(
                        (now + chrono::Duration::seconds(updated.interval_seconds as i64)).to_rfc3339()
                    );
                    let _ = write_heartbeat_config(&project_path, &name, &updated);
                    launchable.push(name);
                }
            } else {
                // Manager agents (manager, coordinator, agent-template, k2so): inbox-based triage
                let inbox = agent_work_dir(&project_path, &name, "inbox");
                let has_inbox = inbox.exists() && fs::read_dir(&inbox)
                    .map(|e| e.flatten().any(|e| e.path().extension().map_or(false, |ext| ext == "md")))
                    .unwrap_or(false);

                if has_inbox {
                    // Quality gate: skip agents with only low-priority inbox items
                    // that already have active work in progress
                    let active_count = count_md_files(&agent_work_dir(&project_path, &name, "active"));
                    let highest_prio = get_highest_inbox_priority(&project_path, &name);
                    if active_count > 0 && highest_prio > priority_rank("high") {
                        // Agent has active work and inbox is only normal/low — defer to next cycle
                        continue;
                    }
                    launchable.push(name);
                }
            }
        }
    }

    // Sort by highest-priority inbox item (critical > high > normal > low)
    // The __lead__ agent always goes first if present
    launchable.sort_by(|a, b| {
        if a == "__lead__" { return std::cmp::Ordering::Less; }
        if b == "__lead__" { return std::cmp::Ordering::Greater; }
        let prio_a = get_highest_inbox_priority(&project_path, a);
        let prio_b = get_highest_inbox_priority(&project_path, b);
        prio_a.cmp(&prio_b) // Lower rank = higher priority
    });

    Ok(launchable)
}

/// Get the highest priority rank of inbox items for an agent (0=critical, 3=low).
fn get_highest_inbox_priority(project_path: &str, agent_name: &str) -> u8 {
    let inbox = agent_work_dir(project_path, agent_name, "inbox");
    if !inbox.exists() { return 3; }
    fs::read_dir(&inbox)
        .ok()
        .map(|entries| {
            entries.flatten()
                .filter(|e| e.path().extension().map_or(false, |ext| ext == "md"))
                .filter_map(|e| {
                    let content = fs::read_to_string(e.path()).ok()?;
                    let fm = parse_frontmatter(&content);
                    Some(priority_rank(fm.get("priority").map(|s| s.as_str()).unwrap_or("normal")))
                })
                .min()
                .unwrap_or(3)
        })
        .unwrap_or(3)
}

/// Save the last Claude session ID for an agent (enables --resume on next launch).
/// Also updates the DB session row.
#[tauri::command]
pub fn k2so_agents_save_session_id(
    project_path: String,
    agent_name: String,
    session_id: String,
) -> Result<(), String> {
    let dir = agent_dir(&project_path, &agent_name);
    if !dir.exists() {
        return Err(format!("Agent '{}' does not exist", agent_name));
    }

    // DB tracking (best-effort)
    if let Ok(conn) = rusqlite::Connection::open(k2so_db_path()) {
        if let Some(project_id) = resolve_project_id(&conn, &project_path) {
            let _ = AgentSession::update_session_id(&conn, &project_id, &agent_name, &session_id);
        }
    }

    // Legacy .last_session file
    let session_file = dir.join(".last_session");
    fs::write(&session_file, &session_id)
        .map_err(|e| format!("Failed to save session ID: {}", e))
}

/// Clear the saved session ID for an agent (called on no-op or when session should be fresh).
/// Also clears the session_id in the DB.
#[tauri::command]
pub fn k2so_agents_clear_session_id(
    project_path: String,
    agent_name: String,
) -> Result<(), String> {
    // DB tracking (best-effort)
    if let Ok(conn) = rusqlite::Connection::open(k2so_db_path()) {
        if let Some(project_id) = resolve_project_id(&conn, &project_path) {
            let _ = AgentSession::clear_session_id(&conn, &project_id, &agent_name);
        }
    }

    // Legacy .last_session file
    let session_file = agent_dir(&project_path, &agent_name).join(".last_session");
    if session_file.exists() {
        let _ = fs::remove_file(&session_file);
    }
    Ok(())
}

/// Record a no-op (agent woke up but had nothing to do) and apply auto-backoff.
#[tauri::command]
pub fn k2so_agents_heartbeat_noop(
    project_path: String,
    agent_name: String,
) -> Result<AgentHeartbeatConfig, String> {
    let mut config = read_heartbeat_config(&project_path, &agent_name);
    config.consecutive_no_ops += 1;

    // Auto-backoff: after 3 consecutive no-ops, increase interval by 1.5x
    // Uses integer arithmetic (3/2) to avoid floating-point precision drift on repeated backoffs.
    // Clamps to both min and max interval bounds.
    if config.auto_backoff && config.consecutive_no_ops >= 3 {
        let new_interval = config.interval_seconds.saturating_mul(3) / 2; // 1.5x without floats
        config.interval_seconds = new_interval
            .max(config.min_interval_seconds)
            .min(config.max_interval_seconds);
        log_agent_warning(
            &project_path,
            &agent_name,
            &format!(
                "Auto-backoff: {} consecutive no-ops, interval now {}s",
                config.consecutive_no_ops, config.interval_seconds
            ),
        );
    }

    // Prune wasteful session: clear the saved session ID so next launch is fresh
    // (no point resuming a session that was just "I have nothing to do")
    let session_file = agent_dir(&project_path, &agent_name).join(".last_session");
    if session_file.exists() {
        let _ = fs::remove_file(&session_file);
    }

    write_heartbeat_config(&project_path, &agent_name, &config)?;
    Ok(config)
}

/// Record that an agent took action — reset consecutive_no_ops counter.
#[tauri::command]
pub fn k2so_agents_heartbeat_action(
    project_path: String,
    agent_name: String,
) -> Result<AgentHeartbeatConfig, String> {
    let mut config = read_heartbeat_config(&project_path, &agent_name);
    config.consecutive_no_ops = 0;
    write_heartbeat_config(&project_path, &agent_name, &config)?;
    Ok(config)
}

/// Check if the current time is within the active hours window.
/// NOTE: The `timezone` field is accepted but currently compared against local system time.
/// Full timezone support (chrono-tz) is planned for a future release.
fn is_within_active_hours(hours: &ActiveHours, _now: &chrono::DateTime<chrono::Utc>) -> bool {
    let parse_hhmm = |s: &str| -> Option<(u32, u32)> {
        let parts: Vec<&str> = s.split(':').collect();
        if parts.len() == 2 {
            let h: u32 = parts[0].parse().ok()?;
            let m: u32 = parts[1].parse().ok()?;
            // Validate ranges: hours 0-23, minutes 0-59
            if h > 23 || m > 59 {
                return None;
            }
            Some((h, m))
        } else {
            None
        }
    };

    let (start_h, start_m) = match parse_hhmm(&hours.start) {
        Some(v) => v,
        None => {
            log_debug!("[heartbeat] Invalid active_hours start format: '{}' — allowing wake", hours.start);
            return true; // Invalid format — don't block (permissive)
        }
    };
    let (end_h, end_m) = match parse_hhmm(&hours.end) {
        Some(v) => v,
        None => {
            log_debug!("[heartbeat] Invalid active_hours end format: '{}' — allowing wake", hours.end);
            return true;
        }
    };

    // Use local system time (best approximation without chrono-tz)
    let local_now = chrono::Local::now();
    let hour = local_now.format("%H").to_string().parse::<u32>().unwrap_or(0);
    let minute = local_now.format("%M").to_string().parse::<u32>().unwrap_or(0);
    let now_mins = hour * 60 + minute;
    let start_mins = start_h * 60 + start_m;
    let end_mins = end_h * 60 + end_m;

    if start_mins <= end_mins {
        now_mins >= start_mins && now_mins < end_mins
    } else {
        // Wraps midnight
        now_mins >= start_mins || now_mins < end_mins
    }
}

// ── Project-Level Schedule Evaluation ─────────────────────────────────────

/// Evaluate whether a project's schedule says "fire now".
/// Returns true if the project should trigger agent evaluation this tick.
fn should_project_fire(mode: &str, schedule_json: Option<&str>, last_fire: Option<&str>) -> bool {
    let local_now = chrono::Local::now();

    let parse_hhmm_mins = |s: &str| -> Option<u32> {
        let parts: Vec<&str> = s.split(':').collect();
        if parts.len() == 2 {
            let h: u32 = parts[0].parse().ok()?;
            let m: u32 = parts[1].parse().ok()?;
            Some(h * 60 + m)
        } else {
            None
        }
    };

    let last_fire_time = last_fire.and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok());

    match mode {
        "hourly" => {
            // Parse: {"start":"09:00","end":"17:00","every_seconds":1800}
            let json_str = match schedule_json {
                Some(s) => s,
                None => return false,
            };
            let v: serde_json::Value = match serde_json::from_str(json_str) {
                Ok(v) => v,
                Err(_) => return false,
            };

            let start = v.get("start").and_then(|s| s.as_str()).unwrap_or("00:00");
            let end = v.get("end").and_then(|s| s.as_str()).unwrap_or("23:59");
            let every_secs = v.get("every_seconds").and_then(|s| s.as_u64()).unwrap_or(300);

            // Check time window
            let now_mins = local_now.format("%H").to_string().parse::<u32>().unwrap_or(0) * 60
                + local_now.format("%M").to_string().parse::<u32>().unwrap_or(0);
            let start_mins = parse_hhmm_mins(start).unwrap_or(0);
            let end_mins = parse_hhmm_mins(end).unwrap_or(1439);

            let in_window = if start_mins <= end_mins {
                now_mins >= start_mins && now_mins < end_mins
            } else {
                now_mins >= start_mins || now_mins < end_mins
            };
            if !in_window { return false; }

            // Check elapsed time since last fire
            match last_fire_time {
                Some(lf) => {
                    let elapsed = (local_now.timestamp() - lf.timestamp()) as u64;
                    elapsed >= every_secs
                }
                None => true, // Never fired — fire now
            }
        }
        "scheduled" => {
            // Parse structured schedule JSON
            let json_str = match schedule_json {
                Some(s) => s,
                None => return false,
            };
            let v: serde_json::Value = match serde_json::from_str(json_str) {
                Ok(v) => v,
                Err(_) => return false,
            };

            let frequency = v.get("frequency").and_then(|s| s.as_str()).unwrap_or("daily");
            let time_str = v.get("time").and_then(|s| s.as_str()).unwrap_or("09:00");
            let schedule_mins = parse_hhmm_mins(time_str).unwrap_or(540); // default 9:00
            let now_mins = local_now.format("%H").to_string().parse::<u32>().unwrap_or(0) * 60
                + local_now.format("%M").to_string().parse::<u32>().unwrap_or(0);

            // Must be at or past the scheduled time
            if now_mins < schedule_mins { return false; }

            // Don't fire again if already fired in this window
            if let Some(lf) = &last_fire_time {
                let lf_local = lf.with_timezone(&chrono::Local);
                if lf_local.date_naive() == local_now.date_naive() && frequency == "daily" {
                    return false; // Already fired today
                }
                // For weekly/monthly/yearly — already fired today is enough to skip
                if lf_local.date_naive() == local_now.date_naive() {
                    return false;
                }
            }

            use chrono::Datelike;
            match frequency {
                "daily" => {
                    let _interval = v.get("interval").and_then(|s| s.as_u64()).unwrap_or(1);
                    // For interval > 1, check day-of-year mod interval
                    if _interval > 1 {
                        let day_of_year = local_now.ordinal() as u64;
                        if day_of_year % _interval != 0 { return false; }
                    }
                    true
                }
                "weekly" => {
                    let days = v.get("days").and_then(|d| d.as_array());
                    let weekday = match local_now.weekday() {
                        chrono::Weekday::Mon => "mon",
                        chrono::Weekday::Tue => "tue",
                        chrono::Weekday::Wed => "wed",
                        chrono::Weekday::Thu => "thu",
                        chrono::Weekday::Fri => "fri",
                        chrono::Weekday::Sat => "sat",
                        chrono::Weekday::Sun => "sun",
                    };
                    match days {
                        Some(day_arr) => day_arr.iter().any(|d| d.as_str() == Some(weekday)),
                        None => true, // No days specified — fire any day
                    }
                }
                "monthly" => {
                    let day_of_month = local_now.day();
                    // Check specific days
                    if let Some(days_arr) = v.get("days_of_month").and_then(|d| d.as_array()) {
                        return days_arr.iter().any(|d| d.as_u64() == Some(day_of_month as u64));
                    }
                    // Check ordinal (first/second/third/fourth/last + day type)
                    if let Some(ordinal) = v.get("ordinal").and_then(|s| s.as_str()) {
                        let ordinal_day = v.get("ordinal_day").and_then(|s| s.as_str()).unwrap_or("day");
                        return matches_ordinal_day(local_now.date_naive(), ordinal, ordinal_day);
                    }
                    true
                }
                "yearly" => {
                    let month_name = match local_now.month() {
                        1 => "jan", 2 => "feb", 3 => "mar", 4 => "apr",
                        5 => "may", 6 => "jun", 7 => "jul", 8 => "aug",
                        9 => "sep", 10 => "oct", 11 => "nov", 12 => "dec",
                        _ => return false,
                    };
                    let months = v.get("months").and_then(|d| d.as_array());
                    match months {
                        Some(m_arr) => m_arr.iter().any(|m| m.as_str() == Some(month_name)),
                        None => true,
                    }
                    // Also check ordinal if specified
                    // (simplified: just check month match for now)
                }
                _ => false,
            }
        }
        _ => false, // "off" or unknown
    }
}

/// Check if a date matches an ordinal day pattern like "first monday" or "last weekday".
fn matches_ordinal_day(date: chrono::NaiveDate, ordinal: &str, day_type: &str) -> bool {
    use chrono::Datelike;
    let dom = date.day();
    let weekday = date.weekday();

    // Check day type match
    let day_matches = match day_type {
        "day" => true,
        "weekday" => weekday != chrono::Weekday::Sat && weekday != chrono::Weekday::Sun,
        "mon" | "monday" => weekday == chrono::Weekday::Mon,
        "tue" | "tuesday" => weekday == chrono::Weekday::Tue,
        "wed" | "wednesday" => weekday == chrono::Weekday::Wed,
        "thu" | "thursday" => weekday == chrono::Weekday::Thu,
        "fri" | "friday" => weekday == chrono::Weekday::Fri,
        "sat" | "saturday" => weekday == chrono::Weekday::Sat,
        "sun" | "sunday" => weekday == chrono::Weekday::Sun,
        _ => true,
    };
    if !day_matches { return false; }

    // Check ordinal position
    match ordinal {
        "first" => dom <= 7,
        "second" => dom > 7 && dom <= 14,
        "third" => dom > 14 && dom <= 21,
        "fourth" => dom > 21 && dom <= 28,
        "last" => {
            // Last occurrence: no more matching days this month
            let days_in_month = if date.month() == 12 {
                chrono::NaiveDate::from_ymd_opt(date.year() + 1, 1, 1)
            } else {
                chrono::NaiveDate::from_ymd_opt(date.year(), date.month() + 1, 1)
            }.map(|d| d.pred_opt().map(|p| p.day()).unwrap_or(28)).unwrap_or(28);
            dom + 7 > days_in_month
        }
        _ => true,
    }
}

/// Compute the next N fire times for a schedule (for UI preview).
#[tauri::command]
pub fn k2so_agents_preview_schedule(
    mode: String,
    schedule_json: String,
    count: u32,
) -> Result<Vec<String>, String> {
    let mut results = Vec::new();
    let mut cursor = chrono::Local::now();

    // Step forward in 1-minute increments, checking up to 366 days ahead
    let max_steps = 366 * 24 * 60; // 1 year of minutes
    let mut steps = 0u64;

    while results.len() < count as usize && steps < max_steps {
        if should_project_fire(&mode, Some(&schedule_json), Some(&cursor.to_rfc3339())) {
            // This would fire — but we need to check if it's a NEW fire, not a repeat
            // For scheduled mode, each matching day/time is one fire
            results.push(cursor.format("%Y-%m-%d %H:%M").to_string());
            // Skip ahead past this fire window
            if mode == "hourly" {
                let v: serde_json::Value = serde_json::from_str(&schedule_json).unwrap_or_default();
                let every = v.get("every_seconds").and_then(|s| s.as_u64()).unwrap_or(300);
                cursor = cursor + chrono::Duration::seconds(every as i64);
                steps += every / 60;
                continue;
            } else {
                // Skip to next day for scheduled mode
                cursor = cursor + chrono::Duration::days(1);
                // Reset to start of day
                let next_date = cursor.date_naive();
                if let Some(dt) = next_date.and_hms_opt(0, 0, 0) {
                    use chrono::TimeZone;
                    if let Some(local_dt) = chrono::Local.from_local_datetime(&dt).single() {
                        cursor = local_dt;
                    }
                }
                steps += 24 * 60;
                continue;
            }
        }
        cursor = cursor + chrono::Duration::minutes(1);
        steps += 1;
    }

    Ok(results)
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
        .filter(|p| p.heartbeat_mode != "off")
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
/// Auto-uninstalls the scheduler when no projects have heartbeat enabled,
/// and auto-installs when at least one does.
#[tauri::command]
pub fn k2so_agents_update_heartbeat_projects(
    state: tauri::State<'_, crate::state::AppState>,
) -> Result<(), String> {
    let conn = state.db.lock();
    let projects = crate::db::schema::Project::list(&conn).map_err(|e| e.to_string())?;
    let heartbeat_paths: Vec<String> = projects
        .iter()
        .filter(|p| p.heartbeat_mode != "off")
        .map(|p| p.path.clone())
        .collect();
    drop(conn);

    let k2so_home = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".k2so");
    let paths_file = k2so_home.join("heartbeat-projects.txt");
    fs::write(&paths_file, heartbeat_paths.join("\n"))
        .map_err(|e| format!("Failed to write heartbeat projects: {}", e))?;

    // Auto-manage scheduler lifecycle: uninstall when empty, install when needed
    if heartbeat_paths.is_empty() {
        #[cfg(target_os = "macos")]
        { let _ = uninstall_heartbeat_launchd(); }
        #[cfg(target_os = "linux")]
        { let _ = uninstall_heartbeat_cron(); }
    } else {
        // Ensure scheduler is installed (idempotent — unloads before loading)
        let script_path = k2so_home.join("heartbeat.sh");
        if script_path.exists() {
            #[cfg(target_os = "macos")]
            { let _ = install_heartbeat_launchd(&script_path); }
            #[cfg(target_os = "linux")]
            { let _ = install_heartbeat_cron(&script_path); }
        }
    }

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

# Pure-bash URL encoding (no python3 dependency)
urlencode() {{
    local string="$1" length="${{#1}}" i c
    local encoded=""
    for (( i = 0; i < length; i++ )); do
        c="${{string:i:1}}"
        case "$c" in
            [a-zA-Z0-9._~-]) encoded+="$c" ;;
            *) encoded+=$(printf '%%%02X' "'$c") ;;
        esac
    done
    printf '%s' "$encoded"
}}

# Read K2SO port
if [ ! -f "$PORT_FILE" ]; then
    exit 0
fi
PORT=$(cat "$PORT_FILE" 2>/dev/null)
if [ -z "$PORT" ] || ! [[ "$PORT" =~ ^[0-9]+$ ]]; then
    exit 0
fi

# Check if K2SO is alive (server returns JSON with "status":"ok")
HEALTH=$(curl -s --connect-timeout 2 "http://127.0.0.1:$PORT/health" 2>/dev/null)
if ! echo "$HEALTH" | grep -q '"ok"'; then
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
    echo "$(ts) ERROR: No auth token available — skipping heartbeat" >> "$LOG_FILE"
    exit 0
fi

# Trigger triage for each heartbeat-enabled project
while IFS= read -r project_path; do
    [ -z "$project_path" ] && continue
    ENCODED_PATH=$(urlencode "$project_path")
    RESULT=$(curl -sG "http://127.0.0.1:$PORT/cli/scheduler-tick?token=$TOKEN&project=$ENCODED_PATH" --connect-timeout 5 --max-time 30 2>>"$LOG_FILE")
    if [ $? -ne 0 ]; then
        echo "$(ts) ERROR: curl failed for $project_path" >> "$LOG_FILE"
        continue
    fi
    # Extract count from JSON without python3 (pattern: "count":N)
    COUNT=$(echo "$RESULT" | grep -o '"count":[0-9]*' | grep -o '[0-9]*' || echo 0)
    if [ -n "$COUNT" ] && [ "$COUNT" -gt 0 ] 2>/dev/null; then
        echo "$(ts) Heartbeat: launched $COUNT agents for $project_path" >> "$LOG_FILE"
    fi
done < "$PROJECTS_FILE"

# Trim log (atomic: write to tmp then move)
if [ -f "$LOG_FILE" ]; then
    tail -200 "$LOG_FILE" > "$LOG_FILE.tmp" 2>/dev/null && mv -f "$LOG_FILE.tmp" "$LOG_FILE" 2>/dev/null
fi
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
    <integer>60</integer>
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
    let entry = format!("* * * * * {} {}", script_path.to_string_lossy(), marker);

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

// ── Agent Editor ───────────────────────────────────────────────────────

/// Get full context needed for the AIFileEditor agent editing session.
#[tauri::command]
pub fn k2so_agents_get_editor_context(
    project_path: String,
    agent_name: String,
) -> Result<serde_json::Value, String> {
    let dir = agent_dir(&project_path, &agent_name);
    if !dir.exists() {
        return Err(format!("Agent '{}' does not exist", agent_name));
    }

    let agent_md = fs::read_to_string(dir.join("agent.md")).unwrap_or_default();
    let fm = parse_frontmatter(&agent_md);
    let is_manager = fm.get("pod_leader").map_or(false, |v| v == "true")
        || fm.get("coordinator").map_or(false, |v| v == "true")
        || fm.get("manager").map_or(false, |v| v == "true");
    let role = fm.get("role").cloned().unwrap_or_default();
    let agent_type = fm.get("type").cloned().map(|t| {
        match t.as_str() {
            "pod-leader" | "coordinator" => "manager".to_string(),
            "pod-member" => "agent-template".to_string(),
            other => other.to_string(),
        }
    }).unwrap_or("agent-template".to_string());

    Ok(serde_json::json!({
        "agentName": agent_name,
        "role": role,
        "agentType": agent_type,
        "isManager": is_manager,
        "agentMd": agent_md,
        "agentMdPath": dir.join("agent.md").to_string_lossy(),
        "agentDir": dir.to_string_lossy(),
    }))
}

/// Preview the generated CLAUDE.md for an agent (without writing to disk).
/// Returns the content that would be injected at launch, plus the on-disk CLAUDE.md if it exists.
#[tauri::command]
pub fn k2so_agents_preview_claude_md(
    project_path: String,
    agent_name: String,
) -> Result<serde_json::Value, String> {
    let generated = generate_agent_claude_md_content(&project_path, &agent_name, None)?;

    // Check for on-disk CLAUDE.md (may have user edits)
    let dir = agent_dir(&project_path, &agent_name);
    let on_disk_path = dir.join("CLAUDE.md");
    let on_disk = if on_disk_path.exists() {
        Some(safe_read_to_string(&on_disk_path).unwrap_or_default())
    } else {
        None
    };

    Ok(serde_json::json!({
        "generated": generated,
        "onDisk": on_disk,
        "claudeMdPath": on_disk_path.to_string_lossy(),
    }))
}

/// Regenerate and write CLAUDE.md for an agent (resets to generated defaults).
#[tauri::command]
pub fn k2so_agents_regenerate_claude_md(
    project_path: String,
    agent_name: String,
) -> Result<String, String> {
    let generated = generate_agent_claude_md_content(&project_path, &agent_name, None)?;
    let dir = agent_dir(&project_path, &agent_name);
    let claude_md_path = dir.join("CLAUDE.md");
    atomic_write(&claude_md_path, &generated)?;
    Ok(generated)
}

/// Save an agent's agent.md file, creating a timestamped backup of the previous version.
#[tauri::command]
pub fn k2so_agents_save_agent_md(
    project_path: String,
    agent_name: String,
    content: String,
) -> Result<(), String> {
    let dir = agent_dir(&project_path, &agent_name);
    if !dir.exists() {
        return Err(format!("Agent '{}' does not exist", agent_name));
    }

    let agent_md_path = dir.join("agent.md");

    // Back up existing agent.md before overwriting
    if agent_md_path.exists() {
        let backup_dir = dir.join("agent-backups");
        fs::create_dir_all(&backup_dir).ok();

        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let backup_name = format!("agent-{}.md", timestamp);
        let existing = fs::read_to_string(&agent_md_path).unwrap_or_default();
        fs::write(backup_dir.join(&backup_name), &existing).ok();

        // Keep only the 20 most recent backups
        cleanup_agent_backups(&backup_dir, 20);
    }

    atomic_write(&agent_md_path, &content)
}

/// Remove oldest backups, keeping only the most recent `keep` files.
fn cleanup_agent_backups(backup_dir: &std::path::Path, keep: usize) {
    if let Ok(entries) = fs::read_dir(backup_dir) {
        let mut files: Vec<std::path::PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().map_or(false, |ext| ext == "md"))
            .collect();
        files.sort();
        if files.len() > keep {
            for old in &files[..files.len() - keep] {
                fs::remove_file(old).ok();
            }
        }
    }
}

// ── Agent Sessions (DB-tracked) ──────────────────────────────────────────

#[tauri::command]
pub fn agent_sessions_list(
    state: tauri::State<'_, crate::state::AppState>,
    project_id: String,
) -> Result<Vec<AgentSession>, String> {
    let conn = state.db.lock();
    AgentSession::list_by_project(&conn, &project_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn agent_sessions_get(
    state: tauri::State<'_, crate::state::AppState>,
    project_id: String,
    agent_name: String,
) -> Result<Option<AgentSession>, String> {
    let conn = state.db.lock();
    AgentSession::get_by_agent(&conn, &project_id, &agent_name).map_err(|e| e.to_string())
}

// ── Workspace Relations ─────────────────────────────────────────────────

#[tauri::command]
pub fn workspace_relations_list(
    state: tauri::State<'_, crate::state::AppState>,
    project_id: String,
) -> Result<Vec<WorkspaceRelation>, String> {
    let conn = state.db.lock();
    WorkspaceRelation::list_for_source(&conn, &project_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn workspace_relations_list_incoming(
    state: tauri::State<'_, crate::state::AppState>,
    project_id: String,
) -> Result<Vec<WorkspaceRelation>, String> {
    let conn = state.db.lock();
    WorkspaceRelation::list_for_target(&conn, &project_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn workspace_relations_create(
    state: tauri::State<'_, crate::state::AppState>,
    source_project_id: String,
    target_project_id: String,
    relation_type: Option<String>,
) -> Result<WorkspaceRelation, String> {
    let conn = state.db.lock();
    let id = uuid::Uuid::new_v4().to_string();
    let rel_type = relation_type.unwrap_or_else(|| "oversees".to_string());
    WorkspaceRelation::create(&conn, &id, &source_project_id, &target_project_id, &rel_type)
        .map_err(|e| e.to_string())?;
    // Return the created relation
    Ok(WorkspaceRelation {
        id,
        source_project_id,
        target_project_id,
        relation_type: rel_type,
        created_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64,
    })
}

#[tauri::command]
pub fn workspace_relations_delete(
    state: tauri::State<'_, crate::state::AppState>,
    id: String,
) -> Result<(), String> {
    let conn = state.db.lock();
    WorkspaceRelation::delete(&conn, &id).map_err(|e| e.to_string())?;
    Ok(())
}

// ── Skill File Generation ────────────────────────────────────────────

/// Regenerate SKILL.md files for all agents in a workspace.
/// Called on app startup (migration) and via CLI `k2so skills regenerate`.
#[tauri::command]
pub fn k2so_agents_regenerate_skills(
    project_path: String,
) -> Result<serde_json::Value, String> {
    let agents_dir = PathBuf::from(&project_path).join(".k2so/agents");
    if !agents_dir.exists() {
        return Ok(serde_json::json!({"updated": 0}));
    }

    let project_name = std::path::Path::new(&project_path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "workspace".to_string());

    let mut updated = 0;
    if let Ok(entries) = fs::read_dir(&agents_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() { continue; }
            let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();

            // Determine agent type from agent.md frontmatter
            let agent_md = path.join("agent.md");
            let agent_type = if agent_md.exists() {
                let content = fs::read_to_string(&agent_md).unwrap_or_default();
                let fm = parse_frontmatter(&content);
                let raw = fm.get("type").cloned().unwrap_or_default();
                match raw.as_str() {
                    "pod-leader" | "coordinator" | "manager" => "manager".to_string(),
                    "custom" => "custom".to_string(),
                    "k2so" => "k2so".to_string(),
                    "agent-template" => "agent-template".to_string(),
                    _ => {
                        // Check for manager/coordinator boolean flags
                        let is_mgr = fm.get("manager").map(|v| v == "true").unwrap_or(false)
                            || fm.get("coordinator").map(|v| v == "true").unwrap_or(false)
                            || fm.get("pod_leader").map(|v| v == "true").unwrap_or(false);
                        if is_mgr { "manager".to_string() } else { "agent-template".to_string() }
                    }
                }
            } else {
                "agent-template".to_string()
            };

            let skill_content = match agent_type.as_str() {
                "manager" => generate_manager_skill_content(&project_path, &project_name),
                "custom" | "k2so" => generate_custom_agent_skill_content(&project_name, &name),
                _ => generate_template_skill_content(&project_name, &name),
            };

            let skill_path = path.join("SKILL.md");
            if fs::write(&skill_path, &skill_content).is_ok() {
                updated += 1;
            }
        }
    }

    Ok(serde_json::json!({"updated": updated}))
}

/// Write the workspace-level SKILL.md at the project root.
/// This is the user-facing skill for when humans work directly with an LLM.
pub fn write_workspace_skill_file(project_path: &str) {
    let project_name = std::path::Path::new(project_path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "workspace".to_string());

    let content = generate_workspace_skill_content(&project_name);
    let skill_path = PathBuf::from(project_path).join("SKILL.md");
    let _ = fs::write(&skill_path, &content);
}

/// Write a single agent's SKILL.md. Used internally during launch.
pub fn write_agent_skill_file(project_path: &str, agent_name: &str, agent_type: &str) {
    let project_name = std::path::Path::new(project_path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "workspace".to_string());

    let skill_content = match agent_type {
        "manager" | "coordinator" | "pod-leader" => generate_manager_skill_content(project_path, &project_name),
        "custom" | "k2so" => generate_custom_agent_skill_content(&project_name, agent_name),
        _ => generate_template_skill_content(&project_name, agent_name),
    };

    let skill_path = agent_dir(project_path, agent_name).join("SKILL.md");
    let _ = fs::write(&skill_path, &skill_content);
}

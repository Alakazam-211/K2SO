//! Agent CRUD + work queue commands.
//!
//! This is the business logic behind the bulk of the `k2so agents *`
//! and `k2so work *` CLI surface. Pre-0.33.0 these were Tauri-only
//! (`#[tauri::command]` functions in `src-tauri/src/commands/
//! k2so_agents.rs`); in 0.33.0 they live here so the k2so-daemon can
//! serve the same routes headlessly when the Tauri app is quit.
//!
//! Covers:
//!
//! - **Agent CRUD**: [`list`], [`create`], [`delete`] (+ forced
//!   `delete_inner`), [`get_profile`], [`update_profile`],
//!   [`update_field`] (+ its pure helper [`update_agent_md_field`]).
//! - **Per-agent work queue**: [`work_list`], [`work_create`],
//!   [`work_move`].
//! - **Workspace inbox**: [`workspace_inbox_list`],
//!   [`workspace_inbox_create`].
//! - **Wakeup + backup helpers** used by the commands above:
//!   [`ensure_agent_wakeup`], [`cleanup_agent_backups`].
//!
//! Every function is host-agnostic — uses `db::shared()` +
//! `fs_atomic::*` + core agent-system primitives, no AppHandle, no
//! Tauri command macros.

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::agents::events::push_agent_event;
use crate::agents::scheduler::{agent_work_dir, count_md_files};
use crate::agents::session::simple_date;
use crate::agents::skill_writer::{generate_default_agent_body, write_agent_skill_file};
use crate::agents::wake::{agent_wakeup_path, wakeup_template_for};
use crate::agents::work_item::{atomic_write, read_work_item, WorkItem};
use crate::agents::{agent_dir, agents_dir, parse_frontmatter};
use crate::fs_atomic::{atomic_write_str, log_if_err};

/// Summary row the UI agent-list + `k2so agents list` CLI render.
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

// ── Wakeup scaffolding ────────────────────────────────────────────────

/// Create `<agent_dir>/WAKEUP.md` from the matching template if it
/// doesn't exist. No-op when the agent type doesn't use wake-up or
/// when a heartbeat folder has already claimed ownership of the wake
/// source of truth.
pub fn ensure_agent_wakeup(project_path: &str, agent_name: &str, agent_type: &str) {
    let Some(template) = wakeup_template_for(agent_type) else {
        return;
    };
    let path = agent_wakeup_path(project_path, agent_name);
    if path.exists() {
        return;
    }
    // Multi-heartbeat lives at heartbeats/<name>/wakeup.md — if any
    // heartbeat folder already exists for this agent, we're past the
    // legacy single-slot world and the agent-root wakeup.md is no
    // longer the source of truth. Skip scaffolding to avoid tricking
    // the repair pass into clobbering real content.
    let hb_default = agent_dir(project_path, agent_name)
        .join("heartbeats")
        .join("default")
        .join("WAKEUP.md");
    if hb_default.exists() {
        return;
    }
    log_if_err(
        "ensure_agent_wakeup",
        &path,
        atomic_write_str(&path, template),
    );
}

/// Prune an agent-backups dir down to the most-recent `keep` files.
/// Sorts by filename (which embeds the date stamp from `simple_date`)
/// so "oldest" means lexicographically smallest.
pub fn cleanup_agent_backups(backup_dir: &Path, keep: usize) {
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

// ── Agent CRUD ────────────────────────────────────────────────────────

/// List every agent directory in `.k2so/agents/` with summary counts
/// (inbox / active / done item counts + manager flag + canonical
/// type). Alphabetical.
pub fn list(project_path: String) -> Result<Vec<K2soAgentInfo>, String> {
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
        let agent_md = entry.path().join("AGENT.md");

        let (role, is_manager, agent_type) = if agent_md.exists() {
            let content = fs::read_to_string(&agent_md).unwrap_or_default();
            let fm = parse_frontmatter(&content);
            let role = fm.get("role").cloned().unwrap_or_default();
            // Support old (pod_leader/coordinator) and new (manager) keys.
            let is_mgr = fm.get("pod_leader").map(|v| v == "true").unwrap_or(false)
                || fm.get("coordinator").map(|v| v == "true").unwrap_or(false)
                || fm.get("manager").map(|v| v == "true").unwrap_or(false);
            let agent_type = fm
                .get("type")
                .cloned()
                .map(|t| match t.as_str() {
                    "pod-leader" | "coordinator" => "manager".to_string(),
                    "pod-member" => "agent-template".to_string(),
                    other => other.to_string(),
                })
                .unwrap_or_else(|| {
                    if is_mgr {
                        "manager".to_string()
                    } else {
                        "agent-template".to_string()
                    }
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

/// Create a new agent dir (`.k2so/agents/<name>/`) + frontmatter-
/// wrapped `AGENT.md` + scaffold inbox/active/done + write the
/// per-agent SKILL.md + scaffold WAKEUP.md (for types that use it).
///
/// `agent_type` defaults to `"agent-template"`. Name must be
/// alphanumeric (plus `-` / `_`).
pub fn create(
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
    let _ = fs::create_dir_all(crate::agents::scheduler::workspace_inbox_dir(&project_path));

    let agent_md = dir.join("AGENT.md");
    let mut frontmatter = format!("name: {}\nrole: {}\ntype: {}", name, role, agent_type);
    if is_manager {
        frontmatter.push_str("\nmanager: true");
    }

    let body = if let Some(ref p) = prompt {
        if !p.is_empty() {
            p.clone()
        } else {
            generate_default_agent_body(&agent_type, &name, &role, &project_path)
        }
    } else {
        generate_default_agent_body(&agent_type, &name, &role, &project_path)
    };

    let content = format!("---\n{}\n---\n\n{}\n", frontmatter, body);
    atomic_write(&agent_md, &content)?;

    write_agent_skill_file(&project_path, &name, &agent_type);
    ensure_agent_wakeup(&project_path, &name, &agent_type);

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

/// Delete an agent's dir. Refuses for manager agents or agents with
/// active work items unless `force` is true.
pub fn delete_inner(project_path: &str, name: &str, force: bool) -> Result<(), String> {
    let dir = agent_dir(project_path, name);
    if !dir.exists() {
        return Err(format!("Agent '{}' does not exist", name));
    }

    let agent_md = dir.join("AGENT.md");
    if agent_md.exists() {
        let content = fs::read_to_string(&agent_md).unwrap_or_default();
        let fm = parse_frontmatter(&content);
        if fm
            .get("type")
            .map_or(false, |t| t == "manager" || t == "coordinator" || t == "pod-leader")
            && !force
        {
            return Err("Cannot delete manager agent. Use --force to override.".to_string());
        }
    }

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

/// Non-forced variant — the Tauri command shape.
pub fn delete(project_path: String, name: String) -> Result<(), String> {
    delete_inner(&project_path, &name, false)
}

/// Read an agent's raw `AGENT.md` content.
pub fn get_profile(project_path: String, agent_name: String) -> Result<String, String> {
    let path = agent_dir(&project_path, &agent_name).join("AGENT.md");
    if !path.exists() {
        return Err(format!("Agent '{}' does not exist", agent_name));
    }
    fs::read_to_string(&path).map_err(|e| e.to_string())
}

/// Replace an agent's raw `AGENT.md` content. Caller owns validation
/// of the incoming string.
pub fn update_profile(
    project_path: String,
    agent_name: String,
    content: String,
) -> Result<(), String> {
    let dir = agent_dir(&project_path, &agent_name);
    if !dir.exists() {
        return Err(format!("Agent '{}' does not exist", agent_name));
    }
    let path = dir.join("AGENT.md");
    atomic_write(&path, &content)
}

/// Pure, I/O-free rewrite of an `AGENT.md` content blob with `field`
/// set to `value`. If `field` is a frontmatter key, the value replaces
/// the existing frontmatter line. If `field` is a markdown section
/// (`## heading`), the value replaces everything from the heading to
/// the next `## ` or end-of-body. Unknown sections are appended.
pub fn update_agent_md_field(content: &str, field: &str, value: &str) -> Result<String, String> {
    if !content.starts_with("---") {
        return Err("agent.md missing frontmatter".to_string());
    }
    let end_idx = content[3..]
        .find("---")
        .ok_or_else(|| "Invalid frontmatter in agent.md".to_string())?;
    let frontmatter = &content[3..3 + end_idx];
    let body = &content[3 + end_idx + 3..];

    let fm_keys: Vec<&str> = frontmatter
        .lines()
        .filter_map(|l| l.split_once(':').map(|(k, _)| k.trim()))
        .collect();

    if fm_keys.contains(&field) {
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
        return Ok(format!("---\n{}\n---{}", updated_fm.trim(), body));
    }

    let section_header = format!("## {}", field);
    if let Some(start) = body.find(&section_header) {
        let after_header = start + section_header.len();
        let end = body[after_header..]
            .find("\n## ")
            .map(|pos| after_header + pos)
            .unwrap_or(body.len());
        let mut new_body = String::new();
        new_body.push_str(&body[..start]);
        new_body.push_str(&section_header);
        new_body.push_str("\n\n");
        new_body.push_str(value);
        new_body.push_str("\n\n");
        new_body.push_str(body[end..].trim_start());
        Ok(format!("---\n{}\n---{}", frontmatter.trim(), new_body))
    } else {
        let mut new_body = body.to_string();
        if !new_body.ends_with('\n') {
            new_body.push('\n');
        }
        new_body.push_str(&format!("\n## {}\n\n{}\n", field, value));
        Ok(format!("---\n{}\n---{}", frontmatter.trim(), new_body))
    }
}

/// Command-shape `update_field` — reads AGENT.md, rewrites via
/// [`update_agent_md_field`], drops a timestamped backup into
/// `<agent-dir>/agent-backups/` (capped at 20 via
/// [`cleanup_agent_backups`]), then atomic-writes the new content.
pub fn update_field(
    project_path: String,
    name: String,
    field: String,
    value: String,
) -> Result<String, String> {
    let dir = agent_dir(&project_path, &name);
    if !dir.exists() {
        return Err(format!("Agent '{}' does not exist", name));
    }

    let md_path = dir.join("AGENT.md");
    let content = fs::read_to_string(&md_path)
        .map_err(|e| format!("Failed to read agent.md: {}", e))?;

    let updated = update_agent_md_field(&content, &field, &value)?;

    let backup_dir = dir.join("agent-backups");
    let _ = fs::create_dir_all(&backup_dir);
    let backup_name = format!(
        "agent-{}.md",
        simple_date().replace(' ', "_").replace(':', "-")
    );
    let _ = fs::copy(&md_path, backup_dir.join(&backup_name));
    cleanup_agent_backups(&backup_dir, 20);

    atomic_write(&md_path, &updated)?;
    Ok(updated)
}

// ── Work queue ─────────────────────────────────────────────────────────

/// Build a filename-safe slug from a title — lowercase, non-alphanum
/// collapsed to `-`, empty segments dropped, capped at 60 chars.
fn work_item_slug(title: &str) -> String {
    let slug: String = title
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<&str>>()
        .join("-");
    slug[..slug.len().min(60)].to_string()
}

/// 120-char body preview with ellipsis on truncation. Matches the
/// shape the sidebar + CLI renderers expect.
fn body_preview(body: &str) -> String {
    let trimmed = body.trim();
    let preview: String = trimmed.chars().take(120).collect();
    if trimmed.chars().count() > 120 {
        format!("{}...", preview.trim())
    } else {
        preview
    }
}

/// List an agent's work items across the given folder(s). `folder`
/// None = inbox + active + done. Filters to `.md` only.
pub fn work_list(
    project_path: String,
    agent_name: String,
    folder: Option<String>,
) -> Result<Vec<WorkItem>, String> {
    let folders = match folder.as_deref() {
        Some(f) => vec![f.to_string()],
        None => vec![
            "inbox".to_string(),
            "active".to_string(),
            "done".to_string(),
        ],
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

/// Create a work item in either a specific agent's inbox or the
/// workspace inbox (agent_name = None). Pushes a channel event to
/// the receiving agent's queue so channel-based agents get nudged.
pub fn work_create(
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
            let dir = crate::agents::scheduler::workspace_inbox_dir(&project_path);
            fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            dir
        }
    };

    let priority = priority.unwrap_or_else(|| "normal".to_string());
    let item_type = item_type.unwrap_or_else(|| "task".to_string());
    let source = source.unwrap_or_else(|| "manual".to_string());

    let slug = work_item_slug(&title);
    let filename = format!("{}.md", slug);

    let now = simple_date();
    let content = format!(
        "---\ntitle: {}\npriority: {}\nassigned_by: user\ncreated: {}\ntype: {}\nsource: {}\n---\n\n{}\n",
        title, priority, now, item_type, source, body
    );

    let path = target_dir.join(&filename);
    atomic_write(&path, &content)?;

    let preview = body_preview(&body);
    // Push channel event for persistent agents.
    match &agent_name {
        Some(agent) => push_agent_event(
            &project_path,
            agent,
            "work-item",
            &format!(
                "New work item in your inbox: \"{}\" (priority: {})",
                title, priority
            ),
            &priority,
        ),
        None => push_agent_event(
            &project_path,
            "__lead__",
            "work-item",
            &format!(
                "New item in workspace inbox: \"{}\" (priority: {})",
                title, priority
            ),
            &priority,
        ),
    }

    Ok(WorkItem {
        filename,
        title,
        priority,
        assigned_by: "user".to_string(),
        created: now,
        item_type,
        folder: if agent_name.is_some() {
            "inbox".to_string()
        } else {
            "workspace-inbox".to_string()
        },
        body_preview: preview,
        source,
    })
}

/// Move a work item between folders (inbox ↔ active ↔ done).
pub fn work_move(
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
        return Err(format!(
            "Work item not found: {}/{}",
            from_folder, filename
        ));
    }
    if !target_dir.exists() {
        fs::create_dir_all(&target_dir).map_err(|e| e.to_string())?;
    }

    fs::rename(&source, &target).map_err(|e| format!("Failed to move work item: {}", e))?;
    Ok(())
}

// ── Workspace inbox ────────────────────────────────────────────────────

pub fn workspace_inbox_list(project_path: String) -> Result<Vec<WorkItem>, String> {
    let dir = crate::agents::scheduler::workspace_inbox_dir(&project_path);
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

pub fn workspace_inbox_create(
    workspace_path: String,
    title: String,
    body: String,
    priority: Option<String>,
    item_type: Option<String>,
    assigned_by: Option<String>,
    source: Option<String>,
) -> Result<WorkItem, String> {
    let dir = crate::agents::scheduler::workspace_inbox_dir(&workspace_path);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    let priority = priority.unwrap_or_else(|| "normal".to_string());
    let item_type = item_type.unwrap_or_else(|| "task".to_string());
    let assigned_by = assigned_by.unwrap_or_else(|| "external".to_string());
    let source = source.unwrap_or_else(|| "manual".to_string());

    let slug = work_item_slug(&title);
    let filename = format!("{}.md", slug);

    let now = simple_date();
    let content = format!(
        "---\ntitle: {}\npriority: {}\nassigned_by: {}\ncreated: {}\ntype: {}\nsource: {}\n---\n\n{}\n",
        title, priority, assigned_by, now, item_type, source, body
    );

    let path = dir.join(&filename);
    atomic_write(&path, &content)?;

    Ok(WorkItem {
        filename,
        title,
        priority,
        assigned_by,
        created: now,
        item_type,
        folder: "workspace-inbox".to_string(),
        body_preview: body_preview(&body),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn work_item_slug_lowercases_and_dasheses() {
        assert_eq!(work_item_slug("Fix Auth Module"), "fix-auth-module");
        assert_eq!(work_item_slug("Hello, World!"), "hello-world");
    }

    #[test]
    fn work_item_slug_caps_at_60_chars() {
        let long = "x".repeat(200);
        assert!(work_item_slug(&long).len() <= 60);
    }

    #[test]
    fn body_preview_trims_and_ellipsizes() {
        let preview = body_preview(&"a".repeat(150));
        assert!(preview.ends_with("..."));
        assert!(preview.len() <= 128);
    }

    #[test]
    fn update_agent_md_field_replaces_frontmatter_key() {
        let md = "---\nrole: old\ntype: custom\n---\n\nbody";
        let updated = update_agent_md_field(md, "role", "new").unwrap();
        assert!(updated.contains("role: new"));
        assert!(!updated.contains("role: old"));
    }

    #[test]
    fn update_agent_md_field_replaces_section() {
        let md = "---\nrole: x\n---\n\n## Existing\n\nold body\n\n## Other\n\nkeep";
        let updated = update_agent_md_field(md, "Existing", "new body").unwrap();
        assert!(updated.contains("new body"));
        assert!(!updated.contains("old body"));
        assert!(updated.contains("## Other"));
        assert!(updated.contains("keep"));
    }

    #[test]
    fn update_agent_md_field_appends_unknown_section() {
        let md = "---\nrole: x\n---\n\nbody";
        let updated = update_agent_md_field(md, "New Section", "content").unwrap();
        assert!(updated.contains("## New Section"));
        assert!(updated.contains("content"));
    }

    #[test]
    fn update_agent_md_field_rejects_missing_frontmatter() {
        assert!(update_agent_md_field("no fm", "role", "x").is_err());
    }
}

//! AIFileEditor surface for editing AGENT.md.
//!
//! Phase 2.5d: extracted from the monolithic `agents/commands.rs`. The
//! React `AgentPersonaEditor` consumes these four functions to render
//! the "Manage Persona" UI — fetch context, preview a regenerate,
//! commit a regenerate, save edits.
//!
//! Sibling [`crate::workspace::agent`] hosts the underlying CRUD;
//! [`crate::heartbeats::control`] hosts the per-agent heartbeat surface.

use std::fs;

use crate::agents::work_item::atomic_write;
use crate::agents::{agent_dir, parse_frontmatter};
use crate::workspace::agent::cleanup_agent_backups;


/// Get full context needed for the AIFileEditor agent editing session.
///
/// Returns a JSON bundle the React `AgentPersonaEditor` consumes
/// (agent name, role, type, manager-flag, current AGENT.md contents,
/// and the on-disk path so the editor can render an "open in finder"
/// link). The `agent_type` is normalized post-0.37.0 — old `pod-leader`
/// / `coordinator` types collapse to `manager`, `pod-member` collapses
/// to `agent-template`.
///
/// Phase 2 Unit 7d: moved from
/// `src-tauri/src/commands/k2so_agents.rs::k2so_agents_get_editor_context`.
pub fn k2so_agents_get_editor_context(
    project_path: String,
    agent_name: String,
) -> Result<serde_json::Value, String> {
    let dir = agent_dir(&project_path, &agent_name);
    if !dir.exists() {
        return Err(format!("Agent '{}' does not exist", agent_name));
    }

    let agent_md = fs::read_to_string(dir.join("AGENT.md")).unwrap_or_default();
    let fm = parse_frontmatter(&agent_md);
    let is_manager = fm.get("pod_leader").map_or(false, |v| v == "true")
        || fm.get("coordinator").map_or(false, |v| v == "true")
        || fm.get("manager").map_or(false, |v| v == "true");
    let role = fm.get("role").cloned().unwrap_or_default();
    let agent_type = fm
        .get("type")
        .cloned()
        .map(|t| match t.as_str() {
            "pod-leader" | "coordinator" => "manager".to_string(),
            "pod-member" => "agent-template".to_string(),
            other => other.to_string(),
        })
        .unwrap_or_else(|| "agent-template".to_string());

    Ok(serde_json::json!({
        "agentName": agent_name,
        "role": role,
        "agentType": agent_type,
        "isManager": is_manager,
        "agentMd": agent_md,
        "agentMdPath": dir.join("AGENT.md").to_string_lossy(),
        "agentDir": dir.to_string_lossy(),
    }))
}

/// Preview the agent's context bundle without writing to disk.
/// Returns `{ generated, onDisk, contextPath }`: the freshly-composed
/// system-prompt body, the current on-disk CLAUDE.md content (if any —
/// may contain user edits), and the CLAUDE.md path for caller-side
/// diff UIs. The JSON field is still `claudeMdPath` for back-compat
/// with the React AgentPersonaEditor; new UIs should read
/// `contextPath` once populated.
///
/// Phase 2 Unit 7d: moved from
/// `src-tauri/src/commands/k2so_agents.rs::k2so_agents_preview_agent_context`.
pub fn k2so_agents_preview_agent_context(
    project_path: String,
    agent_name: String,
) -> Result<serde_json::Value, String> {
    let generated = crate::agents::skill_content::generate_agent_claude_md_content(
        &project_path,
        &agent_name,
        None,
    )?;

    let dir = agent_dir(&project_path, &agent_name);
    let on_disk_path = dir.join("CLAUDE.md");
    let on_disk = if on_disk_path.exists() {
        Some(crate::agents::work_item::safe_read_to_string(&on_disk_path).unwrap_or_default())
    } else {
        None
    };

    Ok(serde_json::json!({
        "generated": generated,
        "onDisk": on_disk,
        "contextPath": on_disk_path.to_string_lossy(),
        // Legacy field — React still reads `claudeMdPath` at some
        // call sites. Emit both during the rename window; drop the
        // legacy field once every UI call site has migrated.
        "claudeMdPath": on_disk_path.to_string_lossy(),
    }))
}

/// Regenerate an agent's context bundle: composes the full
/// `--append-system-prompt` body, writes it to the agent's CLAUDE.md,
/// and returns it. Equivalent to `preview_agent_context` followed by
/// an atomic write.
///
/// Phase 2 Unit 7d: moved from
/// `src-tauri/src/commands/k2so_agents.rs::k2so_agents_regenerate_agent_context`.
pub fn k2so_agents_regenerate_agent_context(
    project_path: String,
    agent_name: String,
) -> Result<String, String> {
    let md = crate::agents::skill_content::generate_agent_claude_md_content(
        &project_path,
        &agent_name,
        None,
    )?;
    let claude_md_path = agent_dir(&project_path, &agent_name).join("CLAUDE.md");
    atomic_write(&claude_md_path, &md)?;
    Ok(md)
}

/// Save an agent's AGENT.md file, creating a timestamped backup of
/// the previous version. Keeps the 20 most recent backups in
/// `<agent>/agent-backups/`.
///
/// Phase 2 Unit 7d: moved from
/// `src-tauri/src/commands/k2so_agents.rs::k2so_agents_save_agent_md`.
pub fn k2so_agents_save_agent_md(
    project_path: String,
    agent_name: String,
    content: String,
) -> Result<(), String> {
    let dir = agent_dir(&project_path, &agent_name);
    if !dir.exists() {
        return Err(format!("Agent '{}' does not exist", agent_name));
    }

    let agent_md_path = dir.join("AGENT.md");

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

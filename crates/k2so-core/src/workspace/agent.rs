//! Agent CRUD commands.
//!
//! Phase 2.5d: extracted from the monolithic `agents/commands.rs`. This
//! is the business logic behind the bulk of the `k2so agents *` CLI
//! surface — list / create / delete / get_profile / update_profile /
//! update_field — plus the shared helpers `cleanup_agent_backups` and
//! `log_agent_warning` they all rely on. Every function is
//! host-agnostic (no Tauri, no AppHandle) so the daemon and the Tauri
//! app serve identical semantics.
//!
//! Sibling modules:
//! - [`crate::heartbeats::control`] — per-agent heartbeat CRUD +
//!   adaptive backoff (`ensure_agent_wakeup`, `get_heartbeat`,
//!   `set_heartbeat`, `heartbeat_noop`, `heartbeat_action`)
//! - [`crate::workspace::agent_editor`] — AIFileEditor surface for
//!   editing AGENT.md (get_editor_context, preview/regenerate/save)
//! - [`crate::workspace::relations`] — `workspace_sessions` and
//!   `workspace_relations` DB accessors.


use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::agents::session::simple_date;
use crate::agents::skill_writer::{generate_default_agent_body, write_agent_skill_file};
use crate::agents::work_item::atomic_write;
use crate::agents::{agent_dir, agent_type_for, agents_dir, parse_frontmatter};
use crate::heartbeats::control::ensure_agent_wakeup;
use crate::workspace::scheduler::{agent_work_dir, count_md_files};

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

// ── Helpers ───────────────────────────────────────────────────────────

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

/// Append a warning line to `<agent-dir>/agent.log`. Silent on any I/O
/// failure — logging a warning shouldn't itself cause a crash.
pub fn log_agent_warning(project_path: &str, agent_name: &str, message: &str) {
    let log_path = agent_dir(project_path, agent_name).join("agent.log");
    let entry = format!("[{}] WARN: {}\n", simple_date(), message);
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
    {
        use std::io::Write;
        let _ = file.write_all(entry.as_bytes());
    }
}

// ── Agent CRUD ────────────────────────────────────────────────────────

// ── Agent CRUD ────────────────────────────────────────────────────────

/// List every skill directory in the workspace with summary counts
/// (inbox / active / done item counts + manager flag + canonical
/// type). Alphabetical.
///
/// Post-Phase-2.5b: reads from `.k2so/skills/<name>/` (the unified
/// home for documentation profiles). Pre-migration workspaces fall
/// back to enumerating the legacy `.k2so/agents/` tree so the call
/// keeps returning useful data during the boot-cycle window where
/// the daemon hasn't run `consolidate_skills_v1` yet.
pub fn list(project_path: String) -> Result<Vec<K2soAgentInfo>, String> {
    let new_home = crate::agents::skills_dir(&project_path);
    let dir = if new_home.exists() {
        new_home
    } else {
        // Pre-Phase-2.5b workspaces still answer from the legacy tree.
        // The daemon's first-boot sweep migrates this away; after that
        // `.k2so/skills/` exists and the branch above is taken.
        agents_dir(&project_path)
    };
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
        // Skip hidden / system directories. `.archive` is the trash bin
        // for retired agents (see `archive_orphan_top_tier_agents`); it
        // sits inside `.k2so/agents/` for proximity but must never
        // appear in the workspace's agent list. Pre-0.36.0 it leaked
        // through and the WorkspacePanel rendered ".archive" as the
        // primary agent name. Any other dotfile dir is excluded for
        // the same reason.
        if name.starts_with('.') {
            continue;
        }
        // Post-Phase-2.5b skills carry SKILL.md; pre-migration
        // workspaces carry AGENT.md. Read whichever exists — the
        // frontmatter shape is identical.
        let skill_md = entry.path().join("SKILL.md");
        let agent_md = entry.path().join("AGENT.md");
        let profile_md = if skill_md.exists() { skill_md } else { agent_md.clone() };

        let (role, is_manager, agent_type) = if profile_md.exists() {
            let content = fs::read_to_string(&profile_md).unwrap_or_default();
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

    // 0.37.0 unification awareness: if the workspace has been migrated
    // and the unified primary at .k2so/agent/ already exists, "creating"
    // the workspace's named primary is a no-op success — the persona
    // is already there. The pre-0.37.0 contract was "create the agent's
    // dir + scaffold AGENT.md if missing"; post-migration the dir is
    // always present (the migration created it). Frontend "Manage
    // Persona" buttons that reflexively call `create` to ensure the
    // agent exists used to error here ("Agent already exists"); now
    // they get back the existing K2soAgentInfo and proceed to the
    // edit flow.
    let unified_primary = std::path::PathBuf::from(&project_path)
        .join(".k2so")
        .join("agent");
    if unified_primary.join("AGENT.md").exists() {
        let existing_type = agent_type_for(&project_path, &name);
        return Ok(K2soAgentInfo {
            name,
            role,
            inbox_count: 0,
            active_count: 0,
            done_count: 0,
            is_manager: existing_type == "manager" || existing_type == "coordinator",
            agent_type: existing_type,
        });
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
    // Post-Phase-2.1: scaffold the unified workspace inbox at
    // `.k2so/inbox/` (not the retired `.k2so/work/inbox/`). Best-effort —
    // an existing inbox is fine.
    let _ = fs::create_dir_all(crate::inbox::inbox_root(std::path::Path::new(&project_path)));

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

    // **0.37.6:** route to recycle bin instead of permanent unlink.
    // Agent dir contains AGENT.md (user-editable persona file) +
    // potentially user-authored work items, heartbeat config,
    // skill content. Recoverable from Trash if the user changes
    // their mind.
    crate::safe_delete::trash(&dir)
        .map_err(|e| format!("Failed to delete agent: {}", e))?;
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

// Phase 2.1 wrap-up (0.39.0f) — the `work_item_slug` + `body_preview`
// helpers (last used by `workspace_inbox_create`) were removed with
// the function itself. The post-Phase-2.1 inbox primitive has its own
// slug + preview helpers in `k2so_core::inbox`.

// Phase 2.1c Item 2 — `work_list`, `work_create`, `work_move`, and
// `workspace_inbox_list` removed (zero remaining callers; the
// renderer migrated to `k2so_core::inbox::*` via the new
// `commands::inbox::k2so_inbox_*` Tauri shims). The daemon CLI
// surface for these had already been hard-deprecated in Phase 2.1b.
//
// Phase 2.1 wrap-up (0.39.0f) — `workspace_inbox_create` removed.
// Its sole caller (the daemon's `workspace_msg::deliver_to_inbox`)
// was retired in the same commit. The function wrote to the legacy
// `.k2so/work/inbox/` layout, which the post-Phase-2.1 migration
// hook (in `inbox::migrate_work_to_inbox`) now sends to the macOS
// Recycle Bin. New inbox-delivery callers should use
// `k2so_core::inbox::compose` so they land in `.k2so/inbox/` where
// the renderer + CLI actually read from.

#[cfg(test)]
mod tests {
    use super::*;

    // `work_item_slug_*` and `body_preview_*` tests removed alongside
    // their helpers in the Phase 2.1 wrap-up. Equivalent coverage for
    // the `k2so_core::inbox` slug/preview helpers lives in that
    // module's own test suite.

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

    // ── 0.37.0 post-unification idempotency ──────────────────────
    //
    // The AIFileEditor "Manage Persona" button calls `create()` to
    // ensure the agent exists before opening the edit flow. Pre-fix,
    // post-unification workspaces (where `.k2so/agent/AGENT.md`
    // exists) errored "Agent already exists" because agent_dir's
    // layout-aware probe correctly resolved to the unified path —
    // which always exists post-migration. Tests below pin the
    // short-circuit: when the unified primary's AGENT.md is on
    // disk, create() returns Ok with the existing K2soAgentInfo
    // instead of erroring.

    fn temp_workspace_with_unified_agent(persona_type: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "k2so-commands-create-test-{}-{}-{}",
            persona_type,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join(".k2so/agent")).unwrap();
        let body = format!("---\nname: cortana\ntype: {}\n---\n# persona body\n", persona_type);
        std::fs::write(dir.join(".k2so/agent/AGENT.md"), body).unwrap();
        dir
    }

    #[test]
    fn create_short_circuits_when_unified_primary_exists() {
        let dir = temp_workspace_with_unified_agent("custom");
        let path = dir.to_string_lossy().into_owned();

        // Pre-fix this would error "Agent 'cortana' already exists"
        // because agent_dir resolved to .k2so/agent/ which has
        // AGENT.md. Post-fix it should return Ok with the existing
        // info (read from the on-disk frontmatter).
        let result = create(
            path.clone(),
            "cortana".to_string(),
            "test role".to_string(),
            None,
            Some("custom".to_string()),
        );
        assert!(result.is_ok(), "create should short-circuit, got: {result:?}");
        let info = result.unwrap();
        assert_eq!(info.name, "cortana");
        assert_eq!(info.agent_type, "custom",
            "agent_type must come from the on-disk AGENT.md frontmatter");
        assert!(!info.is_manager, "custom-type primary is not a manager");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn create_short_circuit_recognizes_manager_type() {
        let dir = temp_workspace_with_unified_agent("manager");
        let path = dir.to_string_lossy().into_owned();

        let result = create(
            path,
            "anything".to_string(), // name doesn't matter; workspace primary wins
            "test role".to_string(),
            None,
            None,
        );
        assert!(result.is_ok());
        let info = result.unwrap();
        assert_eq!(info.agent_type, "manager",
            "frontmatter type='manager' must propagate");
        assert!(info.is_manager, "manager type should set is_manager=true");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn create_does_not_short_circuit_pre_unification() {
        // No .k2so/agent/AGENT.md on disk → workspace hasn't been
        // migrated yet. create() should fall through to its
        // legacy path (which writes to .k2so/agents/<name>/).
        // Verify create() succeeds (writes AGENT.md to the legacy
        // path) for a fresh workspace.
        let dir = std::env::temp_dir().join(format!(
            "k2so-commands-create-pre-unif-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.to_string_lossy().into_owned();

        // Sanity: no unified path exists pre-call.
        assert!(!dir.join(".k2so/agent/AGENT.md").exists());

        let result = create(
            path.clone(),
            "fresh-agent".to_string(),
            "test role".to_string(),
            None,
            Some("custom".to_string()),
        );
        assert!(result.is_ok(), "fresh create should succeed: {result:?}");

        // Post-call the legacy path got written.
        assert!(
            dir.join(".k2so/agents/fresh-agent/AGENT.md").exists(),
            "legacy path .k2so/agents/<name>/AGENT.md should be written when no unified primary"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}

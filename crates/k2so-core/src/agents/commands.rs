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
//! - **Wakeup + backup helpers** used by the commands above:
//!   [`ensure_agent_wakeup`], [`cleanup_agent_backups`].
//!
//! Phase 2.1c Item 2 — removed the per-agent work-queue functions
//! (`work_list`, `work_create`, `work_move`) and the legacy workspace
//! `workspace_inbox_list`. They had no remaining callers after the
//! React frontend migrated to the workspace inbox primitive
//! (`k2so_core::inbox::*`).
//!
//! Phase 2.1 wrap-up (0.39.0f) — removed `workspace_inbox_create`
//! (the legacy `.k2so/work/inbox/` writer). Its sole caller, the
//! daemon's `workspace_msg::deliver_to_inbox`, was retired in the
//! same commit. New inbox-delivery callers should compose against
//! `k2so_core::inbox::compose` directly so they land in the
//! canonical `.k2so/inbox/` primitive the renderer reads.
//!
//! Every function is host-agnostic — uses `db::shared()` +
//! `fs_atomic::*` + core agent-system primitives, no AppHandle, no
//! Tauri command macros.

use std::fs;

use crate::agents::work_item::atomic_write;
use crate::agents::{agent_dir, parse_frontmatter};
use crate::db::schema::WorkspaceSession;






// Phase 2.5d: back-compat re-exports. The agent CRUD cluster moved to
// `crate::workspace::agent`; per-agent heartbeat control moved to
// `crate::heartbeats::control`. Existing call sites in src-tauri and
// the daemon still spell these paths through `crate::agents::commands`
// — the aliases keep them working through Tier B. Retire together with
// `agents/` in Tier C.
pub use crate::workspace::agent::{
    cleanup_agent_backups, create, delete, delete_inner, get_profile, list, log_agent_warning,
    update_agent_md_field, update_field, update_profile, K2soAgentInfo,
};
pub use crate::heartbeats::control::{
    ensure_agent_wakeup, get_heartbeat, heartbeat_action, heartbeat_noop, set_heartbeat,
};

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




// ── Skill regeneration ─────────────────────────────────────────────────
//
// Walks every agent directory, reads its AGENT.md frontmatter, and
// writes a fresh SKILL.md via the upgrade protocol. Also fans each
// agent's skill out to every harness discovery path via
// `write_skill_to_all_harnesses` so Claude Code / OpenCode / Pi pick
// it up on the next session.

use crate::agents::skill::{
    ensure_skill_up_to_date, SKILL_VERSION_CUSTOM_AGENT, SKILL_VERSION_K2SO_AGENT,
    SKILL_VERSION_MANAGER, SKILL_VERSION_TEMPLATE,
};
use crate::agents::skill_content::{
    generate_custom_agent_skill_content, generate_k2so_agent_skill_content,
    generate_manager_skill_content, generate_template_skill_content,
};
use crate::agents::skill_writer::write_skill_to_all_harnesses;

/// Regenerate SKILL.md files for every agent in a workspace. Called
/// on app startup (migration sweep) and via `k2so skills regenerate`
/// from the CLI.
///
/// Returns `{ "updated": N }` on success. Missing `.k2so/agents/` is
/// a benign no-op that returns `{ "updated": 0 }`.
pub fn regenerate_skills(project_path: String) -> Result<serde_json::Value, String> {
    let agents_root = std::path::PathBuf::from(&project_path).join(".k2so/agents");
    if !agents_root.exists() {
        return Ok(serde_json::json!({"updated": 0}));
    }

    let project_name = std::path::Path::new(&project_path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "workspace".to_string());

    let mut updated = 0;
    if let Ok(entries) = fs::read_dir(&agents_root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();

            let agent_md = path.join("AGENT.md");
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
                        let is_mgr = fm.get("manager").map(|v| v == "true").unwrap_or(false)
                            || fm.get("coordinator").map(|v| v == "true").unwrap_or(false)
                            || fm.get("pod_leader").map(|v| v == "true").unwrap_or(false);
                        if is_mgr {
                            "manager".to_string()
                        } else {
                            "agent-template".to_string()
                        }
                    }
                }
            } else {
                "agent-template".to_string()
            };

            let (skill_content, skill_type_tag, skill_version) = match agent_type.as_str() {
                "manager" => (
                    generate_manager_skill_content(&project_path, &project_name),
                    "manager",
                    SKILL_VERSION_MANAGER,
                ),
                "k2so" => (
                    generate_k2so_agent_skill_content(&project_name, &name),
                    "k2so-agent",
                    SKILL_VERSION_K2SO_AGENT,
                ),
                "custom" => (
                    generate_custom_agent_skill_content(&project_name, &name),
                    "custom-agent",
                    SKILL_VERSION_CUSTOM_AGENT,
                ),
                _ => (
                    generate_template_skill_content(&project_name, &name),
                    "agent-template",
                    SKILL_VERSION_TEMPLATE,
                ),
            };

            let skill_path = path.join("SKILL.md");
            ensure_skill_up_to_date(
                &skill_path,
                skill_type_tag,
                skill_version,
                &skill_content,
                None,
            );
            updated += 1;

            let description = match agent_type.as_str() {
                "manager" => format!("K2SO Workspace Manager commands for {}", name),
                "k2so" => format!("K2SO Agent commands for {} — full surface", name),
                "custom" => format!("K2SO agent commands for {}", name),
                _ => format!("K2SO agent template commands for {}", name),
            };
            write_skill_to_all_harnesses(
                &project_path,
                &format!("k2so-{}", name),
                skill_type_tag,
                skill_version,
                &description,
                &skill_content,
                false,
            );
        }
    }

    Ok(serde_json::json!({"updated": updated}))
}

// ══════════════════════════════════════════════════════════════════════
// Agent editor + persona save — Phase 2 Unit 7d
// ══════════════════════════════════════════════════════════════════════
//
// Moved from `src-tauri/src/commands/k2so_agents.rs`. The Tauri-side
// `#[tauri::command]` wrappers now forward into the functions below.

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

// ══════════════════════════════════════════════════════════════════════
// Workspace session + relations DB accessors — Phase 2 Unit 7d
// ══════════════════════════════════════════════════════════════════════
//
// Moved from `src-tauri/src/commands/k2so_agents.rs`. Pre-Unit-7d
// these used `state.db.lock()` (Tauri's `AppState`); post-7d they use
// the shared `db::shared()` handle directly so the daemon can serve
// the same calls without the Tauri state container.

/// Fetch the workspace_sessions row for a project, if one exists.
/// Returns `None` for projects with no pinned chat session yet.
pub fn workspace_session_get(
    project_id: String,
) -> Result<Option<WorkspaceSession>, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    WorkspaceSession::get(&conn, &project_id).map_err(|e| e.to_string())
}

/// List every workspace_relations row where the given project is the
/// SOURCE (this workspace "oversees" / "depends-on" the targets).
pub fn workspace_relations_list(
    project_id: String,
) -> Result<Vec<crate::db::schema::WorkspaceRelation>, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    crate::db::schema::WorkspaceRelation::list_for_source(&conn, &project_id)
        .map_err(|e| e.to_string())
}

/// List every workspace_relations row where the given project is the
/// TARGET (other workspaces "oversee" this one).
pub fn workspace_relations_list_incoming(
    project_id: String,
) -> Result<Vec<crate::db::schema::WorkspaceRelation>, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    crate::db::schema::WorkspaceRelation::list_for_target(&conn, &project_id)
        .map_err(|e| e.to_string())
}

/// Create a new workspace_relations row.
pub fn workspace_relations_create(
    source_project_id: String,
    target_project_id: String,
    relation_type: Option<String>,
) -> Result<crate::db::schema::WorkspaceRelation, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let id = uuid::Uuid::new_v4().to_string();
    let rel_type = relation_type.unwrap_or_else(|| "oversees".to_string());
    crate::db::schema::WorkspaceRelation::create(
        &conn,
        &id,
        &source_project_id,
        &target_project_id,
        &rel_type,
    )
    .map_err(|e| e.to_string())?;
    Ok(crate::db::schema::WorkspaceRelation {
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

/// Delete a workspace_relations row by id.
pub fn workspace_relations_delete(id: String) -> Result<(), String> {
    let db = crate::db::shared();
    let conn = db.lock();
    crate::db::schema::WorkspaceRelation::delete(&conn, &id).map_err(|e| e.to_string())?;
    Ok(())
}

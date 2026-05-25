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

use crate::agents::parse_frontmatter;






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
pub use crate::workspace::agent_editor::{
    k2so_agents_get_editor_context, k2so_agents_preview_agent_context,
    k2so_agents_regenerate_agent_context, k2so_agents_save_agent_md,
};
pub use crate::workspace::relations::{
    workspace_relations_create, workspace_relations_delete, workspace_relations_list,
    workspace_relations_list_incoming, workspace_session_get,
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

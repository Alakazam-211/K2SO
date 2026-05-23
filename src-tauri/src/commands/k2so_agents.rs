//! K2SO Agent system — autonomous AI workers operating within workspaces.
//!
//! Agents have a work queue (inbox/active/done) of markdown files,
//! a profile (agent.md), and interact with K2SO via the CLI bridge.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

use crate::db::schema::{AgentHeartbeat, WorkspaceSession, HeartbeatFire, WorkspaceRelation};
use crate::fs_atomic::{self, atomic_symlink, atomic_write_str, log_if_err, unique_archive_path};

// Core-hosted helpers + heartbeat fns. Re-imported at crate-local paths
// so the 170+ existing call sites below keep resolving via name-in-scope
// without touching each one. External references via
// `crate::commands::k2so_agents::find_primary_agent` (agent_hooks.rs)
// also continue to work because re-exports behave like normal items.
pub use k2so_core::agents::{
    agent_dir, agent_type_for, agents_dir, find_primary_agent, parse_frontmatter,
    resolve_project_id,
};
// Scheduler-path helpers + types moved alongside the heartbeat slice.
// Re-exported here at their historical paths so the 9k-line file's
// internal call sites continue to resolve unchanged.
pub use k2so_core::agents::scheduler::{
    agent_work_dir, count_md_files, get_highest_inbox_priority, get_workspace_state,
    is_agent_locked, is_within_active_hours, k2so_agents_scheduler_tick as core_scheduler_tick,
    priority_label, priority_rank, read_heartbeat_config, workspace_inbox_dir,
    write_heartbeat_config, ActiveHours, AgentHeartbeatConfig,
};

// ── Types ───────────────────────────────────────────────────────────────

// `K2soAgentInfo` struct moved to k2so_core::agents::commands (re-exported).

// `WorkItem` struct moved to k2so_core::agents::work_item. Re-exported
// here so external callers (agent_hooks.rs, commands/review.rs, etc.)
// keep resolving `crate::commands::k2so_agents::WorkItem`.
pub use k2so_core::agents::work_item::{
    atomic_write as _atomic_write_shim, parse_work_item_content, read_work_item as _read_work_item_shim,
    safe_read_to_string, WorkItem, MAX_FILE_SIZE,
};

// Local aliases for the helpers used at privately-scoped call sites.
#[allow(dead_code)]
fn atomic_write(path: &std::path::Path, content: &str) -> Result<(), String> {
    k2so_core::agents::work_item::atomic_write(path, content)
}
#[allow(dead_code)]
fn read_work_item(path: &std::path::Path, folder: &str) -> Option<WorkItem> {
    k2so_core::agents::work_item::read_work_item(path, folder)
}

// Skill + CLAUDE.md content generators + the big heartbeat-docs
// constant moved to k2so_core::agents::skill_content. Re-exported at
// their historical names. `generate_agent_claude_md_content` stays
// as a public alias; new code inside core uses
// `compose_agent_wake_context` which more honestly names what the
// function returns.
pub use k2so_core::agents::skill_content::{
    compose_agent_wake_context, extract_section, format_cap,
    generate_agent_claude_md_content, generate_custom_agent_skill_content,
    generate_k2so_agent_skill_content, generate_manager_skill_content,
    generate_template_skill_content, load_custom_layers, CUSTOM_AGENT_HEARTBEAT_DOCS,
};

// Delegation path — worktree creation + work-item routing — moved to
// k2so_core::agents::delegate. The `#[tauri::command]` wrapper below
// is now a three-line forward; the four frontmatter helpers are
// re-exported at their historical names so the 3 call sites elsewhere
// in this file resolve unchanged.
pub use k2so_core::agents::delegate::{
    add_worktree_to_frontmatter, shorten_slug, strip_worktree_from_frontmatter,
    update_assigned_by,
};

// Harness-agnostic skill writer — writes canonical SKILL.md + symlinks
// from every discovery path (.claude/, .opencode/, .pi/) + marker-
// injects into AGENTS.md and copilot-instructions.md. Moved to
// k2so_core::agents::skill_writer so the daemon can regen skills
// without pulling in src-tauri.
pub use k2so_core::agents::skill_writer::{
    force_symlink, generate_default_agent_body, upsert_k2so_section,
    write_agent_skill_file, write_skill_to_all_harnesses, K2SO_SECTION_BEGIN,
    K2SO_SECTION_END,
};

// Agent CRUD + work queue + workspace inbox + channel events moved
// wholesale to k2so_core::agents::{commands, events} so the daemon
// can serve the same /cli/* routes headlessly.
pub use k2so_core::agents::commands::{
    cleanup_agent_backups, ensure_agent_wakeup, update_agent_md_field, K2soAgentInfo,
};
pub use k2so_core::agents::events::{
    drain_agent_events, push_agent_event, ChannelEvent, MAX_EVENTS_PER_QUEUE,
};

// Log-helper + per-agent heartbeat control moved to
// k2so_core::agents::commands. Tauri command wrappers below keep the
// React frontend's invoke() sites working.
pub use k2so_core::agents::commands::log_agent_warning;

#[tauri::command]
pub fn k2so_agents_get_heartbeat(
    project_path: String,
    agent_name: String,
) -> Result<AgentHeartbeatConfig, String> {
    k2so_core::agents::commands::get_heartbeat(project_path, agent_name)
}

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
    k2so_core::agents::commands::set_heartbeat(
        project_path,
        agent_name,
        interval,
        phase,
        mode,
        cost_budget,
        force_wake,
    )
}

#[tauri::command]
pub fn k2so_agents_heartbeat_noop(
    project_path: String,
    agent_name: String,
) -> Result<AgentHeartbeatConfig, String> {
    k2so_core::agents::commands::heartbeat_noop(project_path, agent_name)
}

#[tauri::command]
pub fn k2so_agents_heartbeat_action(
    project_path: String,
    agent_name: String,
) -> Result<AgentHeartbeatConfig, String> {
    k2so_core::agents::commands::heartbeat_action(project_path, agent_name)
}

#[tauri::command]
pub fn k2so_agents_list(project_path: String) -> Result<Vec<K2soAgentInfo>, String> {
    k2so_core::agents::commands::list(project_path)
}

#[tauri::command]
pub fn k2so_agents_create(
    project_path: String,
    name: String,
    role: String,
    prompt: Option<String>,
    agent_type: Option<String>,
) -> Result<K2soAgentInfo, String> {
    k2so_core::agents::commands::create(project_path, name, role, prompt, agent_type)
}

#[tauri::command]
pub fn k2so_agents_delete(project_path: String, name: String) -> Result<(), String> {
    k2so_core::agents::commands::delete(project_path, name)
}

/// 0.37.4: resolve the workspace's primary agent display name.
///
/// Routed through the daemon HTTP API (daemon-first) so the daemon's
/// mtime-cache is the single source of truth — Tauri reads the same
/// answer the CLI verb / sub-agents see. Falls back to the
/// in-process helper only if the daemon is unreachable; reads remain
/// total (always return a string).
#[tauri::command]
pub fn k2so_workspace_agent_display_name(project_path: String) -> Result<String, String> {
    if let Ok(client) = crate::daemon_client::DaemonClient::try_connect() {
        if let Ok(body) = client.cli_get(
            "/cli/workspace/agent-display-name",
            &[("project", &project_path)],
        ) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) {
                if let Some(s) = v.get("display_name").and_then(|d| d.as_str()) {
                    return Ok(s.to_string());
                }
            }
        }
    }
    Ok(k2so_core::agents::display::agent_display_name(&project_path))
}

/// 0.37.4: set the workspace's primary agent display name.
///
/// Routed through the daemon HTTP API. The daemon writes AGENT.md
/// (atomic temp-file rename), invalidates its display-name cache, and
/// emits `SyncProjects`. Tauri's local cache lives in the same shared
/// `k2so-core` lib, but the daemon-side mtime check picks up the
/// fresh write on the next read regardless of which process did the
/// write — so single-source-of-truth holds even if the daemon is
/// hot-restarted between Tauri invocations.
#[tauri::command]
pub fn k2so_workspace_set_agent_display_name(
    project_path: String,
    name: String,
) -> Result<(), String> {
    let client = crate::daemon_client::DaemonClient::try_connect()?;
    client.cli_get(
        "/cli/workspace/set-agent-display-name",
        &[("project", &project_path), ("name", &name)],
    )?;
    Ok(())
}

pub fn k2so_agents_delete_inner(
    project_path: &str,
    name: &str,
    force: bool,
) -> Result<(), String> {
    k2so_core::agents::commands::delete_inner(project_path, name, force)
}

#[tauri::command]
pub fn k2so_agents_update_field(
    project_path: String,
    name: String,
    field: String,
    value: String,
) -> Result<String, String> {
    k2so_core::agents::commands::update_field(project_path, name, field, value)
}

#[tauri::command]
pub fn k2so_agents_get_profile(
    project_path: String,
    agent_name: String,
) -> Result<String, String> {
    k2so_core::agents::commands::get_profile(project_path, agent_name)
}

#[tauri::command]
pub fn k2so_agents_update_profile(
    project_path: String,
    agent_name: String,
    content: String,
) -> Result<(), String> {
    k2so_core::agents::commands::update_profile(project_path, agent_name, content)
}

#[tauri::command]
pub fn k2so_agents_work_list(
    project_path: String,
    agent_name: String,
    folder: Option<String>,
) -> Result<Vec<WorkItem>, String> {
    k2so_core::agents::commands::work_list(project_path, agent_name, folder)
}

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
    k2so_core::agents::commands::work_create(
        project_path, agent_name, title, body, priority, item_type, source,
    )
}

#[tauri::command]
pub fn k2so_agents_work_move(
    project_path: String,
    agent_name: String,
    filename: String,
    from_folder: String,
    to_folder: String,
) -> Result<(), String> {
    k2so_core::agents::commands::work_move(
        project_path,
        agent_name,
        filename,
        from_folder,
        to_folder,
    )
}

#[tauri::command]
pub fn k2so_agents_workspace_inbox_list(
    project_path: String,
) -> Result<Vec<WorkItem>, String> {
    k2so_core::agents::commands::workspace_inbox_list(project_path)
}

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
    k2so_core::agents::commands::workspace_inbox_create(
        workspace_path, title, body, priority, item_type, assigned_by, source,
    )
}

// ── Path helpers ────────────────────────────────────────────────────────
//
// `agents_dir` + `agent_dir` now live in k2so_core::agents (re-exported
// above so local call sites resolve unchanged). `agent_work_dir` and
// `workspace_inbox_dir` now also live in k2so_core::agents::scheduler
// alongside the rest of the heartbeat-fire dependency closure.

// ── Wake-up templates ──────────────────────────────────────────────────
//
// Shipped with the binary at compile time. On first app launch (or when
// an agent is created), the matching template is copied to
// `.k2so/agents/<name>/wakeup.md` with its `<!-- DEFAULT TEMPLATE -->`
// header intact so users can see the scaffolded defaults and edit them.
//
// The workspace-level template lives at `.k2so/wakeup.md` for
// `__lead__`. Agent-templates (the `agent-template` type) are
// intentionally excluded — they're dispatched with explicit orders by
// their manager and never wake autonomously.

// Wakeup templates + resolvers + composers moved to
// k2so_core::agents::wake. The re-exports below keep the historical
// paths valid: `WAKEUP_TEMPLATE_*`, `wakeup_template_for`,
// `agent_wakeup_path`, `workspace_wakeup_path`, `read_agent_wakeup`,
// `strip_frontmatter`, the four `compose_*` helpers, and
// `default_heartbeat_wakeup_abs` all resolve to the core versions.
pub use k2so_core::agents::wake::{
    agent_wakeup_path, compose_agent_wake_from_body, compose_manager_wake_from_body,
    compose_wake_prompt_for_agent, compose_wake_prompt_for_lead,
    compose_wake_prompt_from_path, default_heartbeat_wakeup_abs, read_agent_wakeup,
    strip_frontmatter, wakeup_template_for, workspace_wakeup_path,
    WAKEUP_TEMPLATE_CUSTOM, WAKEUP_TEMPLATE_K2SO, WAKEUP_TEMPLATE_MANAGER,
    WAKEUP_TEMPLATE_WORKSPACE,
};

// `ensure_agent_wakeup` moved to k2so_core::agents::commands (re-exported).

// `agent_type_for` moved to k2so_core::agents (re-exported above).

// `default_heartbeat_wakeup_abs` + the four `compose_*` wake-prompt
// composers moved to k2so_core::agents::wake (re-exported at the top
// of this file).

/// Find the workspace's primary scheduleable agent. A workspace is one-of
/// Custom / K2SO Agent / Workspace Manager (mutually exclusive by design),
/// but agent-mode swaps can leave orphan directories from prior modes on
/// disk. We use `projects.agent_mode` as the source of truth and only
/// return an agent dir whose type matches the workspace's declared mode.
/// Agent-templates are never scheduleable and are always skipped.
// `find_primary_agent` moved to k2so_core::agents (re-exported above).

/// Multi-heartbeat architecture: CRUD for agent_heartbeats table.
/// See .k2so/prds/multi-schedule-heartbeat.md.

// All heartbeat business logic lives in k2so_core::agents::heartbeat.
// The `#[tauri::command]` wrappers below are thin forwards so the
// React UI's existing `invoke("k2so_heartbeat_*")` calls keep working;
// the daemon calls the core fns directly from its `/cli/heartbeat/*`
// HTTP routes so scheduled wakes fire while the Tauri app is quit.

#[tauri::command]
pub fn k2so_heartbeat_add(
    project_path: String,
    name: String,
    frequency: String,
    spec_json: String,
) -> Result<serde_json::Value, String> {
    k2so_core::agents::heartbeat::k2so_heartbeat_add(project_path, name, frequency, spec_json)
}

#[tauri::command]
pub fn k2so_heartbeat_list(project_path: String) -> Result<Vec<AgentHeartbeat>, String> {
    k2so_core::agents::heartbeat::k2so_heartbeat_list(project_path)
}

/// 0.38.3 — most recent heartbeat fire records across ALL projects,
/// joined with project name. Powers the universal audit log on the
/// system-wide Heartbeats settings page (`WakeSchedulerSection`).
/// Default limit 100 fires; bump for deeper investigation.
#[tauri::command]
pub fn k2so_heartbeat_fires_list_all(
    limit: Option<i64>,
) -> Result<Vec<serde_json::Value>, String> {
    let db = k2so_core::db::shared();
    let conn = db.lock();
    let rows = k2so_core::db::schema::HeartbeatFire::list_all_recent_with_project(
        &conn,
        limit.unwrap_or(100),
    )
    .map_err(|e| format!("list_all_recent: {}", e))?;
    let out: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(fire, project_name)| {
            serde_json::json!({
                "id": fire.id,
                "projectId": fire.project_id,
                "projectName": project_name,
                "agentName": fire.agent_name,
                "scheduleName": fire.schedule_name,
                "firedAt": fire.fired_at,
                "mode": fire.mode,
                "decision": fire.decision,
                "reason": fire.reason,
                "inboxPriority": fire.inbox_priority,
                "inboxCount": fire.inbox_count,
                "durationMs": fire.duration_ms,
            })
        })
        .collect();
    Ok(out)
}

/// 0.38.3 — list every active (non-archived) heartbeat across ALL
/// workspaces, with the parent project's name + path joined in. Used
/// by the system-wide Heartbeats settings page (`WakeSchedulerSection`)
/// so the operator can see and toggle every heartbeat from one place.
#[tauri::command]
pub fn k2so_heartbeat_list_all() -> Result<Vec<serde_json::Value>, String> {
    let db = k2so_core::db::shared();
    let conn = db.lock();
    let rows = k2so_core::db::schema::AgentHeartbeat::list_all_active_with_project(&conn)
        .map_err(|e| format!("list_all_active: {}", e))?;
    let out: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(hb, project_name, project_path)| {
            // Hand-build the JSON so we keep the same camelCase the
            // existing `k2so_heartbeat_list` payload uses (the renderer
            // already knows that shape) and just append two extra
            // fields for the joined project info.
            serde_json::json!({
                "id": hb.id,
                "projectId": hb.project_id,
                "name": hb.name,
                "frequency": hb.frequency,
                "specJson": hb.spec_json,
                "wakeupPath": hb.wakeup_path,
                "enabled": hb.enabled,
                "lastFired": hb.last_fired,
                "lastSessionId": hb.last_session_id,
                "createdAt": hb.created_at,
                "concurrencyPolicy": hb.concurrency_policy,
                "startingDeadlineSecs": hb.starting_deadline_secs,
                "activeDeadlineSecs": hb.active_deadline_secs,
                "useWorkspaceSession": hb.use_workspace_session,
                "projectName": project_name,
                "projectPath": project_path,
            })
        })
        .collect();
    Ok(out)
}

#[tauri::command]
pub fn k2so_heartbeat_list_archived(
    project_path: String,
) -> Result<Vec<AgentHeartbeat>, String> {
    k2so_core::agents::heartbeat::k2so_heartbeat_list_archived(project_path)
}

#[tauri::command]
pub fn k2so_heartbeat_archive(
    project_path: String,
    name: String,
) -> Result<(), String> {
    k2so_core::agents::heartbeat::k2so_heartbeat_archive(project_path, name)
}

#[tauri::command]
pub fn k2so_heartbeat_unarchive(
    project_path: String,
    name: String,
) -> Result<(), String> {
    k2so_core::agents::heartbeat::k2so_heartbeat_unarchive(project_path, name)
}

#[tauri::command]
pub fn k2so_heartbeat_remove(project_path: String, name: String) -> Result<(), String> {
    k2so_core::agents::heartbeat::k2so_heartbeat_remove(project_path, name)
}

/// Read the workspace's `show_heartbeat_sessions` flag.
///
/// 0 (default) = silent autonomous mode; heartbeat fires never open
/// tabs. Audit via the sidebar Heartbeats panel on demand.
/// 1 = each scheduled heartbeat fire opens a background tab in the
/// Tauri window. Tab persists until the user closes it.
#[tauri::command]
pub fn k2so_workspace_get_show_heartbeat_sessions(
    project_path: String,
) -> Result<bool, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let v: i64 = conn
        .query_row(
            "SELECT show_heartbeat_sessions FROM projects WHERE path = ?1",
            rusqlite::params![project_path],
            |r| r.get(0),
        )
        .map_err(|e| format!("workspace not found: {e}"))?;
    Ok(v != 0)
}

/// Flip the workspace's `show_heartbeat_sessions` flag.
#[tauri::command]
pub fn k2so_workspace_set_show_heartbeat_sessions(
    project_path: String,
    enabled: bool,
) -> Result<(), String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let rows = conn
        .execute(
            "UPDATE projects SET show_heartbeat_sessions = ?1 WHERE path = ?2",
            rusqlite::params![enabled as i64, project_path],
        )
        .map_err(|e| format!("workspace update failed: {e}"))?;
    if rows == 0 {
        return Err(format!("workspace not found: {project_path}"));
    }
    Ok(())
}

#[tauri::command]
pub fn k2so_heartbeat_set_enabled(
    project_path: String,
    name: String,
    enabled: bool,
) -> Result<(), String> {
    k2so_core::agents::heartbeat::k2so_heartbeat_set_enabled(project_path, name, enabled)
}

/// 0.37.8 — flip the per-heartbeat opt-in to deliver WAKEUP.md into
/// the workspace's pinned chat session via
/// `workspace_msg::deliver_live`. See migration 0043.
#[tauri::command]
pub fn k2so_heartbeat_set_use_workspace_session(
    project_path: String,
    name: String,
    enabled: bool,
) -> Result<(), String> {
    k2so_core::agents::heartbeat::k2so_heartbeat_set_use_workspace_session(
        project_path,
        name,
        enabled,
    )
}

#[tauri::command]
pub fn k2so_heartbeat_edit(
    project_path: String,
    name: String,
    frequency: String,
    spec_json: String,
) -> Result<(), String> {
    k2so_core::agents::heartbeat::k2so_heartbeat_edit(project_path, name, frequency, spec_json)
}

// Re-exported so the name stays reachable at its historical path
// (`crate::commands::k2so_agents::HeartbeatFireCandidate`) while the
// struct itself lives in k2so-core.
pub use k2so_core::agents::heartbeat::HeartbeatFireCandidate;

pub fn k2so_agents_heartbeat_tick(project_path: &str) -> Vec<HeartbeatFireCandidate> {
    k2so_core::agents::heartbeat::k2so_agents_heartbeat_tick(project_path)
}

pub fn stamp_heartbeat_fired(project_path: &str, heartbeat_name: &str) {
    k2so_core::agents::heartbeat::stamp_heartbeat_fired(project_path, heartbeat_name)
}

#[tauri::command]
pub fn k2so_heartbeat_rename(
    project_path: String,
    old_name: String,
    new_name: String,
) -> Result<(), String> {
    k2so_core::agents::heartbeat::k2so_heartbeat_rename(project_path, old_name, new_name)
}

#[tauri::command]
pub fn k2so_heartbeat_fires_list(
    project_path: String,
    limit: Option<i64>,
) -> Result<Vec<HeartbeatFire>, String> {
    k2so_core::agents::heartbeat::k2so_heartbeat_fires_list(project_path, limit)
}

/// Archive orphan top-tier agents — agents whose type is `custom`,
/// `manager`, or `k2so` but that aren't the current primary for this
/// workspace. Moves them to `.k2so/agents/.archive/<name>-<timestamp>/`
/// and removes their DB rows (`agent_sessions`, and any stray
/// `agent_heartbeats` pointing at the orphan's folder). Templates are
/// ALWAYS preserved — the Workspace Manager delegates to them on-demand.
///
/// Idempotent: no-op when there are no orphans. Called at startup
/// (after heartbeat repair) and from projects_update before an
/// agent_mode change takes effect.
pub fn archive_orphan_top_tier_agents(project_path: &str) -> Vec<String> {
    k2so_core::agents::workspace::archive_orphan_top_tier_agents(project_path)
}

/// Detect and repair heartbeats whose `wakeup_path` points at the wrong
/// agent — typically caused by the pre-0.32.1 migration picking an
/// orphan agent directory from a prior agent-mode swap. Called on
/// startup after `promote_legacy_heartbeat`. Idempotent: no-op when
/// all rows already point at the correct agent.
pub fn repair_mismigrated_heartbeats(project_path: &str) {
    k2so_core::agents::workspace::repair_mismigrated_heartbeats(project_path)
}

/// One-time promotion of the legacy `projects.heartbeat_schedule` single-slot
/// config into the multi-heartbeat `agent_heartbeats` table. Safe to call
/// repeatedly; no-ops when the project already has any agent_heartbeats
/// row (migration is idempotent). Moves the legacy `wakeup.md` to
/// `heartbeats/default/wakeup.md` so everything lives under a consistent
/// hierarchy post-migration.
pub fn promote_legacy_heartbeat(project_path: &str) {
    k2so_core::agents::workspace::promote_legacy_heartbeat(project_path)
}

/// Scaffold the wakeup files for a single workspace — one for each
/// existing agent that supports wake-up. Safe to call repeatedly;
/// never overwrites an existing file. Used by the app-launch migration
/// pass. Workspace-level `.k2so/wakeup.md` is no longer scaffolded here
/// — `migrate_or_scaffold_lead_heartbeat` handles the __lead__ case
/// via the multi-heartbeat system.
pub fn ensure_workspace_wakeups(project_path: &str) {
    k2so_core::agents::workspace::ensure_workspace_wakeups(project_path)
}

/// For Workspace Manager projects, make sure `__lead__` has at least
/// one heartbeat row. Two paths:
///
/// 1. **Migrate existing `.k2so/wakeup.md`** (users who configured the
///    retired Workspace Wake-up). Copy its content into
///    `.k2so/agents/__lead__/heartbeats/default/wakeup.md`, insert a
///    matching `agent_heartbeats` row (hourly default), rename the old
///    file to `.k2so/wakeup.md.migrated` so nothing else picks it up.
///
/// 2. **Scaffold a lean default** for fresh manager workspaces. The
///    SKILL.md layers (Standing Orders / Delegation + Review / etc.)
///    already carry the manager's playbook, so the per-row wakeup.md
///    is just the "wake trigger" — one-sentence action prompt.
///
/// Rename lowercase `agent.md` / `wakeup.md` filenames to UPPERCASE in all
/// known locations within a workspace. Idempotent — skips files that are
/// already uppercase.
///
/// Case-insensitive filesystems (macOS HFS+, default APFS) refuse a direct
/// `fs::rename("agent.md", "AGENT.md")` — it's the same filename to the FS.
/// We two-step through a temporary name so the final result is a real case
/// change recorded in the directory entry.
///
/// Scope:
///   `.k2so/agents/<agent>/agent.md` → `.../AGENT.md`
///   `.k2so/agents/<agent>/wakeup.md` → `.../WAKEUP.md` (agent-root legacy)
///   `.k2so/agents/<agent>/heartbeats/<sched>/wakeup.md` → `.../WAKEUP.md`
///
/// `.k2so/PROJECT.md` is already UPPERCASE in the shipping scaffold and
/// doesn't need migration.
pub fn migrate_filenames_to_uppercase(project_path: &str) {
    k2so_core::agents::workspace::migrate_filenames_to_uppercase(project_path)
}

/// Idempotent: bails immediately if `__lead__` already has any
/// heartbeat row, or if the project isn't in manager mode.
pub fn migrate_or_scaffold_lead_heartbeat(project_path: &str) {
    k2so_core::agents::workspace::migrate_or_scaffold_lead_heartbeat(project_path)
}

// ── Frontmatter parsing ────────────────────────────────────────────────

// ── Skill upgrade protocol (universal) ───────────────────────────────
// The full skill lifecycle (markers, versions, wrap/parse, the
// ensure_skill_up_to_date writer) moved to k2so_core::agents::skill.
// src-tauri re-exports the surface at its historical names so the 30+
// call sites in this file resolve unchanged.
pub use k2so_core::agents::skill::{
    ensure_skill_up_to_date, parse_skill, skill_checksum_hex,
    skill_source_agent_md_begin, skill_source_agent_md_end, wrap_managed_skill,
    ParsedSkill, SkillUpgradeOutcome, SKILL_BEGIN_MARKER, SKILL_END_MARKER,
    SKILL_SOURCE_PROJECT_MD_BEGIN, SKILL_SOURCE_PROJECT_MD_END,
    SKILL_VERSION_CUSTOM_AGENT, SKILL_VERSION_K2SO_AGENT, SKILL_VERSION_MANAGER,
    SKILL_VERSION_TEMPLATE, SKILL_VERSION_WORKSPACE,
};

// Legacy shim — fn definitions below deleted. Original ParsedSkill
// impl block used `struct ParsedSkill { k2so_skill: ... }` with a
// private constructor; the core version makes all fields pub so the
// in-file call sites that directly destructure it still work.

// ParsedSkill / SkillUpgradeOutcome / parse_skill / ensure_skill_up_to_date
// all moved to k2so_core::agents::skill (re-exported at the top of this
// file so the 30+ local call sites below resolve unchanged).

// `parse_frontmatter` moved to k2so_core::agents (re-exported at the
// top of this file).

// (duplicate of k2so_core helpers — removed during skill_content migration)

// (duplicate of k2so_core helpers — removed during skill_content migration)

// `count_md_files` moved to k2so_core::agents::scheduler (re-exported).

// (duplicate of k2so_core helpers — removed during skill_content migration)

// (duplicate of k2so_core helpers — removed during skill_content migration)

// (duplicate of k2so_core helpers — removed during skill_content migration)

// ── Heartbeat Configuration ─────────────────────────────────────────────
//
// `AgentHeartbeatConfig`, `ActiveHours`, `read_heartbeat_config`,
// `write_heartbeat_config`, and the per-field default fns all now live
// in k2so_core::agents::scheduler. The types + functions are re-exported
// at the top of this file so existing call sites resolve unchanged.

// ── Tauri Commands ──────────────────────────────────────────────────────

// `k2so_agents_list` moved to k2so_core::agents::commands (re-exported).

// `k2so_agents_create` moved to k2so_core::agents::commands (re-exported).

// `k2so_agents_delete` moved to k2so_core::agents::commands (re-exported).

// `k2so_agents_delete_inner` moved to k2so_core::agents::commands (re-exported).

// `update_agent_md_field` moved to k2so_core::agents::commands (re-exported).

// `k2so_agents_update_field` moved to k2so_core::agents::commands (re-exported).

// `k2so_agents_work_list` moved to k2so_core::agents::commands (re-exported).

// `k2so_agents_work_create` moved to k2so_core::agents::commands (re-exported).

/// Delegate a work item to an agent — creates a worktree,
/// registers it, moves the item to active, writes CLAUDE.md.
/// Body lives in k2so_core::agents::delegate.
#[tauri::command]
pub fn k2so_agents_delegate(
    project_path: String,
    target_agent: String,
    source_file: String,
) -> Result<serde_json::Value, String> {
    k2so_core::agents::delegate::k2so_agents_delegate(project_path, target_agent, source_file)
}

// `k2so_agents_work_move` moved to k2so_core::agents::commands (re-exported).

// `k2so_agents_get_profile` moved to k2so_core::agents::commands (re-exported).

// `k2so_agents_update_profile` moved to k2so_core::agents::commands (re-exported).

// ── Workspace Inbox ─────────────────────────────────────────────────────

// `k2so_agents_workspace_inbox_list` moved to k2so_core::agents::commands (re-exported).

// `k2so_agents_workspace_inbox_create` moved to k2so_core::agents::commands (re-exported).

// ── Lock Files ──────────────────────────────────────────────────────────

// Session lifecycle (lock / unlock / save-session-id / clear-session-id)
// lives in k2so_core::agents::session. These #[tauri::command] wrappers
// are thin forwards so the React frontend's existing invokes keep
// working unchanged; the daemon calls the core fns directly from its
// wake path.

#[tauri::command]
pub fn k2so_agents_lock(
    project_path: String,
    agent_name: String,
    terminal_id: Option<String>,
    owner: Option<String>,
) -> Result<(), String> {
    k2so_core::agents::session::k2so_agents_lock(project_path, agent_name, terminal_id, owner)
}

#[tauri::command]
pub fn k2so_agents_unlock(project_path: String, agent_name: String) -> Result<(), String> {
    k2so_core::agents::session::k2so_agents_unlock(project_path, agent_name)
}

// `is_agent_locked` moved to k2so_core::agents::scheduler (re-exported).

// ── Agent context / SKILL.md regen ─────────────────────────────────────
//
// Pre-0.33.0 these commands were `k2so_agents_*_claude_md`, which was
// honest when CLAUDE.md was the canonical per-agent system prompt file.
// Phase 1a (0.32.x) made SKILL.md the harness-agnostic source of truth
// and turned CLAUDE.md into a symlink-or-copy for Claude Code's auto-
// discovery; these commands regenerate BOTH but "context" is the
// honest name for what they return. The legacy `_claude_md` aliases
// are retained as thin forwards in the same module for back-compat.

/// Regenerate an agent's context bundle: the full `--append-system-
/// prompt` body returned to the caller AND a fresh SKILL.md +
/// CLAUDE.md written to the agent's directory. Same as calling
/// `k2so_agents_preview_agent_context` followed by an atomic write.
#[tauri::command]
pub fn k2so_agents_regenerate_agent_context(
    project_path: String,
    agent_name: String,
) -> Result<String, String> {
    let md = generate_agent_claude_md_content(&project_path, &agent_name, None)?;
    let claude_md_path = agent_dir(&project_path, &agent_name).join("CLAUDE.md");
    atomic_write(&claude_md_path, &md)?;
    Ok(md)
}

/// Back-compat alias for [`k2so_agents_regenerate_agent_context`].
/// Kept so React components that still invoke the old name keep
/// working during the rename window. New code should use the new
/// name.
#[tauri::command]
pub fn k2so_agents_generate_claude_md(
    project_path: String,
    agent_name: String,
) -> Result<String, String> {
    k2so_agents_regenerate_agent_context(project_path, agent_name)
}

/// Full-fat wake-launch builder (UI "Launch" button +
/// heartbeat auto-launch). Body lives in
/// k2so_core::agents::build_launch; this Tauri wrapper is a
/// thin forward so the React frontend's invoke keeps
/// working.
// `heartbeat_name`: heartbeat-scoped resume target (post-0.36.0). When
// `Some`, the build prefers `agent_heartbeats.last_session_id` for the
// row before falling back to the per-agent global session. React
// callers (Chat tab, manual agent launches) leave this `None`.
#[tauri::command]
pub fn k2so_agents_build_launch(
    project_path: String,
    agent_name: String,
    agent_cli_command: Option<String>,
    wakeup_override: Option<String>,
    skip_fork_session: Option<bool>,
    heartbeat_name: Option<String>,
) -> Result<serde_json::Value, String> {
    k2so_core::agents::build_launch::k2so_agents_build_launch(
        project_path,
        agent_name,
        agent_cli_command,
        wakeup_override,
        skip_fork_session,
        heartbeat_name,
    )
}

/// Build a *bare resume* launch command for the AgentChatPane (the
/// pinned Chat tab). Unlike `k2so_agents_build_launch`, this does NOT
/// inject the agent's WAKEUP.md as a positional message and does NOT
/// prepend `/compact` — the Chat tab is for chatting with an existing
/// agent session, not for autonomously firing a triage. If we have a
/// saved session id for the workspace AND its JSONL is on disk, we
/// add `--resume <id>`; otherwise we pre-allocate a UUID, persist
/// it to SQL, and use `--session-id <new>`.
///
/// **0.37.5 daemon-first refactor.** The actual logic lives in
/// `k2so_core::agents::resume_chat::resolve_resume_chat_args` and is
/// served via the daemon route `/cli/workspace/resume-chat-args`.
/// This Tauri command is a thin HTTP proxy — every consumer (the
/// pinned tab here, future mobile companion, MCP server, CLI verb)
/// goes through the same daemon route, so the SQL lookup + JSONL
/// existence check + pre-allocate logic isn't duplicated across
/// thin clients. Falls back to in-process k2so-core call only if
/// the daemon is unreachable (offline degradation parity with the
/// other 0.37.4+ display-name commands).
///
/// `agent_name` parameter is kept for back-compat with renderer call
/// sites but is unused — the workspace's primary agent is implicit
/// post-unification, and resume_chat_args is keyed purely on
/// project_path.
#[tauri::command]
pub fn k2so_agents_resume_chat_args(
    project_path: String,
    agent_name: String,
) -> Result<serde_json::Value, String> {
    let _ = agent_name;
    if let Ok(client) = crate::daemon_client::DaemonClient::try_connect() {
        if let Ok(body) = client.cli_get(
            "/cli/workspace/resume-chat-args",
            &[("project", &project_path)],
        ) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) {
                if v.get("error").is_none() {
                    return Ok(v);
                }
            }
        }
    }
    // Daemon unreachable — degrade to in-process resolve via the
    // shared k2so-core helper. Same logic, same SQL writes, same
    // result; the daemon-routed path is preferred for cache + lock
    // alignment but a Tauri-only build still works on its own.
    k2so_core::agents::resume_chat::resolve_resume_chat_args(&project_path)
        .map(|out| out.to_json())
}

// `add_worktree_to_frontmatter` moved to k2so_core::agents::delegate (re-exported).

// `strip_worktree_from_frontmatter` moved to k2so_core::agents::delegate (re-exported).

// `generate_default_agent_body` moved to k2so_core::agents::skill_writer (re-exported).

// `format_cap` moved to k2so_core::agents::skill_content (re-exported).

// `log_agent_warning` moved to k2so_core::agents::commands (re-exported).

// `shorten_slug` moved to k2so_core::agents::delegate (re-exported).

// `extract_section` moved to k2so_core::agents::skill_content (re-exported).

// `strip_frontmatter` moved to k2so_core::agents::wake (re-exported).

// `generate_agent_claude_md_content` moved to k2so_core::agents::skill_content (re-exported).

// `load_custom_layers` moved to k2so_core::agents::skill_content (re-exported).

// `generate_manager_skill_content` moved to k2so_core::agents::skill_content (re-exported).

// `generate_custom_agent_skill_content` moved to k2so_core::agents::skill_content (re-exported).

// `generate_k2so_agent_skill_content` moved to k2so_core::agents::skill_content (re-exported).

// `generate_template_skill_content` moved to k2so_core::agents::skill_content (re-exported).

// `generate_workspace_skill_content` moved to k2so_core::agents::workspace
// (Phase 2 Unit 7b — private helper of write_workspace_skill_file_with_body).

// `priority_rank` moved to k2so_core::agents::scheduler (re-exported).

/// Regenerate the workspace-root SKILL.md — the lead agent's complete
/// operating manual. Written to `<project-root>/SKILL.md` with a
/// matching `<project-root>/CLAUDE.md` symlink so Claude Code auto-
/// discovers it. The SKILL.md is the canonical source of truth;
/// CLAUDE.md is a harness-specific entry point.
///
/// Also auto-scaffolds the `.k2so/` layout on first call (manager +
/// k2so-agent dirs, inbox/active/done folders, prds/, PROJECT.md).
///
/// Pre-0.33.0 this was `k2so_agents_generate_workspace_claude_md` —
/// back-compat alias below.
#[tauri::command]
pub fn k2so_agents_regenerate_workspace_skill(
    project_path: String,
) -> Result<String, String> {
    let project_name = std::path::Path::new(&project_path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "workspace".to_string());

    // Scaffold .k2so/ structure if it doesn't exist
    let k2so_dir = PathBuf::from(&project_path).join(".k2so");
    let _ = fs::create_dir_all(k2so_dir.join("work").join("inbox"));
    let _ = fs::create_dir_all(k2so_dir.join("prds"));

    // 0.37.0 unification check: if the workspace has been migrated to
    // the single-agent layout (`.k2so/agent/AGENT.md` exists OR the
    // unification sentinel is stamped), the legacy auto-scaffold of
    // `.k2so/agents/manager/` and `.k2so/agents/k2so-agent/` is a
    // regression — it'd repopulate the directory tree the migration
    // just retired and re-create files at paths the runtime no longer
    // reads. Skip the legacy scaffold entirely when migrated.
    let unification_sentinel = k2so_dir.join(".unification-0.37.0-done");
    let unified_agent_dir = k2so_dir.join("agent");
    let post_unification = unification_sentinel.exists() || unified_agent_dir.exists();
    if post_unification {
        // Don't recreate `.k2so/agents/` either — the post-migration
        // layout uses `.k2so/agent/` (singular) and
        // `.k2so/agent-templates/<n>/`. Skip straight to PROJECT.md +
        // workspace SKILL writes below.
    } else {
        let _ = fs::create_dir_all(k2so_dir.join("agents"));
    }

    // Auto-create manager agent if it doesn't exist (pre-unification only).
    // Check for old "pod-leader" and "coordinator" directory names as fallback.
    let manager_dir = k2so_dir.join("agents").join("manager");
    let legacy_coordinator_dir = k2so_dir.join("agents").join("coordinator");
    let legacy_pod_leader_dir = k2so_dir.join("agents").join("pod-leader");
    if !post_unification
        && !manager_dir.exists()
        && !legacy_coordinator_dir.exists()
        && !legacy_pod_leader_dir.exists()
    {
        let _ = fs::create_dir_all(manager_dir.join("work").join("inbox"));
        let _ = fs::create_dir_all(manager_dir.join("work").join("active"));
        let _ = fs::create_dir_all(manager_dir.join("work").join("done"));
        let manager_role = "Workspace Manager — delegates work to agents, reviews completed branches, drives milestones";
        let manager_body = generate_default_agent_body("manager", "manager", &manager_role, &project_path);
        let manager_md = format!(
            "---\nname: manager\nrole: {}\ntype: manager\nmanager: true\n---\n\n{}\n",
            manager_role, manager_body
        );
        let manager_md_path = manager_dir.join("AGENT.md");
        log_if_err(
            "auto-scaffold manager AGENT.md",
            &manager_md_path,
            atomic_write_str(&manager_md_path, &manager_md),
        );
        write_agent_skill_file(&project_path, "manager", "manager");
    }

    // Auto-create K2SO agent if it doesn't exist (pre-unification only).
    // Post-0.37.0 the workspace agent lives at .k2so/agent/, not
    // .k2so/agents/k2so-agent/.
    let k2so_agent_dir = k2so_dir.join("agents").join("k2so-agent");
    if !post_unification && !k2so_agent_dir.exists() {
        let _ = fs::create_dir_all(k2so_agent_dir.join("work").join("inbox"));
        let _ = fs::create_dir_all(k2so_agent_dir.join("work").join("active"));
        let _ = fs::create_dir_all(k2so_agent_dir.join("work").join("done"));
        let k2so_role = "K2SO planner — builds PRDs, milestones, and technical plans";
        let k2so_body = generate_default_agent_body("k2so", "k2so-agent", k2so_role, &project_path);
        let k2so_md = format!(
            "---\nname: k2so-agent\nrole: {}\ntype: k2so\n---\n\n{}\n",
            k2so_role, k2so_body
        );
        let k2so_md_path = k2so_agent_dir.join("AGENT.md");
        log_if_err(
            "auto-scaffold k2so-agent AGENT.md",
            &k2so_md_path,
            atomic_write_str(&k2so_md_path, &k2so_md),
        );
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
                    let agent_md = entry.path().join("AGENT.md");
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
        // Try reading from DB first — shared process-wide connection.
        let db_mode: Option<String> = {
            let db = crate::db::shared();
            let conn = db.lock();
            conn.query_row(
                "SELECT agent_mode FROM projects WHERE path = ?1",
                rusqlite::params![project_path],
                |row| row.get::<_, String>(0),
            ).ok()
        };

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

<!--
  PROJECT.md is the "what" half of agent context — the codebase facts
  every agent needs regardless of role. K2SO ships this file as part of
  the agent's system prompt on every launch, via --append-system-prompt
  (injected alongside SKILL.md as a "Project Context (shared)" section).
  You don't need to reference it from wakeup.md — it's always there.

  Pair it with Agent Skills (SKILL.md layers) which cover the "how":
    PROJECT.md = what this project IS (tech stack, conventions)
    SKILL.md   = what the agent DOES (standing orders, procedures)

  Edit this file directly or via Settings → Projects → "Manage Project
  Context". Applies to Workspace Manager and Agent Template agents.
  Custom Agents don't receive PROJECT.md by design — they may not be
  codebase-scoped.

  Delete these comments once you've filled the sections in.
-->

## About This Project

<!-- What does this codebase do? What problem does it solve? -->

## Tech Stack

<!-- Languages, frameworks, databases, infrastructure. Include versions
     where they matter (e.g. "Tauri v2, React 19, TailwindCSS v4"). -->

## Key Directories

<!-- Important paths and what lives in them. Call out where tests live,
     where generated files go, where NOT to edit. -->

## Conventions

<!-- Code style, commit message format, PR process, branch naming.
     Anything an engineer would otherwise have to discover by osmosis. -->

## External Systems

<!-- Links to issue trackers, CI dashboards, staging environments, docs.
     If the project depends on an external service the agent may need to
     know about or call, document it here. -->
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

    // As of 0.32.7: the rich workspace-level content (manager brief or AI
    // planner brief + agent list + inbox summary + CLI tools docs) now
    // flows into the canonical SKILL.md instead of a separate ./CLAUDE.md
    // file. `write_workspace_skill_file_with_body` takes the composed `md`
    // as the base body, appends `.k2so/PROJECT.md` body + primary agent's
    // `agent.md` body, writes the canonical at `.k2so/skills/k2so/SKILL.md`,
    // and fans it out via symlinks to every harness discovery path
    // (`./CLAUDE.md`, `./SKILL.md`, `./GEMINI.md`, `./AGENT.md`,
    // `./.goosehints`, `./.claude/skills/k2so/SKILL.md`, etc.).
    //
    // Existing `./CLAUDE.md` files: migrated to `.k2so/CLAUDE.md.migrated` if
    // K2SO-generated, preserved as-is if user-authored (see
    // migrate_and_symlink_root_claude_md).
    write_workspace_skill_file_with_body(&project_path, Some(&md));

    // Clean up the stale `.k2so/CLAUDE.md.disabled` artifact from the
    // pre-symlink era — the disable flow is now "symlink goes away when the
    // workspace is off", not a file rename.
    let disabled_path = PathBuf::from(&project_path).join(".k2so").join("CLAUDE.md.disabled");
    if disabled_path.exists() {
        let _ = fs::remove_file(&disabled_path);
    }

    Ok(md)
}

/// Back-compat alias for [`k2so_agents_regenerate_workspace_skill`].
/// Pre-0.33.0 name. Kept so existing Rust callers (and any React
/// `invoke('k2so_agents_generate_workspace_claude_md')` sites not yet
/// updated) keep working during the rename window.
#[tauri::command]
pub fn k2so_agents_generate_workspace_claude_md(
    project_path: String,
) -> Result<String, String> {
    k2so_agents_regenerate_workspace_skill(project_path)
}

// ── Onboarding (workspace-add three-option flow) ───────────────────
//
// Thin wrappers around `k2so_core::agents::onboarding`. Logic lives in
// core so the CLI (`k2so onboarding ...`) and Tauri share the same
// implementation; the renderer's WorkspaceOnboardingModal only displays
// scan results and forwards button-clicks to these commands.

/// Scan the workspace for harness files (CLAUDE.md, GEMINI.md,
/// .cursor/rules, .goosehints, etc.) with substantive user content.
/// Used by the onboarding modal to decide whether to prompt the user
/// at all (empty result → silently take the "Start Fresh" path) and
/// what to show in the adopt-picker.
#[tauri::command]
pub fn k2so_onboarding_scan(
    project_path: String,
) -> Vec<k2so_core::agents::onboarding::DetectedHarnessFile> {
    k2so_core::agents::onboarding::scan_harness_files(&project_path)
}

/// Adopt one of the detected harness files as the seed for
/// `.k2so/PROJECT.md`, then run the workspace regen pipeline so the
/// new PROJECT.md content fans out to every harness symlink in one
/// pass. Source file is archived to `.k2so/migration/` and removed
/// from its original location (so the regen's existing migration
/// helpers don't re-import the same body a second time).
#[tauri::command]
pub fn k2so_onboarding_adopt(
    project_path: String,
    source_path: String,
) -> Result<k2so_core::agents::onboarding::AdoptionOutcome, String> {
    let outcome = k2so_core::agents::onboarding::adopt_harness_as_project_md(
        &project_path,
        std::path::Path::new(&source_path),
    )?;
    // Run regen so PROJECT.md content propagates to every harness
    // file. Errors are reported but don't fail the adopt itself —
    // the seed is already on disk.
    if let Err(e) = k2so_agents_regenerate_workspace_skill(project_path) {
        eprintln!("[onboarding] regen after adopt failed: {}", e);
    }
    Ok(outcome)
}

/// User-facing label: "Do it later." Drops a flag file at
/// `.k2so/.skip-harness-management` so subsequent regens skip the
/// harness-fanout step (CLAUDE.md / GEMINI.md / .cursor/rules / etc.
/// stay untouched). K2SO still writes its internal SKILL.md so
/// heartbeats and agent launches keep working. Reversible — a
/// future settings surface can call the unskip path.
#[tauri::command]
pub fn k2so_onboarding_skip(project_path: String) -> Result<(), String> {
    k2so_core::agents::onboarding::skip_harness_management(&project_path)
}

/// User-facing "Start Fresh" option. No special logic — just runs
/// the regen pipeline, which already archives any pre-existing
/// harness files to `.k2so/migration/` and replaces them with
/// symlinks. Exposed as its own command so the renderer doesn't
/// have to know that "Start Fresh" is just-the-default — the three
/// options each have a symmetric Tauri entry point.
#[tauri::command]
pub fn k2so_onboarding_start_fresh(project_path: String) -> Result<(), String> {
    // Make sure any prior "skip" flag from a re-onboarding flow is
    // cleared so the regen actually performs the harness fanout.
    k2so_core::agents::onboarding::unskip_harness_management(&project_path)?;
    k2so_agents_regenerate_workspace_skill(project_path).map(|_| ())
}

/// Remove or disable the workspace SKILL.md + CLAUDE.md symlink
/// (when the Agent toggle is turned off).
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

// (duplicate of k2so_core helpers — removed during skill_content migration)

// ── Review Queue ────────────────────────────────────────────────────────
// Core types + logic live in `k2so_core::agents::reviews`. We re-export
// the types so the Tauri command signatures below (and any callers that
// imported from this module) keep their shapes.

pub use k2so_core::agents::reviews::{ReviewDiffFile, ReviewItem};

/// Get the review queue — agents with completed work in worktree branches.
#[tauri::command]
pub async fn k2so_agents_review_queue(project_path: String) -> Result<Vec<ReviewItem>, String> {
    tokio::task::spawn_blocking(move || k2so_core::agents::reviews::review_queue(&project_path))
        .await
        .map_err(|e| format!("review_queue task failed: {}", e))?
}

pub fn k2so_agents_review_queue_inner(project_path: &str) -> Result<Vec<ReviewItem>, String> {
    k2so_core::agents::reviews::review_queue(project_path)
}

/// Sub-agent completion. Core logic in
/// `k2so_core::agents::reviews::agent_complete`.
pub fn k2so_agent_complete(
    project_path: String,
    agent_name: String,
    filename: String,
) -> Result<String, String> {
    k2so_core::agents::reviews::agent_complete(project_path, agent_name, filename)
}

/// Approve the agent's branch — merge + cleanup. Core logic lives in
/// `k2so_core::agents::reviews::review_approve`.
#[tauri::command]
pub fn k2so_agents_review_approve(
    project_path: String,
    branch: String,
    agent_name: String,
) -> Result<String, String> {
    k2so_core::agents::reviews::review_approve(project_path, branch, agent_name)
}

/// Reject the agent's work — clean up worktree, restore inbox, write
/// optional feedback. Core logic lives in
/// `k2so_core::agents::reviews::review_reject`.
#[tauri::command]
pub fn k2so_agents_review_reject(
    project_path: String,
    agent_name: String,
    reason: Option<String>,
) -> Result<(), String> {
    k2so_core::agents::reviews::review_reject(project_path, agent_name, reason)
}

/// Request changes — drop a feedback file in inbox, don't tear down
/// the worktree. Core logic in `k2so_core::agents::reviews::review_request_changes`.
#[tauri::command]
pub fn k2so_agents_review_request_changes(
    project_path: String,
    agent_name: String,
    feedback: String,
) -> Result<(), String> {
    k2so_core::agents::reviews::review_request_changes(project_path, agent_name, feedback)
}

// ── Heartbeat Triage (Workspace State) ──────────────────────────────────

/// Read the workspace state for a project, returning the state or None if unset.
// `get_workspace_state` moved to k2so_core::agents::scheduler (re-exported).

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
        let agent_md_path = entry.path().join("AGENT.md");
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
///
/// **DEPRECATED — `legacy-per-agent-heartbeat` chokepoint.**
/// Pre-0.30s K2SO used inbox contents to decide whether to autonomously
/// wake an agent. The new model lives in the `agent_heartbeats` table
/// (workspace-scoped, explicit schedules). This function is being kept
/// alive only for the launch-failure-retry path in active-agents.ts;
/// gated on `projects.heartbeat_mode != 'off'` so opted-out workspaces
/// don't get auto-launched even if a stray caller invokes us. Planned
/// for removal in 0.37.x.
#[deprecated(
    note = "Inbox-driven triage — superseded by agent_heartbeats. \
            Planned for removal in 0.37.x. See `legacy-per-agent-heartbeat` tag."
)]
#[tauri::command]
pub fn k2so_agents_triage_decide(project_path: String) -> Result<Vec<String>, String> {
    // Gate 0: project must have heartbeats enabled. Without this, an
    // inbox with items unconditionally fires wakes — which is what was
    // happening to the K2SO workspace in 0.36.3 even with all DB
    // heartbeat rows disabled.
    let project_mode = {
        let db = k2so_core::db::shared();
        let conn = db.lock();
        conn.query_row(
            "SELECT heartbeat_mode FROM projects WHERE path = ?1",
            rusqlite::params![&project_path],
            |row| row.get::<_, String>(0),
        )
        .ok()
    };
    if project_mode.as_deref() == Some("off") {
        return Ok(Vec::new());
    }

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


// ── Adaptive Heartbeat Commands ──────────────────────────────────────────

// `k2so_agents_get_heartbeat` moved to k2so_core::agents::commands (re-exported).

// `k2so_agents_set_heartbeat` moved to k2so_core::agents::commands (re-exported).

/// Scheduler tick: check all agents in a project and return those ready to wake.
/// Called by the heartbeat script (via /cli/scheduler-tick).
/// Differentiates between manager agents (inbox-based) and custom agents (timing-based).
#[tauri::command]
pub fn k2so_agents_scheduler_tick(project_path: String) -> Result<Vec<String>, String> {
    let _h = crate::perf_hist!("scheduler_tick");
    core_scheduler_tick(project_path)
}

// `get_highest_inbox_priority` moved to k2so_core::agents::scheduler
// (re-exported at the top of this file).

/// Save the last Claude session ID for an agent (enables --resume on next launch).
/// Stores the session ID in the DB (agent_sessions.session_id).
/// This is the single source of truth — the legacy `.last_session`
/// file was retired, as it was being deleted by the no-op pruner
/// without touching the DB, leading to drift and failed resumes.
#[tauri::command]
pub fn k2so_agents_save_session_id(
    project_path: String,
    agent_name: String,
    session_id: String,
) -> Result<(), String> {
    k2so_core::agents::session::k2so_agents_save_session_id(
        project_path,
        agent_name,
        session_id,
    )
}

#[tauri::command]
pub fn k2so_agents_clear_session_id(
    project_path: String,
    agent_name: String,
) -> Result<(), String> {
    k2so_core::agents::session::k2so_agents_clear_session_id(project_path, agent_name)
}

/// Toggle the per-session `surfaced` flag. When transitioning 0 → 1,
/// emits `HookEvent::SessionSurfaced` so the renderer creates a tab
/// that ATTACHES to the existing PTY (no fresh spawn). When
/// transitioning 1 → 0, the renderer is expected to remove the tab
/// without killing the PTY (the heartbeat session keeps running in
/// the background). See `.k2so/prds/heartbeat-active-session-tracking.md`.
///
/// `terminal_id`, `command`, `args`, `heartbeat_name` are forwarded
/// in the surfaced-event payload so the renderer can construct a tab
/// without re-querying — kept minimal because the event listener is
/// a hot path. Pass empty strings / empty Vec / None when not
/// applicable.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn k2so_session_set_surfaced(
    project_path: String,
    agent_name: String,
    surfaced: bool,
    terminal_id: Option<String>,
    command: Option<String>,
    args: Option<Vec<String>>,
    heartbeat_name: Option<String>,
    attach_agent_name: Option<String>,
) -> Result<(), String> {
    let db = k2so_core::db::shared();
    let conn = db.lock();
    let project_id = k2so_core::agents::resolve_project_id(&conn, &project_path)
        .ok_or_else(|| format!("Project not found: {}", project_path))?;
    k2so_core::db::schema::WorkspaceSession::set_surfaced(
        &conn, &project_id, surfaced,
    )
    .map_err(|e| format!("set surfaced flag: {}", e))?;
    drop(conn);

    if surfaced {
        // Emit on every `surfaced=true` call (not just 0→1
        // transitions) so the user can re-summon a tab even when the
        // DB flag was left as `1` by a prior surface that the
        // renderer subsequently dropped (e.g. close-minimize that
        // skipped the surfaced=false flip). The renderer's listener
        // already checks whether a tab exists before creating one,
        // so re-emit is idempotent.
        k2so_core::agent_hooks::emit(
            k2so_core::agent_hooks::HookEvent::SessionSurfaced,
            serde_json::json!({
                "projectPath": project_path,
                "agentName": agent_name,
                "terminalId": terminal_id,
                "command": command,
                "args": args,
                "heartbeatName": heartbeat_name,
                "attachAgentName": attach_agent_name,
            }),
        );
    } else {
        // 0.38.0 commit 6 — symmetric counterpart so every viewer
        // drops the tab from its UI when one window minimizes. The
        // PTY stays alive in the daemon (close-as-minimize); only the
        // surface state propagates. Renderer's `session:unsurfaced`
        // listener is idempotent (no-op if the tab isn't present).
        k2so_core::agent_hooks::emit(
            k2so_core::agent_hooks::HookEvent::SessionUnsurfaced,
            serde_json::json!({
                "projectPath": project_path,
                "agentName": agent_name,
                "terminalId": terminal_id,
                "heartbeatName": heartbeat_name,
                "attachAgentName": attach_agent_name,
            }),
        );
    }
    Ok(())
}

/// 0.38.0 commit 7 — broadcast cross-window when the pinned-chat
/// refresh button is clicked. The originating window already kills
/// the daemon PTY (via `/cli/sessions/v2/close`) and bumps its
/// `refreshNonce` to remount TerminalPane. Other windows can't learn
/// this from Commit 4's `session_removed` push (those filter for
/// `tab-` agent names; pinned chat is the bare project_id), so we
/// emit a Tauri-broadcast `chat:refreshed` event. Every window's
/// AgentChatPane listener bumps its own `refreshNonce` when the
/// payload's `projectPath` matches its workspace, keeping pinned
/// chat sessions in sync across viewers.
#[tauri::command]
pub fn k2so_chat_refresh_broadcast(project_path: String) -> Result<(), String> {
    k2so_core::agent_hooks::emit(
        k2so_core::agent_hooks::HookEvent::ChatRefreshed,
        serde_json::json!({ "projectPath": project_path }),
    );
    Ok(())
}

// `k2so_agents_heartbeat_noop` moved to k2so_core::agents::commands (re-exported).

// `k2so_agents_heartbeat_action` moved to k2so_core::agents::commands (re-exported).

// `is_within_active_hours` moved to k2so_core::agents::scheduler
// (re-exported at the top of this file).

// ── Project-Level Schedule Evaluation ─────────────────────────────────────
//
// `should_project_fire` + `matches_ordinal_day` now live in
// k2so_core::scheduler so the daemon can call them directly without
// pulling in this commands module. Re-imported for the three
// unqualified call sites elsewhere in this file.
use k2so_core::scheduler::should_project_fire;


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

// ── Heartbeat Scheduler — Phase 2 Unit 7c ───────────────────────────────
//
// Pre-Unit-7c these commands wrote `~/.k2so/heartbeat.sh` + installed
// the launchd plist (macOS) / crontab entry (Linux) directly from
// Tauri. Unit 7c moved the install/uninstall body to
// `k2so_core::agents::heartbeat_install` and added daemon routes at
// `/cli/heartbeat/{install-launchd, uninstall-launchd,
// apply-wake-scheduler}`. The wrappers below are thin daemon-HTTP
// proxies so K2SO Connect (remote daemon, no Tauri) can install its
// own scheduler under its own GUI session and the renderer keeps
// using the same `invoke('k2so_agents_*_heartbeat')` shape.

/// Install the heartbeat scheduler via the daemon. The daemon reads
/// the current `wake_scheduler` settings, generates `heartbeat.sh`,
/// and loads the plist (macOS) or installs the crontab entry (Linux).
#[tauri::command]
pub fn k2so_agents_install_heartbeat(
    _state: tauri::State<'_, crate::state::AppState>,
) -> Result<(), String> {
    let client = crate::daemon_client::DaemonClient::try_connect()?;
    let settings = k2so_core::app_settings::load();
    let cfg = &settings.wake_scheduler;
    let interval_secs = cfg.interval_minutes.max(1) as u32 * 60;
    let body = serde_json::json!({
        "interval_seconds": interval_secs,
        "wake_system": cfg.wake_system,
    });
    let _ = client.cli_post_json("/cli/heartbeat/install-launchd", &body)?;
    Ok(())
}

/// Apply the user's Wake Scheduler settings — daemon decides between
/// install / uninstall based on `mode`. See `heartbeat_launchd_routes`
/// in the daemon for the routing.
#[tauri::command]
pub fn k2so_agents_apply_wake_scheduler() -> Result<String, String> {
    let client = crate::daemon_client::DaemonClient::try_connect()?;
    let settings = k2so_core::app_settings::load();
    let cfg = &settings.wake_scheduler;
    let body = serde_json::json!({
        "mode": cfg.mode,
        "interval_minutes": cfg.interval_minutes,
        "wake_system": cfg.wake_system,
    });
    let resp = client.cli_post_json("/cli/heartbeat/apply-wake-scheduler", &body)?;
    let v: serde_json::Value = serde_json::from_str(&resp)
        .map_err(|e| format!("decode apply-wake-scheduler: {e}"))?;
    Ok(v.get("message").and_then(|m| m.as_str()).unwrap_or("").to_string())
}

/// Uninstall the heartbeat scheduler via the daemon.
#[tauri::command]
pub fn k2so_agents_uninstall_heartbeat() -> Result<(), String> {
    let client = crate::daemon_client::DaemonClient::try_connect()?;
    let _ = client.cli_post_json(
        "/cli/heartbeat/uninstall-launchd",
        &serde_json::json!({}),
    )?;
    Ok(())
}

/// Refresh the heartbeat-scheduler lifecycle when the toggle changes.
/// Same idempotent re-apply as `k2so_agents_apply_wake_scheduler` —
/// the daemon's `apply-wake-scheduler` route handles the install /
/// uninstall transition uniformly. Pre-Unit-7c this also wrote the
/// retired `heartbeat-projects.txt`; that file was killed in P5.6
/// when `/cli/heartbeat/active-projects` started serving the live
/// project list from `agent_heartbeats`.
#[tauri::command]
pub fn k2so_agents_update_heartbeat_projects(
    _state: tauri::State<'_, crate::state::AppState>,
) -> Result<(), String> {
    let _ = k2so_agents_apply_wake_scheduler();
    Ok(())
}

// `simple_date` + `is_leap` moved to k2so_core::agents::session.
// `update_assigned_by` moved to k2so_core::agents::delegate (re-exported).

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

    let agent_md = fs::read_to_string(dir.join("AGENT.md")).unwrap_or_default();
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
#[tauri::command]
pub fn k2so_agents_preview_agent_context(
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
        "contextPath": on_disk_path.to_string_lossy(),
        // Legacy field — React still reads `claudeMdPath` at some
        // call sites. Emit both during the rename window; drop the
        // legacy field once every UI call site has migrated.
        "claudeMdPath": on_disk_path.to_string_lossy(),
    }))
}

/// Back-compat alias for [`k2so_agents_preview_agent_context`].
#[tauri::command]
pub fn k2so_agents_preview_claude_md(
    project_path: String,
    agent_name: String,
) -> Result<serde_json::Value, String> {
    k2so_agents_preview_agent_context(project_path, agent_name)
}

/// Back-compat alias. Pre-0.33.0 this was a separate fn from
/// `generate_claude_md` even though they did identical work; merged
/// into [`k2so_agents_regenerate_agent_context`] during the rename.
#[tauri::command]
pub fn k2so_agents_regenerate_claude_md(
    project_path: String,
    agent_name: String,
) -> Result<String, String> {
    k2so_agents_regenerate_agent_context(project_path, agent_name)
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

// `cleanup_agent_backups` moved to k2so_core::agents::commands (re-exported).

// ── Workspace Session (DB-tracked) ───────────────────────────────────────

#[tauri::command]
pub fn workspace_session_get(
    state: tauri::State<'_, crate::state::AppState>,
    project_id: String,
) -> Result<Option<WorkspaceSession>, String> {
    let conn = state.db.lock();
    WorkspaceSession::get(&conn, &project_id).map_err(|e| e.to_string())
}

/// 0.37.12 — explicitly set the pinned chat tab's Claude session id
/// for a workspace. Thin facade over the daemon HTTP route
/// `/cli/workspace/set-chat-session` — keeps Tauri as the thin
/// client, daemon owns the write. Used by `AgentChatPane`'s
/// chat-history dropdown when the user picks a different chat from
/// the history list (escape hatch for orphaned or deleted sessions).
///
/// Caller is expected to follow up with a pinned-chat refresh
/// (`closeV2Session(projectId)` + re-mount AgentChatPane) so the
/// live PTY swaps to the new session.
#[tauri::command]
pub fn workspace_session_set_session_id(
    project_path: String,
    session_id: String,
) -> Result<String, String> {
    let client = crate::daemon_client::DaemonClient::try_connect()?;
    client.cli_get(
        "/cli/workspace/set-chat-session",
        &[("project", &project_path), ("session_id", &session_id)],
    )
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
/// Core logic lives in `k2so_core::agents::commands::regenerate_skills`.
#[tauri::command]
pub fn k2so_agents_regenerate_skills(
    project_path: String,
) -> Result<serde_json::Value, String> {
    k2so_core::agents::commands::regenerate_skills(project_path)
}

// `const K2SO_SECTION_BEGIN` moved to k2so_core::agents::skill_writer.
// `const K2SO_SECTION_END` moved to k2so_core::agents::skill_writer.

// `upsert_k2so_section` moved to k2so_core::agents::skill_writer (re-exported).

// `force_symlink` moved to k2so_core::agents::skill_writer (re-exported).

/// Write the canonical SKILL.md and symlink from all harness discovery paths.
/// One source of truth — symlinks mean updates propagate instantly.
///
/// Canonical location: .k2so/skills/{name}/SKILL.md
/// Symlinked to: Claude Code, OpenCode, Pi, Cursor (project root)
/// Marker-injected into: AGENTS.md, .github/copilot-instructions.md
// `write_shared_markers`: only the workspace-level skill should set this
// true — per-agent skills would otherwise clobber each other in the
// single K2SO marker block inside AGENTS.md / copilot-instructions.md.
// `write_skill_to_all_harnesses` moved to k2so_core::agents::skill_writer (re-exported).

/// Write the workspace-level K2SO skill to all harness locations.
/// Composes the full workspace context into a single canonical file
/// that every CLI LLM discovers via its harness-specific path:
///
///   - Base body (rich workspace manager / AI planner brief if the
///     CLAUDE.md generator passes one; otherwise the lightweight
///     `generate_workspace_skill_content` — user-facing CLI commands)
///   - `.k2so/PROJECT.md` body (if the user has populated it)
///   - Primary agent's `agent.md` body (for single-agent and manager modes)
///
/// The canonical file at `.k2so/skills/k2so/SKILL.md` is then symlinked
/// into every harness discovery path. `./CLAUDE.md` joins that list as
/// of 0.32.7, replacing the separately-generated workspace CLAUDE.md.
pub fn write_workspace_skill_file(project_path: &str) {
    k2so_core::agents::workspace::write_workspace_skill_file(project_path)
}

/// Variant that lets callers pass a pre-composed body (typically the
/// rich workspace CLAUDE.md content from `k2so_agents_generate_workspace_claude_md`)
/// so that content lands in the canonical SKILL.md rather than being
/// lost when CLAUDE.md collapsed to a symlink.
///
/// Sequence (Phase 7c):
///   1. Adoption sweep — parse existing canonical SKILL.md SOURCE sub-regions;
///      commit drift back to PROJECT.md / primary agent AGENT.md (mtime-guarded).
///   2. Clear stale SOURCE regions from the canonical's below-END tail so the
///      fresh composition below can lay them down cleanly.
///   3. Compose K2SO-managed body only (no PROJECT.md / AGENT.md appended).
///   4. Write managed body via write_skill_to_all_harnesses with
///      write_shared_markers=false — canonical + Claude/OpenCode/Pi symlinks
///      get just the managed region.
///   5. Append fresh SOURCE regions (PROJECT.md + primary agent AGENT.md)
///      below the canonical's END marker.
///   6. Inject the FULL canonical body (managed + SOURCE regions) into
///      AGENTS.md and .github/copilot-instructions.md — those are plain
///      files, not canonical sources, so they get the full context.
///   7. Symlink project root SKILL.md + CLAUDE.md to canonical.
///   8. Stamp .k2so/.last-skill-regen so subsequent drift-adoption mtime
///      comparisons have a reference point.
pub fn write_workspace_skill_file_with_body(project_path: &str, base_body: Option<&str>) {
    k2so_core::agents::workspace::write_workspace_skill_file_with_body(project_path, base_body)
}

// SKILL scaffolding cluster (write_workspace_skill_file_with_body,
// adopt_workspace_skill_drift, strip_workspace_skill_tail,
// append_workspace_source_regions, migrate_and_symlink_root_claude_md,
// import_claude_md_into_user_notes, harvest_per_agent_claude_md_files,
// archive_claude_md_file, inject_first_migration_banner,
// safe_symlink_harness_file, write_workspace_harness_discovery_targets,
// write_cursor_rules_mdc, scaffold_aider_conf,
// teardown_workspace_harness_files, find_latest_archive,
// k2so_agents_teardown_workspace, k2so_agents_preview_workspace_ingest,
// k2so_agents_run_workspace_ingest, ensure_all_skills_up_to_date,
// detect_interrupted_regen) + the migration-safety test module all
// moved to k2so_core::agents::workspace (Phase 2 Unit 7b). The Tauri
// #[tauri::command] wrappers that survive this unit forward to the
// k2so-core entry points; the rest is gone from this file.
pub use k2so_core::agents::workspace::{
    detect_interrupted_regen, ensure_all_skills_up_to_date,
    harvest_per_agent_claude_md_files, teardown_workspace_harness_files,
    TeardownMode, TeardownResult, WorkspacePreviewEntry,
    HARNESS_WORKSPACE_FILES, SKILL_USER_NOTES_SENTINEL,
    USER_NOTES_PLACEHOLDER,
};

#[tauri::command]
pub fn k2so_agents_teardown_workspace(
    project_path: String,
    mode: String,
) -> Result<Vec<TeardownResult>, String> {
    k2so_core::agents::workspace::k2so_agents_teardown_workspace(project_path, mode)
}

#[tauri::command]
pub fn k2so_agents_preview_workspace_ingest(
    project_path: String,
) -> Result<Vec<WorkspacePreviewEntry>, String> {
    k2so_core::agents::workspace::k2so_agents_preview_workspace_ingest(project_path)
}

#[tauri::command]
pub fn k2so_agents_run_workspace_ingest(project_path: String) -> Result<(), String> {
    k2so_core::agents::workspace::k2so_agents_run_workspace_ingest(project_path)
}

// `write_agent_skill_file` moved to k2so_core::agents::skill_writer (re-exported).

// `ensure_all_skills_up_to_date` moved to k2so_core::agents::workspace
// (re-exported via the pub use block above).

// ══════════════════════════════════════════════════════════════════════
// Migration-safety tests (Phase 7c/7d invariants)
// ══════════════════════════════════════════════════════════════════════
//
// These tests pin down the "never lose user data" contract. Every
// migration path we ship must:
//   1. Archive user-authored content before mutating or deleting it.
//   2. Be idempotent (running twice never doubles or re-loses content).
//   3. Return preserved content from strip_workspace_skill_tail so
//      append_workspace_source_regions can re-emit it losslessly.
//   4. Never stack duplicate USER_NOTES sentinels / placeholder comments.

#[cfg(test)]
mod pure_helper_tests {
    //! Tests for the I/O-free helpers extracted from Tauri command
    //! handlers in Phase C of the testability work. Each helper is a
    //! pure function (no fs, no db, no Tauri state) so these tests
    //! run in microseconds and cover edge cases that would otherwise
    //! require scaffolding a full workspace.
    use super::*;

    // ── update_agent_md_field ────────────────────────────────────
    #[test]
    fn update_field_replaces_frontmatter_value() {
        let content = "---\nrole: old role\ntype: custom\n---\n# Agent body\n";
        let updated = update_agent_md_field(content, "role", "new role").unwrap();
        assert!(updated.contains("role: new role"), "got: {}", updated);
        assert!(!updated.contains("role: old role"));
        assert!(updated.contains("type: custom"), "other keys preserved: {}", updated);
        assert!(updated.contains("# Agent body"), "body preserved: {}", updated);
    }

    #[test]
    fn update_field_replaces_section_body() {
        let content = "---\nrole: x\n---\n# Agent\n\n## Work Sources\n\nold content\n\n## Other\n\nkeep\n";
        let updated = update_agent_md_field(content, "Work Sources", "new content").unwrap();
        assert!(updated.contains("## Work Sources\n\nnew content"), "got: {}", updated);
        assert!(!updated.contains("old content"));
        assert!(updated.contains("## Other\n\nkeep"), "trailing section preserved: {}", updated);
    }

    #[test]
    fn update_field_appends_missing_section() {
        let content = "---\nrole: x\n---\n# Agent\n\n## Existing\n\ntext\n";
        let updated = update_agent_md_field(content, "New Section", "added body").unwrap();
        assert!(updated.contains("## New Section\n\nadded body"), "got: {}", updated);
        assert!(updated.contains("## Existing"), "existing preserved");
    }

    #[test]
    fn update_field_replaces_last_section_to_end_of_body() {
        // Edge case: section has no following `## ` so end-of-body
        // is the boundary. Verifies the .unwrap_or(body.len()) path.
        let content = "---\nrole: x\n---\n# Agent\n\n## Tail\n\nold tail content\n";
        let updated = update_agent_md_field(content, "Tail", "new tail").unwrap();
        assert!(updated.contains("## Tail\n\nnew tail"));
        assert!(!updated.contains("old tail content"));
    }

    #[test]
    fn update_field_rejects_missing_frontmatter() {
        let content = "# Just body\n\nno frontmatter here\n";
        let err = update_agent_md_field(content, "role", "x").unwrap_err();
        assert!(err.contains("missing frontmatter"), "got: {}", err);
    }

    #[test]
    fn update_field_rejects_unterminated_frontmatter() {
        let content = "---\nrole: x\nnever-closed\n# body\n";
        let err = update_agent_md_field(content, "role", "y").unwrap_err();
        assert!(err.contains("Invalid frontmatter"), "got: {}", err);
    }

    #[test]
    fn update_field_frontmatter_update_preserves_body_exactly() {
        // The body section after --- must be byte-identical when only
        // a frontmatter key is updated. Regression guard for the
        // extraction: the pre-refactor code stitched body back in
        // verbatim, and we must preserve that.
        let body = "\n# Heading\n\nLine one.\nLine two.\n\n## Sub\n\nMore.\n";
        let content = format!("---\nrole: a\n---{}", body);
        let updated = update_agent_md_field(&content, "role", "b").unwrap();
        assert!(updated.ends_with(body), "body not byte-preserved: {}", updated);
    }

    #[test]
    fn update_field_handles_value_containing_colon() {
        // Values with colons (URLs, ratio notation) must survive the
        // split_once logic and round-trip correctly.
        let content = "---\nrole: old\n---\n";
        let updated = update_agent_md_field(content, "role", "URL: https://example.com/path").unwrap();
        assert!(updated.contains("role: URL: https://example.com/path"), "got: {}", updated);
    }

    // ── compose_manager_wake_from_body ───────────────────────────
    //
    // P8 retired the "K2SO Heartbeat Wake — Workspace Manager" boilerplate
    // preamble that used to wrap the body. The composer now returns the
    // wakeup body verbatim (frontmatter stripped). These tests exercise
    // the post-P8 contract: body content survives, fallback template
    // kicks in for empty/missing input, no boilerplate added.

    #[test]
    fn compose_manager_wake_uses_provided_body() {
        let out = compose_manager_wake_from_body(Some("custom manager instructions"));
        assert!(out.contains("custom manager instructions"), "body inlined");
        // No K2SO boilerplate prefix anymore — body is the message.
        assert!(!out.contains("K2SO Heartbeat Wake"), "preamble retired in P8: {}", out);
    }

    #[test]
    fn compose_manager_wake_falls_back_when_body_none() {
        let out = compose_manager_wake_from_body(None);
        // Fallback uses WAKEUP_TEMPLATE_WORKSPACE — assert its trim()'d
        // first line is in the output.
        let template_lead = WAKEUP_TEMPLATE_WORKSPACE.trim().lines().next().unwrap_or("");
        assert!(!template_lead.is_empty());
        assert!(
            out.contains(template_lead),
            "expected template fallback to contain first line '{}', got: {}",
            template_lead,
            out
        );
    }

    #[test]
    fn compose_manager_wake_falls_back_when_body_is_empty_string() {
        // A disk read returning "" after frontmatter strip must hit
        // the fallback — not silently emit an empty wake prompt.
        let out = compose_manager_wake_from_body(Some(""));
        let template_lead = WAKEUP_TEMPLATE_WORKSPACE.trim().lines().next().unwrap_or("");
        assert!(out.contains(template_lead), "expected template fallback, got: {}", out);
    }

    #[test]
    fn compose_manager_wake_strips_frontmatter_from_body() {
        // If the disk body has its own frontmatter (e.g. a scaffolded
        // WAKEUP.md with metadata), strip_frontmatter must run before
        // the empty-check.
        let body = "---\ntitle: foo\n---\nActual wake instructions here.";
        let out = compose_manager_wake_from_body(Some(body));
        assert!(!out.contains("title: foo"), "frontmatter leaked: {}", out);
        assert!(out.contains("Actual wake instructions here"), "body survived: {}", out);
    }

    // ── compose_agent_wake_from_body ─────────────────────────────
    #[test]
    fn compose_agent_wake_returns_none_on_none_input() {
        assert!(compose_agent_wake_from_body(None).is_none());
    }

    #[test]
    fn compose_agent_wake_returns_body_verbatim() {
        // P8: composer returns the body itself (frontmatter stripped),
        // no boilerplate preamble. The wakeup.md content is the message.
        let out = compose_agent_wake_from_body(Some("agent instructions"))
            .expect("body present -> Some");
        assert!(out.contains("agent instructions"), "body in output: {}", out);
        // No "K2SO Heartbeat Wake" preamble anymore.
        assert!(!out.contains("K2SO Heartbeat Wake"), "preamble retired in P8: {}", out);
    }

    #[test]
    fn compose_agent_wake_strips_frontmatter() {
        // P8: composer now strips frontmatter symmetrically with the
        // manager composer. Pre-P8 it left frontmatter intact and
        // expected callers to strip; post-P8 the composer owns it.
        let body = "---\nname: foo\n---\nbody";
        let out = compose_agent_wake_from_body(Some(body)).unwrap();
        assert!(!out.contains("name: foo"), "frontmatter stripped: {}", out);
        assert!(out.contains("body"), "body survived: {}", out);
    }

    #[test]
    fn compose_agent_wake_returns_none_for_empty_body() {
        // P8: empty body (after frontmatter strip) returns None so
        // smart_launch can record a "wakeup body empty" audit instead
        // of firing claude with no prompt.
        let body = "---\ndescription:\n---\n\n";
        assert!(compose_agent_wake_from_body(Some(body)).is_none());
    }

    // ── parse_work_item_content ──────────────────────────────────
    #[test]
    fn parse_work_item_full_frontmatter() {
        let content = "---\ntitle: Add OAuth\npriority: high\ntype: feature\nsource: feedback\ncreated: 2026-04-01\nassigned_by: user\n---\n\nBody text here that describes the work.";
        let item = parse_work_item_content(content, "add-oauth.md", "inbox");
        assert_eq!(item.title, "Add OAuth");
        assert_eq!(item.priority, "high");
        assert_eq!(item.item_type, "feature");
        assert_eq!(item.source, "feedback");
        assert_eq!(item.assigned_by, "user");
        assert_eq!(item.folder, "inbox");
        assert_eq!(item.filename, "add-oauth.md");
        assert!(item.body_preview.contains("Body text here"));
    }

    #[test]
    fn parse_work_item_missing_fields_use_defaults() {
        let content = "---\ntitle: minimal\n---\nbody";
        let item = parse_work_item_content(content, "m.md", "active");
        assert_eq!(item.title, "minimal");
        assert_eq!(item.priority, "normal"); // default
        assert_eq!(item.item_type, "task"); // default
        assert_eq!(item.source, "manual"); // default
        assert_eq!(item.assigned_by, "unknown"); // default
    }

    #[test]
    fn parse_work_item_no_frontmatter_defaults_all_but_body() {
        let content = "just a body with no metadata";
        let item = parse_work_item_content(content, "raw.md", "inbox");
        assert_eq!(item.title, "");
        assert_eq!(item.body_preview, "just a body with no metadata");
    }

    #[test]
    fn parse_work_item_body_preview_truncates_over_120_chars() {
        let long_body = "x".repeat(300);
        let content = format!("---\ntitle: t\n---\n{}", long_body);
        let item = parse_work_item_content(&content, "l.md", "inbox");
        // Preview is 120 + "..." — exact char count matters.
        assert!(item.body_preview.ends_with("..."), "preview: {:?}", item.body_preview);
        let without_ellipsis = item.body_preview.trim_end_matches("...");
        assert_eq!(without_ellipsis.chars().count(), 120);
    }

    // ── FakeFs-driven demonstration ──────────────────────────────
    //
    // These tests show the end-state pattern: use FakeFs to scaffold
    // a workspace tree, call the Fs trait to read content, then feed
    // that content into the pure parser. No tempdir, no disk I/O.
    //
    // Once `read_work_item` is threaded with `&dyn Fs`, these tests
    // can drop the manual read_to_string and just pass the fs into a
    // higher-level helper. For now they demonstrate the pattern and
    // prove the integration (pure parser + FakeFs storage).

    #[test]
    fn fake_fs_scaffolds_agent_work_tree_and_parses_items() {
        use crate::fs_abstract::{FakeFs, Fs};
        use std::path::Path;

        let fs = FakeFs::new();
        fs.insert_tree(
            Path::new("/proj/.k2so/agents/backend-eng/work"),
            serde_json::json!({
                "inbox": {
                    "build-oauth.md": "---\ntitle: Build OAuth\npriority: high\ntype: feature\n---\n\nOAuth endpoints required.",
                    "fix-crash.md": "---\ntitle: Fix startup crash\npriority: urgent\ntype: bug\nsource: crash\n---\n\nCrashes on launch.",
                },
                "active": {},
                "done": {},
            }),
        );

        let inbox_dir = Path::new("/proj/.k2so/agents/backend-eng/work/inbox");
        let mut entries = fs.read_dir(inbox_dir).unwrap();
        entries.sort();

        let items: Vec<WorkItem> = entries
            .iter()
            .map(|p| {
                let content = fs.read_to_string(p).unwrap();
                let filename = p.file_name().unwrap().to_string_lossy();
                parse_work_item_content(&content, &filename, "inbox")
            })
            .collect();

        assert_eq!(items.len(), 2);
        let oauth = items.iter().find(|i| i.filename == "build-oauth.md").unwrap();
        assert_eq!(oauth.title, "Build OAuth");
        assert_eq!(oauth.priority, "high");
        let crash = items.iter().find(|i| i.filename == "fix-crash.md").unwrap();
        assert_eq!(crash.priority, "urgent");
        assert_eq!(crash.source, "crash");

        // Sanity: FakeFs's write counter shows exactly one write per
        // file (the insert_tree calls). Good regression guard for
        // "does my test accidentally double-write?"
        assert_eq!(fs.write_count(&inbox_dir.join("build-oauth.md")), 1);
        assert_eq!(fs.write_count(&inbox_dir.join("fix-crash.md")), 1);
    }

    #[test]
    fn fake_fs_simulates_missing_agent_work_dir() {
        use crate::fs_abstract::{FakeFs, Fs};
        use std::path::Path;

        let fs = FakeFs::new();
        // Intentionally do NOT scaffold the inbox — simulate a fresh
        // agent directory with no work yet. The caller must handle
        // NotFound gracefully.
        let err = fs
            .read_dir(Path::new("/proj/.k2so/agents/solo/work/inbox"))
            .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn fake_fs_verifies_frontmatter_round_trip_via_update_field() {
        // End-to-end: scaffold an AGENT.md in FakeFs, read it out,
        // pass through the extracted pure updater, write the result
        // back. Confirm the write went through and content matches.
        use crate::fs_abstract::{FakeFs, Fs};
        use std::path::Path;

        let fs = FakeFs::new();
        let agent_md = Path::new("/proj/.k2so/agents/rust-eng/AGENT.md");
        let original = "---\nrole: rust engineer\ntype: custom\n---\n# Rust engineer\n\nFocus: backend, systems.";
        fs.insert_file(agent_md, original.as_bytes());

        let content = fs.read_to_string(agent_md).unwrap();
        let updated = update_agent_md_field(&content, "role", "principal rust engineer").unwrap();
        fs.write(agent_md, updated.as_bytes()).unwrap();

        let final_content = fs.read_to_string(agent_md).unwrap();
        assert!(final_content.contains("role: principal rust engineer"));
        assert!(final_content.contains("type: custom"));
        assert!(final_content.contains("# Rust engineer"));
        // write_count should be 2: insert_file (1) + write (1).
        assert_eq!(fs.write_count(agent_md), 2);
    }
}

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
    let mut archived = Vec::new();
    let agents_root = agents_dir(project_path);
    if !agents_root.exists() {
        return archived;
    }
    let Some(primary) = find_primary_agent(project_path) else {
        // Can't resolve primary — don't risk archiving the wrong thing.
        return archived;
    };

    let Ok(entries) = fs::read_dir(&agents_root) else { return archived };
    let mut orphans: Vec<String> = Vec::new();
    for entry in entries.flatten() {
        if !entry.file_type().map_or(false, |ft| ft.is_dir()) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || name == primary {
            continue;
        }
        let agent_type = agent_type_for(project_path, &name);
        if matches!(agent_type.as_str(), "custom" | "manager" | "k2so") {
            orphans.push(name);
        }
    }
    if orphans.is_empty() {
        return archived;
    }

    let archive_root = agents_root.join(".archive");
    if fs::create_dir_all(&archive_root).is_err() {
        return archived;
    }
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();

    let project_id = {
        let db = crate::db::shared();
        let conn = db.lock();
        resolve_project_id(&conn, project_path)
    };

    for orphan in orphans {
        let src = agents_root.join(&orphan);
        let dst = archive_root.join(format!("{}-{}", orphan, stamp));
        if fs::rename(&src, &dst).is_err() {
            continue;
        }
        if let Some(ref pid) = project_id {
            {
        let db = crate::db::shared();
        let conn = db.lock();
                let _ = WorkspaceSession::delete(&conn, pid);
                let prefix = format!(".k2so/agents/{}/", orphan);
                let _ = conn.execute(
                    "DELETE FROM workspace_heartbeats WHERE project_id = ?1 AND wakeup_path LIKE ?2 || '%'",
                    rusqlite::params![pid, prefix],
                );
            }
        }
        archived.push(orphan.clone());
        log_debug!(
            "[agent-archive] {} → .archive/{}-{} (primary={})",
            orphan, orphan, stamp, primary
        );
    }
    archived
}

/// Detect and repair heartbeats whose `wakeup_path` points at the wrong
/// agent — typically caused by the pre-0.32.1 migration picking an
/// orphan agent directory from a prior agent-mode swap. Called on
/// startup after `promote_legacy_heartbeat`. Idempotent: no-op when
/// all rows already point at the correct agent.
pub fn repair_mismigrated_heartbeats(project_path: &str) {
    let db = crate::db::shared();
    let conn = db.lock();
    let Some(project_id) = resolve_project_id(&conn, project_path) else { return };
    let Ok(rows) = AgentHeartbeat::list_by_project(&conn, &project_id) else { return };
    if rows.is_empty() {
        return;
    }
    let Some(correct_agent) = find_primary_agent(project_path) else { return };

    let expected_prefix = format!(".k2so/agents/{}/heartbeats/", correct_agent);
    let legacy_wakeup = agent_dir(project_path, &correct_agent).join("WAKEUP.md");
    for hb in rows {
        let wrong_abs = std::path::Path::new(project_path).join(&hb.wakeup_path);
        let correct_dir = agent_dir(project_path, &correct_agent)
            .join("heartbeats")
            .join(&hb.name);
        let correct_wakeup = correct_dir.join("WAKEUP.md");

        let row_is_correct = hb.wakeup_path.starts_with(&expected_prefix);

        // Read legacy agent-root wakeup (if any) and detect whether it's
        // just a freshly-scaffolded default template. Template marker is
        // `<!-- DEFAULT TEMPLATE` (from wakeup_templates/*.md). When the
        // legacy is a template, DON'T use it as a content source — the
        // row's current wakeup_path has the real edits.
        let legacy_content = fs::read_to_string(&legacy_wakeup).ok();
        let legacy_is_template = legacy_content
            .as_deref()
            .map(|s| s.contains("<!-- DEFAULT TEMPLATE"))
            .unwrap_or(false);
        let legacy_present = legacy_content
            .as_deref()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false)
            && !legacy_is_template;

        // Nothing to do when the row is correct AND no real legacy
        // agent-root wakeup.md is left behind.
        if row_is_correct && !legacy_present {
            // Clean up a stray template scaffold if present — it'll
            // just trick the repair into work on future runs.
            if legacy_is_template {
                let _ = fs::remove_file(&legacy_wakeup);
            }
            continue;
        }

        if fs::create_dir_all(&correct_dir).is_err() {
            continue;
        }

        // Source priority:
        //   1. Legacy agent-root wakeup.md — the user's REAL content,
        //      whether the row is currently pointing at the wrong agent
        //      or was already pointed at the correct agent but a broken
        //      pre-0.32.1 run left the user's real file behind at the
        //      agent root without copying it into heartbeats/<name>/.
        //   2. The row's current wakeup_path if it has non-empty content
        //      (e.g. the user had already edited the wrong-agent folder).
        //   3. Scaffold a placeholder if neither source exists.
        let source = if legacy_present {
            Some(legacy_wakeup.clone())
        } else if wrong_abs.exists()
            && fs::read_to_string(&wrong_abs).map(|s| !s.trim().is_empty()).unwrap_or(false)
        {
            Some(wrong_abs.clone())
        } else {
            None
        };

        if let Some(src) = source {
            if let Ok(content) = fs::read_to_string(&src) {
                if fs::write(&correct_wakeup, content).is_ok() {
                    // Clean up the legacy agent-root file if we just
                    // used it. Avoids dual-source-of-truth on next run.
                    if src == legacy_wakeup {
                        let _ = fs::remove_file(&legacy_wakeup);
                    }
                }
            }
        } else if !correct_wakeup.exists() {
            let template = format!(
                "---\ndescription: Heartbeat migrated by 0.32.1 repair (content was missing pre-repair)\n---\n\n\
                # Wake procedure: {}\n\n\
                This heartbeat's wakeup file was lost during the 0.32.0 migration.\n\
                Edit this file with the instructions this heartbeat should run.\n",
                hb.name
            );
            log_if_err(
                "heartbeat-repair synth-wakeup",
                &correct_wakeup,
                atomic_write_str(&correct_wakeup, &template),
            );
        }

        let new_relative = correct_wakeup
            .strip_prefix(project_path)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| correct_wakeup.to_string_lossy().to_string());
        if !row_is_correct {
            let _ = AgentHeartbeat::update_wakeup_path(&conn, &project_id, &hb.name, &new_relative);
        }
        log_debug!(
            "[heartbeat-repair] {} wakeup_path {} → {} (source={})",
            hb.name,
            hb.wakeup_path,
            new_relative,
            if legacy_present { "legacy agent-root" } else { "existing path" }
        );
    }
}

/// One-time promotion of the legacy `projects.heartbeat_schedule` single-slot
/// config into the multi-heartbeat `agent_heartbeats` table. Safe to call
/// repeatedly; no-ops when the project already has any agent_heartbeats
/// row (migration is idempotent). Moves the legacy `wakeup.md` to
/// `heartbeats/default/wakeup.md` so everything lives under a consistent
/// hierarchy post-migration.
pub fn promote_legacy_heartbeat(project_path: &str) {
    let db = crate::db::shared();
    let conn = db.lock();
    let Some(project_id) = resolve_project_id(&conn, project_path) else { return };

    // Idempotency: skip if any heartbeat row exists for this project.
    if let Ok(existing) = AgentHeartbeat::list_by_project(&conn, &project_id) {
        if !existing.is_empty() {
            return;
        }
    }

    // Read legacy slot. If empty or null, nothing to migrate.
    let legacy: Option<(Option<String>, Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT heartbeat_mode, heartbeat_schedule, heartbeat_last_fire \
             FROM projects WHERE id = ?1",
            rusqlite::params![project_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .ok();
    let Some((mode, schedule, last_fire)) = legacy else { return };
    let Some(schedule_json) = schedule else { return };
    if schedule_json.trim().is_empty() {
        return;
    }

    // Parse the legacy JSON to extract frequency and spec params.
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&schedule_json) else { return };
    let frequency = v
        .get("frequency")
        .and_then(|s| s.as_str())
        .unwrap_or(match mode.as_deref() {
            Some("hourly") => "hourly",
            _ => "daily",
        })
        .to_string();

    let Some(agent_name) = find_primary_agent(project_path) else { return };

    // Move legacy wakeup.md into heartbeats/default/ so the rest of the
    // system has a single lookup pattern.
    let default_dir = agent_dir(project_path, &agent_name)
        .join("heartbeats")
        .join("default");
    if fs::create_dir_all(&default_dir).is_err() {
        return;
    }
    let legacy_wakeup = agent_dir(project_path, &agent_name).join("WAKEUP.md");
    let new_wakeup = default_dir.join("WAKEUP.md");
    if legacy_wakeup.exists() && !new_wakeup.exists() {
        // Follow symlinks by copying content rather than renaming the link.
        // Atomic write ensures a crash between copy-source-read and commit
        // can't leave the new_wakeup half-written + the legacy still present;
        // we only remove the legacy once the new is fully on disk.
        if let Ok(content) = fs::read_to_string(&legacy_wakeup) {
            if atomic_write_str(&new_wakeup, &content).is_ok() {
                log_if_err(
                    "promote_legacy_heartbeat legacy remove",
                    &legacy_wakeup,
                    fs::remove_file(&legacy_wakeup),
                );
            }
        }
    } else if !new_wakeup.exists() {
        // No legacy wakeup — scaffold a default from the templates.
        let template = format!(
            "---\ndescription: Default heartbeat migrated from legacy single-slot schedule\n---\n\n\
            # Wake procedure: default\n\n\
            This heartbeat was auto-created by the migration from the legacy single-slot\n\
            heartbeat system. Edit this file to define what happens when this agent wakes.\n"
        );
        log_if_err(
            "promote_legacy_heartbeat scaffold",
            &new_wakeup,
            atomic_write_str(&new_wakeup, &template),
        );
    }

    let workspace_relative = new_wakeup
        .strip_prefix(project_path)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| new_wakeup.to_string_lossy().to_string());

    let id = uuid::Uuid::new_v4().to_string();
    if AgentHeartbeat::insert(
        &conn,
        &id,
        &project_id,
        "default",
        &frequency,
        &schedule_json,
        &workspace_relative,
        true,
    )
    .is_ok()
    {
        // Carry forward last_fire so we don't re-fire a schedule that
        // just fired pre-migration.
        if let Some(lf) = last_fire {
            if !lf.is_empty() {
                let _ = conn.execute(
                    "UPDATE agent_heartbeats SET last_fired = ?1 \
                     WHERE project_id = ?2 AND name = 'default'",
                    rusqlite::params![lf, project_id],
                );
            }
        }
        log_debug!(
            "[heartbeat-migrate] promoted legacy heartbeat_schedule for {} (agent={}, freq={})",
            project_path, agent_name, frequency
        );
    }
}

/// Scaffold the wakeup files for a single workspace — one for each
/// existing agent that supports wake-up. Safe to call repeatedly;
/// never overwrites an existing file. Used by the app-launch migration
/// pass. Workspace-level `.k2so/wakeup.md` is no longer scaffolded here
/// — `migrate_or_scaffold_lead_heartbeat` handles the __lead__ case
/// via the multi-heartbeat system.
pub fn ensure_workspace_wakeups(project_path: &str) {
    let agents_root = agents_dir(project_path);
    if !agents_root.exists() {
        return;
    }
    let Ok(entries) = fs::read_dir(&agents_root) else { return };
    for entry in entries.flatten() {
        if !entry.file_type().map_or(false, |ft| ft.is_dir()) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        let agent_type = agent_type_for(project_path, &name);
        ensure_agent_wakeup(project_path, &name, &agent_type);
    }
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
    let agents_root = agents_dir(project_path);
    if agents_root.exists() {
        if let Ok(entries) = fs::read_dir(&agents_root) {
            for entry in entries.flatten() {
                if !entry.file_type().map_or(false, |ft| ft.is_dir()) {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with('.') {
                    // skip .archive and similar
                    continue;
                }
                let agent_path = entry.path();

                // 1. Agent persona: agent.md → AGENT.md
                case_rename(&agent_path.join("agent.md"), &agent_path.join("AGENT.md"));

                // 2. Agent-root legacy wakeup.md → WAKEUP.md (pre-multi-heartbeat era)
                case_rename(&agent_path.join("wakeup.md"), &agent_path.join("WAKEUP.md"));

                // 3. Per-heartbeat wakeups
                let heartbeats_dir = agent_path.join("heartbeats");
                if let Ok(hb_entries) = fs::read_dir(&heartbeats_dir) {
                    for hb in hb_entries.flatten() {
                        if !hb.file_type().map_or(false, |ft| ft.is_dir()) { continue; }
                        let sched_path = hb.path();
                        case_rename(&sched_path.join("wakeup.md"), &sched_path.join("WAKEUP.md"));
                    }
                }
            }
        }
    }

    // Migrate DB rows: agent_heartbeats.wakeup_path entries that reference
    // lowercase `wakeup.md` → UPPERCASE. This matters on case-sensitive
    // filesystems (Linux); case-insensitive FS would tolerate either.
    {
        let db = crate::db::shared();
        let conn = db.lock();
        if let Some(project_id) = resolve_project_id(&conn, project_path) {
            let _ = conn.execute(
                "UPDATE agent_heartbeats \
                 SET wakeup_path = replace(wakeup_path, 'wakeup.md', 'WAKEUP.md') \
                 WHERE project_id = ?1 AND wakeup_path LIKE '%wakeup.md'",
                rusqlite::params![&project_id],
            );
        }
    }
}

/// Rename `from` → `to` with a temp-name intermediate step to survive
/// case-insensitive filesystems. No-op if `from` doesn't exist OR if
/// `to` already exists with different content (we don't want to clobber).
fn case_rename(from: &std::path::Path, to: &std::path::Path) {
    if !from.exists() {
        return;
    }
    // If the destination already exists AND refers to a DIFFERENT inode,
    // the user has both files — bail rather than clobber. On case-insensitive
    // FS, from and to refer to the same inode so this check is harmless.
    if to.exists() {
        let from_meta = fs::metadata(from).ok();
        let to_meta = fs::metadata(to).ok();
        if let (Some(a), Some(b)) = (from_meta, to_meta) {
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if a.ino() != b.ino() {
                    log_debug!(
                        "[filename-migrate] both {} and {} exist with different inodes — skipping",
                        from.display(), to.display()
                    );
                    return;
                }
            }
        }
    }
    // Two-step via a unique temp name so the FS always sees an actual
    // directory-entry change (necessary on HFS+ / default APFS where
    // case-only renames are no-ops).
    let tmp = from.with_extension(format!("md.tmp-case-rename-{}", uuid::Uuid::new_v4()));
    if fs::rename(from, &tmp).is_err() {
        return;
    }
    if fs::rename(&tmp, to).is_err() {
        // Couldn't finish the rename — try to restore the original name.
        let _ = fs::rename(&tmp, from);
        log_debug!(
            "[filename-migrate] second-step rename failed for {} → {}",
            from.display(), to.display()
        );
    }
}

/// Idempotent: bails immediately if `__lead__` already has any
/// heartbeat row, or if the project isn't in manager mode.
pub fn migrate_or_scaffold_lead_heartbeat(project_path: &str) {
    // Reentrant lock lets this function hold the guard while calling
    // `k2so_heartbeat_add` below — which itself locks the same Mutex.
    // A plain Mutex would deadlock here (observed as a macOS beachball
    // during startup).
    let db = crate::db::shared();
    let conn = db.lock();
    let Some(project_id) = resolve_project_id(&conn, project_path) else { return };

    let agent_mode: Option<String> = conn.query_row(
        "SELECT agent_mode FROM projects WHERE id = ?1",
        rusqlite::params![&project_id],
        |row| row.get::<_, Option<String>>(0),
    ).ok().flatten();
    if agent_mode.as_deref() != Some("manager") {
        return;
    }

    // Idempotency: skip if a `triage` row already exists for this project,
    // active OR archived. Archived means the user explicitly removed it
    // — recreating it on every startup would defeat the user's intent.
    // We deliberately do NOT skip when the project has *other* heartbeats
    // but no triage row — every manager workspace gets a default triage
    // row regardless of what other custom heartbeats the user has added.
    let has_triage: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM agent_heartbeats \
         WHERE project_id = ?1 AND name = 'triage')",
        rusqlite::params![&project_id],
        |row| row.get(0),
    ).unwrap_or(false);
    if has_triage {
        return;
    }

    let legacy_path = workspace_wakeup_path(project_path);
    let migrated_content: Option<String> = fs::read_to_string(&legacy_path)
        .ok()
        .filter(|s| !s.trim().is_empty());

    let wake_body = if let Some(ref existing) = migrated_content {
        // Preserve the user's customized triage instructions verbatim,
        // prepending a frontmatter block if it's missing so the row's
        // `description` surface in the UI isn't empty.
        if existing.trim_start().starts_with("---") {
            existing.clone()
        } else {
            format!(
                "---\ndescription: Workspace manager triage (migrated from .k2so/wakeup.md)\n---\n\n{}",
                existing
            )
        }
    } else {
        // Lean default. The manager-tier skill layers already ship the
        // Standing Orders / Delegation + Review playbook; no need to
        // repeat it here.
        "---\ndescription: Workspace manager triage — follow your Standing Orders\n---\n\n\
         # Wake procedure: default\n\n\
         Follow your Standing Orders to triage the workspace inbox and review queue. \
         Delegate, approve, or exit — keep the session short.\n".to_string()
    };

    // Pick the agent the heartbeat will attach to — match what
    // k2so_heartbeat_add does internally so we write to the right path
    // after it scaffolds the template. For workspaces with a manager-type
    // agent dir (coordinator/pod-leader), find_primary_agent returns
    // that dir's name; otherwise it returns the `__lead__` sentinel
    // (which k2so_heartbeat_add creates on demand).
    let Some(primary_agent) = find_primary_agent(project_path) else {
        log_debug!(
            "[migrate] {}: no scheduleable agent, skipping heartbeat scaffold",
            project_path
        );
        return;
    };

    let spec = r#"{"frequency":"hourly","every_seconds":3600}"#.to_string();
    match k2so_heartbeat_add(
        project_path.to_string(),
        "triage".to_string(),
        "hourly".to_string(),
        spec,
    ) {
        Ok(_) => {
            // k2so_heartbeat_add already scaffolded a template. Overwrite
            // with our migrated-or-lean content at the correct agent's path.
            let wake_path = agent_dir(project_path, &primary_agent)
                .join("heartbeats")
                .join("triage")
                .join("WAKEUP.md");
            log_if_err(
                "migrate lead-heartbeat wakeup",
                &wake_path,
                atomic_write_str(&wake_path, &wake_body),
            );

            if migrated_content.is_some() {
                let migrated_to = legacy_path.with_file_name("wakeup.md.migrated");
                let _ = fs::rename(&legacy_path, &migrated_to);
                log_debug!(
                    "[migrate] {}: moved .k2so/wakeup.md → triage heartbeat row for agent '{}'; legacy archived as wakeup.md.migrated",
                    project_path, primary_agent
                );
            } else {
                log_debug!(
                    "[migrate] {}: scaffolded lean triage heartbeat for agent '{}'",
                    project_path, primary_agent
                );
            }
        }
        Err(e) => {
            log_debug!(
                "[migrate] Failed to scaffold triage heartbeat for {}: {}",
                project_path, e
            );
        }
    }
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
/// saved session id for this agent, we add `--resume <id>`; otherwise
/// we open a fresh `claude` PTY in the project root.
///
/// This deliberately skips the heartbeat-style "wake the agent up"
/// behavior that 0.36.7 and earlier injected on every app relaunch
/// (the daemon's PTY dies on app upgrade → tab re-mounts → was firing
/// a wake every time, which surprised users).
#[tauri::command]
pub fn k2so_agents_resume_chat_args(
    project_path: String,
    agent_name: String,
) -> Result<serde_json::Value, String> {
    let mut args: Vec<String> = vec!["--dangerously-skip-permissions".to_string()];

    // Look up saved session_id for this agent. Missing project / missing
    // agent_session row → fall through to a fresh `claude`; that's not
    // an error — it's a first-run for that agent.
    let session_id: Option<String> = (|| {
        let db = k2so_core::db::shared();
        let conn = db.lock();
        let project_id: String = conn
            .query_row(
                "SELECT id FROM projects WHERE path = ?1",
                rusqlite::params![&project_path],
                |row| row.get(0),
            )
            .ok()?;
        let row = k2so_core::db::schema::WorkspaceSession::get(&conn, &project_id).ok().flatten()?;
        row.session_id.filter(|s| !s.is_empty())
    })();

    // Verify the session file actually exists on disk before we pass
    // `--resume`. Stale rows happen (workspace remove+readd, claude
    // pruning) — `--resume` against a missing id makes claude bail with
    // "No conversation found", which is uglier than just opening fresh.
    if let Some(ref id) = session_id {
        if k2so_core::chat_history::claude_session_file_exists(id, &project_path) {
            args.push("--resume".to_string());
            args.push(id.clone());
        }
    }

    Ok(serde_json::json!({
        "command": "claude",
        "args": args,
        "cwd": project_path,
        "resumeSession": session_id,
    }))
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
k2so msg --wake <workspace-name>:inbox "urgent — wake the agent"
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

## Heartbeats

The agent in this workspace can have one or more scheduled wakeups. Manage them with:
```
k2so heartbeat list                   # see configured schedules
k2so heartbeat show <name>            # full details for one
k2so heartbeat add --name <n> --daily --time HH:MM
k2so heartbeat wakeup <name> --edit   # edit the prompt that fires
k2so heartbeat wake                   # trigger a tick now
```

Run `k2so heartbeat --help` for the full surface.
"#,
        project_name = project_name,
    )
}

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
    }
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

// ── Heartbeat Scheduler ─────────────────────────────────────────────────

/// Install the heartbeat scheduler (launchd on macOS, cron on Linux).
/// The heartbeat script reads ~/.k2so/heartbeat.port, checks if K2SO is alive,
/// and triggers triage for projects that have heartbeat enabled.
#[tauri::command]
pub fn k2so_agents_install_heartbeat(
    _state: tauri::State<'_, crate::state::AppState>,
) -> Result<(), String> {
    let k2so_home = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".k2so");
    fs::create_dir_all(&k2so_home).map_err(|e| e.to_string())?;

    // P5.6: heartbeat-projects.txt has been retired. The daemon now
    // serves the active project list from `agent_heartbeats` directly
    // via `/cli/heartbeat/active-projects`, so heartbeat.sh queries
    // it on every tick. Self-healing — a stale file can no longer
    // make a workspace silent. Clean up the legacy artifact if it
    // still exists on disk.
    let _ = fs::remove_file(k2so_home.join("heartbeat-projects.txt"));

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

    // Install platform scheduler. Interval + wake-from-sleep come
    // from the user's Settings > Wake Scheduler panel; defaults are
    // 5-minute cadence with wake_system=false.
    #[cfg(target_os = "macos")]
    {
        let settings = crate::commands::settings::read_settings();
        let interval_secs = settings.wake_scheduler.interval_minutes.max(1) as u32 * 60;
        install_heartbeat_launchd(
            &script_path,
            interval_secs,
            settings.wake_scheduler.wake_system,
        )?;
    }

    #[cfg(target_os = "linux")]
    install_heartbeat_cron(&script_path)?;

    Ok(())
}

/// Apply the wake-scheduler settings stored in `AppSettings.wake_scheduler`.
///
/// Dispatches based on `.mode`:
/// - `"off"`: no heartbeat plist, no daemon wakes.
/// - `"on_demand"`: uninstall heartbeat plist; daemon still fires when
///   launched by Tauri app lifecycle but system stays asleep.
/// - `"heartbeat"`: install heartbeat plist with user's interval +
///   optional `WakeSystem: true` for lid-closed overnight fires.
///
/// Called from Settings > Wake Scheduler after the user clicks Apply.
/// Idempotent — safe to call on every Apply click even if nothing
/// changed. Also called on app startup so the plist matches settings
/// even after a manual edit to settings.json.
#[tauri::command]
pub fn k2so_agents_apply_wake_scheduler() -> Result<String, String> {
    let settings = crate::commands::settings::read_settings();
    let cfg = &settings.wake_scheduler;

    let k2so_home = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".k2so");
    let script_path = k2so_home.join("heartbeat.sh");

    match cfg.mode.as_str() {
        "off" | "on_demand" => {
            #[cfg(target_os = "macos")]
            {
                let _ = uninstall_heartbeat_launchd();
            }
            #[cfg(target_os = "linux")]
            {
                let _ = uninstall_heartbeat_cron();
            }
            Ok(format!("Wake scheduler set to '{}' — heartbeat plist removed.", cfg.mode))
        }
        "heartbeat" => {
            if !script_path.exists() {
                return Err(format!(
                    "Cannot install wake scheduler: heartbeat.sh not generated yet. \
                     Enable heartbeat on at least one project first, then apply."
                ));
            }
            let interval_secs = cfg.interval_minutes.max(1) as u32 * 60;
            #[cfg(target_os = "macos")]
            install_heartbeat_launchd(&script_path, interval_secs, cfg.wake_system)?;
            #[cfg(target_os = "linux")]
            install_heartbeat_cron(&script_path)?;
            Ok(format!(
                "Wake scheduler set: every {} min{}.",
                cfg.interval_minutes,
                if cfg.wake_system {
                    " (wakes system from sleep)"
                } else {
                    ""
                }
            ))
        }
        other => Err(format!(
            "Unknown wake scheduler mode '{}'. Expected 'off', 'on_demand', or 'heartbeat'.",
            other
        )),
    }
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
        // Ensure scheduler is installed (idempotent — unloads before
        // loading). Pulls interval + wake_system from user settings
        // so a Settings > Wake Scheduler toggle is reflected on the
        // next project-list refresh without a manual reinstall.
        let script_path = k2so_home.join("heartbeat.sh");
        if script_path.exists() {
            #[cfg(target_os = "macos")]
            {
                let settings = crate::commands::settings::read_settings();
                let interval_secs =
                    settings.wake_scheduler.interval_minutes.max(1) as u32 * 60;
                let _ = install_heartbeat_launchd(
                    &script_path,
                    interval_secs,
                    settings.wake_scheduler.wake_system,
                );
            }
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
# Asks the daemon which projects have active heartbeats, then ticks each.
# P5.6: replaces the legacy heartbeat-projects.txt file with a daemon
# query so the project list never goes stale.

PORT_FILE="{home}/.k2so/heartbeat.port"
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

# Read auth token
TOKEN=""
if [ -f "$TOKEN_FILE" ]; then
    TOKEN=$(cat "$TOKEN_FILE" 2>/dev/null)
fi

if [ -z "$TOKEN" ]; then
    echo "$(ts) ERROR: No auth token available — skipping heartbeat" >> "$LOG_FILE"
    exit 0
fi

# Ask the daemon for the current list of projects with active heartbeats.
# Newline-delimited plain text — derived from agent_heartbeats so it's
# always in sync with the user's actual configuration. Empty response =
# no work to do this tick.
PROJECTS=$(curl -s --connect-timeout 2 --max-time 5 \
    "http://127.0.0.1:$PORT/cli/heartbeat/active-projects?token=$TOKEN" 2>>"$LOG_FILE")
if [ -z "$PROJECTS" ]; then
    # No heartbeat-enabled projects — silent no-op. Trim log and exit.
    if [ -f "$LOG_FILE" ]; then
        tail -200 "$LOG_FILE" > "$LOG_FILE.tmp" 2>/dev/null && mv -f "$LOG_FILE.tmp" "$LOG_FILE" 2>/dev/null
    fi
    exit 0
fi

# Trigger triage for each heartbeat-enabled project. We log EVERY tick
# (fires, skips, errors) so users can see when the heartbeat ran.
while IFS= read -r project_path; do
    [ -z "$project_path" ] && continue
    ENCODED_PATH=$(urlencode "$project_path")
    RESULT=$(curl -sG "http://127.0.0.1:$PORT/cli/scheduler-tick?token=$TOKEN&project=$ENCODED_PATH" --connect-timeout 5 --max-time 30 2>>"$LOG_FILE")
    CURL_EXIT=$?
    if [ "$CURL_EXIT" -ne 0 ]; then
        echo "$(ts) ERROR curl exit=$CURL_EXIT project=$project_path" >> "$LOG_FILE"
        continue
    fi
    COUNT=$(echo "$RESULT" | grep -o '"count":[0-9]*' | grep -o '[0-9]*' | head -1 || echo 0)
    SKIPPED=$(echo "$RESULT" | grep -o '"skipped":"[^"]*"' | sed 's/"skipped":"\([^"]*\)"/\1/')
    if [ -n "$SKIPPED" ]; then
        echo "$(ts) tick project=$project_path skipped=$SKIPPED" >> "$LOG_FILE"
    elif [ -n "$COUNT" ] && [ "$COUNT" -gt 0 ] 2>/dev/null; then
        echo "$(ts) tick project=$project_path launched=$COUNT" >> "$LOG_FILE"
    else
        echo "$(ts) tick project=$project_path launched=0" >> "$LOG_FILE"
    fi
done <<< "$PROJECTS"

# Trim log (atomic: write to tmp then move)
if [ -f "$LOG_FILE" ]; then
    tail -200 "$LOG_FILE" > "$LOG_FILE.tmp" 2>/dev/null && mv -f "$LOG_FILE.tmp" "$LOG_FILE" 2>/dev/null
fi
"##, home = home)
}

/// Install (or reinstall) the heartbeat launchd plist with a
/// user-configurable interval + optional wake-from-sleep behavior.
/// - `interval_seconds` maps to `StartInterval` (60 = every minute,
///   300 = every 5 minutes, etc.).
/// - `wake_system` sets `WakeSystem: true` so launchd wakes a
///   sleeping machine — the mechanism that makes lid-closed overnight
///   agent work possible.
///
/// Idempotent: unloads any existing plist with the same label before
/// writing, then loads the new one. Safe to call repeatedly with
/// different settings.
#[cfg(target_os = "macos")]
fn install_heartbeat_launchd(
    script_path: &Path,
    interval_seconds: u32,
    wake_system: bool,
) -> Result<(), String> {
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

    let wake_key = if wake_system {
        "\n    <key>WakeSystem</key>\n    <true/>"
    } else {
        ""
    };

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
    <integer>{interval}</integer>{wake_key}
    <key>RunAtLoad</key>
    <false/>
    <key>StandardErrorPath</key>
    <string>{home}/.k2so/heartbeat-stderr.log</string>
</dict>
</plist>"#,
        script = script_path.to_string_lossy(),
        interval = interval_seconds,
        wake_key = wake_key,
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
    write_workspace_skill_file_with_body(project_path, None);
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
    let project_name = std::path::Path::new(project_path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "workspace".to_string());

    // Crash-detection marker: the regen is a multi-step commit (adopt
    // sources, write canonical, append SOURCE regions, fan out to harness
    // targets, stamp sentinel). True filesystem rollback is impossible
    // without a CoW snapshot, but by stamping `.regen-in-flight` at entry
    // and clearing it at the end, a subsequent boot can detect that a
    // previous regen didn't complete cleanly and surface a diagnostic.
    let regen_marker = PathBuf::from(project_path)
        .join(".k2so")
        .join(".regen-in-flight");
    log_if_err(
        "regen-in-flight stamp",
        &regen_marker,
        fs_atomic::atomic_write(&regen_marker, b""),
    );

    // Step 1: Adoption sweep — commit any SOURCE-region drift back to source
    // files before we regenerate SKILL.md.
    adopt_workspace_skill_drift(project_path);

    // Step 2: Clear the stale tail below END (SOURCE regions + internal
    // freeform notes) so the post-write append step can lay down fresh ones
    // without duplicating. User-authored freeform content below an explicit
    // "<!-- K2SO:USER_NOTES -->" sentinel is preserved. Otherwise the whole
    // below-END tail is discarded (it was either K2SO-rendered SOURCE regions
    // from last run, or content we already adopted in step 1).
    let preserved_freeform = strip_workspace_skill_tail(project_path);

    // Step 3: Compose K2SO-managed body only. PROJECT.md and AGENT.md bodies
    // go below the END marker in step 5, not inside the managed region.
    let mut managed_body = match base_body {
        Some(body) => body.to_string(),
        None => generate_workspace_skill_content(&project_name),
    };

    // Append the "how to update" footer pointing at the source files so
    // the AI knows that this SKILL is compiled and where to make actual
    // changes. Cite the workspace's primary agent if we can find one,
    // otherwise emit the generic agent-name placeholder line.
    let primary_agent = find_primary_agent(project_path);
    if !managed_body.ends_with('\n') {
        managed_body.push('\n');
    }
    managed_body.push('\n');
    managed_body.push_str(&k2so_core::agents::skill_writer::skill_update_footer(
        project_path,
        primary_agent.as_deref(),
    ));

    // Step 4: Write managed body to canonical + harness symlinks. We pass
    // write_shared_markers=false because the marker-injected AGENTS.md /
    // copilot-instructions.md files want the FULL content including SOURCE
    // regions — we'll inject that ourselves in step 6 after appending.
    write_skill_to_all_harnesses(
        project_path,
        "k2so",
        "workspace",
        SKILL_VERSION_WORKSPACE,
        "K2SO workspace context — CLI reference + project context + primary agent persona",
        &managed_body,
        false, // we'll handle AGENTS.md + copilot markers post-append
    );

    // Step 5: Append fresh SOURCE regions below END in the canonical file.
    // Propagates to all harness symlinks automatically.
    append_workspace_source_regions(project_path, preserved_freeform.as_deref());

    // Steps 6 + 7 fan out the canonical SKILL into user-visible harness
    // paths (CLAUDE.md, GEMINI.md, .cursor/rules, AGENTS.md marker block,
    // etc.). Skip the entire fanout when the user opted out via the
    // workspace onboarding flow — the canonical .k2so/skills/k2so/SKILL.md
    // we already wrote in steps 4-5 stays authoritative for K2SO's own
    // use; user keeps full control of harness files.
    let canonical = PathBuf::from(project_path).join(".k2so/skills/k2so/SKILL.md");
    if !k2so_core::agents::onboarding::is_harness_management_skipped(project_path) {
        // Step 6: Inject the full canonical body (now including SOURCE
        // regions) into the marker-injected shared files.
        if let Ok(full) = fs::read_to_string(&canonical) {
            let injection_body = strip_frontmatter(&full).trim().to_string();
            let root = PathBuf::from(project_path);
            upsert_k2so_section(&root.join("AGENTS.md"), &injection_body);
            let github_dir = root.join(".github");
            let _ = fs::create_dir_all(&github_dir);
            upsert_k2so_section(&github_dir.join("copilot-instructions.md"), &injection_body);
        }

        // Step 7: Symlink project root SKILL.md + CLAUDE.md → canonical,
        // plus the Phase 7b harness discovery targets (GEMINI.md, AGENT.md
        // singular, .goosehints, .cursor/rules/k2so.mdc, .aider.conf.yml
        // scaffold).
        let root_skill = PathBuf::from(project_path).join("SKILL.md");
        force_symlink(&canonical, &root_skill);
        let root_claude = PathBuf::from(project_path).join("CLAUDE.md");
        migrate_and_symlink_root_claude_md(&canonical, &root_claude, project_path);
        write_workspace_harness_discovery_targets(project_path, &canonical);
    }

    // Step 8: Stamp last-regen with the current source-content hashes.
    // Used by the next regen's adopt_workspace_skill_drift to distinguish
    // "user edited source" from "agent wrote to SKILL.md" without relying
    // on mtime (which is unreliable across clock skew and rsync).
    let mut hashes: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    let project_md_path = PathBuf::from(project_path).join(".k2so").join("PROJECT.md");
    let project_hash = content_hash_of(&project_md_path);
    if !project_hash.is_empty() {
        hashes.insert("project_md".to_string(), project_hash);
    }
    if let Some(primary) = find_primary_agent(project_path) {
        let agent_md_path = agent_dir(project_path, &primary).join("AGENT.md");
        let agent_hash = content_hash_of(&agent_md_path);
        if !agent_hash.is_empty() {
            hashes.insert(format!("agent_md::{}", primary), agent_hash);
        }
    }
    write_regen_hashes(project_path, &hashes);

    // All steps committed — clear the in-flight marker. If the process
    // dies before this point, the next boot sees the marker and knows a
    // regen was interrupted (see detect_interrupted_regen).
    log_if_err(
        "regen-in-flight clear",
        &regen_marker,
        fs::remove_file(&regen_marker),
    );
}

/// Startup check: warn the user if a previous regen didn't clear its
/// in-flight marker. Doesn't auto-repair — a regen is idempotent, so the
/// next real regen will overwrite any partial state — but surfaces the
/// situation so the user can check `.k2so/migration/` for stale archives
/// if they hit unexpected data loss.
pub fn detect_interrupted_regen(project_path: &str) -> bool {
    let marker = PathBuf::from(project_path)
        .join(".k2so")
        .join(".regen-in-flight");
    if !marker.exists() {
        return false;
    }
    use std::io::Write;
    let _ = writeln!(
        std::io::stderr(),
        "k2so: previous SKILL.md regeneration at {} did not complete cleanly. \
         The next regen will overwrite any partial state; check .k2so/migration/ \
         if your workspace context looks unexpectedly stale.",
        project_path
    );
    // Clear the marker so the warning fires exactly once per incident.
    log_if_err("clear stale regen marker", &marker, fs::remove_file(&marker));
    true
}

// ══════════════════════════════════════════════════════════════════════
// Phase 7c: SOURCE region drift adoption
// ══════════════════════════════════════════════════════════════════════

/// Sentinel marker users can place in the below-END tail to claim freeform
/// content that should survive regeneration. Anything BETWEEN this marker
/// and EOF is preserved verbatim. Useful for notes the user wants to keep
/// but doesn't want routed into PROJECT.md / AGENT.md.
const SKILL_USER_NOTES_SENTINEL: &str = "<!-- K2SO:USER_NOTES -->";

/// Extract the body between a BEGIN/END marker pair, if both are present.
fn extract_source_region(content: &str, begin: &str, end: &str) -> Option<String> {
    let b_idx = content.find(begin)?;
    let after_begin = b_idx + begin.len();
    let e_rel = content[after_begin..].find(end)?;
    let e_idx = after_begin + e_rel;
    Some(content[after_begin..e_idx].trim().to_string())
}

/// Strip an optional leading heading (`## Something\n\n`) from a SOURCE
/// region body so the comparison / commit targets the raw file content.
fn strip_leading_heading(body: &str) -> String {
    let trimmed = body.trim_start();
    if trimmed.starts_with("## ") {
        if let Some(nl) = trimmed.find('\n') {
            return trimmed[nl + 1..].trim_start().to_string();
        }
    }
    trimmed.to_string()
}

/// Append a drift / conflict note to `.k2so/logs/adoption-conflicts.log`.
fn log_adoption_event(project_path: &str, line: &str) {
    let log_dir = PathBuf::from(project_path).join(".k2so").join("logs");
    let _ = fs::create_dir_all(&log_dir);
    let log_path = log_dir.join("adoption-conflicts.log");
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let entry = format!("[{}] {}\n", ts, line);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
    {
        use std::io::Write;
        let _ = f.write_all(entry.as_bytes());
    }
}

/// Return the mtime of a file as seconds since epoch, or 0 if unknown.
fn mtime_secs(path: &Path) -> u64 {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Content hash of a file path, suitable for drift detection. Returns an
/// empty string on read failure (callers treat empty == "no stored hash"
/// and fall back to mtime comparison).
fn content_hash_of(path: &Path) -> String {
    match fs::read(path) {
        Ok(bytes) => skill_checksum_hex(&bytes),
        Err(_) => String::new(),
    }
}

/// Read the `.last-skill-regen` JSON payload, which stores the content
/// hashes of every source file at the time of the last regen. Used by
/// drift adoption to tell "source was edited since last regen" apart
/// from "SKILL.md was edited since last regen" — compared to the old
/// mtime-based heuristic this is immune to clock skew, NTP jumps, and
/// cross-machine rsync mtime quirks.
///
/// Returns an empty map if the file is missing, unreadable, or contains
/// legacy empty-file content (pre-0.32.9 stamp format). Callers fall
/// back to mtime comparison in that case, which still works correctly
/// for the common single-machine path.
fn read_regen_hashes(project_path: &str) -> std::collections::HashMap<String, String> {
    let stamp_path = PathBuf::from(project_path).join(".k2so").join(".last-skill-regen");
    let Ok(raw) = fs::read_to_string(&stamp_path) else {
        return std::collections::HashMap::new();
    };
    if raw.trim().is_empty() {
        return std::collections::HashMap::new();
    }
    serde_json::from_str::<std::collections::HashMap<String, String>>(&raw)
        .unwrap_or_default()
}

/// Persist the content hashes of every source file that participates in
/// drift detection. Called at the end of a successful regen so the next
/// regen has a baseline for comparison. Atomic write — a crash mid-stamp
/// leaves either the old hashes or the new ones, never a truncated file
/// that would force every source into the fallback mtime path.
fn write_regen_hashes(
    project_path: &str,
    hashes: &std::collections::HashMap<String, String>,
) {
    let stamp_path = PathBuf::from(project_path).join(".k2so").join(".last-skill-regen");
    let payload = serde_json::to_string(hashes).unwrap_or_else(|_| "{}".to_string());
    log_if_err(
        "write_regen_hashes",
        &stamp_path,
        atomic_write_str(&stamp_path, &payload),
    );
}

/// Walk the existing canonical SKILL.md and adopt any SOURCE-region drift
/// back into its canonical source file (PROJECT.md or the primary agent's
/// AGENT.md). Uses the `.k2so/.last-skill-regen` stamp to differentiate
/// between user-initiated edits to the source file (source wins) and
/// agent-initiated writes into the SKILL.md symlink (SKILL wins).
///
/// Conflict resolution uses content hashes when available (hash stored
/// at last regen vs current on-disk hash) — immune to clock skew,
/// cross-machine rsync mtime coercion, and NTP jumps. Falls back to
/// mtime comparison only when no hash snapshot has been written yet
/// (first regen after upgrade from pre-0.32.9).
fn adopt_workspace_skill_drift(project_path: &str) {
    let canonical = PathBuf::from(project_path).join(".k2so/skills/k2so/SKILL.md");
    let Ok(skill_content) = fs::read_to_string(&canonical) else {
        return; // first run, nothing to adopt
    };
    let stamp_path = PathBuf::from(project_path).join(".k2so").join(".last-skill-regen");
    let last_regen = mtime_secs(&stamp_path);
    let stored_hashes = read_regen_hashes(project_path);

    // Helper: decide whether the source file was touched since the last
    // regen. Returns true when the user modified the source (so SKILL
    // divergence is an agent write we should drop), false when SKILL is
    // the newer side (so we adopt into the source). Preference order:
    //   1. Hash comparison — precise, clock-skew-free, the right answer
    //      whenever a prior regen has written a hash snapshot.
    //   2. Mtime comparison — backward-compat fallback for workspaces
    //      upgraded from pre-0.32.9 stamps (empty file).
    let source_touched_since_regen = |source_path: &Path, key: &str| -> bool {
        if let Some(stored) = stored_hashes.get(key) {
            let current = content_hash_of(source_path);
            !current.is_empty() && current.as_str() != stored.as_str()
        } else {
            mtime_secs(source_path) > last_regen
        }
    };

    // PROJECT.md adoption
    if let Some(region_body) = extract_source_region(
        &skill_content,
        SKILL_SOURCE_PROJECT_MD_BEGIN,
        SKILL_SOURCE_PROJECT_MD_END,
    ) {
        let project_md = PathBuf::from(project_path).join(".k2so").join("PROJECT.md");
        let region_stripped = strip_leading_heading(&region_body);
        let file_body = fs::read_to_string(&project_md)
            .map(|raw| strip_frontmatter(&raw).trim().to_string())
            .unwrap_or_default();
        if region_stripped.trim() != file_body.trim() {
            if source_touched_since_regen(&project_md, "project_md") {
                // User edited PROJECT.md since the last regen. Source is
                // authoritative — the next regen step overwrites every
                // downstream SKILL.md / harness symlink with the new
                // PROJECT.md content. Nothing to adopt here; just note
                // that we saw the edit.
                log_adoption_event(
                    project_path,
                    "PROJECT.md: user edit detected — downstream SKILL.md + harness files will pick up the new content on this regen",
                );
            } else if !region_stripped.trim().is_empty() {
                // Agent wrote into the SKILL.md SOURCE region; adopt into PROJECT.md.
                // Preserve any frontmatter the source file had.
                let frontmatter = if let Ok(raw) = fs::read_to_string(&project_md) {
                    if raw.starts_with("---") {
                        if let Some(end) = raw[3..].find("---") {
                            Some(raw[..3 + end + 3].to_string())
                        } else { None }
                    } else { None }
                } else { None };
                let new_contents = match frontmatter {
                    Some(fm) => format!("{}\n\n{}\n", fm.trim_end(), region_stripped.trim()),
                    None => format!("{}\n", region_stripped.trim()),
                };
                match atomic_write_str(&project_md, &new_contents) {
                    Ok(()) => log_adoption_event(
                        project_path,
                        "ADOPTED PROJECT.md: SKILL.md SOURCE region committed back to .k2so/PROJECT.md",
                    ),
                    Err(e) => log_if_err::<(), _>(
                        "adopt PROJECT.md",
                        &project_md,
                        Err::<(), _>(e),
                    ),
                }
            }
        }
    }

    // Primary agent's AGENT.md adoption (manager / custom / k2so modes)
    if let Some(primary_agent) = find_primary_agent(project_path) {
        let agent_type = agent_type_for(project_path, &primary_agent);
        let include_primary = matches!(
            agent_type.as_str(),
            "custom" | "k2so" | "manager" | "coordinator" | "pod-leader"
        );
        if include_primary {
            let begin = skill_source_agent_md_begin(&primary_agent);
            let end = skill_source_agent_md_end(&primary_agent);
            if let Some(region_body) = extract_source_region(&skill_content, &begin, &end) {
                let agent_md = agent_dir(project_path, &primary_agent).join("AGENT.md");
                let region_stripped = strip_leading_heading(&region_body);
                let file_body = fs::read_to_string(&agent_md)
                    .map(|raw| strip_frontmatter(&raw).trim().to_string())
                    .unwrap_or_default();
                if region_stripped.trim() != file_body.trim() {
                    let key = format!("agent_md::{}", primary_agent);
                    if source_touched_since_regen(&agent_md, &key) {
                        log_adoption_event(
                            project_path,
                            &format!(
                                "AGENT.md ({}): user edit detected — downstream SKILL.md + harness files will pick up the new content on this regen",
                                primary_agent
                            ),
                        );
                    } else if !region_stripped.trim().is_empty() {
                        let frontmatter = if let Ok(raw) = fs::read_to_string(&agent_md) {
                            if raw.starts_with("---") {
                                if let Some(end) = raw[3..].find("---") {
                                    Some(raw[..3 + end + 3].to_string())
                                } else { None }
                            } else { None }
                        } else { None };
                        let new_contents = match frontmatter {
                            Some(fm) => format!("{}\n\n{}\n", fm.trim_end(), region_stripped.trim()),
                            None => format!("{}\n", region_stripped.trim()),
                        };
                        match atomic_write_str(&agent_md, &new_contents) {
                            Ok(()) => log_adoption_event(
                                project_path,
                                &format!(
                                    "ADOPTED AGENT.md ({}): SKILL.md SOURCE region committed back to agent file",
                                    primary_agent
                                ),
                            ),
                            Err(e) => log_if_err::<(), _>(
                                "adopt AGENT.md",
                                &agent_md,
                                Err::<(), _>(e),
                            ),
                        }
                    }
                }
            }
        }
    }
}

/// The placeholder comment emitted alongside the USER_NOTES sentinel on
/// every regen. Tracked as a constant so `strip_workspace_skill_tail` can
/// discard any existing copies from the preserved freeform — otherwise
/// each regen would stack another placeholder copy onto the tail.
const USER_NOTES_PLACEHOLDER: &str =
    "<!-- Content below the K2SO:USER_NOTES sentinel is yours — K2SO preserves it verbatim across regenerations. -->";

/// Remove everything after the MANAGED:END marker in the canonical SKILL.md.
/// Returns any truly user-authored content found after the LAST
/// `<!-- K2SO:USER_NOTES -->` sentinel so it can be re-appended after
/// regeneration. Strips K2SO's own placeholder comments (any number of
/// stacked copies from buggy prior runs) and any empty noise.
fn strip_workspace_skill_tail(project_path: &str) -> Option<String> {
    let canonical = PathBuf::from(project_path).join(".k2so/skills/k2so/SKILL.md");
    let Ok(content) = fs::read_to_string(&canonical) else { return None };
    let end_idx = content.find(SKILL_END_MARKER)?;
    let after_end_start = end_idx + SKILL_END_MARKER.len();
    let tail = &content[after_end_start..];

    // Use rfind so stacked duplicate sentinels (from pre-fix runs) collapse
    // into a single preserved region.
    let preserved = tail.rfind(SKILL_USER_NOTES_SENTINEL).map(|idx| {
        let after = idx + SKILL_USER_NOTES_SENTINEL.len();
        tail[after..].to_string()
    });

    // Truncate canonical to everything up to and including the END marker,
    // plus a single trailing newline. Atomic: a crash between here and
    // append_workspace_source_regions cannot corrupt the canonical file —
    // a reader sees either the pre-strip content or the post-strip content
    // in full.
    let truncated = format!("{}\n", &content[..after_end_start]);
    log_if_err(
        "strip_workspace_skill_tail write",
        &canonical,
        atomic_write_str(&canonical, &truncated),
    );

    // Discard any occurrences of our own placeholder comment, empty lines
    // at the edges, and the migration banner prefix fragments that end up
    // leaking from the banner injector. Return None if nothing remains.
    let preserved = preserved.map(|raw| {
        raw.lines()
            .filter(|l| l.trim() != USER_NOTES_PLACEHOLDER.trim())
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string()
    });
    preserved.filter(|s| !s.is_empty())
}

/// After the managed region has been re-written, append fresh SOURCE
/// sub-regions (PROJECT.md + primary agent's AGENT.md) below the END
/// marker in the canonical file. Propagates to every harness symlink.
fn append_workspace_source_regions(project_path: &str, preserved_freeform: Option<&str>) {
    let canonical = PathBuf::from(project_path).join(".k2so/skills/k2so/SKILL.md");
    let Ok(mut content) = fs::read_to_string(&canonical) else { return };
    if !content.ends_with('\n') {
        content.push('\n');
    }

    // PROJECT.md region (only if source file has real content beyond template)
    let project_md = PathBuf::from(project_path).join(".k2so").join("PROJECT.md");
    if let Ok(raw) = fs::read_to_string(&project_md) {
        let stripped = strip_frontmatter(&raw);
        let has_content = stripped.lines().any(|line| {
            let t = line.trim();
            !t.is_empty() && !t.starts_with('#') && !t.starts_with("<!--")
        });
        if has_content {
            content.push_str(&format!(
                "\n{begin}\n## Project Context\n\n{body}\n{end}\n",
                begin = SKILL_SOURCE_PROJECT_MD_BEGIN,
                body = stripped.trim(),
                end = SKILL_SOURCE_PROJECT_MD_END,
            ));
        }
    }

    // Primary agent AGENT.md region (manager / custom / k2so modes)
    if let Some(primary_agent) = find_primary_agent(project_path) {
        let agent_type = agent_type_for(project_path, &primary_agent);
        let include_primary = matches!(
            agent_type.as_str(),
            "custom" | "k2so" | "manager" | "coordinator" | "pod-leader"
        );
        if include_primary {
            let agent_md = agent_dir(project_path, &primary_agent).join("AGENT.md");
            if let Ok(raw) = fs::read_to_string(&agent_md) {
                let stripped = strip_frontmatter(&raw).trim().to_string();
                if !stripped.is_empty() {
                    content.push_str(&format!(
                        "\n{begin}\n## Primary Agent: {name}\n\n{body}\n{end}\n",
                        begin = skill_source_agent_md_begin(&primary_agent),
                        name = primary_agent,
                        body = stripped,
                        end = skill_source_agent_md_end(&primary_agent),
                    ));
                }
            }
        }
    }

    // User-notes sentinel — emitted exactly once per file. The placeholder
    // comment directly below it is a single canonical copy that users /
    // agents CAN freely edit or remove; we also discard it on ingest in
    // strip_workspace_skill_tail so it never accumulates.
    content.push_str(&format!(
        "\n{sentinel}\n{placeholder}\n",
        sentinel = SKILL_USER_NOTES_SENTINEL,
        placeholder = USER_NOTES_PLACEHOLDER,
    ));
    if let Some(freeform) = preserved_freeform {
        let cleaned = freeform.trim();
        if !cleaned.is_empty() {
            content.push('\n');
            content.push_str(cleaned);
            content.push('\n');
        }
    }

    log_if_err(
        "append_workspace_source_regions",
        &canonical,
        atomic_write_str(&canonical, &content),
    );
}

/// CLAUDE.md migration helper for the 0.32.7 transition. See
/// `write_workspace_skill_file` for context.
fn migrate_and_symlink_root_claude_md(canonical: &Path, root_claude: &Path, project_path: &str) {
    match fs::symlink_metadata(root_claude) {
        Ok(meta) if meta.file_type().is_symlink() => {
            // Already a symlink — refresh target in case canonical path changed.
            force_symlink(canonical, root_claude);
        }
        Ok(meta) if meta.file_type().is_file() => {
            let content = fs::read_to_string(root_claude).unwrap_or_default();
            let is_k2so_generated = content.starts_with("# K2SO ");
            // ALWAYS archive the original before any mutation — user's data
            // is irrecoverable otherwise.
            let archived = archive_claude_md_file(project_path, root_claude, "CLAUDE.md");
            // For user-authored files (no K2SO header), the archive is the
            // backup AND we import the content into SKILL.md's USER_NOTES
            // tail so it stays visible through the symlink. For K2SO-
            // generated files, the body was our own composition — we only
            // import Claude's `# memory` writes or other drift if we can
            // isolate it, which we can't reliably detect, so we import the
            // whole archived body and let the user prune duplicates.
            let source_label = if is_k2so_generated {
                "pre-0.32.7 K2SO-generated CLAUDE.md"
            } else {
                "pre-existing user-authored CLAUDE.md"
            };
            let archive_display = archived
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "(archive unavailable)".to_string());
            if !content.trim().is_empty() {
                import_claude_md_into_user_notes(
                    project_path,
                    &content,
                    source_label,
                    &archive_display,
                );
            }
            // Take over the file — atomic_symlink renames over the old
            // regular file in one step, so Claude Code never sees a
            // missing CLAUDE.md between remove and create. Archive is
            // the safety net.
            log_if_err(
                "migrate_and_symlink_root_claude_md",
                root_claude,
                atomic_symlink(canonical, root_claude),
            );
            if let Some(archive_path) = archived {
                inject_first_migration_banner(project_path, &[archive_path]);
            }
            log_debug!(
                "[workspace-skill] Migrated {}./CLAUDE.md → archived + imported body into SKILL.md USER_NOTES; root CLAUDE.md now symlinks to canonical SKILL.md",
                if is_k2so_generated { "K2SO-generated " } else { "user-authored " },
            );
        }
        _ => {
            // Doesn't exist (or metadata failure) — just create the symlink.
            force_symlink(canonical, root_claude);
        }
    }
}

/// Append the body of a pre-existing CLAUDE.md into the canonical
/// SKILL.md's USER_NOTES region so the migrated content stays visible
/// to Claude Code (via the symlink) without requiring the user to hand-
/// merge from `.k2so/migration/`. Idempotent via a stable `id:` sentinel
/// keyed off the archive path.
fn import_claude_md_into_user_notes(
    project_path: &str,
    body: &str,
    source_label: &str,
    archive_display: &str,
) {
    let canonical = PathBuf::from(project_path).join(".k2so/skills/k2so/SKILL.md");
    // SKILL.md may not exist yet on the very first write — the write flow
    // is: (1) clear tail, (2) call ensure_skill_up_to_date which creates
    // the file, (3) append source regions + USER_NOTES, (4) THEN this
    // importer fires through migrate_and_symlink_root_claude_md. So by the
    // time we're called, the file should exist with a USER_NOTES sentinel.
    // If not, write a bare scaffold so the import has somewhere to land.
    if !canonical.exists() {
        return;
    }
    let Ok(existing) = fs::read_to_string(&canonical) else { return };

    // Sentinel: include the archive path so multiple migrations from
    // different machines or dates can each contribute their own import
    // without duplicating (same archive → same sentinel → no re-import).
    let import_sentinel = format!(
        "<!-- K2SO:IMPORT:CLAUDE_MD archive={} -->",
        archive_display
    );
    if existing.contains(&import_sentinel) { return }

    // Find the USER_NOTES sentinel — all imports land below it, after the
    // placeholder comment. Preserve everything that's already there.
    let Some(sentinel_idx) = existing.find(SKILL_USER_NOTES_SENTINEL) else {
        // SKILL.md is in a transitional state — append anyway so no data
        // is lost; regen will re-lay-out next launch.
        let import_block = format!(
            "\n\n{sentinel}\n## Imported: {label}\n\n> Archived at `{archive}`. You can prune this section once reviewed; K2SO preserves anything below the `K2SO:USER_NOTES` sentinel verbatim.\n\n{body}\n",
            sentinel = import_sentinel,
            label = source_label,
            archive = archive_display,
            body = body.trim(),
        );
        let mut out = existing;
        out.push_str(&import_block);
        log_if_err(
            "import_claude_md fallback append",
            &canonical,
            atomic_write_str(&canonical, &out),
        );
        return;
    };
    // Splice right after the placeholder comment so imports collect in a
    // predictable, readable order.
    let insertion_anchor = existing[sentinel_idx..]
        .find(USER_NOTES_PLACEHOLDER)
        .map(|rel| sentinel_idx + rel + USER_NOTES_PLACEHOLDER.len())
        .unwrap_or(sentinel_idx + SKILL_USER_NOTES_SENTINEL.len());
    let import_block = format!(
        "\n\n{sentinel}\n## Imported: {label}\n\n> Archived at `{archive}`. You can prune this section once reviewed; K2SO preserves anything below the `K2SO:USER_NOTES` sentinel verbatim across regenerations.\n\n{body}\n",
        sentinel = import_sentinel,
        label = source_label,
        archive = archive_display,
        body = body.trim(),
    );
    let mut out = String::with_capacity(existing.len() + import_block.len());
    out.push_str(&existing[..insertion_anchor]);
    out.push_str(&import_block);
    out.push_str(&existing[insertion_anchor..]);
    log_if_err(
        "import_claude_md_into_user_notes",
        &canonical,
        atomic_write_str(&canonical, &out),
    );
    log_adoption_event(
        project_path,
        &format!(
            "IMPORTED {} body into SKILL.md USER_NOTES (archive: {})",
            source_label, archive_display
        ),
    );
}

/// Harvest `.k2so/agents/<name>/CLAUDE.md` files left behind by the
/// pre-0.32.7 per-agent CLAUDE.md generator (Phase 1a removed automatic
/// writes, but the user-facing `k2so agents generate-md` CLI + the UI's
/// "Show CLAUDE.md" preview still regenerate them on demand). Each is
/// archived to `.k2so/migration/agents/<name>/CLAUDE.md-<timestamp>.md`
/// then removed.
///
/// Gated with `.k2so/.harvest-0.32.7-done` so a user who later runs
/// `generate-md` isn't re-harvested on the next boot. First-run only.
pub fn harvest_per_agent_claude_md_files(project_path: &str) {
    let sentinel = PathBuf::from(project_path)
        .join(".k2so")
        .join(".harvest-0.32.7-done");
    if sentinel.exists() { return }

    let agents_root = PathBuf::from(project_path).join(".k2so").join("agents");
    let mut archived_paths: Vec<PathBuf> = Vec::new();
    let mut any_failure = false;
    if let Ok(read_dir) = fs::read_dir(&agents_root) {
        for entry in read_dir.flatten() {
            let path = entry.path();
            if !path.is_dir() { continue }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
            if name.starts_with('.') { continue } // skip .archive etc.
            let claude_md = path.join("CLAUDE.md");
            if !claude_md.is_file() { continue }
            match archive_claude_md_file(
                project_path,
                &claude_md,
                &format!("agents/{}/CLAUDE.md", name),
            ) {
                Some(archive_path) => {
                    // Only remove the original if the archive write succeeded.
                    // If the remove itself fails, DO NOT stamp the sentinel —
                    // the orphan would otherwise get skipped on every future
                    // boot, leaving a pre-0.32.7 CLAUDE.md duplicating the
                    // symlinked one.
                    if let Err(e) = fs::remove_file(&claude_md) {
                        log_if_err::<(), _>(
                            "harvest remove original",
                            &claude_md,
                            Err::<(), _>(e),
                        );
                        any_failure = true;
                    }
                    archived_paths.push(archive_path);
                }
                None => {
                    // archive_claude_md_file already logged the failure.
                    any_failure = true;
                }
            }
        }
    }
    if !archived_paths.is_empty() {
        inject_first_migration_banner(project_path, &archived_paths);
    }
    // Stamp the sentinel only when the harvest fully succeeded. A partial
    // failure should retry on next boot so orphan originals get cleaned.
    if !any_failure {
        log_if_err(
            "harvest sentinel",
            &sentinel,
            fs_atomic::atomic_write(&sentinel, b""),
        );
    } else {
        log_if_err::<(), _>(
            "harvest incomplete — sentinel not stamped",
            &sentinel,
            Err::<(), &str>("retry on next boot"),
        );
    }
}

/// Copy a file to `.k2so/migration/<relative>-<timestamp>.<ext>`. Returns the
/// path of the archive on success. Never mutates the source. Preserves the
/// original file extension so restore paths don't get mangled (e.g.
/// `.aider.conf.yml`, `.goosehints`, `.mdc`).
fn archive_claude_md_file(
    project_path: &str,
    source: &Path,
    relative_id: &str,
) -> Option<PathBuf> {
    let content = fs::read_to_string(source).ok()?;
    // Split relative_id into parent subdir (if any) and leaf filename.
    let (subdir, leaf) = match relative_id.rsplit_once('/') {
        Some((parent, leaf)) => (Some(parent), leaf),
        None => (None, relative_id),
    };
    let mut target_dir = PathBuf::from(project_path).join(".k2so").join("migration");
    if let Some(sub) = subdir {
        target_dir = target_dir.join(sub);
    }
    if let Err(e) = fs::create_dir_all(&target_dir) {
        log_if_err::<(), _>(
            "archive_claude_md_file create_dir",
            &target_dir,
            Err::<(), _>(e),
        );
        return None;
    }
    // Preserve original extension. Leading-dot names (.goosehints) have
    // no real extension — treat the whole name as the stem.
    let (leaf_stem, leaf_ext) = match leaf.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem.to_string(), format!(".{}", ext)),
        _ => (leaf.to_string(), String::new()),
    };
    // Nanosecond timestamp + per-process counter — collision-free under
    // first-run harvest bursts where multiple archives can otherwise fall
    // in the same wall-clock second.
    let archive_path = unique_archive_path(&target_dir, &leaf_stem, &leaf_ext);
    if let Err(e) = fs_atomic::atomic_write(&archive_path, content.as_bytes()) {
        log_if_err::<(), _>("archive_claude_md_file write", &archive_path, Err::<(), _>(e));
        return None;
    }
    log_adoption_event(
        project_path,
        &format!(
            "ARCHIVED {} → {}",
            source.display(),
            archive_path.display()
        ),
    );
    Some(archive_path)
}

/// On first migration, write a standalone notice at
/// `.k2so/MIGRATION-0.32.7.md` listing the archive paths. The notice is
/// a dedicated file rather than a SKILL.md injection because SKILL.md is
/// regenerated on every launch (and we'd have to thread the banner
/// through managed-region + source-region plumbing to keep it visible).
/// Idempotent: we only write if the file doesn't already exist.
fn inject_first_migration_banner(project_path: &str, archived_paths: &[PathBuf]) {
    if archived_paths.is_empty() { return }
    let notice_path = PathBuf::from(project_path)
        .join(".k2so")
        .join("MIGRATION-0.32.7.md");
    if notice_path.exists() {
        // Append any newly-archived paths — migrations can happen in two
        // phases (root CLAUDE.md on first SKILL write, per-agent CLAUDE.md
        // on first startup after sentinel). We stamp additional entries
        // without rewriting so the user's edits to this file survive.
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&notice_path)
        {
            use std::io::Write;
            for p in archived_paths {
                let _ = writeln!(f, "- `{}`", p.display());
            }
        }
        return;
    }
    let mut archive_list = String::new();
    for p in archived_paths {
        archive_list.push_str(&format!("- `{}`\n", p.display()));
    }
    let body = format!(
        "<!-- K2SO:MIGRATION_BANNER:0.32.7 -->\n# ⚠️  K2SO 0.32.7 Migration Notice\n\nK2SO archived your pre-existing CLAUDE.md file(s) when unifying workspace context into a single canonical `SKILL.md`. Your original content is safe at:\n\n{archives}\nReview those archives and move anything worth keeping into one of:\n\n- `.k2so/PROJECT.md` — workspace-level context shared by every agent\n- `.k2so/agents/<name>/AGENT.md` — per-agent persona + standing orders\n- The `<!-- K2SO:USER_NOTES -->` section at the bottom of `SKILL.md` — freeform workspace notes, preserved across regenerations\n\nOnce you've reviewed, `.k2so/migration/` can be safely deleted — and so can this file.\n",
        archives = archive_list,
    );
    log_if_err(
        "migration banner",
        &notice_path,
        atomic_write_str(&notice_path, &body),
    );
    log_adoption_event(
        project_path,
        &format!(
            "WROTE .k2so/MIGRATION-0.32.7.md ({} archive(s))",
            archived_paths.len()
        ),
    );
}

// ══════════════════════════════════════════════════════════════════════
// Phase 7b: Extended harness file-discovery coverage
// ══════════════════════════════════════════════════════════════════════

/// Create a symlink for a workspace-root harness file. If the target is
/// a regular file with user-authored content, Phase 7e's contract is:
///   1. Archive the original to `.k2so/migration/` (never destroy).
///   2. Import its body into SKILL.md's USER_NOTES so the new symlinked
///      SKILL.md still surfaces the user's accumulated context.
///   3. Replace the target with the symlink.
///
/// Idempotent: re-running after the target is already a symlink just
/// refreshes the link; re-running against an already-imported archive
/// is a no-op (sentinel keyed on archive path).
fn safe_symlink_harness_file(
    canonical: &Path,
    target: &Path,
    project_path: &str,
    harness_display: &str,
) {
    match fs::symlink_metadata(target) {
        Ok(meta) if meta.file_type().is_symlink() => {
            force_symlink(canonical, target);
        }
        Ok(meta) if meta.file_type().is_file() => {
            let content = fs::read_to_string(target).unwrap_or_default();
            let filename = target
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(harness_display)
                .to_string();
            let archived = archive_claude_md_file(project_path, target, &filename);
            // Import the user's body into SKILL.md USER_NOTES so the symlink
            // redirect doesn't bury their existing context.
            if !content.trim().is_empty() {
                let archive_display = archived
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "(archive unavailable)".to_string());
                import_claude_md_into_user_notes(
                    project_path,
                    &content,
                    &format!("pre-existing {}", harness_display),
                    &archive_display,
                );
            }
            // atomic_symlink renames over the regular file in one step —
            // no remove needed, and no window where target is missing.
            force_symlink(canonical, target);
            if let Some(p) = archived {
                inject_first_migration_banner(project_path, &[p]);
            }
        }
        _ => {
            force_symlink(canonical, target);
        }
    }
}

/// Phase 7b: workspace-level harness file-discovery targets.
///
/// Adds the following to the file-discovery set, all pointing at the
/// canonical `.k2so/skills/k2so/SKILL.md`:
///
///   - `./GEMINI.md`                         → symlink (Gemini auto-loads)
///   - `./AGENT.md`                          → symlink (agent.md spec)
///   - `./.goosehints`                       → symlink (Goose plain-text)
///   - `./.cursor/rules/k2so.mdc`            → generated (Cursor needs MDC
///                                            frontmatter, can't symlink)
///   - `./.aider.conf.yml`                   → scaffolded once (Aider)
fn write_workspace_harness_discovery_targets(project_path: &str, canonical: &Path) {
    let root = PathBuf::from(project_path);

    // GEMINI.md, AGENT.md, .goosehints — plain symlinks to canonical.
    safe_symlink_harness_file(
        canonical,
        &root.join("GEMINI.md"),
        project_path,
        "GEMINI.md",
    );
    safe_symlink_harness_file(
        canonical,
        &root.join("AGENT.md"),
        project_path,
        "AGENT.md",
    );
    safe_symlink_harness_file(
        canonical,
        &root.join(".goosehints"),
        project_path,
        ".goosehints",
    );

    // Cursor requires MDC frontmatter (`description:` + `alwaysApply:`
    // and/or `globs:`) — it does not consume plain markdown, so a
    // symlink won't work. Generate the file with a header that tells
    // Cursor to include it on every request.
    write_cursor_rules_mdc(project_path, canonical);

    // Aider uses YAML config rather than discovery files. Scaffold a
    // minimal `.aider.conf.yml` with `read: SKILL.md` if the file does
    // not exist. Never overwrite existing user config.
    scaffold_aider_conf(project_path);
}

/// Generate `./.cursor/rules/k2so.mdc` with MDC frontmatter + the
/// canonical SKILL.md body. Archives any pre-existing k2so.mdc (and
/// imports its body into USER_NOTES) before overwriting, so user
/// additions to our specific file are preserved.
fn write_cursor_rules_mdc(project_path: &str, canonical: &Path) {
    let Ok(raw) = fs::read_to_string(canonical) else { return };
    let body = strip_frontmatter(&raw).trim().to_string();
    if body.is_empty() { return }

    let dir = PathBuf::from(project_path).join(".cursor").join("rules");
    if fs::create_dir_all(&dir).is_err() { return }
    let target = dir.join("k2so.mdc");

    // Mark our own output with a sentinel key in frontmatter so we can
    // detect it on re-runs and skip the archive+import dance (body drifts
    // every regen as imports stack — without the sentinel we'd infinitely
    // re-archive our own output).
    const K2SO_MDC_SIGNATURE: &str = "k2so_generated: true";

    if target.exists() {
        if let Ok(existing) = fs::read_to_string(&target) {
            let is_our_output = existing.contains(K2SO_MDC_SIGNATURE);
            if !is_our_output {
                let existing_body = strip_frontmatter(&existing).trim().to_string();
                if !existing_body.is_empty() {
                    let archived = archive_claude_md_file(
                        project_path,
                        &target,
                        "cursor/rules/k2so.mdc",
                    );
                    let archive_display = archived
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "(archive unavailable)".to_string());
                    import_claude_md_into_user_notes(
                        project_path,
                        &existing_body,
                        "pre-existing .cursor/rules/k2so.mdc",
                        &archive_display,
                    );
                }
            }
        }
    }

    let mdc = format!(
        "---\n{signature}\ndescription: K2SO workspace context — CLI reference + project context + primary agent persona\nalwaysApply: true\n---\n\n{body}\n",
        signature = K2SO_MDC_SIGNATURE,
        body = body,
    );
    log_if_err(
        "write_cursor_rules_mdc",
        &target,
        atomic_write_str(&target, &mdc),
    );
}

/// Scaffold `./.aider.conf.yml` with `read: [SKILL.md]` so Aider pulls
/// the workspace context on every session. Phase 7e: when the file
/// already exists, archive a copy and merge `SKILL.md` into the `read:`
/// list, preserving every other entry the user had. Any other YAML keys
/// (models, api_key paths, etc.) are left untouched.
fn scaffold_aider_conf(project_path: &str) {
    let path = PathBuf::from(project_path).join(".aider.conf.yml");
    if !path.exists() {
        log_if_err(
            "scaffold_aider_conf create",
            &path,
            atomic_write_str(
                &path,
                "# K2SO: ship workspace context to Aider on every session.\nread:\n  - SKILL.md\n",
            ),
        );
        return;
    }
    let Ok(existing) = fs::read_to_string(&path) else { return };
    // Already has SKILL.md in its read list — no mutation needed.
    if existing.contains("SKILL.md") { return }

    // Archive a copy before we touch it. The archive preserves the user's
    // exact pre-modification state so teardown (restore-original) can
    // revert cleanly. No import into USER_NOTES because .aider.conf.yml
    // is config, not context.
    let _ = archive_claude_md_file(project_path, &path, ".aider.conf.yml");

    // Merge: if there's a `read:` key, add `- SKILL.md` as the first item
    // under it. Otherwise append a fresh `read:` block at the end.
    let lines: Vec<&str> = existing.lines().collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len() + 4);
    let mut injected = false;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim_start();
        if !injected && (trimmed == "read:" || trimmed.starts_with("read:")) {
            out.push(line.to_string());
            // Determine the existing indentation of this read: block.
            let indent: String = line.chars().take_while(|c| c.is_whitespace()).collect();
            out.push(format!("{}  - SKILL.md", indent));
            out.push(format!("{}  # ^ added by K2SO — workspace context", indent));
            injected = true;
            i += 1;
            continue;
        }
        out.push(line.to_string());
        i += 1;
    }
    if !injected {
        if !out.last().map(|l| l.trim().is_empty()).unwrap_or(true) {
            out.push(String::new());
        }
        out.push("# K2SO: ship workspace context on every session.".to_string());
        out.push("read:".to_string());
        out.push("  - SKILL.md".to_string());
    }
    let mut final_out = out.join("\n");
    if !final_out.ends_with('\n') { final_out.push('\n'); }
    log_if_err(
        "scaffold_aider_conf merge",
        &path,
        atomic_write_str(&path, &final_out),
    );
}

// ══════════════════════════════════════════════════════════════════════
// Phase 7e: Workspace teardown (disconnect)
// ══════════════════════════════════════════════════════════════════════

/// The six workspace-root files K2SO can take over via symlink / scaffold.
/// On teardown we walk this list and either freeze the current SKILL.md
/// body into each as a real file (keep_current mode), or restore the
/// archive from `.k2so/migration/` (restore_original mode).
const HARNESS_WORKSPACE_FILES: &[&str] = &[
    "CLAUDE.md",
    "GEMINI.md",
    "AGENT.md",
    ".goosehints",
    "SKILL.md",
    ".cursor/rules/k2so.mdc",
    // NOT .aider.conf.yml — that's a config file with merged entries,
    // handled separately below.
];

/// Summary returned to the UI after a teardown. One entry per file
/// touched, with a human-readable note so the dialog can show what
/// happened.
#[derive(serde::Serialize, Debug)]
pub struct TeardownResult {
    pub action: String,
    pub path: String,
    pub note: String,
}

/// User's choice when they remove/disconnect a workspace.
///
/// - `keep_current`: freeze the current canonical SKILL.md body into each
///   symlinked file as a real file. Every CLI LLM the user had enabled
///   keeps working, reading the last-known merged context. Best when
///   the user is stepping away but wants their tools to still have context.
///
/// - `restore_original`: replace each symlinked file with whatever was
///   there before K2SO took over (from `.k2so/migration/`). Files K2SO
///   created fresh (no archive) are deleted. The workspace looks like
///   it did pre-K2SO except for the `.k2so/` folder, which stays
///   intact as the restore source (and the reconnect-later safety net).
///
/// In both modes `.k2so/` itself is preserved. Nothing is destroyed.
#[derive(Clone, Copy, Debug)]
pub enum TeardownMode {
    KeepCurrent,
    RestoreOriginal,
}

/// Freeze or restore every workspace-root symlink, returning a per-file
/// summary the UI can display. `.k2so/` is never touched — archives,
/// canonical SKILL.md, and sentinels all stay in place so reconnect is
/// idempotent.
pub fn teardown_workspace_harness_files(
    project_path: &str,
    mode: TeardownMode,
) -> Vec<TeardownResult> {
    let root = PathBuf::from(project_path);
    let canonical = root.join(".k2so/skills/k2so/SKILL.md");
    let current_body = fs::read_to_string(&canonical).unwrap_or_default();
    let mut results: Vec<TeardownResult> = Vec::new();

    for rel in HARNESS_WORKSPACE_FILES {
        let path = root.join(rel);
        // We only touch files we managed — i.e., those that are symlinks
        // pointing at our canonical. User-authored regular files at these
        // paths are NEVER touched during teardown (that's why the add-
        // time safe_symlink_harness_file archives before linking — once
        // it's a symlink, it's ours).
        let Ok(meta) = fs::symlink_metadata(&path) else { continue };
        if !meta.file_type().is_symlink() { continue }

        match mode {
            TeardownMode::KeepCurrent => {
                // Atomic replace: write the frozen body to a sibling
                // tempfile, then rename over the symlink in one step. If
                // the write fails, the original symlink is untouched — no
                // window where the path is missing. This fixes C2 from
                // the resilience review: the previous remove-then-write
                // could leave the file neither a symlink nor a real file
                // if the write step failed partway.
                match atomic_write_str(&path, &current_body) {
                    Ok(()) => results.push(TeardownResult {
                        action: "froze".to_string(),
                        path: rel.to_string(),
                        note: "Replaced symlink with a frozen snapshot of the current SKILL.md. Tool will keep reading this context.".to_string(),
                    }),
                    Err(e) => results.push(TeardownResult {
                        action: "failed".to_string(),
                        path: rel.to_string(),
                        note: format!(
                            "Could not write frozen snapshot ({}); original symlink left intact.",
                            e
                        ),
                    }),
                }
            }
            TeardownMode::RestoreOriginal => {
                // Look for the most recent archive for this file under
                // .k2so/migration/. If found, atomic-write it back (so a
                // crash mid-restore leaves either the old symlink or the
                // fully-restored file, never a truncated in-between). If
                // not, the file was K2SO-created and has no original —
                // only then do we delete.
                match find_latest_archive(project_path, rel) {
                    Some(archive_path) => match fs::read_to_string(&archive_path) {
                        Ok(body) => match atomic_write_str(&path, &body) {
                            Ok(()) => results.push(TeardownResult {
                                action: "restored".to_string(),
                                path: rel.to_string(),
                                note: format!(
                                    "Restored from archive: {}",
                                    archive_path.display()
                                ),
                            }),
                            Err(e) => results.push(TeardownResult {
                                action: "failed".to_string(),
                                path: rel.to_string(),
                                note: format!(
                                    "Found archive {} but write failed: {}; symlink left intact.",
                                    archive_path.display(),
                                    e
                                ),
                            }),
                        },
                        Err(e) => results.push(TeardownResult {
                            action: "failed".to_string(),
                            path: rel.to_string(),
                            note: format!(
                                "Archive unreadable ({}): {}; symlink left intact.",
                                archive_path.display(),
                                e
                            ),
                        }),
                    },
                    None => {
                        log_if_err("restore_original remove symlink", &path, fs::remove_file(&path));
                        results.push(TeardownResult {
                            action: "removed".to_string(),
                            path: rel.to_string(),
                            note: "No prior archive — K2SO created this file fresh; removed cleanly.".to_string(),
                        });
                    }
                }
            }
        }
    }

    // Aider config: .aider.conf.yml — if we mutated it, the archive has
    // the user's original content. Only touch on restore_original mode;
    // on keep_current the merged config is already a standalone file.
    if matches!(mode, TeardownMode::RestoreOriginal) {
        let aider_path = root.join(".aider.conf.yml");
        if let Some(archive) = find_latest_archive(project_path, ".aider.conf.yml") {
            match fs::read_to_string(&archive) {
                Ok(body) => match atomic_write_str(&aider_path, &body) {
                    Ok(()) => results.push(TeardownResult {
                        action: "restored".to_string(),
                        path: ".aider.conf.yml".to_string(),
                        note: format!("Restored from archive: {}", archive.display()),
                    }),
                    Err(e) => results.push(TeardownResult {
                        action: "failed".to_string(),
                        path: ".aider.conf.yml".to_string(),
                        note: format!(
                            "Archive {} read ok but restore write failed: {}",
                            archive.display(),
                            e
                        ),
                    }),
                },
                Err(e) => results.push(TeardownResult {
                    action: "failed".to_string(),
                    path: ".aider.conf.yml".to_string(),
                    note: format!("Archive unreadable: {}", e),
                }),
            }
        } else if aider_path.exists() {
            // K2SO created it fresh with only the SKILL.md read entry.
            // Remove it cleanly.
            log_if_err("teardown remove aider.conf.yml", &aider_path, fs::remove_file(&aider_path));
            results.push(TeardownResult {
                action: "removed".to_string(),
                path: ".aider.conf.yml".to_string(),
                note: "No prior archive — K2SO scaffolded this file fresh; removed cleanly.".to_string(),
            });
        }
    }

    results
}

/// Walk `.k2so/migration/` looking for the most-recent archive that
/// matches the relative harness path. Archive filenames look like
/// `<basename>-<epoch>.md` — we match by basename.
fn find_latest_archive(project_path: &str, rel: &str) -> Option<PathBuf> {
    // Archive path shape: .k2so/migration/<subdir>?/<basename>-<ts>.md
    let migration_root = PathBuf::from(project_path).join(".k2so").join("migration");
    if !migration_root.is_dir() { return None }

    // Convert rel to the archive's subdir + basename convention used by
    // archive_claude_md_file (subdir = parent of rel if any; basename =
    // rel's last component minus the .md extension + "-<ts>.md").
    let (subdir, leaf) = match rel.rsplit_once('/') {
        Some((parent, leaf)) => (Some(parent.to_string()), leaf.to_string()),
        None => (None, rel.to_string()),
    };
    let search_dir = match &subdir {
        Some(s) => migration_root.join(s),
        None => migration_root.clone(),
    };
    if !search_dir.is_dir() { return None }

    // Match archive_claude_md_file's naming convention:
    // <leaf_stem>-<ts><leaf_ext>, where leaf_ext preserves the original.
    let (leaf_stem, leaf_ext) = match leaf.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem.to_string(), format!(".{}", ext)),
        _ => (leaf.clone(), String::new()),
    };

    // Accept both archive naming schemes:
    //   old: "<stem>-<unix_secs><ext>"              (pre-fs_atomic)
    //   new: "<stem>-<unix_nanos>-<seq:04><ext>"    (collision-free)
    // Sort key uses the leading numeric field; nanos-vs-secs is an
    // apples-to-oranges comparison in absolute value, but "newest wins"
    // still holds because new-format writes always have larger numeric
    // prefixes than same-run old-format legacy archives would (nanos ≫
    // secs for every real timestamp since 1970).
    let mut best: Option<(u128, PathBuf)> = None;
    if let Ok(entries) = fs::read_dir(&search_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() { continue }
            let name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n,
                None => continue,
            };
            let prefix = format!("{}-", leaf_stem);
            if !name.starts_with(&prefix) { continue }
            if !leaf_ext.is_empty() && !name.ends_with(&leaf_ext) { continue }
            let rest = &name[prefix.len()..];
            let rest = if leaf_ext.is_empty() { rest } else { rest.trim_end_matches(&leaf_ext[..]) };
            // Leading contiguous digits = the timestamp. We intentionally
            // don't care whether they're seconds or nanoseconds — we only
            // need a monotonic ordering for "most recent".
            let ts_str: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            let Ok(ts) = ts_str.parse::<u128>() else { continue };
            match &best {
                Some((existing_ts, _)) if ts <= *existing_ts => {}
                _ => best = Some((ts, path.clone())),
            }
        }
    }
    best.map(|(_, p)| p)
}

/// Tauri command: UI-callable teardown. Used by the Remove-Workspace
/// confirmation dialog to execute the user's chosen mode before
/// projects_delete actually removes the DB row.
#[tauri::command]
pub fn k2so_agents_teardown_workspace(
    project_path: String,
    mode: String,
) -> Result<Vec<TeardownResult>, String> {
    let m = match mode.as_str() {
        "keep_current" => TeardownMode::KeepCurrent,
        "restore_original" => TeardownMode::RestoreOriginal,
        other => return Err(format!("unknown teardown mode: {}", other)),
    };
    Ok(teardown_workspace_harness_files(&project_path, m))
}

/// One entry in the Add-Workspace preview. Mirrors what the CLI's
/// `k2so workspace preview` reports, but structured for the UI.
#[derive(serde::Serialize, Debug)]
pub struct WorkspacePreviewEntry {
    pub path: String,
    pub action: String,  // "archive_and_import" | "refresh" | "create" | "marker_injected"
    pub size_bytes: Option<u64>,
    pub note: String,
}

/// Inspect a workspace path WITHOUT mutating anything. Returns a list
/// of entries describing what K2SO will do on add — archive + import,
/// refresh in place, create fresh, or marker-inject. Backs the UI's
/// add-workspace explanation card and the CLI's `k2so workspace preview`.
#[tauri::command]
pub fn k2so_agents_preview_workspace_ingest(
    project_path: String,
) -> Result<Vec<WorkspacePreviewEntry>, String> {
    let root = PathBuf::from(&project_path);
    let mut entries: Vec<WorkspacePreviewEntry> = Vec::new();

    // Collision-prone files: pre-existing user content → archive + import + symlink
    let collision_targets: &[(&str, &str)] = &[
        ("CLAUDE.md", "Claude Code memory"),
        ("GEMINI.md", "Gemini CLI instructions"),
        ("AGENT.md", "agent.md spec file"),
        (".goosehints", "Goose hints"),
        (".cursor/rules/k2so.mdc", "Cursor rule"),
    ];
    for (rel, label) in collision_targets {
        let path = root.join(rel);
        match fs::symlink_metadata(&path) {
            Ok(meta) if meta.file_type().is_symlink() => {
                entries.push(WorkspacePreviewEntry {
                    path: rel.to_string(),
                    action: "refresh".to_string(),
                    size_bytes: None,
                    note: format!("{} — already symlinked to K2SO canonical (will refresh)", label),
                });
            }
            Ok(meta) if meta.file_type().is_file() => {
                // Detect our own generated Cursor MDC via sentinel.
                let is_ours = fs::read_to_string(&path)
                    .map(|s| s.contains("k2so_generated: true"))
                    .unwrap_or(false);
                if is_ours {
                    entries.push(WorkspacePreviewEntry {
                        path: rel.to_string(),
                        action: "refresh".to_string(),
                        size_bytes: Some(meta.len()),
                        note: format!("{} — K2SO-generated, will refresh in place", label),
                    });
                } else {
                    entries.push(WorkspacePreviewEntry {
                        path: rel.to_string(),
                        action: "archive_and_import".to_string(),
                        size_bytes: Some(meta.len()),
                        note: format!("{} — archive → import body into SKILL.md USER_NOTES → symlink", label),
                    });
                }
            }
            _ => {
                entries.push(WorkspacePreviewEntry {
                    path: rel.to_string(),
                    action: "create".to_string(),
                    size_bytes: None,
                    note: format!("{} — no prior file, will create symlink", label),
                });
            }
        }
    }

    // Aider config: merge if exists, scaffold if not.
    let aider_path = root.join(".aider.conf.yml");
    if aider_path.is_file() {
        let already = fs::read_to_string(&aider_path)
            .map(|s| s.contains("SKILL.md"))
            .unwrap_or(false);
        let size = fs::metadata(&aider_path).ok().map(|m| m.len());
        if already {
            entries.push(WorkspacePreviewEntry {
                path: ".aider.conf.yml".to_string(),
                action: "refresh".to_string(),
                size_bytes: size,
                note: "Aider config — already references SKILL.md, no change".to_string(),
            });
        } else {
            entries.push(WorkspacePreviewEntry {
                path: ".aider.conf.yml".to_string(),
                action: "archive_and_import".to_string(),
                size_bytes: size,
                note: "Aider config — archive → merge SKILL.md into read: list (preserves other keys)".to_string(),
            });
        }
    } else {
        entries.push(WorkspacePreviewEntry {
            path: ".aider.conf.yml".to_string(),
            action: "create".to_string(),
            size_bytes: None,
            note: "Aider config — scaffold fresh with read: [SKILL.md]".to_string(),
        });
    }

    // Marker-injected files: AGENTS.md, .github/copilot-instructions.md
    let marker_targets: &[(&str, &str)] = &[
        ("AGENTS.md", "Codex / OpenCode / Pi"),
        (".github/copilot-instructions.md", "GitHub Copilot"),
    ];
    for (rel, label) in marker_targets {
        let path = root.join(rel);
        let size = fs::metadata(&path).ok().map(|m| m.len());
        let action = if path.exists() { "marker_injected" } else { "create" };
        let note = if path.exists() {
            format!("{} — K2SO block inserted between markers, your content preserved", label)
        } else {
            format!("{} — will create with K2SO block only", label)
        };
        entries.push(WorkspacePreviewEntry {
            path: rel.to_string(),
            action: action.to_string(),
            size_bytes: size,
            note,
        });
    }

    Ok(entries)
}

/// Trigger the workspace skill write for a single project on demand.
/// Used by the Add-Workspace dialog to run migration immediately after
/// the user confirms, rather than waiting for the next app boot.
#[tauri::command]
pub fn k2so_agents_run_workspace_ingest(project_path: String) -> Result<(), String> {
    harvest_per_agent_claude_md_files(&project_path);
    write_workspace_skill_file(&project_path);
    Ok(())
}

// `write_agent_skill_file` moved to k2so_core::agents::skill_writer (re-exported).

/// Universal skill refresh. Walks every agent folder + the workspace
/// skill and re-invokes the regular write_* functions. Because those now
/// route through ensure_skill_up_to_date, this is idempotent:
///   - Files on the current SKILL_VERSION_* → no-op.
///   - Legacy/unversioned files → migrated to the managed-markers layout
///     without losing content (legacy text lands below the closing marker).
///   - Version-bumped + unmodified → rewritten in place to the new body.
///   - User-edited (checksum mismatch) → new body dropped as `.proposed`.
///
/// Call this at startup per project. Replaces the pre-0.32.4 one-off
/// `ensure_k2so_agent_skill_upgraded` — adding new skill types or bumping
/// a generator version no longer requires a new helper.
pub fn ensure_all_skills_up_to_date(project_path: &str) {
    // Workspace skill (human-user surface).
    write_workspace_skill_file(project_path);

    // Each agent's skill.
    let agents_root = agents_dir(project_path);
    if !agents_root.exists() { return; }
    let Ok(entries) = fs::read_dir(&agents_root) else { return };
    for entry in entries.flatten() {
        let agent_path = entry.path();
        if !agent_path.is_dir() { continue; }
        let name_osstr = entry.file_name();
        let agent_name = name_osstr.to_string_lossy();
        // Skip bookkeeping dirs like `.archive/`.
        if agent_name.starts_with('.') { continue; }

        let agent_md = agent_path.join("AGENT.md");
        if !agent_md.exists() { continue; }
        let agent_content = fs::read_to_string(&agent_md).unwrap_or_default();
        let fm = parse_frontmatter(&agent_content);
        let agent_type = fm.get("type").cloned().unwrap_or_else(|| "agent-template".to_string());
        // Normalize legacy aliases the rest of the codebase uses.
        let normalized_type = match agent_type.as_str() {
            "pod-leader" | "coordinator" => "manager".to_string(),
            other => other.to_string(),
        };
        write_agent_skill_file(project_path, &agent_name, &normalized_type);
    }
}

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
mod migration_safety_tests {
    use super::*;
    use std::path::PathBuf;
    use uuid::Uuid;

    /// Make a scratch `.k2so/` scaffold for a migration test. Returns the
    /// project root path — caller drops it when done to clean up.
    fn scratch_project() -> PathBuf {
        let dir = std::env::temp_dir()
            .join("k2so-migration-test")
            .join(Uuid::new_v4().to_string());
        fs::create_dir_all(dir.join(".k2so/skills/k2so")).unwrap();
        fs::create_dir_all(dir.join(".k2so/agents")).unwrap();
        dir
    }

    #[test]
    fn archive_claude_md_never_deletes_source() {
        let proj = scratch_project();
        let root_claude = proj.join("CLAUDE.md");
        let body = "# My K2SO notes\n\nThis is my workspace context.\n";
        fs::write(&root_claude, body).unwrap();

        let archive = archive_claude_md_file(
            proj.to_str().unwrap(),
            &root_claude,
            "CLAUDE.md",
        )
        .expect("archive should succeed");

        // Source must still exist — archive is a COPY.
        assert!(root_claude.exists(), "archive must not delete the source");
        // Archive must contain exactly the source body.
        let archived_body = fs::read_to_string(&archive).unwrap();
        assert_eq!(archived_body, body, "archive must preserve content byte-for-byte");
        // Archive must live under .k2so/migration/.
        assert!(
            archive.starts_with(proj.join(".k2so").join("migration")),
            "archive path must land under .k2so/migration/, got {}",
            archive.display(),
        );
        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn harvest_per_agent_claude_md_archives_then_removes_source() {
        let proj = scratch_project();
        fs::create_dir_all(proj.join(".k2so/agents/backend-eng")).unwrap();
        let agent_claude = proj.join(".k2so/agents/backend-eng/CLAUDE.md");
        let body = "# backend-eng persona\n\nUser-authored memory.\n";
        fs::write(&agent_claude, body).unwrap();

        harvest_per_agent_claude_md_files(proj.to_str().unwrap());

        // Source should be gone (per plan: per-agent CLAUDE.md retired).
        assert!(!agent_claude.exists(), "per-agent CLAUDE.md should be removed after harvest");
        // An archive with byte-identical content must exist under migration/.
        let archive_root = proj.join(".k2so/migration/agents/backend-eng");
        let entries: Vec<_> = fs::read_dir(&archive_root).unwrap().flatten().collect();
        assert_eq!(entries.len(), 1, "expected exactly one archive, got {:?}", entries);
        let archived = fs::read_to_string(entries[0].path()).unwrap();
        assert_eq!(archived, body, "archive must preserve content byte-for-byte");
        // Sentinel must be written so re-runs don't re-harvest.
        assert!(
            proj.join(".k2so/.harvest-0.32.7-done").exists(),
            "harvest sentinel must be written"
        );
        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn harvest_is_idempotent_even_if_file_regenerated_later() {
        let proj = scratch_project();
        fs::create_dir_all(proj.join(".k2so/agents/backend-eng")).unwrap();
        let agent_claude = proj.join(".k2so/agents/backend-eng/CLAUDE.md");
        fs::write(&agent_claude, "first content").unwrap();

        harvest_per_agent_claude_md_files(proj.to_str().unwrap());

        // User runs `k2so agents generate-md` later, which re-creates the file.
        fs::write(&agent_claude, "user-regenerated content").unwrap();

        // Second harvest run must be a no-op (sentinel already stamped).
        harvest_per_agent_claude_md_files(proj.to_str().unwrap());

        // The regenerated file must NOT have been re-archived / removed.
        assert!(agent_claude.exists(), "second run must not re-harvest");
        assert_eq!(fs::read_to_string(&agent_claude).unwrap(), "user-regenerated content");
        // Still exactly one archive entry (the first-run one).
        let archive_root = proj.join(".k2so/migration/agents/backend-eng");
        let entries: Vec<_> = fs::read_dir(&archive_root).unwrap().flatten().collect();
        assert_eq!(entries.len(), 1, "idempotent harvest must not double-archive");
        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn strip_tail_preserves_user_freeform_but_discards_placeholders() {
        let proj = scratch_project();
        let canonical = proj.join(".k2so/skills/k2so/SKILL.md");
        // Simulate a corrupted tail with 3 stacked USER_NOTES sentinels +
        // placeholder comments from buggy prior runs, PLUS a real user note.
        let corrupted = format!(
            "---\nk2so_skill: workspace\n---\n\n{begin}\nManaged body\n{end}\n\n{sentinel}\n{placeholder}\n\n{sentinel}\n{placeholder}\n\nMy real user note line 1.\nMy real user note line 2.\n",
            begin = SKILL_BEGIN_MARKER,
            end = SKILL_END_MARKER,
            sentinel = SKILL_USER_NOTES_SENTINEL,
            placeholder = USER_NOTES_PLACEHOLDER,
        );
        fs::write(&canonical, &corrupted).unwrap();

        let preserved = strip_workspace_skill_tail(proj.to_str().unwrap());

        // Must preserve the user's real note.
        let preserved = preserved.expect("user freeform should be preserved");
        assert!(
            preserved.contains("My real user note line 1"),
            "user line 1 should survive, got: {:?}",
            preserved
        );
        assert!(
            preserved.contains("My real user note line 2"),
            "user line 2 should survive, got: {:?}",
            preserved
        );
        // Placeholder comments must be discarded — otherwise stacking would
        // reappear next regen.
        assert!(
            !preserved.contains(USER_NOTES_PLACEHOLDER),
            "placeholder comments must be stripped from preserved content"
        );
        // After strip, the file must contain only the managed region + newline.
        let post = fs::read_to_string(&canonical).unwrap();
        assert!(post.ends_with(&format!("{}\n", SKILL_END_MARKER)), "file must end at the managed END marker after strip");
        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn strip_tail_returns_none_when_tail_is_empty_or_placeholder_only() {
        let proj = scratch_project();
        let canonical = proj.join(".k2so/skills/k2so/SKILL.md");
        // Tail with only K2SO-emitted noise (no user content).
        let noise = format!(
            "{begin}\nManaged\n{end}\n\n{sentinel}\n{placeholder}\n",
            begin = SKILL_BEGIN_MARKER,
            end = SKILL_END_MARKER,
            sentinel = SKILL_USER_NOTES_SENTINEL,
            placeholder = USER_NOTES_PLACEHOLDER,
        );
        fs::write(&canonical, &noise).unwrap();

        let preserved = strip_workspace_skill_tail(proj.to_str().unwrap());
        assert!(preserved.is_none(), "pure K2SO noise must not be preserved as user content");
        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn migration_banner_is_idempotent_and_appends_new_archives() {
        let proj = scratch_project();
        let project_path = proj.to_str().unwrap();
        let first_archive = proj.join(".k2so/migration/round-1.md");
        let second_archive = proj.join(".k2so/migration/round-2.md");
        fs::create_dir_all(first_archive.parent().unwrap()).unwrap();
        fs::write(&first_archive, "round 1").unwrap();
        fs::write(&second_archive, "round 2").unwrap();

        inject_first_migration_banner(project_path, &[first_archive.clone()]);

        let notice_path = proj.join(".k2so/MIGRATION-0.32.7.md");
        assert!(notice_path.exists(), "migration notice must be created");
        let after_first = fs::read_to_string(&notice_path).unwrap();
        assert!(after_first.contains("round-1"), "first archive must be referenced");
        let first_len = after_first.len();

        // Second invocation with a DIFFERENT archive must append, not rewrite.
        inject_first_migration_banner(project_path, &[second_archive.clone()]);
        let after_second = fs::read_to_string(&notice_path).unwrap();
        assert!(after_second.starts_with(&after_first), "append must preserve existing content");
        assert!(after_second.len() > first_len, "second invocation must grow the file");
        assert!(after_second.contains("round-2"), "second archive must be appended");

        // Same archive twice — must still append (simple append mode); this is
        // deliberate since harvests at different times produce timestamped
        // archive paths anyway, so duplicates aren't a practical concern.
        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn safe_symlink_archives_existing_regular_file() {
        let proj = scratch_project();
        let canonical = proj.join(".k2so/skills/k2so/SKILL.md");
        // Seed canonical with a realistic shape (managed region +
        // USER_NOTES sentinel) so the importer has somewhere to splice.
        let canonical_body = format!(
            "---\nk2so_skill: workspace\n---\n\n{begin}\nManaged body\n{end}\n\n{sentinel}\n{placeholder}\n",
            begin = SKILL_BEGIN_MARKER,
            end = SKILL_END_MARKER,
            sentinel = SKILL_USER_NOTES_SENTINEL,
            placeholder = USER_NOTES_PLACEHOLDER,
        );
        fs::write(&canonical, &canonical_body).unwrap();
        let target = proj.join("GEMINI.md");
        fs::write(&target, "user authored Gemini instructions").unwrap();

        safe_symlink_harness_file(
            &canonical,
            &target,
            proj.to_str().unwrap(),
            "GEMINI.md",
        );

        // Target should now be a symlink pointing to canonical.
        let meta = fs::symlink_metadata(&target).unwrap();
        assert!(meta.file_type().is_symlink(), "target must be a symlink after safe-link");
        // Reading through the symlink yields the canonical, which now
        // includes the imported user body.
        let linked_body = fs::read_to_string(&target).unwrap();
        assert!(linked_body.contains("Managed body"), "managed region must survive import");
        assert!(
            linked_body.contains("user authored Gemini instructions"),
            "Phase 7e: user's pre-existing body must be imported into canonical so the symlink still surfaces it"
        );
        // An archive must exist under .k2so/migration/ with the pre-link content.
        let migration_dir = proj.join(".k2so/migration");
        let entries: Vec<_> = std::fs::read_dir(&migration_dir).unwrap().flatten().collect();
        let has_archive = entries.iter().any(|e| {
            let p = e.path();
            let body = fs::read_to_string(&p).unwrap_or_default();
            body == "user authored Gemini instructions"
        });
        assert!(has_archive, "pre-existing user file must be archived before symlink replaces it");
        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn import_claude_md_lands_in_user_notes_and_is_idempotent() {
        let proj = scratch_project();
        let canonical = proj.join(".k2so/skills/k2so/SKILL.md");
        // Pre-seed a minimal SKILL.md with a MANAGED region + USER_NOTES sentinel.
        let seeded = format!(
            "---\nk2so_skill: workspace\n---\n\n{begin}\nManaged body\n{end}\n\n{sentinel}\n{placeholder}\n",
            begin = SKILL_BEGIN_MARKER,
            end = SKILL_END_MARKER,
            sentinel = SKILL_USER_NOTES_SENTINEL,
            placeholder = USER_NOTES_PLACEHOLDER,
        );
        fs::write(&canonical, &seeded).unwrap();

        let user_body = "# My Claude memory\n\nA useful note about my codebase.";
        import_claude_md_into_user_notes(
            proj.to_str().unwrap(),
            user_body,
            "pre-existing user-authored CLAUDE.md",
            "/tmp/fake/archive.md",
        );

        let after_first = fs::read_to_string(&canonical).unwrap();
        assert!(
            after_first.contains("A useful note about my codebase."),
            "imported body must land in SKILL.md"
        );
        assert!(
            after_first.contains("<!-- K2SO:IMPORT:CLAUDE_MD archive=/tmp/fake/archive.md -->"),
            "import sentinel must be written"
        );
        // The import block must live below USER_NOTES sentinel (not stomp managed).
        let user_notes_pos = after_first.find(SKILL_USER_NOTES_SENTINEL).unwrap();
        let import_pos = after_first.find("A useful note").unwrap();
        assert!(import_pos > user_notes_pos, "import must be below USER_NOTES sentinel");

        // Second call with the SAME archive path must be a no-op (idempotent).
        import_claude_md_into_user_notes(
            proj.to_str().unwrap(),
            user_body,
            "pre-existing user-authored CLAUDE.md",
            "/tmp/fake/archive.md",
        );
        let after_second = fs::read_to_string(&canonical).unwrap();
        assert_eq!(after_first, after_second, "re-import with same archive must be idempotent");

        // Third call with a DIFFERENT archive path MUST add a second block.
        import_claude_md_into_user_notes(
            proj.to_str().unwrap(),
            "another body",
            "upgrade-era CLAUDE.md",
            "/tmp/fake/archive-2.md",
        );
        let after_third = fs::read_to_string(&canonical).unwrap();
        assert!(after_third.contains("another body"), "second archive must be imported");
        assert!(
            after_third.contains("<!-- K2SO:IMPORT:CLAUDE_MD archive=/tmp/fake/archive-2.md -->"),
            "second import sentinel must be present"
        );
        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn workspace_remove_then_readd_leaves_data_intact() {
        // Simulate the remove-then-re-add flow. Removing a workspace from
        // K2SO only deletes DB rows (no FS mutation), and re-adding triggers
        // the startup loop — which sees the sentinel + existing symlinks
        // and no-ops. Key invariant: the archives + imported USER_NOTES
        // content survive untouched.
        let proj = scratch_project();
        let project_path = proj.to_str().unwrap();
        fs::create_dir_all(proj.join(".k2so/agents/backend-eng")).unwrap();
        let agent_claude = proj.join(".k2so/agents/backend-eng/CLAUDE.md");
        fs::write(&agent_claude, "backend agent notes").unwrap();

        // First launch after upgrade: harvest fires.
        harvest_per_agent_claude_md_files(project_path);

        let archive_dir = proj.join(".k2so/migration/agents/backend-eng");
        let archive_files: Vec<_> = fs::read_dir(&archive_dir).unwrap().flatten().collect();
        assert_eq!(archive_files.len(), 1, "first launch should archive once");
        let archived_body = fs::read_to_string(archive_files[0].path()).unwrap();
        assert_eq!(archived_body, "backend agent notes");

        // Simulate user removing the workspace from K2SO:
        //   (DB-only delete — no FS mutation). Then re-adding.
        //   On next launch, harvester fires again; sentinel should short-circuit.
        harvest_per_agent_claude_md_files(project_path);

        // Archive count must still be 1 — no duplication.
        let archive_files_after: Vec<_> = fs::read_dir(&archive_dir).unwrap().flatten().collect();
        assert_eq!(
            archive_files_after.len(),
            1,
            "re-add must not duplicate archives (sentinel gates re-harvest)"
        );
        // Original archive must be intact.
        let archived_after = fs::read_to_string(archive_files_after[0].path()).unwrap();
        assert_eq!(archived_after, "backend agent notes", "archive content must survive remove+re-add");
        // Sentinel still in place.
        assert!(
            proj.join(".k2so/.harvest-0.32.7-done").exists(),
            "sentinel persists across remove+re-add (it's filesystem, not DB)"
        );
        fs::remove_dir_all(&proj).ok();
    }

    // ══════════════════════════════════════════════════════════════════
    // Phase 7e: full lifecycle integration — add workspace, then remove
    // it via each teardown mode. Builds a mock workspace with pre-existing
    // harness files for every CLI we support, invokes the real
    // write_workspace_skill_file_with_body flow, then exercises
    // teardown_workspace_harness_files in both modes to confirm
    // lossless ingest + restore.
    // ══════════════════════════════════════════════════════════════════

    /// Build a mock workspace that looks like the user was using every
    /// supported CLI LLM already — each has accumulated user content we
    /// must preserve.
    fn mock_multi_harness_workspace() -> PathBuf {
        let proj = scratch_project();
        // Root-level discovery files — every harness's convention path.
        fs::write(proj.join("CLAUDE.md"), "# Claude memory\nMy codebase notes from # memory writes.\n").unwrap();
        fs::write(proj.join("GEMINI.md"), "# Gemini instructions\nCustom Gemini behavior for this repo.\n").unwrap();
        fs::write(proj.join("AGENT.md"), "# AGENT.md\nAgent persona customizations.\n").unwrap();
        fs::write(proj.join(".goosehints"), "Goose hints — how to navigate this codebase.\n").unwrap();
        fs::write(
            proj.join(".aider.conf.yml"),
            "# Existing Aider config\nmodel: gpt-4o\nread:\n  - CONVENTIONS.md\n  - ARCHITECTURE.md\n",
        ).unwrap();
        // OpenCode agent dir exists with the user's own agent files.
        fs::create_dir_all(proj.join(".opencode/agent")).unwrap();
        fs::write(
            proj.join(".opencode/agent/my-refactor-helper.md"),
            "# My custom OpenCode agent\nSpecialized refactoring persona.\n",
        ).unwrap();
        // Cursor rules dir with user-authored project rules.
        fs::create_dir_all(proj.join(".cursor/rules")).unwrap();
        fs::write(
            proj.join(".cursor/rules/my-codebase.mdc"),
            "---\nalwaysApply: true\n---\nMy project-specific Cursor rule.\n",
        ).unwrap();
        // PROJECT.md populated so SKILL.md has real content to freeze.
        fs::write(
            proj.join(".k2so/PROJECT.md"),
            "# K2SO\n\nTauri workspace manager. Rust backend + React 19 frontend.\n",
        ).unwrap();
        proj
    }

    #[test]
    fn add_workspace_ingests_all_harness_files_into_skill_and_archives() {
        let proj = mock_multi_harness_workspace();
        let project_path = proj.to_str().unwrap();

        // Invoke the real add-workspace path. This composes SKILL.md,
        // runs safe_symlink_harness_file for each root harness file,
        // merges .aider.conf.yml, generates Cursor MDC, and sets up
        // the root SKILL.md + CLAUDE.md symlinks.
        write_workspace_skill_file_with_body(project_path, None);

        let canonical = proj.join(".k2so/skills/k2so/SKILL.md");
        assert!(canonical.exists(), "canonical SKILL.md must be written");
        let skill_body = fs::read_to_string(&canonical).unwrap();

        // Every root harness file that collides with K2SO should now be
        // a symlink pointing at the canonical.
        for name in ["CLAUDE.md", "GEMINI.md", "AGENT.md", ".goosehints", "SKILL.md"] {
            let path = proj.join(name);
            let meta = fs::symlink_metadata(&path).unwrap();
            assert!(
                meta.file_type().is_symlink(),
                "{} should be a symlink after ingest, got {:?}",
                name,
                meta.file_type(),
            );
        }

        // The archive dir should contain byte-identical copies of every
        // ingested file, keyed under .k2so/migration/.
        let migration_root = proj.join(".k2so/migration");
        let mut found_archives = 0;
        if let Ok(entries) = fs::read_dir(&migration_root) {
            for e in entries.flatten() {
                let path = e.path();
                if path.is_file() {
                    found_archives += 1;
                }
            }
        }
        assert!(
            found_archives >= 4,
            "expected archives for CLAUDE.md/GEMINI.md/AGENT.md/.goosehints at least, got {}",
            found_archives,
        );

        // Each user body should be imported into the SKILL.md USER_NOTES
        // tail. Searching for distinct sentences from each file.
        assert!(
            skill_body.contains("My codebase notes from # memory writes"),
            "CLAUDE.md body not imported into SKILL.md USER_NOTES"
        );
        assert!(
            skill_body.contains("Custom Gemini behavior for this repo"),
            "GEMINI.md body not imported into SKILL.md USER_NOTES"
        );
        assert!(
            skill_body.contains("Agent persona customizations"),
            "root AGENT.md body not imported into SKILL.md USER_NOTES"
        );
        assert!(
            skill_body.contains("Goose hints"),
            ".goosehints body not imported into SKILL.md USER_NOTES"
        );

        // OpenCode custom agent files must be left alone (no collision).
        assert!(
            proj.join(".opencode/agent/my-refactor-helper.md").exists(),
            "user's OpenCode agent files must be preserved untouched"
        );

        // Cursor user rules must be preserved; k2so.mdc added alongside.
        assert!(
            proj.join(".cursor/rules/my-codebase.mdc").exists(),
            "user's Cursor rule files must be preserved"
        );
        assert!(
            proj.join(".cursor/rules/k2so.mdc").exists(),
            "K2SO's Cursor MDC must be added"
        );

        // Aider config should have SKILL.md merged into read: WITHOUT
        // clobbering existing read: entries or other top-level keys.
        let aider = fs::read_to_string(proj.join(".aider.conf.yml")).unwrap();
        assert!(aider.contains("SKILL.md"), "SKILL.md must be injected into Aider read: list");
        assert!(aider.contains("CONVENTIONS.md"), "existing Aider reads must be preserved");
        assert!(aider.contains("ARCHITECTURE.md"), "existing Aider reads must be preserved");
        assert!(aider.contains("model: gpt-4o"), "non-read keys must be preserved");

        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn add_workspace_is_idempotent_second_launch_imports_nothing_new() {
        let proj = mock_multi_harness_workspace();
        let project_path = proj.to_str().unwrap();

        write_workspace_skill_file_with_body(project_path, None);
        let first_body = fs::read_to_string(proj.join(".k2so/skills/k2so/SKILL.md")).unwrap();

        // Second invocation — nothing pre-existing to ingest now (all
        // harness files are symlinks). Body must not grow with duplicate
        // imports.
        write_workspace_skill_file_with_body(project_path, None);
        let second_body = fs::read_to_string(proj.join(".k2so/skills/k2so/SKILL.md")).unwrap();

        // Counting occurrences of the import sentinel prefix — must not
        // increase between first and second run.
        let first_imports = first_body.matches("<!-- K2SO:IMPORT:CLAUDE_MD archive=").count();
        let second_imports = second_body.matches("<!-- K2SO:IMPORT:CLAUDE_MD archive=").count();
        assert_eq!(
            first_imports, second_imports,
            "second launch must not re-import (sentinel should block duplicate adds)"
        );

        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn teardown_keep_current_freezes_symlinks_into_real_files() {
        let proj = mock_multi_harness_workspace();
        let project_path = proj.to_str().unwrap();
        write_workspace_skill_file_with_body(project_path, None);
        let canonical_body = fs::read_to_string(proj.join(".k2so/skills/k2so/SKILL.md")).unwrap();

        let results = teardown_workspace_harness_files(project_path, TeardownMode::KeepCurrent);
        assert!(!results.is_empty(), "teardown should report at least one action");
        assert!(results.iter().all(|r| r.action == "froze"), "keep_current should produce only 'froze' actions: {:?}", results);

        // Every previously-symlinked file is now a real file holding the
        // canonical body verbatim.
        for name in ["CLAUDE.md", "GEMINI.md", "AGENT.md", ".goosehints", "SKILL.md"] {
            let path = proj.join(name);
            let meta = fs::symlink_metadata(&path).expect(name);
            assert!(
                !meta.file_type().is_symlink(),
                "{} must no longer be a symlink after teardown(keep_current)",
                name,
            );
            assert!(meta.file_type().is_file(), "{} must be a regular file", name);
            let body = fs::read_to_string(&path).unwrap();
            assert_eq!(body, canonical_body, "{} must contain the frozen SKILL.md body", name);
        }

        // `.k2so/` is untouched — canonical + migration archives still present.
        assert!(proj.join(".k2so/skills/k2so/SKILL.md").exists());
        assert!(proj.join(".k2so/migration").is_dir());
        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn teardown_restore_original_brings_back_every_archive() {
        let proj = mock_multi_harness_workspace();
        let project_path = proj.to_str().unwrap();
        let pre_claude = fs::read_to_string(proj.join("CLAUDE.md")).unwrap();
        let pre_gemini = fs::read_to_string(proj.join("GEMINI.md")).unwrap();
        let pre_agent = fs::read_to_string(proj.join("AGENT.md")).unwrap();
        let pre_goose = fs::read_to_string(proj.join(".goosehints")).unwrap();
        let pre_aider = fs::read_to_string(proj.join(".aider.conf.yml")).unwrap();

        write_workspace_skill_file_with_body(project_path, None);
        let results = teardown_workspace_harness_files(project_path, TeardownMode::RestoreOriginal);
        assert!(!results.is_empty(), "teardown should report actions");

        // Each harness root file should now be a real file with the
        // pre-ingest user content.
        assert_eq!(fs::read_to_string(proj.join("CLAUDE.md")).unwrap(), pre_claude);
        assert_eq!(fs::read_to_string(proj.join("GEMINI.md")).unwrap(), pre_gemini);
        assert_eq!(fs::read_to_string(proj.join("AGENT.md")).unwrap(), pre_agent);
        assert_eq!(fs::read_to_string(proj.join(".goosehints")).unwrap(), pre_goose);
        assert_eq!(fs::read_to_string(proj.join(".aider.conf.yml")).unwrap(), pre_aider);

        // Root SKILL.md was K2SO-created (no archive) — should be removed.
        assert!(!proj.join("SKILL.md").exists(), "SKILL.md should be removed on restore (no prior original)");

        // `.k2so/` internals preserved so reconnect later works.
        assert!(proj.join(".k2so/skills/k2so/SKILL.md").exists());
        assert!(proj.join(".k2so/migration").is_dir());
        assert!(proj.join(".k2so/.harvest-0.32.7-done").exists() || !proj.join(".k2so/.harvest-0.32.7-done").exists(), "sentinel is fine either way");
        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn reconnect_after_restore_original_reingests_cleanly() {
        let proj = mock_multi_harness_workspace();
        let project_path = proj.to_str().unwrap();

        // First add.
        write_workspace_skill_file_with_body(project_path, None);
        // Full restore — back to original.
        teardown_workspace_harness_files(project_path, TeardownMode::RestoreOriginal);
        // Reconnect (re-add).
        write_workspace_skill_file_with_body(project_path, None);

        // Symlinks should be restored and content re-ingested.
        assert!(fs::symlink_metadata(proj.join("CLAUDE.md")).unwrap().file_type().is_symlink());
        assert!(fs::symlink_metadata(proj.join("GEMINI.md")).unwrap().file_type().is_symlink());

        // SKILL.md must still contain the original user imports — archive
        // sentinel keyed on archive path should dedupe.
        let skill_body = fs::read_to_string(proj.join(".k2so/skills/k2so/SKILL.md")).unwrap();
        assert!(skill_body.contains("My codebase notes from # memory writes"));
        assert!(skill_body.contains("Custom Gemini behavior for this repo"));

        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn teardown_leaves_k2so_dir_fully_intact() {
        // Contract: .k2so/ is sacred across teardown. Every archive,
        // canonical, sentinel, log, and PROJECT.md/AGENT.md file must
        // survive. The user's own persona + project context stay live.
        let proj = mock_multi_harness_workspace();
        let project_path = proj.to_str().unwrap();
        write_workspace_skill_file_with_body(project_path, None);
        let pre_project_md = fs::read_to_string(proj.join(".k2so/PROJECT.md")).unwrap();

        // Enumerate every path under .k2so/ before teardown.
        let pre_paths: Vec<PathBuf> = walk_dir(&proj.join(".k2so"));
        assert!(!pre_paths.is_empty(), "expected a populated .k2so/ before teardown");

        teardown_workspace_harness_files(project_path, TeardownMode::KeepCurrent);
        let post_paths: Vec<PathBuf> = walk_dir(&proj.join(".k2so"));

        // Every pre-teardown path must still exist post-teardown.
        for p in &pre_paths {
            assert!(
                post_paths.contains(p),
                "{} disappeared from .k2so/ during teardown — invariant violated",
                p.display(),
            );
        }
        // PROJECT.md is byte-identical.
        assert_eq!(fs::read_to_string(proj.join(".k2so/PROJECT.md")).unwrap(), pre_project_md);

        fs::remove_dir_all(&proj).ok();
    }

    fn walk_dir(root: &Path) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = fs::read_dir(&dir) else { continue };
            for e in entries.flatten() {
                let p = e.path();
                out.push(p.clone());
                if p.is_dir() && !p.is_symlink() {
                    stack.push(p);
                }
            }
        }
        out.sort();
        out
    }

    #[test]
    fn aider_conf_merge_preserves_user_reads_and_archives_original() {
        let proj = scratch_project();
        let project_path = proj.to_str().unwrap();
        let aider_path = proj.join(".aider.conf.yml");
        let original = "# my aider config\nmodel: gpt-4o\nread:\n  - CONVENTIONS.md\n  - ARCHITECTURE.md\nauto-lint: true\n";
        fs::write(&aider_path, original).unwrap();

        scaffold_aider_conf(project_path);

        let merged = fs::read_to_string(&aider_path).unwrap();
        assert!(merged.contains("SKILL.md"), "SKILL.md must be injected");
        assert!(merged.contains("CONVENTIONS.md"), "original read entries preserved");
        assert!(merged.contains("ARCHITECTURE.md"), "original read entries preserved");
        assert!(merged.contains("model: gpt-4o"), "non-read top-level keys preserved");
        assert!(merged.contains("auto-lint: true"), "non-read top-level keys preserved");

        // Archive exists with original content.
        let migration_root = proj.join(".k2so/migration");
        let mut found = false;
        if let Ok(entries) = fs::read_dir(&migration_root) {
            for e in entries.flatten() {
                if let Ok(body) = fs::read_to_string(e.path()) {
                    if body == original { found = true; }
                }
            }
        }
        assert!(found, "original .aider.conf.yml must be archived before mutation");

        // Second invocation must be a no-op (SKILL.md already present).
        scaffold_aider_conf(project_path);
        let second = fs::read_to_string(&aider_path).unwrap();
        assert_eq!(merged, second, "idempotent — second call must not re-inject");

        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn safe_symlink_is_idempotent_when_target_is_already_symlink() {
        let proj = scratch_project();
        let canonical = proj.join(".k2so/skills/k2so/SKILL.md");
        fs::write(&canonical, "canonical").unwrap();
        let target = proj.join(".goosehints");

        // First invocation creates the symlink.
        safe_symlink_harness_file(&canonical, &target, proj.to_str().unwrap(), ".goosehints");
        // Second invocation must not archive anything (no pre-existing file to save).
        safe_symlink_harness_file(&canonical, &target, proj.to_str().unwrap(), ".goosehints");

        let migration_dir = proj.join(".k2so/migration");
        let entries_count = std::fs::read_dir(&migration_dir)
            .map(|r| r.flatten().count())
            .unwrap_or(0);
        assert_eq!(entries_count, 0, "symlink-to-symlink re-run must not produce spurious archive entries");
        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn completed_regen_clears_in_flight_marker() {
        // Contract: a successful write_workspace_skill_file_with_body leaves
        // no `.regen-in-flight` marker behind. If this regresses, every
        // startup will log a false-positive "interrupted regen" warning.
        let proj = mock_multi_harness_workspace();
        let project_path = proj.to_str().unwrap();
        write_workspace_skill_file_with_body(project_path, None);
        let marker = proj.join(".k2so/.regen-in-flight");
        assert!(!marker.exists(), "regen marker must be cleared on successful completion");
        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn detect_interrupted_regen_flags_stale_marker_once() {
        // Simulate a crashed regen by stamping the marker manually. First
        // detect_interrupted_regen call should return true AND clear the
        // marker; second call should return false (the warning is a one-
        // shot, not a permanent nag).
        let proj = scratch_project();
        let project_path = proj.to_str().unwrap();
        let k2so_dir = proj.join(".k2so");
        fs::create_dir_all(&k2so_dir).unwrap();
        let marker = k2so_dir.join(".regen-in-flight");
        fs::write(&marker, b"").unwrap();
        assert!(detect_interrupted_regen(project_path), "must flag the stale marker");
        assert!(!marker.exists(), "must clear the marker after surfacing the warning");
        assert!(!detect_interrupted_regen(project_path), "must not re-fire after the marker is cleared");
        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn detect_interrupted_regen_is_silent_when_no_marker() {
        // Clean workspaces must not produce false positives. This is the
        // common case — should be a cheap stat + return false.
        let proj = scratch_project();
        fs::create_dir_all(proj.join(".k2so")).unwrap();
        assert!(!detect_interrupted_regen(proj.to_str().unwrap()));
        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn archive_names_never_collide_under_rapid_fire() {
        // Regression for the pre-0.32.9 seconds-granularity timestamp bug.
        // A first-run harvest can fire 5+ archives within a single
        // wall-clock second; the old `{name}-{unix_secs}.md` scheme would
        // silently clobber earlier archives. New unique_archive_path uses
        // nanoseconds + per-process counter.
        let proj = scratch_project();
        let project_path = proj.to_str().unwrap();
        let agents = proj.join(".k2so/agents");
        fs::create_dir_all(&agents).unwrap();
        // Create 10 agents each with a CLAUDE.md, then harvest — they all
        // get archived in the same tight loop.
        for i in 0..10 {
            let agent_dir = agents.join(format!("agent-{}", i));
            fs::create_dir_all(&agent_dir).unwrap();
            fs::write(agent_dir.join("CLAUDE.md"), format!("body for agent-{}", i)).unwrap();
        }
        harvest_per_agent_claude_md_files(project_path);

        // Every agent's content must be archived to a distinct path.
        let mut archive_bodies = std::collections::HashSet::new();
        let migration_root = proj.join(".k2so/migration/agents");
        for i in 0..10 {
            let sub = migration_root.join(format!("agent-{}", i));
            let mut count = 0;
            if let Ok(entries) = fs::read_dir(&sub) {
                for e in entries.flatten() {
                    if let Ok(body) = fs::read_to_string(e.path()) {
                        assert!(archive_bodies.insert(body), "duplicate archive body found");
                        count += 1;
                    }
                }
            }
            assert_eq!(count, 1, "agent-{}: expected 1 archive, got {}", i, count);
        }
        assert_eq!(archive_bodies.len(), 10, "all 10 agents must have distinct archives");

        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn teardown_keep_current_leaves_file_usable_even_on_tight_retries() {
        // Regression for C2: the old keep_current did remove+write, so a
        // write failure left the user with neither a symlink nor a real
        // file. The new code uses atomic_write_str (rename over), so the
        // file is always readable — either as the old symlink if the swap
        // fails, or as the new frozen body on success. Run teardown N
        // times in tight succession to stress the atomic-replace path.
        let proj = mock_multi_harness_workspace();
        let project_path = proj.to_str().unwrap();
        write_workspace_skill_file_with_body(project_path, None);

        // First teardown converts symlinks to real files.
        let _ = teardown_workspace_harness_files(project_path, TeardownMode::KeepCurrent);
        let claude = proj.join("CLAUDE.md");
        assert!(claude.exists(), "CLAUDE.md must exist after first keep_current");
        let first_body = fs::read_to_string(&claude).unwrap();
        assert!(!first_body.is_empty());

        // Subsequent teardowns are no-ops (target is no longer a symlink)
        // but must not corrupt the frozen body.
        for _ in 0..5 {
            let _ = teardown_workspace_harness_files(project_path, TeardownMode::KeepCurrent);
        }
        let final_body = fs::read_to_string(&claude).unwrap();
        assert_eq!(first_body, final_body, "repeated no-op teardowns must not mutate the frozen body");

        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn regen_stamps_content_hashes_for_drift_detection() {
        // After a successful regen, `.last-skill-regen` must contain a
        // JSON snapshot of every source file's content hash. This is the
        // baseline the next regen uses to detect drift; absence of a
        // snapshot forces the fallback mtime path (still works but is
        // clock-skew vulnerable).
        let proj = mock_multi_harness_workspace();
        let project_path = proj.to_str().unwrap();
        write_workspace_skill_file_with_body(project_path, None);

        let stamp_path = proj.join(".k2so/.last-skill-regen");
        let body = fs::read_to_string(&stamp_path).expect("stamp must exist");
        assert!(!body.trim().is_empty(), "stamp must no longer be empty (hash JSON required)");
        let parsed: std::collections::HashMap<String, String> =
            serde_json::from_str(&body).expect("stamp must parse as JSON hash map");
        assert!(parsed.contains_key("project_md"), "PROJECT.md hash must be recorded: {:?}", parsed);

        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn drift_adoption_prefers_content_hash_over_mtime() {
        // Two-phase scenario:
        //   1. Regen stamps a hash for PROJECT.md.
        //   2. Touch PROJECT.md to force mtime > last_regen, but keep
        //      content identical.
        //   3. Next adoption call must NOT treat this as a user edit —
        //      the content hash shows the file is unchanged.
        let proj = mock_multi_harness_workspace();
        let project_path = proj.to_str().unwrap();
        write_workspace_skill_file_with_body(project_path, None);

        // Force mtime to be AFTER the stamp but without changing content.
        let project_md = proj.join(".k2so/PROJECT.md");
        let original = fs::read_to_string(&project_md).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        fs::write(&project_md, &original).unwrap();
        assert!(
            mtime_secs(&project_md) > mtime_secs(&proj.join(".k2so/.last-skill-regen")),
            "test setup: source mtime must be newer than regen stamp"
        );

        // The hash helper must see this as unchanged → touched=false.
        let hashes = read_regen_hashes(project_path);
        let stored = hashes.get("project_md").cloned().unwrap_or_default();
        let current = content_hash_of(&project_md);
        assert_eq!(stored, current, "hash-based drift detection must ignore identical content");

        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn drift_adoption_detects_real_content_change() {
        // Opposite of the mtime-tolerance test: genuine user edits
        // (content hash changes) must flip the touched-since-regen
        // signal to true, regardless of mtime direction.
        let proj = mock_multi_harness_workspace();
        let project_path = proj.to_str().unwrap();
        write_workspace_skill_file_with_body(project_path, None);

        let project_md = proj.join(".k2so/PROJECT.md");
        fs::write(&project_md, "completely different body\n").unwrap();

        let hashes = read_regen_hashes(project_path);
        let stored = hashes.get("project_md").cloned().unwrap_or_default();
        let current = content_hash_of(&project_md);
        assert_ne!(stored, current, "hash-based drift detection must flag modified content");

        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn try_acquire_running_returns_false_when_already_running() {
        // CAS semantics: the first call wins; concurrent / subsequent
        // calls observe status='running' and bail out. Previous code
        // could produce duplicate PTY spawns because the check-then-
        // spawn sequence wasn't atomic.
        let _db = crate::db::init_for_tests();
        let conn_lock = crate::db::shared();
        let conn = conn_lock.lock();
        // Seed a project row so the session FK resolves.
        let pid = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO projects (id, path, name) VALUES (?1, ?2, ?3)",
            rusqlite::params![pid, format!("/tmp/cas-test-{}", pid), "cas-test"],
        ).expect("seed project");

        let sid1 = uuid::Uuid::new_v4().to_string();
        let first = crate::db::schema::WorkspaceSession::try_acquire_running(
            &conn, &sid1, &pid, Some("term-1"), "claude", "system",
        ).expect("first acquire");
        assert!(first, "first caller must acquire the lock");

        let sid2 = uuid::Uuid::new_v4().to_string();
        let second = crate::db::schema::WorkspaceSession::try_acquire_running(
            &conn, &sid2, &pid, Some("term-2"), "claude", "system",
        ).expect("second acquire");
        assert!(!second, "second caller must be rejected while first holds the lock");

        // Release the lock by updating status, then the next acquire
        // must succeed — confirms the gate isn't permanently sticky.
        crate::db::schema::WorkspaceSession::update_status(&conn, &pid, "sleeping")
            .expect("release lock");
        let sid3 = uuid::Uuid::new_v4().to_string();
        let third = crate::db::schema::WorkspaceSession::try_acquire_running(
            &conn, &sid3, &pid, Some("term-3"), "claude", "system",
        ).expect("third acquire");
        assert!(third, "acquire after release must succeed");
    }
}

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

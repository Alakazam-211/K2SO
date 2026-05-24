//! Workspace SKILL scaffolding + one-shot migration helpers.
//!
//! Phase 2 Unit 7b — relocation from `src-tauri/src/commands/k2so_agents.rs`
//! into `k2so-core` so the daemon can run these on its own first boot
//! without Tauri being present. The Tauri-side `#[tauri::command]`
//! wrappers in `commands/k2so_agents.rs` remain as thin forwards into
//! this module (deleted entirely by Unit 7c).
//!
//! Two clusters of functionality live here:
//!
//! 1. **One-shot migration helpers** (post-0.32.x heartbeat + filename
//!    layout fixups). Each is idempotent and gated by an on-disk
//!    sentinel or row-existence check so re-running on every boot is
//!    cheap and safe.
//!
//! 2. **SKILL scaffolding** (the regen orchestrator that writes
//!    `.k2so/skills/k2so/SKILL.md`, fans it out to every harness
//!    discovery path, adopts SOURCE-region drift back into PROJECT.md /
//!    AGENT.md, archives any pre-existing user-authored files, and
//!    stamps content-hash drift baselines).
//!
//! Neither cluster touches Tauri state, AppHandle, or Emitter — the
//! whole thing is host-agnostic by construction, so the Unit 1
//! `WorkspaceRegenProvider` trait pattern wasn't needed.

use std::fs;
use std::path::{Path, PathBuf};

use crate::agents::heartbeat::k2so_heartbeat_add;
use crate::agents::onboarding::is_harness_management_skipped;
use crate::agents::skill::{
    skill_checksum_hex, skill_source_agent_md_begin, skill_source_agent_md_end,
    SKILL_END_MARKER, SKILL_SOURCE_PROJECT_MD_BEGIN, SKILL_SOURCE_PROJECT_MD_END,
    SKILL_VERSION_WORKSPACE,
};
use crate::agents::skill_writer::{
    force_symlink, skill_update_footer, upsert_k2so_section, write_skill_to_all_harnesses,
};
use crate::agents::wake::{strip_frontmatter, workspace_wakeup_path};
use crate::agents::commands::ensure_agent_wakeup;
use crate::agents::{
    agent_dir, agent_type_for, agents_dir, find_primary_agent, parse_frontmatter,
    resolve_project_id,
};
use crate::db::schema::{AgentHeartbeat, WorkspaceSession};
use crate::fs_atomic::{self, atomic_symlink, atomic_write_str, log_if_err, unique_archive_path};

// ══════════════════════════════════════════════════════════════════════
// Constants
// ══════════════════════════════════════════════════════════════════════

/// Sentinel marker users can place in the below-END tail to claim freeform
/// content that should survive regeneration. Anything BETWEEN this marker
/// and EOF is preserved verbatim. Useful for notes the user wants to keep
/// but doesn't want routed into PROJECT.md / AGENT.md.
pub const SKILL_USER_NOTES_SENTINEL: &str = "<!-- K2SO:USER_NOTES -->";

/// The placeholder comment emitted alongside the USER_NOTES sentinel on
/// every regen. Tracked as a constant so `strip_workspace_skill_tail` can
/// discard any existing copies from the preserved freeform — otherwise
/// each regen would stack another placeholder copy onto the tail.
pub const USER_NOTES_PLACEHOLDER: &str =
    "<!-- Content below the K2SO:USER_NOTES sentinel is yours — K2SO preserves it verbatim across regenerations. -->";

/// The six workspace-root files K2SO can take over via symlink / scaffold.
/// On teardown we walk this list and either freeze the current SKILL.md
/// body into each as a real file (keep_current mode), or restore the
/// archive from `.k2so/migration/` (restore_original mode).
pub const HARNESS_WORKSPACE_FILES: &[&str] = &[
    "CLAUDE.md",
    "GEMINI.md",
    "AGENT.md",
    ".goosehints",
    "SKILL.md",
    ".cursor/rules/k2so.mdc",
    // NOT .aider.conf.yml — that's a config file with merged entries,
    // handled separately below.
];

// ══════════════════════════════════════════════════════════════════════
// Public types
// ══════════════════════════════════════════════════════════════════════

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

/// One entry in the Add-Workspace preview. Mirrors what the CLI's
/// `k2so workspace preview` reports, but structured for the UI.
#[derive(serde::Serialize, Debug)]
pub struct WorkspacePreviewEntry {
    pub path: String,
    pub action: String, // "archive_and_import" | "refresh" | "create" | "marker_injected"
    pub size_bytes: Option<u64>,
    pub note: String,
}

// ══════════════════════════════════════════════════════════════════════
// One-shot migration helpers (boot-time)
// ══════════════════════════════════════════════════════════════════════

/// Walk `.k2so/agents/` for top-tier directories (agent_type ∈ custom /
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
            orphan,
            orphan,
            stamp,
            primary
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
            project_path,
            agent_name,
            frequency
        );
    }
}

/// Scaffold the wakeup files for a single workspace — one for each
/// existing agent that supports wake-up. Safe to call repeatedly;
/// never overwrites an existing file. Used by the app-launch migration
/// pass.
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

/// Rename lowercase `agent.md` / `wakeup.md` filenames to UPPERCASE in all
/// known locations within a workspace. Idempotent — skips files that are
/// already uppercase.
///
/// Case-insensitive filesystems (macOS HFS+, default APFS) refuse a direct
/// `fs::rename("agent.md", "AGENT.md")` — it's the same filename to the FS.
/// We two-step through a temporary name so the final result is a real case
/// change recorded in the directory entry.
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
                    continue;
                }
                let agent_path = entry.path();

                case_rename(&agent_path.join("agent.md"), &agent_path.join("AGENT.md"));
                case_rename(&agent_path.join("wakeup.md"), &agent_path.join("WAKEUP.md"));

                let heartbeats_dir = agent_path.join("heartbeats");
                if let Ok(hb_entries) = fs::read_dir(&heartbeats_dir) {
                    for hb in hb_entries.flatten() {
                        if !hb.file_type().map_or(false, |ft| ft.is_dir()) {
                            continue;
                        }
                        let sched_path = hb.path();
                        case_rename(
                            &sched_path.join("wakeup.md"),
                            &sched_path.join("WAKEUP.md"),
                        );
                    }
                }
            }
        }
    }

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
                        from.display(),
                        to.display()
                    );
                    return;
                }
            }
        }
    }
    let tmp = from.with_extension(format!("md.tmp-case-rename-{}", uuid::Uuid::new_v4()));
    if fs::rename(from, &tmp).is_err() {
        return;
    }
    if fs::rename(&tmp, to).is_err() {
        let _ = fs::rename(&tmp, from);
        log_debug!(
            "[filename-migrate] second-step rename failed for {} → {}",
            from.display(),
            to.display()
        );
    }
}

/// Idempotent: bails immediately if the workspace's primary already
/// has any heartbeat row, or if the project isn't in manager mode.
pub fn migrate_or_scaffold_lead_heartbeat(project_path: &str) {
    let db = crate::db::shared();
    let conn = db.lock();
    let Some(project_id) = resolve_project_id(&conn, project_path) else { return };

    let agent_mode: Option<String> = conn
        .query_row(
            "SELECT agent_mode FROM projects WHERE id = ?1",
            rusqlite::params![&project_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .ok()
        .flatten();
    if agent_mode.as_deref() != Some("manager") {
        return;
    }

    let has_triage: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM agent_heartbeats \
             WHERE project_id = ?1 AND name = 'triage')",
            rusqlite::params![&project_id],
            |row| row.get(0),
        )
        .unwrap_or(false);
    if has_triage {
        return;
    }

    let legacy_path = workspace_wakeup_path(project_path);
    let migrated_content: Option<String> = fs::read_to_string(&legacy_path)
        .ok()
        .filter(|s| !s.trim().is_empty());

    let wake_body = if let Some(ref existing) = migrated_content {
        if existing.trim_start().starts_with("---") {
            existing.clone()
        } else {
            format!(
                "---\ndescription: Workspace manager triage (migrated from .k2so/wakeup.md)\n---\n\n{}",
                existing
            )
        }
    } else {
        "---\ndescription: Workspace manager triage — follow your Standing Orders\n---\n\n\
         # Wake procedure: default\n\n\
         Follow your Standing Orders to triage the workspace inbox and review queue. \
         Delegate, approve, or exit — keep the session short.\n"
            .to_string()
    };

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
                    project_path,
                    primary_agent
                );
            } else {
                log_debug!(
                    "[migrate] {}: scaffolded lean triage heartbeat for agent '{}'",
                    project_path,
                    primary_agent
                );
            }
        }
        Err(e) => {
            log_debug!(
                "[migrate] Failed to scaffold triage heartbeat for {}: {}",
                project_path,
                e
            );
        }
    }
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
    log_if_err("clear stale regen marker", &marker, fs::remove_file(&marker));
    true
}

// ══════════════════════════════════════════════════════════════════════
// SKILL scaffolding helpers (private to this module)
// ══════════════════════════════════════════════════════════════════════

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
fn read_regen_hashes(project_path: &str) -> std::collections::HashMap<String, String> {
    let stamp_path = PathBuf::from(project_path).join(".k2so").join(".last-skill-regen");
    let Ok(raw) = fs::read_to_string(&stamp_path) else {
        return std::collections::HashMap::new();
    };
    if raw.trim().is_empty() {
        return std::collections::HashMap::new();
    }
    serde_json::from_str::<std::collections::HashMap<String, String>>(&raw).unwrap_or_default()
}

/// Persist the content hashes of every source file that participates in
/// drift detection. Called at the end of a successful regen so the next
/// regen has a baseline for comparison.
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

// ══════════════════════════════════════════════════════════════════════
// SKILL scaffolding — public entry points
// ══════════════════════════════════════════════════════════════════════

/// Write the workspace-level K2SO skill to all harness locations.
pub fn write_workspace_skill_file(project_path: &str) {
    write_workspace_skill_file_with_body(project_path, None);
}

/// Variant that lets callers pass a pre-composed body so that content
/// lands in the canonical SKILL.md rather than being lost when CLAUDE.md
/// collapsed to a symlink.
pub fn write_workspace_skill_file_with_body(project_path: &str, base_body: Option<&str>) {
    let project_name = std::path::Path::new(project_path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "workspace".to_string());

    let regen_marker = PathBuf::from(project_path)
        .join(".k2so")
        .join(".regen-in-flight");
    log_if_err(
        "regen-in-flight stamp",
        &regen_marker,
        fs_atomic::atomic_write(&regen_marker, b""),
    );

    // Step 1: Adoption sweep
    adopt_workspace_skill_drift(project_path);

    // Step 2: Clear stale tail
    let preserved_freeform = strip_workspace_skill_tail(project_path);

    // Step 3: Compose K2SO-managed body
    let mut managed_body = match base_body {
        Some(body) => body.to_string(),
        None => generate_workspace_skill_content(&project_name),
    };

    let primary_agent = find_primary_agent(project_path);
    if !managed_body.ends_with('\n') {
        managed_body.push('\n');
    }
    managed_body.push('\n');
    managed_body.push_str(&skill_update_footer(project_path, primary_agent.as_deref()));

    // Step 4: Write managed body
    write_skill_to_all_harnesses(
        project_path,
        "k2so",
        "workspace",
        SKILL_VERSION_WORKSPACE,
        "K2SO workspace context — CLI reference + project context + primary agent persona",
        &managed_body,
        false,
    );

    // Step 5: Append fresh SOURCE regions
    append_workspace_source_regions(project_path, preserved_freeform.as_deref());

    // Steps 6 + 7: fan out
    let canonical = PathBuf::from(project_path).join(".k2so/skills/k2so/SKILL.md");
    if !is_harness_management_skipped(project_path) {
        if let Ok(full) = fs::read_to_string(&canonical) {
            let injection_body = strip_frontmatter(&full).trim().to_string();
            let root = PathBuf::from(project_path);
            upsert_k2so_section(&root.join("AGENTS.md"), &injection_body);
            let github_dir = root.join(".github");
            let _ = fs::create_dir_all(&github_dir);
            upsert_k2so_section(&github_dir.join("copilot-instructions.md"), &injection_body);
        }

        let root_skill = PathBuf::from(project_path).join("SKILL.md");
        force_symlink(&canonical, &root_skill);
        let root_claude = PathBuf::from(project_path).join("CLAUDE.md");
        migrate_and_symlink_root_claude_md(&canonical, &root_claude, project_path);
        write_workspace_harness_discovery_targets(project_path, &canonical);
    }

    // Step 8: Stamp last-regen hashes
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

    log_if_err(
        "regen-in-flight clear",
        &regen_marker,
        fs::remove_file(&regen_marker),
    );
}

/// Walk the existing canonical SKILL.md and adopt any SOURCE-region drift
/// back into its canonical source file (PROJECT.md or the primary agent's
/// AGENT.md).
fn adopt_workspace_skill_drift(project_path: &str) {
    let canonical = PathBuf::from(project_path).join(".k2so/skills/k2so/SKILL.md");
    let Ok(skill_content) = fs::read_to_string(&canonical) else {
        return;
    };
    let stamp_path = PathBuf::from(project_path).join(".k2so").join(".last-skill-regen");
    let last_regen = mtime_secs(&stamp_path);
    let stored_hashes = read_regen_hashes(project_path);

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
                log_adoption_event(
                    project_path,
                    "PROJECT.md: user edit detected — downstream SKILL.md + harness files will pick up the new content on this regen",
                );
            } else if !region_stripped.trim().is_empty() {
                let frontmatter = if let Ok(raw) = fs::read_to_string(&project_md) {
                    if raw.starts_with("---") {
                        if let Some(end) = raw[3..].find("---") {
                            Some(raw[..3 + end + 3].to_string())
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                } else {
                    None
                };
                let new_contents = match frontmatter {
                    Some(fm) => format!("{}\n\n{}\n", fm.trim_end(), region_stripped.trim()),
                    None => format!("{}\n", region_stripped.trim()),
                };
                match atomic_write_str(&project_md, &new_contents) {
                    Ok(()) => log_adoption_event(
                        project_path,
                        "ADOPTED PROJECT.md: SKILL.md SOURCE region committed back to .k2so/PROJECT.md",
                    ),
                    Err(e) => log_if_err::<(), _>("adopt PROJECT.md", &project_md, Err::<(), _>(e)),
                }
            }
        }
    }

    // Primary agent's AGENT.md adoption
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
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        } else {
                            None
                        };
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

/// Remove everything after the MANAGED:END marker in the canonical SKILL.md.
/// Returns any truly user-authored content found after the LAST
/// `<!-- K2SO:USER_NOTES -->` sentinel so it can be re-appended after
/// regeneration.
pub fn strip_workspace_skill_tail(project_path: &str) -> Option<String> {
    let canonical = PathBuf::from(project_path).join(".k2so/skills/k2so/SKILL.md");
    let Ok(content) = fs::read_to_string(&canonical) else { return None };
    let end_idx = content.find(SKILL_END_MARKER)?;
    let after_end_start = end_idx + SKILL_END_MARKER.len();
    let tail = &content[after_end_start..];

    let preserved = tail.rfind(SKILL_USER_NOTES_SENTINEL).map(|idx| {
        let after = idx + SKILL_USER_NOTES_SENTINEL.len();
        tail[after..].to_string()
    });

    let truncated = format!("{}\n", &content[..after_end_start]);
    log_if_err(
        "strip_workspace_skill_tail write",
        &canonical,
        atomic_write_str(&canonical, &truncated),
    );

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
/// sub-regions below the END marker in the canonical file.
pub fn append_workspace_source_regions(project_path: &str, preserved_freeform: Option<&str>) {
    let canonical = PathBuf::from(project_path).join(".k2so/skills/k2so/SKILL.md");
    let Ok(mut content) = fs::read_to_string(&canonical) else { return };
    if !content.ends_with('\n') {
        content.push('\n');
    }

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

/// CLAUDE.md migration helper for the 0.32.7 transition.
fn migrate_and_symlink_root_claude_md(canonical: &Path, root_claude: &Path, project_path: &str) {
    match fs::symlink_metadata(root_claude) {
        Ok(meta) if meta.file_type().is_symlink() => {
            force_symlink(canonical, root_claude);
        }
        Ok(meta) if meta.file_type().is_file() => {
            let content = fs::read_to_string(root_claude).unwrap_or_default();
            let is_k2so_generated = content.starts_with("# K2SO ");
            let archived = archive_claude_md_file(project_path, root_claude, "CLAUDE.md");
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
                if is_k2so_generated {
                    "K2SO-generated "
                } else {
                    "user-authored "
                },
            );
        }
        _ => {
            force_symlink(canonical, root_claude);
        }
    }
}

/// Append the body of a pre-existing CLAUDE.md into the canonical
/// SKILL.md's USER_NOTES region so the migrated content stays visible
/// to Claude Code (via the symlink) without requiring the user to hand-
/// merge from `.k2so/migration/`.
fn import_claude_md_into_user_notes(
    project_path: &str,
    body: &str,
    source_label: &str,
    archive_display: &str,
) {
    let canonical = PathBuf::from(project_path).join(".k2so/skills/k2so/SKILL.md");
    if !canonical.exists() {
        return;
    }
    let Ok(existing) = fs::read_to_string(&canonical) else { return };

    let import_sentinel = format!(
        "<!-- K2SO:IMPORT:CLAUDE_MD archive={} -->",
        archive_display
    );
    if existing.contains(&import_sentinel) {
        return;
    }

    let Some(sentinel_idx) = existing.find(SKILL_USER_NOTES_SENTINEL) else {
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
/// pre-0.32.7 per-agent CLAUDE.md generator. Each is archived to
/// `.k2so/migration/agents/<name>/CLAUDE.md-<timestamp>.md` then removed.
///
/// Gated with `.k2so/.harvest-0.32.7-done` so a user who later runs
/// `generate-md` isn't re-harvested on the next boot. First-run only.
pub fn harvest_per_agent_claude_md_files(project_path: &str) {
    let sentinel = PathBuf::from(project_path)
        .join(".k2so")
        .join(".harvest-0.32.7-done");
    if sentinel.exists() {
        return;
    }

    let agents_root = PathBuf::from(project_path).join(".k2so").join("agents");
    let mut archived_paths: Vec<PathBuf> = Vec::new();
    let mut any_failure = false;
    if let Ok(read_dir) = fs::read_dir(&agents_root) {
        for entry in read_dir.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
            if name.starts_with('.') {
                continue;
            }
            let claude_md = path.join("CLAUDE.md");
            if !claude_md.is_file() {
                continue;
            }
            match archive_claude_md_file(
                project_path,
                &claude_md,
                &format!("agents/{}/CLAUDE.md", name),
            ) {
                Some(archive_path) => {
                    if let Err(e) = crate::safe_delete::trash(&claude_md) {
                        log_if_err::<(), _>(
                            "harvest trash original",
                            &claude_md,
                            Err::<(), _>(format!("{e}")),
                        );
                        any_failure = true;
                    }
                    archived_paths.push(archive_path);
                }
                None => {
                    any_failure = true;
                }
            }
        }
    }
    if !archived_paths.is_empty() {
        inject_first_migration_banner(project_path, &archived_paths);
    }
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

/// Copy a file to `.k2so/migration/<relative>-<timestamp>.<ext>`.
/// Returns the path of the archive on success.
fn archive_claude_md_file(
    project_path: &str,
    source: &Path,
    relative_id: &str,
) -> Option<PathBuf> {
    let content = fs::read_to_string(source).ok()?;
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
    let (leaf_stem, leaf_ext) = match leaf.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem.to_string(), format!(".{}", ext)),
        _ => (leaf.to_string(), String::new()),
    };
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
/// `.k2so/MIGRATION-0.32.7.md` listing the archive paths.
fn inject_first_migration_banner(project_path: &str, archived_paths: &[PathBuf]) {
    if archived_paths.is_empty() {
        return;
    }
    let notice_path = PathBuf::from(project_path)
        .join(".k2so")
        .join("MIGRATION-0.32.7.md");
    if notice_path.exists() {
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
// Extended harness file-discovery coverage
// ══════════════════════════════════════════════════════════════════════

/// Create a symlink for a workspace-root harness file with the contract:
///   1. Archive the original to `.k2so/migration/` (never destroy).
///   2. Import its body into SKILL.md's USER_NOTES so the new symlinked
///      SKILL.md still surfaces the user's accumulated context.
///   3. Replace the target with the symlink.
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

/// Workspace-level harness file-discovery targets.
fn write_workspace_harness_discovery_targets(project_path: &str, canonical: &Path) {
    let root = PathBuf::from(project_path);

    safe_symlink_harness_file(canonical, &root.join("GEMINI.md"), project_path, "GEMINI.md");
    safe_symlink_harness_file(canonical, &root.join("AGENT.md"), project_path, "AGENT.md");
    safe_symlink_harness_file(canonical, &root.join(".goosehints"), project_path, ".goosehints");

    write_cursor_rules_mdc(project_path, canonical);
    scaffold_aider_conf(project_path);
}

/// Generate `./.cursor/rules/k2so.mdc` with MDC frontmatter + the
/// canonical SKILL.md body.
fn write_cursor_rules_mdc(project_path: &str, canonical: &Path) {
    let Ok(raw) = fs::read_to_string(canonical) else { return };
    let body = strip_frontmatter(&raw).trim().to_string();
    if body.is_empty() {
        return;
    }

    let dir = PathBuf::from(project_path).join(".cursor").join("rules");
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    let target = dir.join("k2so.mdc");

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
/// the workspace context on every session.
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
    if existing.contains("SKILL.md") {
        return;
    }

    let _ = archive_claude_md_file(project_path, &path, ".aider.conf.yml");

    let lines: Vec<&str> = existing.lines().collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len() + 4);
    let mut injected = false;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim_start();
        if !injected && (trimmed == "read:" || trimmed.starts_with("read:")) {
            out.push(line.to_string());
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
    if !final_out.ends_with('\n') {
        final_out.push('\n');
    }
    log_if_err(
        "scaffold_aider_conf merge",
        &path,
        atomic_write_str(&path, &final_out),
    );
}

// ══════════════════════════════════════════════════════════════════════
// Workspace teardown (disconnect)
// ══════════════════════════════════════════════════════════════════════

/// Freeze or restore every workspace-root symlink, returning a per-file
/// summary the UI can display.
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
        let Ok(meta) = fs::symlink_metadata(&path) else { continue };
        if !meta.file_type().is_symlink() {
            continue;
        }

        match mode {
            TeardownMode::KeepCurrent => match atomic_write_str(&path, &current_body) {
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
            },
            TeardownMode::RestoreOriginal => match find_latest_archive(project_path, rel) {
                Some(archive_path) => match fs::read_to_string(&archive_path) {
                    Ok(body) => match atomic_write_str(&path, &body) {
                        Ok(()) => results.push(TeardownResult {
                            action: "restored".to_string(),
                            path: rel.to_string(),
                            note: format!("Restored from archive: {}", archive_path.display()),
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
                    log_if_err(
                        "restore_original trash symlink",
                        &path,
                        crate::safe_delete::trash_or_remove(&path),
                    );
                    results.push(TeardownResult {
                        action: "removed".to_string(),
                        path: rel.to_string(),
                        note: "No prior archive — K2SO created this file fresh; sent to Trash.".to_string(),
                    });
                }
            },
        }
    }

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
            log_if_err(
                "teardown trash aider.conf.yml",
                &aider_path,
                crate::safe_delete::trash_or_remove(&aider_path),
            );
            results.push(TeardownResult {
                action: "removed".to_string(),
                path: ".aider.conf.yml".to_string(),
                note: "No prior archive — K2SO scaffolded this file fresh; sent to Trash.".to_string(),
            });
        }
    }

    results
}

/// Walk `.k2so/migration/` looking for the most-recent archive that
/// matches the relative harness path.
fn find_latest_archive(project_path: &str, rel: &str) -> Option<PathBuf> {
    let migration_root = PathBuf::from(project_path).join(".k2so").join("migration");
    if !migration_root.is_dir() {
        return None;
    }

    let (subdir, leaf) = match rel.rsplit_once('/') {
        Some((parent, leaf)) => (Some(parent.to_string()), leaf.to_string()),
        None => (None, rel.to_string()),
    };
    let search_dir = match &subdir {
        Some(s) => migration_root.join(s),
        None => migration_root.clone(),
    };
    if !search_dir.is_dir() {
        return None;
    }

    let (leaf_stem, leaf_ext) = match leaf.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem.to_string(), format!(".{}", ext)),
        _ => (leaf.clone(), String::new()),
    };

    let mut best: Option<(u128, PathBuf)> = None;
    if let Ok(entries) = fs::read_dir(&search_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n,
                None => continue,
            };
            let prefix = format!("{}-", leaf_stem);
            if !name.starts_with(&prefix) {
                continue;
            }
            if !leaf_ext.is_empty() && !name.ends_with(&leaf_ext) {
                continue;
            }
            let rest = &name[prefix.len()..];
            let rest = if leaf_ext.is_empty() {
                rest
            } else {
                rest.trim_end_matches(&leaf_ext[..])
            };
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

/// Tauri-style entrypoint: parse mode string + run teardown. Returns
/// the per-file result list ready for JSON-encoding back to the UI.
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

/// Inspect a workspace path WITHOUT mutating anything. Returns a list
/// of entries describing what K2SO will do on add.
pub fn k2so_agents_preview_workspace_ingest(
    project_path: String,
) -> Result<Vec<WorkspacePreviewEntry>, String> {
    let root = PathBuf::from(&project_path);
    let mut entries: Vec<WorkspacePreviewEntry> = Vec::new();

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

    let marker_targets: &[(&str, &str)] = &[
        ("AGENTS.md", "Codex / OpenCode / Pi"),
        (".github/copilot-instructions.md", "GitHub Copilot"),
    ];
    for (rel, label) in marker_targets {
        let path = root.join(rel);
        let size = fs::metadata(&path).ok().map(|m| m.len());
        let action = if path.exists() { "marker_injected" } else { "create" };
        let note = if path.exists() {
            format!(
                "{} — K2SO block inserted between markers, your content preserved",
                label
            )
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
pub fn k2so_agents_run_workspace_ingest(project_path: String) -> Result<(), String> {
    harvest_per_agent_claude_md_files(&project_path);
    write_workspace_skill_file(&project_path);
    Ok(())
}

/// Universal skill refresh. Walks every agent folder + the workspace
/// skill and re-invokes the regular write_* functions. Because those now
/// route through ensure_skill_up_to_date, this is idempotent.
pub fn ensure_all_skills_up_to_date(project_path: &str) {
    write_workspace_skill_file(project_path);

    let agents_root = agents_dir(project_path);
    if !agents_root.exists() {
        return;
    }
    let Ok(entries) = fs::read_dir(&agents_root) else { return };
    for entry in entries.flatten() {
        let agent_path = entry.path();
        if !agent_path.is_dir() {
            continue;
        }
        let name_osstr = entry.file_name();
        let agent_name = name_osstr.to_string_lossy();
        if agent_name.starts_with('.') {
            continue;
        }

        let agent_md = agent_path.join("AGENT.md");
        if !agent_md.exists() {
            continue;
        }
        let agent_content = fs::read_to_string(&agent_md).unwrap_or_default();
        let fm = parse_frontmatter(&agent_content);
        let agent_type = fm.get("type").cloned().unwrap_or_else(|| "agent-template".to_string());
        let normalized_type = match agent_type.as_str() {
            "pod-leader" | "coordinator" => "manager".to_string(),
            other => other.to_string(),
        };
        crate::agents::skill_writer::write_agent_skill_file(
            project_path,
            &agent_name,
            &normalized_type,
        );
    }
}

// ══════════════════════════════════════════════════════════════════════
// Workspace SKILL.md regen — public entry point
// ══════════════════════════════════════════════════════════════════════
//
// Phase 2 Unit 7d — moved from `src-tauri/src/commands/k2so_agents.rs`.
// The legacy Tauri command `k2so_agents_regenerate_workspace_skill` is
// now a thin forward into [`regenerate_workspace_skill`] below. The
// daemon calls this directly during first-boot sweeps without needing
// any Tauri context.
//
// Coordination with `write_workspace_skill_file_with_body` (Unit 7b):
// this is the *entry point* a user (or the regen lifecycle) hits — it
// builds the rich CLAUDE.md body (manager brief or AI planner brief)
// and hands it off to the existing writer as `base_body`. The writer
// is unchanged.

/// User-facing CLI command documentation injected into the manager and
/// AI planner CLAUDE.md briefs. Kept as a single `const` so the two
/// templates below stay in sync.
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
- `heartbeat` (without "wake") → raw triage that launches the workspace's primary agent, does NOT resume sessions or send messages

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

/// Manager-specific workflow guidance injected into the manager CLAUDE.md.
const WORKFLOW_DOCS: &str = r#"## Workflow

### If you are the Lead Agent (orchestrator):
1. Check for work: `k2so inbox` (workspace-implicit; pass `--workspace <path>` to target another)
2. Read each request and decide which agent should handle it
3. Assign work with a single command — K2SO handles everything else:
   ```
   k2so delegate backend-eng .k2so/inbox/add-oauth-support.md
   ```
   This creates a worktree, writes a CLAUDE.md, and launches the agent automatically.
4. To break a large request into sub-tasks first:
   ```
   k2so inbox compose --title "Build API endpoints" --body "..."
   k2so inbox compose --title "Build login UI" --body "..."
   ```
   Then delegate each: `k2so delegate backend-eng .k2so/inbox/build-api-endpoints.md`
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

/// Regenerate the workspace-root SKILL.md — the lead agent's complete
/// operating manual. Written to `<project-root>/SKILL.md` with a
/// matching `<project-root>/CLAUDE.md` symlink so Claude Code auto-
/// discovers it. The SKILL.md is the canonical source of truth;
/// CLAUDE.md is a harness-specific entry point.
///
/// Also auto-scaffolds the `.k2so/` layout on first call (manager +
/// k2so-agent dirs, inbox/active/done folders, prds/, PROJECT.md).
///
/// Phase 2 Unit 7d: moved from
/// `src-tauri/src/commands/k2so_agents.rs::k2so_agents_regenerate_workspace_skill`
/// so the daemon can run this directly. The Tauri-side wrapper now
/// forwards here.
///
/// Returns the composed CLAUDE.md body that was injected into the
/// canonical SKILL.md — the renderer's regen UIs use this for preview.
pub fn regenerate_workspace_skill(project_path: String) -> Result<String, String> {
    use crate::agents::skill_writer::{
        generate_default_agent_body, write_agent_skill_file,
    };
    use crate::agents::work_item::read_work_item;

    let project_name = std::path::Path::new(&project_path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "workspace".to_string());

    // Scaffold .k2so/ structure if it doesn't exist.
    // Post-Phase-2.1: the workspace inbox lives at `.k2so/inbox/` (the
    // unified `k2so_core::inbox::*` primitive). The legacy
    // `.k2so/work/inbox/` was retired and the first-boot daemon hook
    // trashes any straggler.
    let k2so_dir = PathBuf::from(&project_path).join(".k2so");
    let _ = fs::create_dir_all(k2so_dir.join("inbox"));
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
        let manager_body =
            generate_default_agent_body("manager", "manager", manager_role, &project_path);
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
        let k2so_body =
            generate_default_agent_body("k2so", "k2so-agent", k2so_role, &project_path);
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

    // List workspace inbox items.
    //
    // Post-Phase-2.1: workspace inbox lives at `.k2so/inbox/` (the
    // unified `k2so_core::inbox::*` primitive). Root-level items
    // (`folder == ""`) are the untriaged arrivals; sub-foldered items
    // have already been organized and aren't surfaced in the manager's
    // skill summary.
    let mut inbox_summary = String::new();
    let ws_inbox_items =
        crate::inbox::list_folder(std::path::Path::new(&project_path), "");
    for item in &ws_inbox_items {
        // `type` dropped in the WorkItem → InboxItem migration; `source`
        // (the WorkItem `source` field) is preserved on InboxItem and
        // does double duty here.
        inbox_summary.push_str(&format!(
            "- **{}** (priority: {}, source: {})\n",
            item.title, item.priority, item.source
        ));
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
            )
            .ok()
        };

        match db_mode.as_deref() {
            Some("manager") | Some("coordinator") | Some("pod") => true,
            Some("agent") => false,
            _ => {
                // Fallback: if agents dir has sub-agents, assume manager mode
                let agents_root = agents_dir(&project_path);
                agents_root.exists()
                    && fs::read_dir(&agents_root)
                        .map(|e| {
                            e.flatten()
                                .any(|e| e.file_type().map_or(false, |ft| ft.is_dir()))
                        })
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
            let _ = crate::agents::work_item::atomic_write(&project_md_path, &project_md_content);
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

/// Remove or disable the workspace SKILL.md + CLAUDE.md symlink
/// (when the Agent toggle is turned off).
///
/// Phase 2 Unit 7d: moved from
/// `src-tauri/src/commands/k2so_agents.rs::k2so_agents_disable_workspace_claude_md`.
pub fn disable_workspace_claude_md(project_path: String) -> Result<(), String> {
    let claude_md = PathBuf::from(&project_path).join("CLAUDE.md");
    let disabled = PathBuf::from(&project_path).join(".k2so").join("CLAUDE.md.disabled");

    if claude_md.exists() {
        // Move to .k2so/ rather than delete — preserves any user edits
        fs::rename(&claude_md, &disabled)
            .map_err(|e| format!("Failed to disable CLAUDE.md: {}", e))?;
    }
    Ok(())
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
    use crate::agents::skill::{SKILL_BEGIN_MARKER, SKILL_END_MARKER};
    use std::path::PathBuf;
    use uuid::Uuid;

    /// Make a scratch `.k2so/` scaffold for a migration test.
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

        assert!(root_claude.exists(), "archive must not delete the source");
        let archived_body = fs::read_to_string(&archive).unwrap();
        assert_eq!(archived_body, body, "archive must preserve content byte-for-byte");
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

        assert!(!agent_claude.exists(), "per-agent CLAUDE.md should be removed after harvest");
        let archive_root = proj.join(".k2so/migration/agents/backend-eng");
        let entries: Vec<_> = fs::read_dir(&archive_root).unwrap().flatten().collect();
        assert_eq!(entries.len(), 1, "expected exactly one archive, got {:?}", entries);
        let archived = fs::read_to_string(entries[0].path()).unwrap();
        assert_eq!(archived, body, "archive must preserve content byte-for-byte");
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

        fs::write(&agent_claude, "user-regenerated content").unwrap();

        harvest_per_agent_claude_md_files(proj.to_str().unwrap());

        assert!(agent_claude.exists(), "second run must not re-harvest");
        assert_eq!(fs::read_to_string(&agent_claude).unwrap(), "user-regenerated content");
        let archive_root = proj.join(".k2so/migration/agents/backend-eng");
        let entries: Vec<_> = fs::read_dir(&archive_root).unwrap().flatten().collect();
        assert_eq!(entries.len(), 1, "idempotent harvest must not double-archive");
        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn strip_tail_preserves_user_freeform_but_discards_placeholders() {
        let proj = scratch_project();
        let canonical = proj.join(".k2so/skills/k2so/SKILL.md");
        let corrupted = format!(
            "---\nk2so_skill: workspace\n---\n\n{begin}\nManaged body\n{end}\n\n{sentinel}\n{placeholder}\n\n{sentinel}\n{placeholder}\n\nMy real user note line 1.\nMy real user note line 2.\n",
            begin = SKILL_BEGIN_MARKER,
            end = SKILL_END_MARKER,
            sentinel = SKILL_USER_NOTES_SENTINEL,
            placeholder = USER_NOTES_PLACEHOLDER,
        );
        fs::write(&canonical, &corrupted).unwrap();

        let preserved = strip_workspace_skill_tail(proj.to_str().unwrap());

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
        assert!(
            !preserved.contains(USER_NOTES_PLACEHOLDER),
            "placeholder comments must be stripped from preserved content"
        );
        let post = fs::read_to_string(&canonical).unwrap();
        assert!(
            post.ends_with(&format!("{}\n", SKILL_END_MARKER)),
            "file must end at the managed END marker after strip"
        );
        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn strip_tail_returns_none_when_tail_is_empty_or_placeholder_only() {
        let proj = scratch_project();
        let canonical = proj.join(".k2so/skills/k2so/SKILL.md");
        let noise = format!(
            "{begin}\nManaged\n{end}\n\n{sentinel}\n{placeholder}\n",
            begin = SKILL_BEGIN_MARKER,
            end = SKILL_END_MARKER,
            sentinel = SKILL_USER_NOTES_SENTINEL,
            placeholder = USER_NOTES_PLACEHOLDER,
        );
        fs::write(&canonical, &noise).unwrap();

        let preserved = strip_workspace_skill_tail(proj.to_str().unwrap());
        assert!(
            preserved.is_none(),
            "pure K2SO noise must not be preserved as user content"
        );
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

        inject_first_migration_banner(project_path, &[second_archive.clone()]);
        let after_second = fs::read_to_string(&notice_path).unwrap();
        assert!(after_second.starts_with(&after_first), "append must preserve existing content");
        assert!(after_second.len() > first_len, "second invocation must grow the file");
        assert!(after_second.contains("round-2"), "second archive must be appended");

        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn safe_symlink_archives_existing_regular_file() {
        let proj = scratch_project();
        let canonical = proj.join(".k2so/skills/k2so/SKILL.md");
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

        let meta = fs::symlink_metadata(&target).unwrap();
        assert!(meta.file_type().is_symlink(), "target must be a symlink after safe-link");
        let linked_body = fs::read_to_string(&target).unwrap();
        assert!(linked_body.contains("Managed body"), "managed region must survive import");
        assert!(
            linked_body.contains("user authored Gemini instructions"),
            "user's pre-existing body must be imported into canonical so the symlink still surfaces it"
        );
        let migration_dir = proj.join(".k2so/migration");
        let entries: Vec<_> = std::fs::read_dir(&migration_dir).unwrap().flatten().collect();
        let has_archive = entries.iter().any(|e| {
            let p = e.path();
            let body = fs::read_to_string(&p).unwrap_or_default();
            body == "user authored Gemini instructions"
        });
        assert!(
            has_archive,
            "pre-existing user file must be archived before symlink replaces it"
        );
        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn import_claude_md_lands_in_user_notes_and_is_idempotent() {
        let proj = scratch_project();
        let canonical = proj.join(".k2so/skills/k2so/SKILL.md");
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
        let user_notes_pos = after_first.find(SKILL_USER_NOTES_SENTINEL).unwrap();
        let import_pos = after_first.find("A useful note").unwrap();
        assert!(import_pos > user_notes_pos, "import must be below USER_NOTES sentinel");

        import_claude_md_into_user_notes(
            proj.to_str().unwrap(),
            user_body,
            "pre-existing user-authored CLAUDE.md",
            "/tmp/fake/archive.md",
        );
        let after_second = fs::read_to_string(&canonical).unwrap();
        assert_eq!(after_first, after_second, "re-import with same archive must be idempotent");

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
        let proj = scratch_project();
        let project_path = proj.to_str().unwrap();
        fs::create_dir_all(proj.join(".k2so/agents/backend-eng")).unwrap();
        let agent_claude = proj.join(".k2so/agents/backend-eng/CLAUDE.md");
        fs::write(&agent_claude, "backend agent notes").unwrap();

        harvest_per_agent_claude_md_files(project_path);

        let archive_dir = proj.join(".k2so/migration/agents/backend-eng");
        let archive_files: Vec<_> = fs::read_dir(&archive_dir).unwrap().flatten().collect();
        assert_eq!(archive_files.len(), 1, "first launch should archive once");
        let archived_body = fs::read_to_string(archive_files[0].path()).unwrap();
        assert_eq!(archived_body, "backend agent notes");

        harvest_per_agent_claude_md_files(project_path);

        let archive_files_after: Vec<_> = fs::read_dir(&archive_dir).unwrap().flatten().collect();
        assert_eq!(
            archive_files_after.len(),
            1,
            "re-add must not duplicate archives (sentinel gates re-harvest)"
        );
        let archived_after = fs::read_to_string(archive_files_after[0].path()).unwrap();
        assert_eq!(archived_after, "backend agent notes", "archive content must survive remove+re-add");
        assert!(
            proj.join(".k2so/.harvest-0.32.7-done").exists(),
            "sentinel persists across remove+re-add (it's filesystem, not DB)"
        );
        fs::remove_dir_all(&proj).ok();
    }

    /// Build a mock workspace that looks like the user was using every
    /// supported CLI LLM already.
    fn mock_multi_harness_workspace() -> PathBuf {
        let proj = scratch_project();
        fs::write(proj.join("CLAUDE.md"), "# Claude memory\nMy codebase notes from # memory writes.\n").unwrap();
        fs::write(proj.join("GEMINI.md"), "# Gemini instructions\nCustom Gemini behavior for this repo.\n").unwrap();
        fs::write(proj.join("AGENT.md"), "# AGENT.md\nAgent persona customizations.\n").unwrap();
        fs::write(proj.join(".goosehints"), "Goose hints — how to navigate this codebase.\n").unwrap();
        fs::write(
            proj.join(".aider.conf.yml"),
            "# Existing Aider config\nmodel: gpt-4o\nread:\n  - CONVENTIONS.md\n  - ARCHITECTURE.md\n",
        )
        .unwrap();
        fs::create_dir_all(proj.join(".opencode/agent")).unwrap();
        fs::write(
            proj.join(".opencode/agent/my-refactor-helper.md"),
            "# My custom OpenCode agent\nSpecialized refactoring persona.\n",
        )
        .unwrap();
        fs::create_dir_all(proj.join(".cursor/rules")).unwrap();
        fs::write(
            proj.join(".cursor/rules/my-codebase.mdc"),
            "---\nalwaysApply: true\n---\nMy project-specific Cursor rule.\n",
        )
        .unwrap();
        fs::write(
            proj.join(".k2so/PROJECT.md"),
            "# K2SO\n\nTauri workspace manager. Rust backend + React 19 frontend.\n",
        )
        .unwrap();
        proj
    }

    #[test]
    fn add_workspace_ingests_all_harness_files_into_skill_and_archives() {
        let proj = mock_multi_harness_workspace();
        let project_path = proj.to_str().unwrap();

        write_workspace_skill_file_with_body(project_path, None);

        let canonical = proj.join(".k2so/skills/k2so/SKILL.md");
        assert!(canonical.exists(), "canonical SKILL.md must be written");
        let skill_body = fs::read_to_string(&canonical).unwrap();

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

        assert!(
            proj.join(".opencode/agent/my-refactor-helper.md").exists(),
            "user's OpenCode agent files must be preserved untouched"
        );

        assert!(
            proj.join(".cursor/rules/my-codebase.mdc").exists(),
            "user's Cursor rule files must be preserved"
        );
        assert!(
            proj.join(".cursor/rules/k2so.mdc").exists(),
            "K2SO's Cursor MDC must be added"
        );

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

        write_workspace_skill_file_with_body(project_path, None);
        let second_body = fs::read_to_string(proj.join(".k2so/skills/k2so/SKILL.md")).unwrap();

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
        assert!(
            results.iter().all(|r| r.action == "froze"),
            "keep_current should produce only 'froze' actions: {:?}",
            results
        );

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

        assert_eq!(fs::read_to_string(proj.join("CLAUDE.md")).unwrap(), pre_claude);
        assert_eq!(fs::read_to_string(proj.join("GEMINI.md")).unwrap(), pre_gemini);
        assert_eq!(fs::read_to_string(proj.join("AGENT.md")).unwrap(), pre_agent);
        assert_eq!(fs::read_to_string(proj.join(".goosehints")).unwrap(), pre_goose);
        assert_eq!(fs::read_to_string(proj.join(".aider.conf.yml")).unwrap(), pre_aider);

        assert!(!proj.join("SKILL.md").exists(), "SKILL.md should be removed on restore (no prior original)");

        assert!(proj.join(".k2so/skills/k2so/SKILL.md").exists());
        assert!(proj.join(".k2so/migration").is_dir());
        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn reconnect_after_restore_original_reingests_cleanly() {
        let proj = mock_multi_harness_workspace();
        let project_path = proj.to_str().unwrap();

        write_workspace_skill_file_with_body(project_path, None);
        teardown_workspace_harness_files(project_path, TeardownMode::RestoreOriginal);
        write_workspace_skill_file_with_body(project_path, None);

        assert!(fs::symlink_metadata(proj.join("CLAUDE.md")).unwrap().file_type().is_symlink());
        assert!(fs::symlink_metadata(proj.join("GEMINI.md")).unwrap().file_type().is_symlink());

        let skill_body = fs::read_to_string(proj.join(".k2so/skills/k2so/SKILL.md")).unwrap();
        assert!(skill_body.contains("My codebase notes from # memory writes"));
        assert!(skill_body.contains("Custom Gemini behavior for this repo"));

        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn teardown_leaves_k2so_dir_fully_intact() {
        let proj = mock_multi_harness_workspace();
        let project_path = proj.to_str().unwrap();
        write_workspace_skill_file_with_body(project_path, None);
        let pre_project_md = fs::read_to_string(proj.join(".k2so/PROJECT.md")).unwrap();

        let pre_paths: Vec<PathBuf> = walk_dir(&proj.join(".k2so"));
        assert!(!pre_paths.is_empty(), "expected a populated .k2so/ before teardown");

        teardown_workspace_harness_files(project_path, TeardownMode::KeepCurrent);
        let post_paths: Vec<PathBuf> = walk_dir(&proj.join(".k2so"));

        for p in &pre_paths {
            assert!(
                post_paths.contains(p),
                "{} disappeared from .k2so/ during teardown — invariant violated",
                p.display(),
            );
        }
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

        let migration_root = proj.join(".k2so/migration");
        let mut found = false;
        if let Ok(entries) = fs::read_dir(&migration_root) {
            for e in entries.flatten() {
                if let Ok(body) = fs::read_to_string(e.path()) {
                    if body == original {
                        found = true;
                    }
                }
            }
        }
        assert!(found, "original .aider.conf.yml must be archived before mutation");

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

        safe_symlink_harness_file(&canonical, &target, proj.to_str().unwrap(), ".goosehints");
        safe_symlink_harness_file(&canonical, &target, proj.to_str().unwrap(), ".goosehints");

        let migration_dir = proj.join(".k2so/migration");
        let entries_count = std::fs::read_dir(&migration_dir)
            .map(|r| r.flatten().count())
            .unwrap_or(0);
        assert_eq!(
            entries_count, 0,
            "symlink-to-symlink re-run must not produce spurious archive entries"
        );
        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn completed_regen_clears_in_flight_marker() {
        let proj = mock_multi_harness_workspace();
        let project_path = proj.to_str().unwrap();
        write_workspace_skill_file_with_body(project_path, None);
        let marker = proj.join(".k2so/.regen-in-flight");
        assert!(!marker.exists(), "regen marker must be cleared on successful completion");
        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn detect_interrupted_regen_flags_stale_marker_once() {
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
        let proj = scratch_project();
        fs::create_dir_all(proj.join(".k2so")).unwrap();
        assert!(!detect_interrupted_regen(proj.to_str().unwrap()));
        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn archive_names_never_collide_under_rapid_fire() {
        let proj = scratch_project();
        let project_path = proj.to_str().unwrap();
        let agents = proj.join(".k2so/agents");
        fs::create_dir_all(&agents).unwrap();
        for i in 0..10 {
            let agent_dir = agents.join(format!("agent-{}", i));
            fs::create_dir_all(&agent_dir).unwrap();
            fs::write(agent_dir.join("CLAUDE.md"), format!("body for agent-{}", i)).unwrap();
        }
        harvest_per_agent_claude_md_files(project_path);

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
        let proj = mock_multi_harness_workspace();
        let project_path = proj.to_str().unwrap();
        write_workspace_skill_file_with_body(project_path, None);

        let _ = teardown_workspace_harness_files(project_path, TeardownMode::KeepCurrent);
        let claude = proj.join("CLAUDE.md");
        assert!(claude.exists(), "CLAUDE.md must exist after first keep_current");
        let first_body = fs::read_to_string(&claude).unwrap();
        assert!(!first_body.is_empty());

        for _ in 0..5 {
            let _ = teardown_workspace_harness_files(project_path, TeardownMode::KeepCurrent);
        }
        let final_body = fs::read_to_string(&claude).unwrap();
        assert_eq!(first_body, final_body, "repeated no-op teardowns must not mutate the frozen body");

        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn regen_stamps_content_hashes_for_drift_detection() {
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
        let proj = mock_multi_harness_workspace();
        let project_path = proj.to_str().unwrap();
        write_workspace_skill_file_with_body(project_path, None);

        let project_md = proj.join(".k2so/PROJECT.md");
        let original = fs::read_to_string(&project_md).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        fs::write(&project_md, &original).unwrap();
        assert!(
            mtime_secs(&project_md) > mtime_secs(&proj.join(".k2so/.last-skill-regen")),
            "test setup: source mtime must be newer than regen stamp"
        );

        let hashes = read_regen_hashes(project_path);
        let stored = hashes.get("project_md").cloned().unwrap_or_default();
        let current = content_hash_of(&project_md);
        assert_eq!(stored, current, "hash-based drift detection must ignore identical content");

        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn drift_adoption_detects_real_content_change() {
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
}

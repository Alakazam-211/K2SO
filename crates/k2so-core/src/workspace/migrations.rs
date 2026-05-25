//! Workspace one-shot migration helpers (boot-time).
//!
//! Phase 2.5d: extracted from the monolithic `agents/workspace.rs`. Each
//! function here is idempotent and gated by an on-disk sentinel or
//! row-existence check so re-running on every boot is cheap and safe.
//!
//! Also hosts the small archive-utility helpers
//! [`archive_claude_md_file`], [`inject_first_migration_banner`], and
//! [`log_adoption_event`] used by the SKILL writer + harness clusters.
//! They're `pub(crate)` so the sibling modules can re-import them; the
//! semantics are migration-flavored ("archive user-authored content
//! before mutating"), which is why they live here rather than in a
//! standalone utility module.

use std::fs;
use std::path::{Path, PathBuf};

use crate::db::schema::{AgentHeartbeat, WorkspaceSession};
use crate::fs_atomic::{self, atomic_write_str, log_if_err, unique_archive_path};
use crate::heartbeats::control::ensure_agent_wakeup;
use crate::heartbeats::k2so_heartbeat_add;
use crate::log_debug;
use crate::workspace::agent_identity::{
    agent_dir, agent_type_for, agents_dir, find_primary_agent, resolve_project_id,
};
use crate::workspace::wake_prompts::workspace_wakeup_path;

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

// ══════════════════════════════════════════════════════════════════════
// Archive utility helpers — shared with skill_writer + harness
// ══════════════════════════════════════════════════════════════════════
//
// These three helpers are migration-flavored ("archive user-authored
// content before mutating, log the event, banner if first time") and
// are used by the SKILL writer cluster (`workspace/skill_writer.rs`) and
// the harness file-discovery cluster (`workspace/harness.rs`) as well as
// the migration helpers above. Kept `pub(crate)` so they only escape the
// `workspace/` module family.

/// Copy a file to `.k2so/migration/<relative>-<timestamp>.<ext>`.
/// Returns the path of the archive on success.
pub(crate) fn archive_claude_md_file(
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
pub(crate) fn inject_first_migration_banner(project_path: &str, archived_paths: &[PathBuf]) {
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

/// Append a drift / conflict note to `.k2so/logs/adoption-conflicts.log`.
pub(crate) fn log_adoption_event(project_path: &str, line: &str) {
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

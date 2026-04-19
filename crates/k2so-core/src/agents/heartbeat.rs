//! Multi-heartbeat CRUD + tick evaluation + audit stamping.
//!
//! This is the piece that makes the persistent-agents feature real:
//! when launchd wakes the laptop and fires the heartbeat plist, the
//! daemon calls [`k2so_agents_heartbeat_tick`] to find eligible
//! heartbeats, runs them, and stamps audit rows so
//! `k2so heartbeat status <name>` can show what happened.
//!
//! The entire surface is Tauri-free. src-tauri keeps `#[tauri::command]`
//! wrappers around these functions so the existing UI frontend keeps
//! working unchanged; the daemon calls them directly over its HTTP
//! routes (`/cli/heartbeat/*`).
//!
//! See `.k2so/prds/multi-schedule-heartbeat.md` for the data-model
//! decisions behind this (per-heartbeat folder + `WAKEUP.md`,
//! workspace-relative `wakeup_path`, `heartbeat_fires` audit table).

use std::fs;

use serde::Serialize;

use crate::agents::{agent_dir, find_primary_agent, resolve_project_id};
use crate::db::schema::{AgentHeartbeat, HeartbeatFire};
use crate::log_debug;
use crate::scheduler::should_project_fire;

/// Create a new heartbeat row + scaffold its `WAKEUP.md` file.
///
/// `frequency` is the scheduler mode name (e.g. `"heartbeat"`,
/// `"daily"`, `"weekly"`, `"ordinal-weekday"`) and `spec_json` is the
/// mode-specific JSON payload (interval seconds, cron-ish spec, etc.).
/// Stores the `WAKEUP.md` path as workspace-relative so project moves
/// don't break rows.
pub fn k2so_heartbeat_add(
    project_path: String,
    name: String,
    frequency: String,
    spec_json: String,
) -> Result<serde_json::Value, String> {
    AgentHeartbeat::validate_name(&name).map_err(|e| e.to_string())?;
    let db = crate::db::shared();
    let conn = db.lock();
    let project_id = resolve_project_id(&conn, &project_path)
        .ok_or_else(|| format!("Project not found: {}", project_path))?;

    let agent_name = find_primary_agent(&project_path).ok_or(
        "No scheduleable agent found in this workspace. Enable heartbeat on a Custom, Workspace Manager, or K2SO Agent workspace first.",
    )?;

    // Create heartbeat folder and scaffold wakeup.md
    let hb_dir = agent_dir(&project_path, &agent_name)
        .join("heartbeats")
        .join(&name);
    fs::create_dir_all(&hb_dir)
        .map_err(|e| format!("Failed to create heartbeat folder: {}", e))?;
    let wakeup_file = hb_dir.join("WAKEUP.md");
    if !wakeup_file.exists() {
        let template = format!(
            "---\ndescription: One-line summary of what this heartbeat does (shown in other wakeup's context)\n---\n\n\
            # Wake procedure: {}\n\n\
            Replace this with the operational instructions for this heartbeat.\n\
            Keep it focused on what to do for this specific cadence — other heartbeats\n\
            live in sibling folders and run on their own schedules.\n",
            name
        );
        fs::write(&wakeup_file, template)
            .map_err(|e| format!("Failed to write wakeup.md: {}", e))?;
    }

    // Store workspace-relative path so project moves don't break rows
    let workspace_relative = wakeup_file
        .strip_prefix(&project_path)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| wakeup_file.to_string_lossy().to_string());

    let id = uuid::Uuid::new_v4().to_string();
    AgentHeartbeat::insert(
        &conn,
        &id,
        &project_id,
        &name,
        &frequency,
        &spec_json,
        &workspace_relative,
        true,
    )
    .map_err(|e| format!("Failed to insert heartbeat: {}", e))?;

    Ok(serde_json::json!({
        "id": id,
        "name": name,
        "wakeupPath": workspace_relative,
        "wakeupAbs": wakeup_file.to_string_lossy(),
    }))
}

/// List all heartbeat rows for a workspace (enabled + disabled).
pub fn k2so_heartbeat_list(project_path: String) -> Result<Vec<AgentHeartbeat>, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let project_id = resolve_project_id(&conn, &project_path)
        .ok_or_else(|| format!("Project not found: {}", project_path))?;
    AgentHeartbeat::list_by_project(&conn, &project_id).map_err(|e| e.to_string())
}

/// Delete a heartbeat row + best-effort remove its `WAKEUP.md` folder.
/// Row delete is the source of truth; folder cleanup is advisory.
pub fn k2so_heartbeat_remove(project_path: String, name: String) -> Result<(), String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let project_id = resolve_project_id(&conn, &project_path)
        .ok_or_else(|| format!("Project not found: {}", project_path))?;
    let agent_name = find_primary_agent(&project_path)
        .ok_or("No scheduleable agent in this workspace")?;

    AgentHeartbeat::delete(&conn, &project_id, &name).map_err(|e| e.to_string())?;
    let hb_dir = agent_dir(&project_path, &agent_name)
        .join("heartbeats")
        .join(&name);
    if hb_dir.exists() {
        let _ = fs::remove_dir_all(&hb_dir);
    }
    Ok(())
}

/// Toggle a heartbeat's `enabled` flag. Disabled rows are skipped by
/// the tick evaluator regardless of schedule eligibility.
pub fn k2so_heartbeat_set_enabled(
    project_path: String,
    name: String,
    enabled: bool,
) -> Result<(), String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let project_id = resolve_project_id(&conn, &project_path)
        .ok_or_else(|| format!("Project not found: {}", project_path))?;
    AgentHeartbeat::set_enabled(&conn, &project_id, &name, enabled)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Replace a heartbeat row's `frequency` + `spec_json` in place. Used
/// when the user edits the schedule via the Settings UI.
pub fn k2so_heartbeat_edit(
    project_path: String,
    name: String,
    frequency: String,
    spec_json: String,
) -> Result<(), String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let project_id = resolve_project_id(&conn, &project_path)
        .ok_or_else(|| format!("Project not found: {}", project_path))?;
    AgentHeartbeat::update_schedule(&conn, &project_id, &name, &frequency, &spec_json)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Result of a multi-heartbeat tick — one entry per heartbeat eligible
/// to fire right now. Caller is responsible for locking, spawning, and
/// stamping `last_fired` on success.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HeartbeatFireCandidate {
    pub name: String,
    pub agent_name: String,
    pub wakeup_path_abs: String,
    pub wakeup_path_rel: String,
}

/// Iterate enabled `agent_heartbeats` rows for a project and return the
/// subset whose schedules are due to fire now.
///
/// Does NOT lock, spawn, or stamp — those are the caller's
/// responsibility. Writes audit rows into `heartbeat_fires` for each
/// evaluated candidate (`fired_multi` / `skipped_schedule` /
/// `wakeup_file_missing`) so `k2so heartbeat status <name>` can show
/// what happened.
///
/// Auto-disables a heartbeat whose `WAKEUP.md` has been deleted from
/// disk — filesystem tampering recovery so the user notices.
pub fn k2so_agents_heartbeat_tick(project_path: &str) -> Vec<HeartbeatFireCandidate> {
    let db = crate::db::shared();
    let conn = db.lock();
    let Some(project_id) = resolve_project_id(&conn, project_path) else {
        return vec![];
    };
    let heartbeats = AgentHeartbeat::list_enabled(&conn, &project_id).unwrap_or_default();
    if heartbeats.is_empty() {
        return vec![];
    }
    let Some(agent_name) = find_primary_agent(project_path) else {
        return vec![];
    };

    let tick_start = std::time::Instant::now();
    let mut candidates = Vec::new();
    for hb in heartbeats {
        let eligible = should_project_fire(
            &hb.frequency,
            Some(&hb.spec_json),
            hb.last_fired.as_deref(),
        );
        if !eligible {
            let _ = HeartbeatFire::insert_with_schedule(
                &conn,
                &project_id,
                Some(&agent_name),
                Some(&hb.name),
                &hb.frequency,
                "skipped_schedule",
                Some("window not open"),
                None,
                None,
                Some(tick_start.elapsed().as_millis() as i64),
            );
            continue;
        }

        let wakeup_abs = std::path::Path::new(project_path).join(&hb.wakeup_path);
        if !wakeup_abs.exists() {
            let _ = AgentHeartbeat::set_enabled(&conn, &project_id, &hb.name, false);
            let _ = HeartbeatFire::insert_with_schedule(
                &conn,
                &project_id,
                Some(&agent_name),
                Some(&hb.name),
                &hb.frequency,
                "wakeup_file_missing",
                Some(&format!(
                    "auto-disabled: {} not found",
                    hb.wakeup_path
                )),
                None,
                None,
                Some(tick_start.elapsed().as_millis() as i64),
            );
            log_debug!(
                "[heartbeat-tick] {} wakeup file missing ({}), auto-disabled",
                hb.name,
                hb.wakeup_path
            );
            continue;
        }

        candidates.push(HeartbeatFireCandidate {
            name: hb.name,
            agent_name: agent_name.clone(),
            wakeup_path_abs: wakeup_abs.to_string_lossy().to_string(),
            wakeup_path_rel: hb.wakeup_path,
        });
    }
    candidates
}

/// Stamp `last_fired` on a heartbeat row. Called AFTER `spawn_wake_pty`
/// succeeds. Silent no-op if the row is gone (heartbeat removed
/// mid-run) — audit rows survive independently.
pub fn stamp_heartbeat_fired(project_path: &str, heartbeat_name: &str) {
    let db = crate::db::shared();
    let conn = db.lock();
    let Some(project_id) = resolve_project_id(&conn, project_path) else {
        return;
    };
    let _ = AgentHeartbeat::stamp_last_fired(&conn, &project_id, heartbeat_name);
}

/// Rename a heartbeat — renames the row AND moves the filesystem
/// folder so `wakeup_path` stays in sync. Lets users swap the
/// migration-reserved `default` name for something meaningful without
/// losing audit history.
///
/// Schedule-name on `heartbeat_fires` is denormalized on purpose —
/// audit survives without a cascade (fires referring to the old name
/// stay pointing at the old value, as designed).
pub fn k2so_heartbeat_rename(
    project_path: String,
    old_name: String,
    new_name: String,
) -> Result<(), String> {
    AgentHeartbeat::validate_name(&new_name).map_err(|e| e.to_string())?;
    let db = crate::db::shared();
    let conn = db.lock();
    let project_id = resolve_project_id(&conn, &project_path)
        .ok_or_else(|| format!("Project not found: {}", project_path))?;
    let hb = AgentHeartbeat::get_by_name(&conn, &project_id, &old_name)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Heartbeat '{}' not found", old_name))?;
    if AgentHeartbeat::get_by_name(&conn, &project_id, &new_name)
        .map_err(|e| e.to_string())?
        .is_some()
    {
        return Err(format!("Heartbeat '{}' already exists", new_name));
    }

    let agent_name = find_primary_agent(&project_path)
        .ok_or("No scheduleable agent in this workspace")?;
    let hb_parent = agent_dir(&project_path, &agent_name).join("heartbeats");
    let old_dir = hb_parent.join(&old_name);
    let new_dir = hb_parent.join(&new_name);

    // Tolerate already-moved state for reruns.
    if old_dir.exists() && !new_dir.exists() {
        fs::rename(&old_dir, &new_dir)
            .map_err(|e| format!("Failed to rename heartbeat folder: {}", e))?;
    }

    let new_wakeup = new_dir.join("WAKEUP.md");
    let workspace_relative = new_wakeup
        .strip_prefix(&project_path)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| new_wakeup.to_string_lossy().to_string());

    conn.execute(
        "UPDATE agent_heartbeats SET name = ?1, wakeup_path = ?2 \
         WHERE project_id = ?3 AND name = ?4",
        rusqlite::params![new_name, workspace_relative, project_id, old_name],
    )
    .map_err(|e| format!("Failed to rename row: {}", e))?;

    log_debug!(
        "[heartbeat-rename] {} → {} ({})",
        old_name,
        new_name,
        hb.wakeup_path
    );
    Ok(())
}

/// Return the most recent `limit` fire rows for a workspace. Powers
/// the History panel on the Workspaces Settings page. Newest first.
pub fn k2so_heartbeat_fires_list(
    project_path: String,
    limit: Option<i64>,
) -> Result<Vec<HeartbeatFire>, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let project_id = resolve_project_id(&conn, &project_path)
        .ok_or_else(|| format!("Project not found: {}", project_path))?;
    HeartbeatFire::list_by_project(&conn, &project_id, limit.unwrap_or(50))
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    //! Behaviour lives in src-tauri's integration tests today —
    //! `src-tauri/src/commands/k2so_agents.rs` has 30+ tests that
    //! exercise these same functions under their original call sites.
    //! Once the commands module itself moves into core the tests can
    //! come along.
}

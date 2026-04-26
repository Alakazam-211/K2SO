//! Triage + scheduler-fire for the daemon.
//!
//! Two endpoints live here, with DIFFERENT semantics that the
//! pre-Phase-4 Tauri listener also served distinctly:
//!
//! - **`/cli/agents/triage`** → `handle_triage` — read-only
//!   plain-text summary of what's pending in every agent's
//!   inbox, for the user's `k2so agents triage` verb. Matches
//!   the byte-for-byte shape Tauri's agent_hooks returned in
//!   0.33.0. No side effects.
//!
//! - **`/cli/scheduler-tick`** → `handle_scheduler_fire` — the
//!   destructive heartbeat path: runs `scheduler_tick` + the
//!   multi-heartbeat tick, composes each agent's wake prompt,
//!   and spawns a PTY per agent via `spawn_wake_via_session_stream`
//!   (when the project has `use_session_stream='on'`) or the
//!   legacy `spawn_wake_headless` (otherwise). Called by
//!   `~/.k2so/heartbeat.sh` on launchd's schedule.
//!
//! Phase 4 H7 retired Tauri's HTTP listener and pointed every
//! /cli/* route at the daemon. An earlier iteration of this file
//! put the destructive path at `/cli/agents/triage` — which
//! silently changed user-facing `k2so agents triage` from
//! "show me what's pending" to "launch everything that's pending"
//! AND left `/cli/scheduler-tick` returning a bare decision list,
//! which caused `heartbeat.sh` to silently no-op every tick. This
//! module is the post-H7 correction that restores the 0.33.0
//! Tauri route layout.
//!
//! Helpers here are `pub` so integration tests can invoke each
//! handler directly without the HTTP layer.

use k2so_core::agents::{heartbeat, scheduler, settings, triage_summary, wake};

/// Handler for `/cli/agents/triage` — read-only summary. Matches
/// pre-Phase-4 Tauri's `k2so_agents_triage_summary` shape. Used
/// by the user-facing `k2so agents triage` verb. Never spawns.
pub fn handle_triage(project_path: &str) -> String {
    triage_summary::triage_summary(project_path)
        .unwrap_or_else(|e| format!("Triage error: {}", e))
}

/// Handler for `/cli/scheduler-tick` — the destructive heartbeat
/// path. Returns `{"count":N, "launched":[…], "heartbeats":[…]}`.
///
/// Side-effects in order:
///   1. `scheduler_tick` → list of launchable agent names.
///   2. For each, compose the wake prompt per agent type. Agents
///      whose type has no shipped template (e.g. `agent-template`)
///      never wake autonomously — skipped.
///   3. Spawn via Session Stream or legacy PTY based on
///      `use_session_stream`.
///   4. Multi-heartbeat tick — each candidate carries its own
///      wakeup path; lock-gated per agent.
///   5. `stamp_heartbeat_fired` on the ones that actually spawned.
///   6. Return the count for `heartbeat.sh` to parse.
pub fn handle_scheduler_fire(project_path: &str) -> String {
    let launchable = match scheduler::k2so_agents_scheduler_tick(project_path.to_string()) {
        Ok(v) => v,
        Err(e) => {
            return serde_json::json!({
                "error": e,
                "count": 0,
                "launched": [],
                "heartbeats": [],
            })
            .to_string()
        }
    };

    let use_stream = settings::get_use_session_stream(project_path);

    let mut launched: Vec<String> = Vec::new();
    for agent_name in &launchable {
        let prompt = if agent_name == "__lead__" {
            wake::compose_wake_prompt_for_lead(project_path)
        } else {
            match wake::compose_wake_prompt_for_agent(project_path, agent_name) {
                Some(p) => p,
                None => continue, // agent-template: never wakes autonomously
            }
        };
        // Workspace-manager / non-multi-heartbeat path — no specific heartbeat row.
        let result = dispatch_wake(use_stream, agent_name, project_path, &prompt, None);
        match result {
            Ok(_tid) => launched.push(agent_name.clone()),
            Err(e) => {
                k2so_core::log_debug!(
                    "[daemon/scheduler-fire] spawn failed for {}: {}",
                    agent_name,
                    e
                );
            }
        }
    }

    let candidates = heartbeat::k2so_agents_heartbeat_tick(project_path);
    let mut hb_fired: Vec<String> = Vec::new();
    for cand in &candidates {
        if scheduler::is_agent_locked(project_path, &cand.agent_name) {
            k2so_core::log_debug!(
                "[daemon/scheduler-fire] skipped_locked hb={} agent={}",
                cand.name,
                cand.agent_name
            );
            continue;
        }
        let prompt = match wake::compose_wake_prompt_from_path(std::path::Path::new(
            &cand.wakeup_path_abs,
        )) {
            Some(p) => p,
            None => continue,
        };
        // Multi-heartbeat tick — pass the candidate's heartbeat name so
        // the spawn helper saves to agent_heartbeats.last_session_id and
        // the next tick resumes THIS heartbeat's chat thread (not the
        // agent's global session).
        let result = dispatch_wake(
            use_stream,
            &cand.agent_name,
            project_path,
            &prompt,
            Some(&cand.name),
        );
        match result {
            Ok(_tid) => {
                heartbeat::stamp_heartbeat_fired(project_path, &cand.name);
                hb_fired.push(cand.name.clone());
            }
            Err(e) => {
                k2so_core::log_debug!(
                    "[daemon/scheduler-fire] hb spawn failed for {}/{}: {}",
                    cand.agent_name,
                    cand.name,
                    e
                );
            }
        }
    }

    let mut all = launched.clone();
    all.extend(hb_fired.clone());
    serde_json::json!({
        "count": all.len(),
        "launched": all,
        "heartbeats": hb_fired,
    })
    .to_string()
}

/// Manually fire a single heartbeat by name (the CLI's
/// `k2so heartbeat fire <name>`). Unlike the scheduler tick this
/// does NOT consult the schedule window — if the row is enabled
/// and not archived, it spawns. Locked agents are skipped (matches
/// the scheduler's contract: never double-spawn).
///
/// Returns a JSON string the CLI can parse: `{success, decision,
/// reason?, terminalId?, agent?, name}` — same shape as the
/// audit row decisions so `k2so heartbeat status` and a fired
/// `k2so heartbeat fire` print consistent feedback.
pub fn handle_heartbeat_fire(project_path: &str, name: &str) -> String {
    use k2so_core::agents::resolve_project_id;
    use k2so_core::db::schema::{AgentHeartbeat, HeartbeatFire};

    if name.is_empty() {
        return serde_json::json!({
            "success": false,
            "decision": "error",
            "reason": "missing 'name' parameter",
        }).to_string();
    }

    // Look up the row + validate.
    let (hb, agent_name, project_id) = {
        let db = k2so_core::db::shared();
        let conn = db.lock();
        let Some(project_id) = resolve_project_id(&conn, project_path) else {
            return serde_json::json!({
                "success": false,
                "decision": "error",
                "reason": format!("project not found: {project_path}"),
                "name": name,
            }).to_string();
        };
        let Some(hb) = AgentHeartbeat::get_by_name(&conn, &project_id, name).ok().flatten() else {
            return serde_json::json!({
                "success": false,
                "decision": "error",
                "reason": format!("no heartbeat named '{name}'"),
                "name": name,
            }).to_string();
        };
        if hb.archived_at.is_some() {
            return serde_json::json!({
                "success": false,
                "decision": "skipped_archived",
                "reason": format!("heartbeat '{name}' is archived"),
                "name": name,
            }).to_string();
        }
        let Some(agent_name) = k2so_core::agents::find_primary_agent(project_path) else {
            return serde_json::json!({
                "success": false,
                "decision": "error",
                "reason": "no scheduleable agent in this workspace",
                "name": name,
            }).to_string();
        };
        (hb, agent_name, project_id)
    };

    // Lock check (single-flight: refuse to double-spawn against an agent
    // that's already running). Stamps an audit row so the user sees
    // why nothing happened.
    if scheduler::is_agent_locked(project_path, &agent_name) {
        let db = k2so_core::db::shared();
        let conn = db.lock();
        let _ = HeartbeatFire::insert_with_schedule(
            &conn, &project_id, Some(&agent_name), Some(&hb.name),
            &hb.frequency, "skipped_locked",
            Some("manual fire refused: agent already running"),
            None, None, None,
        );
        return serde_json::json!({
            "success": false,
            "decision": "skipped_locked",
            "reason": format!("agent '{agent_name}' is already running"),
            "name": name,
            "agent": agent_name,
        }).to_string();
    }

    // Read the heartbeat's WAKEUP.md (workspace-relative path).
    let wakeup_abs = std::path::Path::new(project_path).join(&hb.wakeup_path);
    if !wakeup_abs.exists() {
        let db = k2so_core::db::shared();
        let conn = db.lock();
        let _ = HeartbeatFire::insert_with_schedule(
            &conn, &project_id, Some(&agent_name), Some(&hb.name),
            &hb.frequency, "wakeup_file_missing",
            Some(&format!("manual fire failed: {} not found", hb.wakeup_path)),
            None, None, None,
        );
        return serde_json::json!({
            "success": false,
            "decision": "wakeup_file_missing",
            "reason": format!("WAKEUP.md missing at {}", hb.wakeup_path),
            "name": name,
            "agent": agent_name,
        }).to_string();
    }

    let Some(prompt) = wake::compose_wake_prompt_from_path(&wakeup_abs) else {
        return serde_json::json!({
            "success": false,
            "decision": "error",
            "reason": "failed to compose wake prompt",
            "name": name,
            "agent": agent_name,
        }).to_string();
    };

    // Spawn via the same path the scheduler tick uses, threading
    // heartbeat_name so post-spawn save targets agent_heartbeats.last_session_id.
    let use_stream = settings::get_use_session_stream(project_path);
    match dispatch_wake(use_stream, &agent_name, project_path, &prompt, Some(&hb.name)) {
        Ok(terminal_id) => {
            heartbeat::stamp_heartbeat_fired(project_path, &hb.name);
            let db = k2so_core::db::shared();
            let conn = db.lock();
            let _ = HeartbeatFire::insert_with_schedule(
                &conn, &project_id, Some(&agent_name), Some(&hb.name),
                &hb.frequency, "fired",
                Some("manual fire via CLI"),
                None, None, None,
            );
            serde_json::json!({
                "success": true,
                "decision": "fired",
                "name": name,
                "agent": agent_name,
                "terminalId": terminal_id,
            }).to_string()
        }
        Err(e) => {
            let db = k2so_core::db::shared();
            let conn = db.lock();
            let _ = HeartbeatFire::insert_with_schedule(
                &conn, &project_id, Some(&agent_name), Some(&hb.name),
                &hb.frequency, "error",
                Some(&format!("manual fire spawn failed: {e}")),
                None, None, None,
            );
            serde_json::json!({
                "success": false,
                "decision": "error",
                "reason": format!("spawn failed: {e}"),
                "name": name,
                "agent": agent_name,
            }).to_string()
        }
    }
}

/// Choose the spawn path based on the project's
/// `use_session_stream` flag. Exposed for callers (like tests)
/// that want to exercise one branch explicitly without a DB
/// round-trip.
fn dispatch_wake(
    use_stream: bool,
    agent_name: &str,
    project_path: &str,
    prompt: &str,
    heartbeat_name: Option<&str>,
) -> Result<String, String> {
    if use_stream {
        crate::agents_routes::spawn_wake_via_session_stream(
            agent_name, project_path, prompt, heartbeat_name,
        )
    } else {
        wake::spawn_wake_headless(agent_name, project_path, prompt, heartbeat_name)
    }
}

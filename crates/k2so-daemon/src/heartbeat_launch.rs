//! Smart heartbeat launch — single entry point for the Launch button,
//! the `k2so heartbeat launch <name>` CLI verb, and the cron-tick
//! scheduler. Per the daemon-first principle, all the decision +
//! spawn logic lives here in the daemon; the Tauri command and CLI
//! sub-command are thin proxies that hit the `/cli/heartbeat/launch`
//! HTTP route which calls [`smart_launch`].
//!
//! Decision tree (matches Rosson's spec in the heartbeats PRD):
//!
//! 1. `agent_heartbeats.last_session_id` is None
//!    → Fresh fire. Spawns a PTY whose `--append-system-prompt` is
//!      the WAKEUP.md body. Post-spawn deferred-save thread writes
//!      the new Claude session id back to the row.
//!
//! 2. `last_session_id` is Some + a live PTY in `session_lookup`
//!    has `--resume <session_id>` in its args
//!    → Inject. Writes the WAKEUP.md body + `\r` to the live PTY's
//!      input — same content the fresh path would send via
//!      `--append-system-prompt`, just delivered as a turn message
//!      into the running session.
//!
//! 3. `last_session_id` is Some + no live PTY
//!    → Resume + new PTY with both `--resume <session_id>` AND
//!      `--append-system-prompt <wakeup>` so Claude resumes the
//!      saved session and immediately receives the wakeup
//!      directive.
//!
//! In all three cases a `heartbeat_fires` audit row is written so
//! `k2so heartbeat status <name>` reflects the decision.

use std::path::Path;

use k2so_core::agents::{find_primary_agent, resolve_project_id, wake};
use k2so_core::db::schema::{AgentHeartbeat, HeartbeatFire};

use crate::session_lookup;

/// Decision returned by the planner half of smart-launch. Useful for
/// callers (and tests) that want to assert what would happen without
/// performing the spawn / write side effects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchDecision {
    FreshFire,
    Inject {
        /// k2so session id of the live PTY we'll write into.
        target_session_id: String,
    },
    ResumeAndFire {
        claude_session_id: String,
    },
    SkippedArchived,
    SkippedNotFound,
    SkippedNoAgent,
    SkippedWakeupMissing,
}

/// Run the full smart-launch flow. Returns a JSON value matching the
/// shape `triage::handle_heartbeat_fire` returns so existing CLI/UI
/// callers parse it without changes.
pub fn smart_launch(project_path: &str, name: &str) -> serde_json::Value {
    if name.is_empty() {
        return error_value("error", "missing 'name' parameter", name);
    }

    // Look up the heartbeat row + agent.
    let (hb, agent_name, project_id) = match resolve_row(project_path, name) {
        Ok(t) => t,
        Err(decision) => return decision,
    };

    // Validate WAKEUP.md is present so we can deliver content in any
    // of the three branches below.
    let wakeup_abs = Path::new(project_path).join(&hb.wakeup_path);
    if !wakeup_abs.exists() {
        write_audit(&project_id, &agent_name, &hb, "wakeup_file_missing",
            &format!("manual launch failed: {} not found", hb.wakeup_path));
        return error_value("wakeup_file_missing",
            &format!("WAKEUP.md missing at {}", hb.wakeup_path), name);
    }

    // Atomic claim of the in-flight lease — fixes the pre-existing
    // TOCTOU between scheduler eval and spawn. Honors the row's
    // `concurrency_policy`: under `forbid` (default) a second caller
    // sees `in_flight_started_at IS NOT NULL` and gets `false`.
    // Boot-time `sweep_stale_leases` clears leases left behind by
    // a daemon that crashed mid-spawn.
    if !acquire_lease(&project_id, &hb.name) {
        write_audit(&project_id, &agent_name, &hb, "skipped_locked",
            "smart_launch: heartbeat already in flight");
        return serde_json::json!({
            "success": false,
            "decision": "skipped_locked",
            "reason": "heartbeat already in flight",
            "name": hb.name,
        });
    }

    let saved_session = hb.last_session_id
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    // Branch 1: no saved session — fresh fire.
    if saved_session.is_none() {
        return run_fresh_fire(project_path, &project_id, &agent_name, &hb, &wakeup_abs);
    }
    let session_id = saved_session.unwrap();

    // Branch 2: live PTY for this session id — inject.
    if let Some((live_agent, live)) = find_live_for_resume(&session_id) {
        return run_inject(project_path, &project_id, &agent_name, &hb, &wakeup_abs,
            &session_id, live_agent, live);
    }

    // Branch 3: saved session, no live PTY — resume + fire.
    run_resume_and_fire(project_path, &project_id, &agent_name, &hb, &wakeup_abs, &session_id)
}

// ── Implementation ───────────────────────────────────────────────────

fn resolve_row(
    project_path: &str,
    name: &str,
) -> Result<(AgentHeartbeat, String, String), serde_json::Value> {
    let db = k2so_core::db::shared();
    let conn = db.lock();
    let project_id = resolve_project_id(&conn, project_path).ok_or_else(|| {
        error_value("error", &format!("project not found: {project_path}"), name)
    })?;
    let hb = AgentHeartbeat::get_by_name(&conn, &project_id, name).ok().flatten().ok_or_else(|| {
        error_value("error", &format!("no heartbeat named '{name}'"), name)
    })?;
    if hb.archived_at.is_some() {
        return Err(error_value("skipped_archived",
            &format!("heartbeat '{name}' is archived"), name));
    }
    let agent_name = find_primary_agent(project_path).ok_or_else(|| {
        error_value("error", "no scheduleable agent in this workspace", name)
    })?;
    Ok((hb, agent_name, project_id))
}

fn find_live_for_resume(session_id: &str) -> Option<(String, session_lookup::LiveSession)> {
    // Walk every live session in the daemon's two maps; collect every
    // PTY whose args claim ownership of this session_id via either:
    //
    //   --session-id <uuid>   pinned at spawn time (fresh fire path,
    //                         post-P6 default — eliminates the
    //                         deferred-save race)
    //   --resume <uuid>       attached on a resume_and_fire branch,
    //                         on the user's tab, or any subsequent
    //                         interactive resume
    //
    // Multiple PTYs CAN match (e.g. user opens a tab on a session
    // that was just resumed by cron, or has multiple tabs). When that
    // happens we prefer `tab-*` agent names — those are tabs the user
    // is actively watching in the UI — over `__lead__` / agent-named
    // sessions, which are daemon-internal PTYs the user never sees.
    // Without this preference, inject writes into a hidden PTY and
    // the user wonders where their wakeup went.
    let mut matches: Vec<(String, session_lookup::LiveSession)> = Vec::new();
    for (agent, live) in session_lookup::snapshot_all() {
        let args = live.args();
        let mut i = 0;
        let mut found = false;
        while i + 1 < args.len() {
            if (args[i] == "--session-id" || args[i] == "--resume")
                && args[i + 1] == session_id
            {
                found = true;
                break;
            }
            i += 1;
        }
        if found {
            matches.push((agent, live));
        }
    }
    // Stable sort: tab-* first (rank 0), everything else (rank 1).
    matches.sort_by_key(|(agent, _)| if agent.starts_with("tab-") { 0 } else { 1 });
    matches.into_iter().next()
}

fn run_fresh_fire(
    project_path: &str,
    project_id: &str,
    agent_name: &str,
    hb: &AgentHeartbeat,
    wakeup_abs: &Path,
) -> serde_json::Value {
    let Some(prompt) = wake::compose_wake_prompt_from_path(wakeup_abs) else {
        release_lease(project_id, &hb.name);
        write_audit(project_id, agent_name, hb, "error", "failed to compose wake prompt");
        return error_value("error", "failed to compose wake prompt", &hb.name);
    };

    let use_stream = k2so_core::agents::settings::get_use_session_stream(project_path);
    let result = if use_stream {
        crate::agents_routes::spawn_wake_via_session_stream(
            agent_name, project_path, &prompt, Some(&hb.name),
        )
    } else {
        wake::spawn_wake_headless(agent_name, project_path, &prompt, Some(&hb.name))
    };

    match result {
        Ok(terminal_id) => {
            stamp_fired_and_release(project_id, &hb.name);
            write_audit(project_id, agent_name, hb, "fired",
                "smart_launch: no saved session — fresh fire");
            serde_json::json!({
                "success": true,
                "decision": "fired",
                "branch": "fresh_fire",
                "name": hb.name,
                "agent": agent_name,
                "terminalId": terminal_id,
            })
        }
        Err(e) => {
            release_lease(project_id, &hb.name);
            write_audit(project_id, agent_name, hb, "error",
                &format!("fresh fire spawn failed: {e}"));
            error_value("error", &format!("spawn failed: {e}"), &hb.name)
        }
    }
}

fn run_inject(
    project_path: &str,
    project_id: &str,
    agent_name: &str,
    hb: &AgentHeartbeat,
    wakeup_abs: &Path,
    session_id: &str,
    _live_agent: String,
    live: session_lookup::LiveSession,
) -> serde_json::Value {
    let body_raw = match std::fs::read_to_string(wakeup_abs) {
        Ok(s) => s,
        Err(e) => {
            release_lease(project_id, &hb.name);
            write_audit(project_id, agent_name, hb, "error",
                &format!("inject failed reading WAKEUP.md: {e}"));
            return error_value("error",
                &format!("could not read WAKEUP.md: {e}"), &hb.name);
        }
    };
    let body = wake::strip_frontmatter(&body_raw);
    let body_trimmed = body.trim();
    if body_trimmed.is_empty() {
        release_lease(project_id, &hb.name);
        write_audit(project_id, agent_name, hb, "error", "WAKEUP.md body empty");
        return error_value("error", "WAKEUP.md body is empty", &hb.name);
    }

    if let Err(e) = live.write(body_trimmed.as_bytes()) {
        release_lease(project_id, &hb.name);
        write_audit(project_id, agent_name, hb, "error",
            &format!("inject write failed: {e}"));
        return error_value("error",
            &format!("write to live PTY failed: {e}"), &hb.name);
    }
    // Two-phase: paste body, send Enter after a brief settle. Same
    // pattern the awareness-bus inject uses.
    std::thread::sleep(std::time::Duration::from_millis(150));
    let _ = live.write(b"\r");

    stamp_fired_and_release(project_id, &hb.name);
    let target_id = live.session_id().to_string();
    write_audit(project_id, agent_name, hb, "fired",
        &format!("smart_launch: injected into live session {target_id}"));
    serde_json::json!({
        "success": true,
        "decision": "fired",
        "branch": "injected",
        "name": hb.name,
        "agent": agent_name,
        "claudeSessionId": session_id,
        "targetSessionId": target_id,
    })
}

fn run_resume_and_fire(
    project_path: &str,
    project_id: &str,
    agent_name: &str,
    hb: &AgentHeartbeat,
    wakeup_abs: &Path,
    session_id: &str,
) -> serde_json::Value {
    let Some(prompt) = wake::compose_wake_prompt_from_path(wakeup_abs) else {
        release_lease(project_id, &hb.name);
        write_audit(project_id, agent_name, hb, "error", "failed to compose wake prompt");
        return error_value("error", "failed to compose wake prompt", &hb.name);
    };

    // Resume + --print: rejoin the saved conversation, deliver the
    // wakeup as the next user turn, claude responds + exits. PTY is
    // ephemeral so it doesn't accumulate stale entries in the daemon
    // session map. The user's tab (if/when they open one) becomes
    // the canonical long-lived view via openHeartbeatTab's interactive
    // --resume.
    let args = vec![
        "--dangerously-skip-permissions".to_string(),
        "--print".to_string(),
        "--resume".to_string(),
        session_id.to_string(),
        prompt,
    ];

    let outcome = crate::spawn::spawn_agent_session_v2_blocking(
        crate::spawn::SpawnAgentSessionRequest {
            agent_name: agent_name.to_string(),
            cwd: project_path.to_string(),
            command: Some("claude".to_string()),
            args: Some(args),
            cols: 120,
            rows: 38,
        },
    );

    match outcome {
        Ok(out) => {
            // Lock the agent_sessions row so later scheduler ticks don't
            // double-spawn this session.
            let _ = k2so_core::agents::session::k2so_agents_lock(
                project_path.to_string(),
                agent_name.to_string(),
                Some(out.session_id.to_string()),
                Some("system".to_string()),
            );
            // Surface the new PTY to any attached UI so a tab gets
            // created (gated by show_heartbeat_sessions on the
            // renderer side per P2.6).
            k2so_core::agent_hooks::emit(
                k2so_core::agent_hooks::HookEvent::CliTerminalSpawnBackground,
                serde_json::json!({
                    "terminalId": out.session_id.to_string(),
                    "command": "claude",
                    "cwd": project_path,
                    "projectPath": project_path,
                    "agentName": agent_name,
                    "heartbeatName": hb.name,
                }),
            );
            stamp_fired_and_release(&project_id, &hb.name);
            write_audit(project_id, agent_name, hb, "fired",
                "smart_launch: resumed session, fired wakeup");
            serde_json::json!({
                "success": true,
                "decision": "fired",
                "branch": "resume_and_fire",
                "name": hb.name,
                "agent": agent_name,
                "claudeSessionId": session_id,
                "targetSessionId": out.session_id.to_string(),
            })
        }
        Err(e) => {
            release_lease(project_id, &hb.name);
            write_audit(project_id, agent_name, hb, "error",
                &format!("resume spawn failed: {e}"));
            error_value("error", &format!("resume spawn failed: {e}"), &hb.name)
        }
    }
}

// ── Lease + stamp helpers ─────────────────────────────────────────

fn acquire_lease(project_id: &str, hb_name: &str) -> bool {
    let db = k2so_core::db::shared();
    let conn = db.lock();
    AgentHeartbeat::try_acquire_heartbeat(&conn, project_id, hb_name).unwrap_or(false)
}

fn release_lease(project_id: &str, hb_name: &str) {
    let db = k2so_core::db::shared();
    let conn = db.lock();
    let _ = AgentHeartbeat::release_heartbeat_lease(&conn, project_id, hb_name);
}

/// Atomic stamp of `last_fired` + clear the in-flight lease. Called
/// only on successful spawn paths; the failure paths use
/// `release_lease` and leave `last_fired` untouched so the heartbeat
/// stays eligible for the next tick.
fn stamp_fired_and_release(project_id: &str, hb_name: &str) {
    let db = k2so_core::db::shared();
    let conn = db.lock();
    let _ = AgentHeartbeat::stamp_fired_and_release(&conn, project_id, hb_name);
}

fn write_audit(
    project_id: &str,
    agent_name: &str,
    hb: &AgentHeartbeat,
    decision: &str,
    reason: &str,
) {
    let db = k2so_core::db::shared();
    let conn = db.lock();
    let _ = HeartbeatFire::insert_with_schedule(
        &conn, project_id, Some(agent_name), Some(&hb.name),
        &hb.frequency, decision, Some(reason),
        None, None, None,
    );
}

fn error_value(decision: &str, reason: &str, name: &str) -> serde_json::Value {
    serde_json::json!({
        "success": false,
        "decision": decision,
        "reason": reason,
        "name": name,
    })
}

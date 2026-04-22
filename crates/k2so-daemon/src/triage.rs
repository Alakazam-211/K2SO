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
        let result = dispatch_wake(use_stream, agent_name, project_path, &prompt);
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
        let result = dispatch_wake(use_stream, &cand.agent_name, project_path, &prompt);
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

/// Choose the spawn path based on the project's
/// `use_session_stream` flag. Exposed for callers (like tests)
/// that want to exercise one branch explicitly without a DB
/// round-trip.
fn dispatch_wake(
    use_stream: bool,
    agent_name: &str,
    project_path: &str,
    prompt: &str,
) -> Result<String, String> {
    if use_stream {
        crate::agents_routes::spawn_wake_via_session_stream(agent_name, project_path, prompt)
    } else {
        wake::spawn_wake_headless(agent_name, project_path, prompt)
    }
}

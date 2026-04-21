//! H6 of Phase 4 — heartbeat triage (scheduler + multi-heartbeat
//! dispatch) with Session Stream opt-in.
//!
//! This module owns the `/cli/agents/triage` handler. Before H6 it
//! lived inline in `main.rs` and unconditionally called
//! `k2so_core::agents::wake::spawn_wake_headless` — the legacy
//! `TerminalManager::create` path. H6 adds a per-project switch:
//! when `use_session_stream='on'`, each wake dispatches through
//! `agents_routes::spawn_wake_via_session_stream` so the new PTY
//! is daemon-owned and lands in `session_map` (reachable by every
//! other Phase 4 /cli/* route). Flag-off projects keep the legacy
//! path bit-for-bit.
//!
//! Moved out of `main.rs` so integration tests can call it
//! directly without spinning up the HTTP server. The route in
//! `cli.rs` now just forwards to `handle_triage`.

use k2so_core::agents::{heartbeat, scheduler, settings, wake};

/// Serve `/cli/agents/triage`. Runs `scheduler_tick` + the multi-
/// heartbeat tick, composes each agent's wake prompt, and spawns
/// via either the Session Stream pipeline or legacy
/// `spawn_wake_headless` based on the project's
/// `use_session_stream` setting. Always returns JSON; spawn
/// failures are logged, not propagated.
///
/// Intentionally simpler than src-tauri's agent_hooks triage
/// path: skips the worktree-resume + inbox-delegate branches
/// (those require `k2so_agents_build_launch`, which is still
/// src-tauri-only). Lid-closed wakes get the "compose wakeup
/// prompt + launch claude" behavior; resume + delegate stay
/// supervised.
pub fn handle_triage(project_path: &str) -> String {
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

    // Per-project opt-in for the Session Stream wake path. On:
    // spawn via daemon session_map. Off: legacy TerminalManager.
    let use_stream = settings::get_use_session_stream(project_path);

    let mut launched: Vec<String> = Vec::new();
    for agent_name in &launchable {
        // Compose the wake prompt per agent type. `__lead__` uses
        // the workspace manager's wake template + default heartbeat
        // body; other agents use their per-type wakeup.md.
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
                    "[daemon/triage] spawn failed for {}: {}",
                    agent_name,
                    e
                );
            }
        }
    }

    // Multi-heartbeat tick — each candidate carries its own
    // explicit wakeup path so different heartbeats can fire
    // different workflows on the same agent.
    let candidates = heartbeat::k2so_agents_heartbeat_tick(project_path);
    let mut hb_fired: Vec<String> = Vec::new();
    for cand in &candidates {
        if scheduler::is_agent_locked(project_path, &cand.agent_name) {
            k2so_core::log_debug!(
                "[daemon/triage] skipped_locked hb={} agent={}",
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
                    "[daemon/triage] hb spawn failed for {}/{}: {}",
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

//! Daemon-side headless wake spawn (v2).
//!
//! Replaces the pre-0.37.0 `k2so_core::agents::wake::spawn_wake_headless`,
//! which spawned through the in-process `terminal::shared()`
//! `TerminalManager` (Alacritty Legacy backend). 0.37.0 routes every
//! daemon-driven heartbeat fire through `spawn_agent_session_v2_blocking`
//! so the v2 invariants ("daemon-hosted, observable via grid WS,
//! attachable from Tauri later") hold for heartbeat fires too —
//! matching what every other daemon spawn already does (CLI launch,
//! awareness wake auto-launch, delegate worktree spawn).
//!
//! ## What stays the same
//!
//! - `claude --print --session-id <pinned>` semantics: heartbeat
//!   fires are short-lived one-shot invocations. claude reads the
//!   wake prompt, generates a response, persists session JSONL,
//!   exits. The PTY is removed from `v2_session_map` by the
//!   child-exit observer when claude returns.
//! - Pinned `--session-id` so two concurrent fires on the same
//!   agent get distinct deterministic UUIDs.
//! - Synchronous DB writes for heartbeat rows: `save_session_id` +
//!   `save_active_terminal_id` happen immediately after spawn, no
//!   deferred poll needed (the pinned UUID is what claude will use
//!   for its session JSONL).
//! - HookEvent emission so the frontend can react. Auto-surface is
//!   gated by the workspace's `show_heartbeat_sessions` flag —
//!   silent autonomous heartbeats never pop a tab unless the user
//!   has opted in. Once a tab attaches, `WorkspaceSession::set_surfaced`
//!   flips the per-row `surfaced` flag.
//!
//! ## What changes
//!
//! - Backend: `terminal::shared()` (in-process Alacritty Legacy) →
//!   `DaemonPtySession` via `spawn_agent_session_v2_blocking`.
//! - Map: legacy in-process `TerminalManager` registry → unified
//!   `v2_session_map`.
//! - terminal_id: previously a synthetic `wake-<agent>-<uuid>` string;
//!   now it's the v2 `SessionId` (also a UUID). Renderer attach paths
//!   (`openHeartbeatTab` reads `active_terminal_id`) remain compatible.
//! - Deferred-save thread retired for the heartbeat case (already a
//!   no-op there — pinned UUID makes the synchronous save authoritative).
//!   Non-heartbeat scheduler-tick fires (where `heartbeat_name` is None)
//!   still run a deferred save for compatibility with the legacy
//!   "save the chat-tab session id" semantic.

use std::sync::Arc;

use k2so_core::log_debug;

use crate::spawn::{spawn_agent_session_v2_blocking, SpawnWorkspaceSessionRequest};

/// Daemon-side headless wake spawn. Returns the v2 session id as a
/// String (the renderer's `openHeartbeatTab` flow + the daemon's
/// `find_live_for_resume` both work against this id).
///
/// `heartbeat_name` distinguishes heartbeat fires (for which we
/// stamp `agent_heartbeats.last_session_id` + `active_terminal_id`
/// synchronously) from one-off chat-tab wakes (for which we run the
/// legacy deferred-save poll to capture whatever session id claude
/// happens to pick).
pub fn spawn_wake_headless(
    agent_name: &str,
    project_path: &str,
    wake_prompt: &str,
    heartbeat_name: Option<&str>,
) -> Result<String, String> {
    if std::env::var("K2SO_TRACE_WAKE_SPAWN")
        .map(|v| v == "1")
        .unwrap_or(false)
    {
        let bt = std::backtrace::Backtrace::force_capture();
        eprintln!(
            "[wake-spawn-trace] spawn_wake_headless agent={agent_name:?} \
             project={project_path:?} heartbeat_name={heartbeat_name:?} \
             prompt_len={}\n{bt}",
            wake_prompt.len()
        );
    }

    // Pre-allocate claude's session UUID. Pinning via `--session-id`
    // means two concurrent fires on the same agent get distinct,
    // deterministic UUIDs — no race window between spawn and the
    // deferred-save thread guessing wrong.
    let pinned_session_id = uuid::Uuid::new_v4().to_string();

    // `--print` makes claude one-shot: read prompt, respond, persist
    // JSONL, exit. The v2 child-exit observer cleans up
    // `v2_session_map` on exit. Long-lived heartbeat PTYs are
    // anti-pattern (they pollute `find_live_for_resume`'s pick of
    // "which PTY does the user want to attach to").
    let args = vec![
        "--dangerously-skip-permissions".to_string(),
        "--print".to_string(),
        "--session-id".to_string(),
        pinned_session_id.clone(),
        wake_prompt.to_string(),
    ];

    // Test-only override. Integration tests in
    // `crates/k2so-daemon/tests/heartbeat_fire_v2_integration.rs`
    // set this env var to a benign command (e.g. `cat`) so the
    // test can exercise the v2 spawn + post-spawn DB writes
    // without requiring `claude` on PATH or burning API calls.
    // Production never sets this — defaults to `claude` + the
    // args above.
    let (command, args) = match std::env::var("K2SO_WAKE_HEADLESS_TEST_COMMAND") {
        Ok(c) if !c.is_empty() => (c, Vec::<String>::new()),
        _ => ("claude".to_string(), args),
    };

    let project_id = {
        let db = k2so_core::db::shared();
        let conn = db.lock();
        k2so_core::agents::resolve_project_id(&conn, project_path)
    };

    let outcome = spawn_agent_session_v2_blocking(SpawnWorkspaceSessionRequest {
        agent_name: agent_name.to_string(),
        project_id: project_id.clone(),
        cwd: project_path.to_string(),
        command: Some("claude".to_string()),
        args: Some(args),
        cols: 120,
        rows: 38,
    })?;

    let terminal_id = outcome.session_id.to_string();

    log_debug!(
        "[daemon/wake] spawned v2 PTY for {} in {} (id={})",
        agent_name,
        project_path,
        terminal_id
    );

    // Mark the workspace_sessions row 'running' so the next scheduler
    // tick skips this agent. Best-effort — the PTY is alive and will
    // run regardless of the DB write.
    let _ = k2so_core::agents::session::k2so_agents_lock(
        project_path.to_string(),
        agent_name.to_string(),
        Some(terminal_id.clone()),
        Some("system".to_string()),
    );

    // Synchronous per-heartbeat session stamp. With --session-id
    // pinning, we know exactly what UUID claude will use — write to
    // workspace_heartbeats.last_session_id immediately, no race.
    // active_terminal_id stamps the FK pointer that the renderer's
    // openHeartbeatTab uses to find the running PTY.
    if let Some(hb_name) = heartbeat_name {
        let db = k2so_core::db::shared();
        let conn = db.lock();
        if let Some(pid) = project_id.as_deref() {
            let _ = k2so_core::db::schema::AgentHeartbeat::save_session_id(
                &conn, pid, hb_name, &pinned_session_id,
            );
            let _ = k2so_core::db::schema::AgentHeartbeat::save_active_terminal_id(
                &conn, pid, hb_name, &terminal_id,
            );
        }
        log_debug!(
            "[daemon/wake] pinned heartbeat '{}' session id: {} terminal: {}",
            hb_name,
            pinned_session_id,
            terminal_id
        );
    }

    // Emit a HookEvent so the frontend's listener can decide whether
    // to surface a tab. Gated by `projects.show_heartbeat_sessions`
    // on the renderer side — silent fires don't pop a tab unless
    // the user opted in. Same wire format the legacy spawn emitted
    // so existing subscribers don't need to branch.
    k2so_core::agent_hooks::emit(
        k2so_core::agent_hooks::HookEvent::CliTerminalSpawnBackground,
        serde_json::json!({
            "terminalId": &terminal_id,
            "command": "claude",
            "cwd": project_path,
            "heartbeatName": heartbeat_name,
            "projectPath": project_path,
            "agentName": agent_name,
        }),
    );

    // Deferred session-id save (non-heartbeat path only). For chat-tab
    // wakes (heartbeat_name = None), claude's session id wasn't pinned
    // synchronously — poll the chat history dir a few seconds later
    // and stamp `agent_sessions.session_id` for the next --resume.
    // For heartbeat fires the synchronous save above is authoritative.
    if heartbeat_name.is_none() {
        let agent_name_owned = agent_name.to_string();
        let project_path_owned = project_path.to_string();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(5));
            let detected = k2so_core::chat_history::detect_active_session(
                "claude",
                &project_path_owned,
            )
            .ok()
            .flatten();
            let Some(session_id) = detected else { return };
            if session_id.is_empty() {
                return;
            }
            match k2so_core::agents::session::k2so_agents_save_session_id(
                project_path_owned.clone(),
                agent_name_owned.clone(),
                session_id.clone(),
            ) {
                Ok(_) => log_debug!(
                    "[daemon/wake] saved session id for {}: {}",
                    agent_name_owned,
                    session_id
                ),
                Err(e) => log_debug!(
                    "[daemon/wake] save session id for {} failed: {}",
                    agent_name_owned,
                    e
                ),
            }
        });
    }

    // The Arc dropping silently retires the unused outcome metadata.
    let _ = Arc::new(outcome);

    Ok(terminal_id)
}

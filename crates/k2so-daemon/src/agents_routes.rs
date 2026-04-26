//! H5 of Phase 4 — daemon-side `/cli/agents/launch` +
//! `/cli/agents/delegate`.
//!
//! Both endpoints used to live in Tauri's `agent_hooks.rs` and
//! called Tauri's `spawn_wake_pty` (which owns a
//! `TerminalManager::create` under the legacy alacritty path). H5
//! replaces the spawn side with daemon-owned Session Stream —
//! `spawn::spawn_agent_session` — so the new session shows up in
//! `session_map` and is reachable by every route that already
//! knows how to find daemon sessions (H1-H4).
//!
//! The heavy lifting (decision tree for launch, worktree + task
//! CLAUDE.md for delegate) is already in k2so-core:
//! - `k2so_core::agents::build_launch::k2so_agents_build_launch`
//!   walks the three wake branches (resume active / delegate from
//!   inbox / fresh launch) and returns the launch JSON.
//! - `k2so_core::agents::delegate::k2so_agents_delegate` creates
//!   the worktree + moves the inbox item + writes CLAUDE.md and
//!   returns the launch JSON.
//!
//! Each handler:
//!   1. Calls the core entry point to build the launch JSON.
//!   2. Parses `cwd`, `command`, `args` out of that JSON.
//!   3. Hands them to `spawn::spawn_agent_session` so the PTY is
//!      daemon-owned from the start (no Tauri `TerminalManager`).
//!   4. Emits the same HookEvent the Tauri path emitted so
//!      attached UIs see the same wire format.
//!   5. Returns JSON whose shape matches the legacy endpoints.

use std::collections::HashMap;

use k2so_core::agent_hooks::{emit, HookEvent};

use crate::cli_response::CliResponse;
use crate::spawn::{spawn_agent_session_v2_blocking, SpawnAgentSessionRequest};

/// H6: spawn a wake PTY via the Session Stream pipeline (same
/// shape as `k2so_core::agents::wake::spawn_wake_headless` but
/// daemon-owned — the resulting session lands in `session_map`
/// and is reachable by every /cli/* route that looks up by agent
/// name). Caller decides which backend to use based on the
/// project's `use_session_stream` setting.
///
/// Mirrors the side-effects of the legacy helper:
///   1. spawn_agent_session (PTY + dual-emit reader + archive).
///   2. Lock the agent in `agent_sessions` so scheduler skips it
///      on the next tick.
///   3. Emit `CliTerminalSpawnBackground` so any attached UI sees
///      the new session.
///
/// Returns the session id (as a String) on success.
// `heartbeat_name`: when Some, the wake is on behalf of a specific
// scheduled heartbeat. Per-heartbeat session save is currently
// handled by the v2 session-stream itself (the saved session_id is
// the v2 session UUID, not Claude's resume id), so this parameter
// is reserved for symmetry with `spawn_wake_headless` and a future
// hook that mirrors the per-heartbeat resume contract for v2 wakes.
pub fn spawn_wake_via_session_stream(
    agent_name: &str,
    project_path: &str,
    wake_prompt: &str,
    heartbeat_name: Option<&str>,
) -> Result<String, String> {
    // Pre-allocate Claude's session id (P6 fix). Without this, two
    // concurrent fires in the same project root attach to the same
    // claude session via implicit "continue most recent" behavior,
    // and both heartbeat rows end up stamped with the same id.
    // Pinning at spawn time gives each fire a deterministic, unique
    // session — see matching comment in `wake::spawn_wake_headless`.
    let pinned_session_id = uuid::Uuid::new_v4().to_string();

    // --print so claude delivers + exits (no lingering daemon PTY
    // that competes with the user's tab in find_live_for_resume).
    // See longer rationale in wake::spawn_wake_headless.
    let args = vec![
        "--dangerously-skip-permissions".to_string(),
        "--print".to_string(),
        "--session-id".to_string(),
        pinned_session_id.clone(),
        wake_prompt.to_string(),
    ];
    let outcome = spawn_agent_session_v2_blocking(SpawnAgentSessionRequest {
        agent_name: agent_name.to_string(),
        cwd: project_path.to_string(),
        command: Some("claude".to_string()),
        args: Some(args),
        cols: 120,
        rows: 38,
    })?;

    let _ = k2so_core::agents::session::k2so_agents_lock(
        project_path.to_string(),
        agent_name.to_string(),
        Some(outcome.session_id.to_string()),
        Some("system".to_string()),
    );

    // Synchronous per-heartbeat session stamp.
    if let Some(hb_name) = heartbeat_name {
        let db = k2so_core::db::shared();
        let conn = db.lock();
        if let Some(project_id) =
            k2so_core::agents::resolve_project_id(&conn, project_path)
        {
            let _ = k2so_core::db::schema::AgentHeartbeat::save_session_id(
                &conn, &project_id, hb_name, &pinned_session_id,
            );
        }
    }

    emit(
        HookEvent::CliTerminalSpawnBackground,
        serde_json::json!({
            "terminalId": outcome.session_id.to_string(),
            "command": "claude",
            "cwd": project_path,
            "projectPath": project_path,
            "agentName": agent_name,
            "heartbeatName": heartbeat_name,
        }),
    );

    Ok(outcome.session_id.to_string())
}

/// Extract a top-level string field from a launch-info JSON object,
/// falling back to `default` if the field is missing or not a string.
fn str_field<'a>(v: &'a serde_json::Value, key: &str, default: &'a str) -> &'a str {
    v.get(key).and_then(|x| x.as_str()).unwrap_or(default)
}

/// Extract a top-level string-array field, turning each element into
/// an owned String. Returns an empty Vec if the field is absent or
/// not an array.
fn str_array(v: &serde_json::Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(|x| x.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|s| s.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// Handler for `GET /cli/agents/launch?project=<path>&agent=<name>[&command=<cmd>]`.
///
/// Walks the three wake branches in core
/// (`k2so_agents_build_launch`) and spawns the resolved command in
/// the resolved `cwd` as a Session Stream session tagged with the
/// agent name. Emits `CliTerminalSpawnBackground` — matches the
/// legacy Tauri path so UI subscribers render the pane the same
/// way.
pub fn handle_agents_launch(
    params: &HashMap<String, String>,
    project_path: &str,
) -> CliResponse {
    let agent = params.get("agent").cloned().unwrap_or_default();
    if agent.is_empty() {
        return CliResponse::bad_request("missing agent param");
    }
    let cli_command = params.get("command").cloned().filter(|s| !s.is_empty());

    let launch_info = match k2so_core::agents::build_launch::k2so_agents_build_launch(
        project_path.to_string(),
        agent.clone(),
        cli_command,
        None,
        None,
        None, // /cli/agents/launch is a manual launch — use the per-agent global session
    ) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("build_launch failed: {e}")),
    };

    let command = str_field(&launch_info, "command", "claude").to_string();
    let cwd = str_field(&launch_info, "cwd", project_path).to_string();
    let args = str_array(&launch_info, "args");

    let outcome = match spawn_agent_session_v2_blocking(SpawnAgentSessionRequest {
        agent_name: agent.clone(),
        cwd: cwd.clone(),
        command: Some(command.clone()),
        args: if args.is_empty() { None } else { Some(args) },
        cols: 120,
        rows: 38,
    }) {
        Ok(o) => o,
        Err(e) => return CliResponse::bad_request(format!("spawn failed: {e}")),
    };

    // Mark the session `running` in `agent_sessions` so the
    // scheduler skips the agent on subsequent ticks. Best-effort —
    // the PTY is already live and will keep running if the DB
    // write fails.
    let _ = k2so_core::agents::session::k2so_agents_lock(
        project_path.to_string(),
        agent.clone(),
        Some(outcome.session_id.to_string()),
        Some("system".to_string()),
    );

    // Observational event for any UI on the /events WS. Shape
    // matches what src-tauri's spawn_wake_pty emits today so the
    // frontend's listener doesn't need to branch on origin.
    emit(
        HookEvent::CliTerminalSpawnBackground,
        serde_json::json!({
            "terminalId": outcome.session_id.to_string(),
            "command": command,
            "cwd": cwd,
            "projectPath": project_path,
            "agentName": &agent,
        }),
    );

    CliResponse::ok_json(
        serde_json::json!({
            "success": true,
            "terminalId": outcome.session_id.to_string(),
            "agentName": agent,
            "pendingDrained": outcome.pending_drained,
            "note": "Agent session launched by daemon",
        })
        .to_string(),
    )
}

/// Handler for `GET /cli/agents/delegate?project=<path>&target=<agent>&file=<path>`.
///
/// Creates a fresh worktree + writes the task CLAUDE.md (via
/// `agents::delegate::k2so_agents_delegate`), then spawns `claude`
/// in the worktree as a Session Stream session tagged with the
/// target agent's name. Emits `CliTerminalSpawn` +
/// `SyncProjects` — the first opens a UI pane for the new
/// session; the second tells the sidebar a new worktree appeared.
pub fn handle_agents_delegate(
    params: &HashMap<String, String>,
    project_path: &str,
) -> CliResponse {
    let target = params.get("target").cloned().unwrap_or_default();
    let file = params.get("file").cloned().unwrap_or_default();
    if target.is_empty() {
        return CliResponse::bad_request("missing target param");
    }
    if file.is_empty() {
        return CliResponse::bad_request("missing file param");
    }

    let launch_info = match k2so_core::agents::delegate::k2so_agents_delegate(
        project_path.to_string(),
        target.clone(),
        file.clone(),
    ) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("delegate failed: {e}")),
    };

    let command = str_field(&launch_info, "command", "claude").to_string();
    let cwd = str_field(&launch_info, "cwd", project_path).to_string();
    let agent_name = str_field(&launch_info, "agentName", &target).to_string();
    let args = str_array(&launch_info, "args");

    let outcome = match spawn_agent_session_v2_blocking(SpawnAgentSessionRequest {
        agent_name: agent_name.clone(),
        cwd: cwd.clone(),
        command: Some(command.clone()),
        args: if args.is_empty() { None } else { Some(args) },
        cols: 120,
        rows: 38,
    }) {
        Ok(o) => o,
        Err(e) => return CliResponse::bad_request(format!("spawn failed: {e}")),
    };

    let _ = k2so_core::agents::session::k2so_agents_lock(
        project_path.to_string(),
        agent_name.clone(),
        Some(outcome.session_id.to_string()),
        Some("delegated".to_string()),
    );

    emit(
        HookEvent::CliTerminalSpawn,
        serde_json::json!({
            "terminalId": outcome.session_id.to_string(),
            "agentName": &agent_name,
            "command": command,
            "cwd": cwd,
            "projectPath": project_path,
        }),
    );
    // Tell the sidebar a new worktree was registered (delegate
    // adds a row to the `workspaces` table).
    emit(HookEvent::SyncProjects, serde_json::Value::Null);

    // Echo back every field the legacy endpoint returned so CLI
    // clients that read `branch`, `worktreePath`, `taskFile` etc.
    // keep working. Daemon-specific additions (`terminalId`,
    // `pendingDrained`) are inserted alongside.
    let mut out = launch_info.clone();
    if let Some(obj) = out.as_object_mut() {
        obj.insert(
            "terminalId".into(),
            serde_json::Value::String(outcome.session_id.to_string()),
        );
        obj.insert(
            "pendingDrained".into(),
            serde_json::Value::Number(outcome.pending_drained.into()),
        );
        obj.insert("success".into(), serde_json::Value::Bool(true));
    }
    CliResponse::ok_json(serde_json::to_string(&out).unwrap_or_else(|_| "{}".into()))
}

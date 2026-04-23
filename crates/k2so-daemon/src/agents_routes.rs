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
use crate::spawn::{spawn_agent_session_blocking, SpawnAgentSessionRequest};

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
pub fn spawn_wake_via_session_stream(
    agent_name: &str,
    project_path: &str,
    wake_prompt: &str,
) -> Result<String, String> {
    let args = vec![
        "--dangerously-skip-permissions".to_string(),
        "--append-system-prompt".to_string(),
        wake_prompt.to_string(),
    ];
    let outcome = spawn_agent_session_blocking(SpawnAgentSessionRequest {
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

    emit(
        HookEvent::CliTerminalSpawnBackground,
        serde_json::json!({
            "terminalId": outcome.session_id.to_string(),
            "command": "claude",
            "cwd": project_path,
            "projectPath": project_path,
            "agentName": agent_name,
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
    ) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("build_launch failed: {e}")),
    };

    let command = str_field(&launch_info, "command", "claude").to_string();
    let cwd = str_field(&launch_info, "cwd", project_path).to_string();
    let args = str_array(&launch_info, "args");

    let outcome = match spawn_agent_session_blocking(SpawnAgentSessionRequest {
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

    let outcome = match spawn_agent_session_blocking(SpawnAgentSessionRequest {
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

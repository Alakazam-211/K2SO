//! Workspace-targeted messaging — `k2so msg <workspace> "text" [--wake]`.
//!
//! Generalizes the heartbeat smart-launch cascade to the workspace's
//! pinned chat tab. Same three-branch decision tree as
//! `heartbeat_launch::smart_launch`, but keyed on `workspace_sessions`
//! columns instead of `workspace_heartbeats`:
//!
//!   1. `active_terminal_id` is set + alive in v2_session_map → inject
//!      the message body into the live PTY.
//!   2. `active_terminal_id` null/dead but `session_id` (claude UUID)
//!      saved → spawn an interactive `claude --resume <session_id>`,
//!      stamp both columns via the v2_spawn auto-stamp hook, deliver
//!      the body via two-phase PTY write.
//!   3. Neither → spawn an interactive `claude --session-id <new_uuid>`,
//!      stamp both columns synchronously, deliver the body via
//!      two-phase PTY write.
//!
//! The non-`--wake` (default inbox) path delegates to
//! `workspace_inbox_create` so the message becomes a regular inbox
//! file the workspace agent picks up on next heartbeat / triage.

use std::path::Path;

use k2so_core::agents::resolve_project_id;
use k2so_core::db::schema::WorkspaceSession;
use k2so_core::log_debug;
use k2so_core::session::SessionId;

use crate::session_lookup;
use crate::spawn::{spawn_agent_session_v2_blocking, SpawnWorkspaceSessionRequest};

/// Resolve a workspace token (name, absolute path, or UUID) to its
/// canonical filesystem path. Returns `None` when no `projects` row
/// matches.
pub fn resolve_workspace(token: &str) -> Option<String> {
    if token.is_empty() {
        return None;
    }
    let db = k2so_core::db::shared();
    let conn = db.lock();

    // Absolute path (cheapest case — user passes the cwd).
    if token.starts_with('/') {
        return conn
            .query_row(
                "SELECT path FROM projects WHERE path = ?1",
                rusqlite::params![token],
                |r| r.get::<_, String>(0),
            )
            .ok();
    }

    // UUID lookup. `projects.id` is a v4 UUID; cheap to detect by
    // length + dashes without pulling in the uuid crate for parsing.
    if token.len() == 36 && token.chars().filter(|c| *c == '-').count() == 4 {
        if let Ok(path) = conn.query_row(
            "SELECT path FROM projects WHERE id = ?1",
            rusqlite::params![token],
            |r| r.get::<_, String>(0),
        ) {
            return Some(path);
        }
    }

    // Name match. Workspace names are short and usually unique within
    // the user's set; if multiple workspaces share a name we return
    // the first by insertion order (most users won't hit this).
    conn.query_row(
        "SELECT path FROM projects WHERE name = ?1 ORDER BY rowid LIMIT 1",
        rusqlite::params![token],
        |r| r.get::<_, String>(0),
    )
    .ok()
}

/// Deliver `text` to the workspace's inbox as a regular work item.
/// Becomes a markdown file at `<project>/.k2so/inbox/<id>-<title>.md`.
pub fn deliver_to_inbox(
    project_path: &str,
    text: &str,
    sender: &str,
) -> serde_json::Value {
    // Title = first line of the body (truncated). Body is the full text.
    let title = text
        .lines()
        .next()
        .unwrap_or("(empty)")
        .chars()
        .take(80)
        .collect::<String>();
    let result = k2so_core::agents::commands::workspace_inbox_create(
        project_path.to_string(),
        title,
        text.to_string(),
        Some("normal".to_string()),
        Some("message".to_string()),
        Some(sender.to_string()),
        Some("k2so msg".to_string()),
    );
    match result {
        Ok(v) => serde_json::json!({
            "success": true,
            "delivery": "inbox",
            "result": v,
        }),
        Err(e) => serde_json::json!({
            "success": false,
            "delivery": "inbox",
            "error": e.to_string(),
        }),
    }
}

/// Smart-cascade live delivery. Mirrors heartbeat smart_launch.
pub fn deliver_live(project_path: &str, text: &str) -> serde_json::Value {
    let project_id = {
        let db = k2so_core::db::shared();
        let conn = db.lock();
        match resolve_project_id(&conn, project_path) {
            Some(p) => p,
            None => {
                return serde_json::json!({
                    "success": false,
                    "error": format!("project not registered: {project_path}"),
                });
            }
        }
    };

    let row = {
        let db = k2so_core::db::shared();
        let conn = db.lock();
        WorkspaceSession::get(&conn, &project_id).ok().flatten()
    };

    let saved_session = row
        .as_ref()
        .and_then(|r| r.session_id.clone())
        .filter(|s| !s.is_empty());
    let saved_terminal = row
        .as_ref()
        .and_then(|r| r.active_terminal_id.clone())
        .filter(|s| !s.is_empty());

    // Branch 1: active_terminal_id alive → inject.
    if let Some(active_tid) = saved_terminal.as_deref() {
        if let Some(sid) = SessionId::parse(active_tid) {
            if let Some(live) = session_lookup::lookup_by_session_id(&sid) {
                return inject_live(&live, text, "active_terminal_id", &project_id);
            }
        }
        // Stale stamp — clear so callers downstream don't trip on it.
        let db = k2so_core::db::shared();
        let conn = db.lock();
        let _ = WorkspaceSession::clear_active_terminal_id(&conn, &project_id);
    }

    // Branch 1b: argv-scan fallback for the cold-start case (any live
    // PTY running --resume <saved_session>).
    if let Some(claude_sid) = saved_session.as_deref() {
        for (_n, live) in session_lookup::snapshot_all() {
            let args = live.args();
            let mut i = 0;
            let mut found = false;
            while i + 1 < args.len() {
                if (args[i] == "--session-id" || args[i] == "--resume")
                    && args[i + 1] == claude_sid
                {
                    found = true;
                    break;
                }
                i += 1;
            }
            if found {
                return inject_live(&live, text, "argv_scan", &project_id);
            }
        }
    }

    // Branch 2: saved session, no live PTY → resume + fire.
    if let Some(claude_sid) = saved_session.as_deref() {
        return resume_and_fire(project_path, &project_id, claude_sid, text);
    }

    // Branch 3: fresh fire — no saved session at all.
    fresh_fire(project_path, &project_id, text)
}

fn inject_live(
    live: &session_lookup::LiveSession,
    text: &str,
    branch: &str,
    project_id: &str,
) -> serde_json::Value {
    if let Err(e) = live.write(text.as_bytes()) {
        return serde_json::json!({
            "success": false,
            "error": format!("write to live PTY failed: {e}"),
        });
    }
    std::thread::sleep(std::time::Duration::from_millis(150));
    let _ = live.write(b"\r");
    let target_id = live.session_id().to_string();

    // Re-stamp active_terminal_id so subsequent calls fast-path through
    // Branch 1 directly. Idempotent if it already pointed here.
    {
        let db = k2so_core::db::shared();
        let conn = db.lock();
        let _ = WorkspaceSession::save_active_terminal_id(&conn, project_id, &target_id);
    }

    serde_json::json!({
        "success": true,
        "delivery": "live",
        "branch": branch,
        "targetSessionId": target_id,
    })
}

fn resume_and_fire(
    project_path: &str,
    project_id: &str,
    claude_sid: &str,
    text: &str,
) -> serde_json::Value {
    // Resolve the workspace's primary agent name. Spawned under the
    // canonical `<project_id>:<agent>` key so the existing v2_spawn
    // auto-stamp hook will populate workspace_sessions.active_terminal_id.
    let agent_name = match k2so_core::agents::find_primary_agent(project_path) {
        Some(n) => n,
        None => {
            return serde_json::json!({
                "success": false,
                "error": "no primary agent in workspace",
            });
        }
    };
    let args = vec![
        "--dangerously-skip-permissions".to_string(),
        "--resume".to_string(),
        claude_sid.to_string(),
    ];
    spawn_and_inject(project_path, project_id, &agent_name, args, text, "resume_and_fire")
}

fn fresh_fire(project_path: &str, project_id: &str, text: &str) -> serde_json::Value {
    let agent_name = match k2so_core::agents::find_primary_agent(project_path) {
        Some(n) => n,
        None => {
            return serde_json::json!({
                "success": false,
                "error": "no primary agent in workspace",
            });
        }
    };
    let new_sid = uuid::Uuid::new_v4().to_string();
    let args = vec![
        "--dangerously-skip-permissions".to_string(),
        "--session-id".to_string(),
        new_sid.clone(),
    ];

    // Stamp session_id synchronously — pinning means the deferred-save
    // race that the legacy detect-session-id polling tries to win is
    // already won. Without this, fresh_fire would leave session_id
    // unset until the renderer's polling caught it ~5s later.
    {
        let db = k2so_core::db::shared();
        let conn = db.lock();
        let _ = WorkspaceSession::update_session_id(&conn, project_id, &new_sid);
    }

    spawn_and_inject(project_path, project_id, &agent_name, args, text, "fresh_fire")
}

fn spawn_and_inject(
    project_path: &str,
    project_id: &str,
    agent_name: &str,
    args: Vec<String>,
    text: &str,
    branch: &str,
) -> serde_json::Value {
    let outcome = match spawn_agent_session_v2_blocking(SpawnWorkspaceSessionRequest {
        agent_name: agent_name.to_string(),
        project_id: Some(project_id.to_string()),
        cwd: project_path.to_string(),
        command: Some("claude".to_string()),
        args: Some(args),
        cols: 120,
        rows: 38,
    }) {
        Ok(o) => o,
        Err(e) => {
            return serde_json::json!({
                "success": false,
                "error": format!("spawn failed: {e}"),
            });
        }
    };
    let target_id = outcome.session_id.to_string();

    // Stamp `active_terminal_id` synchronously. The auto-stamp hook
    // in `handle_v2_spawn` doesn't fire for spawns that go through
    // `spawn_agent_session_v2_blocking` (the in-process helper), so
    // stamp here directly. Mirror of the heartbeat synchronous stamp
    // in `wake_headless`.
    {
        let db = k2so_core::db::shared();
        let conn = db.lock();
        let _ = WorkspaceSession::save_active_terminal_id(&conn, project_id, &target_id);
    }

    log_debug!(
        "[daemon/workspace-msg] {} session={} agent={}",
        branch,
        target_id,
        agent_name
    );

    // Two-phase write. Wait for claude TUI to draw before sending input.
    let session = session_lookup::lookup_by_session_id(&outcome.session_id);
    if let Some(live) = session {
        let payload = text.to_string();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(1500));
            let _ = live.write(payload.as_bytes());
            std::thread::sleep(std::time::Duration::from_millis(150));
            let _ = live.write(b"\r");
        });
    } else {
        log_debug!(
            "[daemon/workspace-msg] post-spawn lookup miss for session={} — body not delivered",
            target_id
        );
    }

    let _ = Path::new(project_path);
    serde_json::json!({
        "success": true,
        "delivery": "live",
        "branch": branch,
        "targetSessionId": target_id,
        "agent": agent_name,
    })
}

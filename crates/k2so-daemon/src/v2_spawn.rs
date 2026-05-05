//! HTTP handlers for Alacritty_v2 session spawn + close.
//!
//! Endpoints (registered in main.rs):
//!   - `POST /cli/sessions/v2/spawn` — find-or-spawn by agent_name.
//!   - `POST /cli/sessions/v2/close` — explicit session teardown.
//!
//! Parallel to `awareness_ws::handle_sessions_spawn` /
//! `handle_sessions_close` which handle v1 / Kessel-T0's
//! `SessionStreamSession`. Kept separate so the two renderer paths
//! don't step on each other; v1's handlers stay untouched during
//! the v2 transition.
//!
//! Find-or-spawn semantics: the client (Tauri) calls this on every
//! tab mount. If a session already exists for the requested
//! `agent_name` (`tab-<terminalId>`), we return its existing
//! `{sessionId, cols, rows}` with `reused: true`. Otherwise we
//! spawn a fresh `DaemonPtySession` and register it. Tauri always
//! calls the same endpoint whether it's a cold launch or a reattach
//! after workspace swap / app quit. See `.k2so/prds/alacritty-v2.md`
//! phase A4.

use std::collections::HashMap;
use std::path::PathBuf;

use k2so_core::log_debug;
use k2so_core::session::SessionId;
use k2so_core::terminal::{DaemonPtyConfig, DaemonPtySession};

use crate::awareness_ws::HandlerResult;
use crate::pending_live;
use crate::signal_format;
use crate::v2_session_map;

/// Handler for `POST /cli/sessions/v2/spawn`.
///
/// Request body (JSON):
/// ```json
/// {
///   "agent_name": "tab-<terminalId>",
///   "cwd": "/optional/path",
///   "command": "optional program",
///   "args": ["optional", "args"],
///   "cols": 120,
///   "rows": 40,
///   "env": { "KEY": "val" }
/// }
/// ```
///
/// Response body (JSON):
/// ```json
/// {
///   "sessionId": "<uuid>",
///   "agentName": "tab-<terminalId>",
///   "cols": 120,
///   "rows": 40,
///   "reused": false
/// }
/// ```
///
/// `reused: true` means the caller's `agent_name` already had a
/// live session; we returned its handle instead of spawning.
/// Tauri's attach path treats reused and fresh identically.
pub fn handle_v2_spawn(body: &[u8]) -> HandlerResult {
    #[derive(serde::Deserialize)]
    struct SpawnRequest {
        agent_name: String,
        #[serde(default = "default_cwd")]
        cwd: String,
        #[serde(default)]
        command: Option<String>,
        #[serde(default)]
        args: Option<Vec<String>>,
        #[serde(default = "default_cols")]
        cols: u16,
        #[serde(default = "default_rows")]
        rows: u16,
        #[serde(default)]
        env: Option<HashMap<String, String>>,
    }
    fn default_cwd() -> String {
        std::env::var("HOME").unwrap_or_else(|_| "/tmp".into())
    }
    fn default_cols() -> u16 {
        80
    }
    fn default_rows() -> u16 {
        24
    }

    let __t_total = std::time::Instant::now();

    let req: SpawnRequest = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(e) => {
            return HandlerResult {
                status: "400 Bad Request",
                body: format!(
                    r#"{{"error":"parse v2 SpawnRequest: {}"}}"#,
                    e.to_string().replace('"', "'")
                ),
            }
        }
    };
    if req.agent_name.is_empty() {
        return HandlerResult {
            status: "400 Bad Request",
            body: r#"{"error":"agent_name required"}"#.into(),
        };
    }

    // Find-or-spawn: existing session wins. The response preserves
    // whatever cols/rows the existing session was opened at — the
    // caller will ResizeObserver-correct if its viewport differs.
    let __t_lookup = std::time::Instant::now();
    let existing = v2_session_map::lookup_by_agent_name(&req.agent_name);
    let lookup_ms = __t_lookup.elapsed().as_secs_f64() * 1000.0;

    if let Some(existing) = existing {
        let (cols, rows) = current_dims(&existing);
        let session_id_str = existing.session_id.to_string();
        let total_ms = __t_total.elapsed().as_secs_f64() * 1000.0;
        log_debug!(
            "[v2-perf] side=daemon SPAWN_SUMMARY session={} agent={} reused=true total_ms={:.3} lookup_ms={:.3} dpty_spawn_ms=0",
            session_id_str, req.agent_name, total_ms, lookup_ms
        );
        let out = serde_json::json!({
            "sessionId": session_id_str,
            "agentName": req.agent_name,
            "cols": cols,
            "rows": rows,
            "reused": true,
        });
        return HandlerResult {
            status: "200 OK",
            body: out.to_string(),
        };
    }

    // Spawn a fresh session.
    let cfg = DaemonPtyConfig {
        session_id: SessionId::new(),
        cols: req.cols,
        rows: req.rows,
        cwd: Some(PathBuf::from(&req.cwd)),
        program: req.command.clone(),
        args: req.args.unwrap_or_default(),
        env: req.env.unwrap_or_default(),
        drain_on_exit: true,
    };
    let session_id_for_response = cfg.session_id;

    let __t_spawn = std::time::Instant::now();
    let session = match DaemonPtySession::spawn(cfg) {
        Ok(s) => s,
        Err(e) => {
            return HandlerResult {
                status: "500 Internal Server Error",
                body: format!(
                    r#"{{"error":"v2 spawn failed: {}"}}"#,
                    e.to_string().replace('"', "'")
                ),
            }
        }
    };
    let dpty_spawn_ms = __t_spawn.elapsed().as_secs_f64() * 1000.0;

    v2_session_map::register(req.agent_name.clone(), session.clone());

    // Stamp the agent_sessions row's `active_terminal_id` (migration
    // 0037). Best-effort: tab-keyed spawns (`tab-<id>`) won't have
    // a matching workspace_sessions row and the UPDATE no-ops;
    // workspace agent spawns do, and the column lets the next chat
    // tab mount re-attach without walking the in-memory
    // v2_session_map. Mirror of the heartbeat smart-launch stamp
    // (`heartbeat_launch.rs`) and the
    // `agent_heartbeats.active_terminal_id` cleanup hook in
    // `v2_session_map::unregister`.
    //
    // 0.37.0: the prefix split from 0.36.14 is gone — the
    // `workspace_sessions` row is keyed purely on `project_id`. We
    // accept either the prefixed `<pid>:<bare>` form (preferred —
    // cheaper, unambiguous) or fall back to resolving from cwd.
    {
        let db = k2so_core::db::shared();
        let conn = db.lock();
        let project_id_opt = if let Some((pid, _bare)) = req.agent_name.split_once(':') {
            if pid.is_empty() {
                k2so_core::agents::resolve_project_id(&conn, &req.cwd)
            } else {
                Some(pid.to_string())
            }
        } else {
            k2so_core::agents::resolve_project_id(&conn, &req.cwd)
        };
        if let Some(project_id) = project_id_opt {
            let _ = k2so_core::db::schema::WorkspaceSession::save_active_terminal_id(
                &conn,
                &project_id,
                &session.session_id.to_string(),
            );
        }
    }

    // Child-exit observer: subscribe to the session's alacritty event
    // broadcast and call v2_session_map::unregister when ChildExit
    // arrives. The unregister hook (in v2_session_map) is what nulls
    // any matching agent_heartbeats.active_terminal_id and flips
    // surfaced=0 on the agent_sessions row. Without this, claude
    // --print sessions exit cleanly and leave the column pointing at
    // a corpse — which the lazy cleanup on read would catch
    // eventually, but eventually-consistent stale data is the kind
    // of "feels haunted" UX we'd rather avoid. See
    // `heartbeat-active-session-tracking` PRD.
    spawn_child_exit_observer(req.agent_name.clone(), session.clone());

    // Drain any pending-live signals that were queued while this
    // agent was offline so they become input to the fresh session.
    // Mirrors `crate::spawn::spawn_agent_session`'s legacy drain so
    // wake-queued signals to v2 agents aren't silently lost on boot.
    //
    // Two-phase write per signal — body, settle, `\r` — same pattern
    // `DaemonInjectProvider::inject` and `heartbeat_launch::run_inject`
    // use. A single combined write would be treated as a multi-line
    // paste by the TUI input widget and the queued message would land
    // typed-but-not-sent.
    //
    // 0.37.0: drain under the spawn's key only. The 0.36.14 dual-key
    // (prefixed + bare-name fallback) drain is retired now that every
    // awareness-bus enqueue carries workspace context via signal.to;
    // both ends of the queue use the same `<project_id>:<bare>` key.
    let pending = pending_live::drain_for_agent(&req.agent_name);
    let pending_drained = pending.len();
    for signal in pending {
        let bytes = signal_format::inject_bytes(&signal);
        session.write(bytes.into_bytes());
        std::thread::sleep(std::time::Duration::from_millis(150));
        session.write(b"\r".to_vec());
    }

    let total_ms = __t_total.elapsed().as_secs_f64() * 1000.0;
    log_debug!(
        "[v2-perf] side=daemon SPAWN_SUMMARY session={} agent={} reused=false total_ms={:.3} lookup_ms={:.3} dpty_spawn_ms={:.3} pending_drained={}",
        session_id_for_response,
        req.agent_name,
        total_ms,
        lookup_ms,
        dpty_spawn_ms,
        pending_drained
    );

    let out = serde_json::json!({
        "sessionId": session_id_for_response.to_string(),
        "agentName": req.agent_name,
        "cols": req.cols,
        "rows": req.rows,
        "reused": false,
    });
    HandlerResult {
        status: "200 OK",
        body: out.to_string(),
    }
}

/// Handler for `POST /cli/sessions/v2/close`.
///
/// Request body: `{"agent_name": "tab-<terminalId>"}`.
/// Response: `{"closed": true|false}`.
///
/// Unregisters the session from `v2_session_map`. The last `Arc`
/// drop triggers `DaemonPtySession::drop`, which closes the PTY
/// master channel; alacritty's IO thread then exits, the child
/// receives SIGHUP, and the session is cleaned up.
///
/// Called only on deliberate tab removal (see A6 wiring in
/// `src/renderer/stores/tabs.ts::removeTab`). Component unmount
/// does NOT call this; the session survives workspace swap + Tauri
/// restart.
pub fn handle_v2_close(body: &[u8]) -> HandlerResult {
    #[derive(serde::Deserialize)]
    struct CloseRequest {
        agent_name: String,
    }

    let req: CloseRequest = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(e) => {
            return HandlerResult {
                status: "400 Bad Request",
                body: format!(
                    r#"{{"error":"parse v2 CloseRequest: {}"}}"#,
                    e.to_string().replace('"', "'")
                ),
            }
        }
    };

    let removed = v2_session_map::unregister(&req.agent_name).is_some();
    HandlerResult {
        status: "200 OK",
        body: serde_json::json!({ "closed": removed }).to_string(),
    }
}

/// Read the current `{cols, rows}` from a session's alacritty Term.
/// Used to populate the response for a reused session so the caller
/// knows the actual dimensions (which may differ from what they
/// requested if an earlier caller already sized the session).
fn current_dims(session: &DaemonPtySession) -> (u16, u16) {
    use k2so_core::terminal::Dimensions;
    let term_mutex = session.term();
    let term = term_mutex.lock();
    let cols = term.columns() as u16;
    let rows = term.screen_lines() as u16;
    (cols, rows)
}

/// Subscribe to a freshly-spawned session's alacritty events on a
/// detached tokio task and call `v2_session_map::unregister(agent)`
/// when ChildExit arrives. The unregister hook is what handles the
/// DB cleanup — see `v2_session_map::unregister`. Detached because
/// we don't have a JoinHandle to track and the task is short-lived
/// (only runs until the child dies, which terminates the underlying
/// broadcast channel and ends our `recv()` loop).
///
/// Holds a Weak reference to the session so the observer task
/// doesn't keep the Arc alive past the last legitimate holder. If
/// every other holder drops first, `Weak::upgrade()` returns None
/// and we exit silently.
fn spawn_child_exit_observer(agent_name: String, session: std::sync::Arc<DaemonPtySession>) {
    use k2so_core::terminal::AlacEvent;
    let weak = std::sync::Arc::downgrade(&session);
    drop(session);
    tokio::spawn(async move {
        // Re-acquire briefly to grab a receiver. If the session was
        // already dropped, exit — nothing to observe.
        let mut rx = match weak.upgrade() {
            Some(s) => s.subscribe_events(),
            None => return,
        };
        // Drop the temporary strong reference so we don't keep the
        // Arc alive ourselves; the receiver alone is enough.
        loop {
            match rx.recv().await {
                Ok(AlacEvent::ChildExit(status)) => {
                    log_debug!(
                        "[daemon/v2-exit] ChildExit observed for agent={} code={:?} — unregistering",
                        agent_name,
                        status.code(),
                    );
                    v2_session_map::unregister(&agent_name);
                    return;
                }
                Ok(_) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // Each test gets its own agent_name so parallel test runs
    // don't stomp on each other's map entries.
    static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
    fn uniq_agent_name() -> String {
        format!("test-v2-{}", NEXT_ID.fetch_add(1, Ordering::SeqCst))
    }

    #[test]
    fn spawn_request_rejects_empty_agent_name() {
        let body = br#"{"agent_name":""}"#;
        let result = handle_v2_spawn(body);
        assert_eq!(result.status, "400 Bad Request");
        assert!(result.body.contains("agent_name required"));
    }

    #[test]
    fn spawn_request_rejects_malformed_json() {
        let body = b"not json at all";
        let result = handle_v2_spawn(body);
        assert_eq!(result.status, "400 Bad Request");
        assert!(result.body.contains("parse v2 SpawnRequest"));
    }

    #[test]
    fn close_noop_returns_closed_false() {
        let agent = uniq_agent_name();
        let body =
            format!(r#"{{"agent_name":"{}"}}"#, agent).into_bytes();
        let result = handle_v2_close(&body);
        assert_eq!(result.status, "200 OK");
        assert!(result.body.contains(r#""closed":false"#));
    }

    // Full spawn-then-lookup + spawn-then-reuse tests live in
    // crates/k2so-daemon/tests/ where a running tokio runtime and
    // the ability to fork a shell are available. They gate A7's
    // parity work.
}

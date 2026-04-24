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

use k2so_core::session::SessionId;
use k2so_core::terminal::{DaemonPtyConfig, DaemonPtySession};

use crate::awareness_ws::HandlerResult;
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
    if let Some(existing) = v2_session_map::lookup_by_agent_name(&req.agent_name) {
        let (cols, rows) = current_dims(&existing);
        let out = serde_json::json!({
            "sessionId": existing.session_id.to_string(),
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

    v2_session_map::register(req.agent_name.clone(), session);

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

//! `/cli/awareness/publish` + `/cli/awareness/subscribe` — daemon-
//! side access to the Awareness Bus.
//!
//! E7 of Phase 3. Two routes:
//!
//!   - `POST /cli/awareness/publish` — JSON body is an
//!     `AgentSignal`. Handler passes it to `awareness::ingress::from_cli`
//!     which delegates to egress. Returns the `DeliveryReport` as JSON.
//!
//!   - `GET /cli/awareness/subscribe` — WS upgrade. Stream every
//!     `bus::publish()`-ed signal to connected clients. Mirror of
//!     the D4 `sessions_ws::serve_session_subscribe_connection`
//!     pattern.
//!
//! Wire format matches `sessions_ws` — each frame is a JSON text
//! message `{"event":"awareness:signal","payload":<AgentSignal>}`.
//! Consumers filter by `event` and destructure `payload`.

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio::sync::broadcast;
use tokio_tungstenite::tungstenite::Message;

use k2so_core::awareness::{self, AgentSignal};
use k2so_core::log_debug;

// ──────────────────────────────────────────────────────────────────────
// POST /cli/awareness/publish
// ──────────────────────────────────────────────────────────────────────

/// Handler for `POST /cli/sessions/spawn`. Called by
/// `main.rs::handle_connection` after token auth + body read.
/// Accepts a JSON body shaped like:
///
/// ```json
/// {
///   "agent_name": "bar",
///   "cwd": "/tmp",
///   "command": "cat",
///   "args": null,
///   "cols": 80,
///   "rows": 24
/// }
/// ```
///
/// Spawns a Session Stream session in the daemon process,
/// registers it in `session_map` keyed by `agent_name`, tags the
/// SessionEntry's agent_name so roster + liveness detection
/// works, and returns `{"sessionId": "<uuid>"}`.
///
/// F2 of Phase 3.1. Makes the daemon's InjectProvider actually
/// usable from external callers (CLI, Tauri, tests) — without a
/// spawn endpoint, session_map always stays empty in a real
/// daemon deployment.
pub fn handle_sessions_spawn(body: &[u8]) -> HandlerResult {
    use k2so_core::session::{registry, SessionId};
    use k2so_core::terminal::{spawn_session_stream, SpawnConfig};

    #[derive(serde::Deserialize)]
    struct SpawnRequest {
        agent_name: String,
        #[serde(default = "default_cwd")]
        cwd: String,
        command: Option<String>,
        args: Option<Vec<String>>,
        #[serde(default = "default_cols")]
        cols: u16,
        #[serde(default = "default_rows")]
        rows: u16,
    }
    fn default_cwd() -> String {
        "/tmp".into()
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
                    r#"{{"error":"parse SpawnRequest: {}"}}"#,
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

    let session_id = SessionId::new();
    let cfg = SpawnConfig {
        session_id,
        cwd: req.cwd,
        command: req.command,
        args: req.args,
        cols: req.cols,
        rows: req.rows,
    };

    let session = match spawn_session_stream(cfg) {
        Ok(s) => s,
        Err(e) => {
            return HandlerResult {
                status: "500 Internal Server Error",
                body: format!(
                    r#"{{"error":"spawn failed: {}"}}"#,
                    e.replace('"', "'")
                ),
            }
        }
    };

    // Tag the core's SessionEntry so liveness lookups (roster,
    // egress::is_agent_live) find this agent under its name.
    if let Some(entry) = registry::lookup(&session_id) {
        entry.set_agent_name(&req.agent_name);
    }

    // Register in the daemon's session_map so InjectProvider can
    // reach this session by agent name.
    let arc = std::sync::Arc::new(session);
    crate::session_map::register(&req.agent_name, arc.clone());

    // F3 drain: inject any pending-live signals that were queued
    // while this agent was offline. Signals are rendered the same
    // way live inject would render them, so the target's harness
    // sees identical bytes regardless of whether the sender had to
    // wait for the wake-and-inject path.
    let pending = crate::pending_live::drain_for_agent(&req.agent_name);
    let pending_count = pending.len();
    for signal in pending {
        let bytes = crate::signal_format::inject_bytes(&signal);
        if let Err(e) = arc.write(bytes.as_bytes()) {
            log_debug!(
                "[daemon/spawn] drain-inject for {} signal id={} failed: {e}",
                req.agent_name,
                signal.id
            );
        }
    }

    let out = serde_json::json!({
        "sessionId": session_id.to_string(),
        "agentName": req.agent_name,
        "pendingDrained": pending_count,
    });
    HandlerResult {
        status: "200 OK",
        body: out.to_string(),
    }
}

/// Handler for `POST /cli/awareness/publish`. Called by
/// `main.rs::handle_connection` after token auth + body read.
///
/// Returns a JSON body describing the response. Errors come back
/// as `{"error":"..."}` with status 400; success returns the
/// `DeliveryReport` fields.
pub fn handle_publish(body: &[u8]) -> HandlerResult {
    let signal: AgentSignal = match serde_json::from_slice(body) {
        Ok(s) => s,
        Err(e) => {
            return HandlerResult {
                status: "400 Bad Request",
                body: format!(
                    r#"{{"error":"parse AgentSignal: {}"}}"#,
                    e.to_string().replace('"', "'")
                ),
            }
        }
    };

    let report = awareness::from_cli(signal);

    let json_body = serde_json::json!({
        "injected_to_pty": report.injected_to_pty,
        "woke_offline_target": report.woke_offline_target,
        "inbox_path": report.inbox_path.map(|p| p.to_string_lossy().into_owned()),
        "activity_feed_row_id": report.activity_feed_row_id,
        "published_to_bus": report.published_to_bus,
    });

    HandlerResult {
        status: "200 OK",
        body: serde_json::to_string(&json_body).unwrap_or_else(|_| "{}".into()),
    }
}

pub struct HandlerResult {
    pub status: &'static str,
    pub body: String,
}

// ──────────────────────────────────────────────────────────────────────
// GET /cli/awareness/subscribe (WebSocket)
// ──────────────────────────────────────────────────────────────────────

/// WS handler. Upgrade the TCP stream, subscribe to the in-core
/// bus, and fan every signal out to the connected client as a JSON
/// text message. Exits when the bus closes (daemon shutdown) or
/// the client disconnects.
pub async fn serve_awareness_subscribe_connection(stream: TcpStream) {
    let ws = match tokio_tungstenite::accept_async(stream).await {
        Ok(ws) => ws,
        Err(e) => {
            log_debug!("[daemon/awareness_ws] handshake failed: {e}");
            return;
        }
    };
    let (mut write, mut read) = ws.split();
    let mut rx = awareness::subscribe();
    log_debug!(
        "[daemon/awareness_ws] subscriber connected (bus subscribers: {})",
        awareness::subscriber_count()
    );

    loop {
        tokio::select! {
            recv = rx.recv() => {
                match recv {
                    Ok(signal) => {
                        let msg = match serialize_signal_event(&signal) {
                            Ok(m) => m,
                            Err(e) => {
                                log_debug!(
                                    "[daemon/awareness_ws] serialize failed: {e}"
                                );
                                continue;
                            }
                        };
                        if let Err(e) = write.send(Message::Text(msg)).await {
                            log_debug!(
                                "[daemon/awareness_ws] send failed (client gone): {e}"
                            );
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        log_debug!(
                            "[daemon/awareness_ws] subscriber lagged {n} signals"
                        );
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        log_debug!(
                            "[daemon/awareness_ws] bus closed, disconnecting subscriber"
                        );
                        break;
                    }
                }
            }
            msg = read.next() => {
                match msg {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(Message::Ping(p))) => {
                        if let Err(e) = write.send(Message::Pong(p)).await {
                            log_debug!("[daemon/awareness_ws] pong failed: {e}");
                            break;
                        }
                    }
                    Some(Ok(_)) => {}
                    Some(Err(e)) => {
                        log_debug!("[daemon/awareness_ws] read error: {e}");
                        break;
                    }
                }
            }
        }
    }
}

fn serialize_signal_event(signal: &AgentSignal) -> Result<String, serde_json::Error> {
    serde_json::to_string(&serde_json::json!({
        "event": "awareness:signal",
        "payload": signal,
    }))
}

// ──────────────────────────────────────────────────────────────────────
// Unit tests — serialization shape
// ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use k2so_core::awareness::{AgentAddress, Delivery, SignalKind, WorkspaceId};

    fn minimal_signal() -> AgentSignal {
        AgentSignal::new(
            AgentAddress::Broadcast,
            AgentAddress::Broadcast,
            SignalKind::Status {
                text: "t".into(),
            },
        )
    }

    #[test]
    fn awareness_signal_event_shape() {
        let s = minimal_signal();
        let msg = serialize_signal_event(&s).unwrap();
        assert!(msg.contains(r#""event":"awareness:signal""#), "{msg}");
        assert!(msg.contains(r#""delivery":"live""#), "{msg}");
    }

    #[test]
    fn handle_publish_returns_delivery_report_fields() {
        // Test-util DB makes activity_feed insert safe under
        // k2so-daemon unit-test context (dev-dep enables
        // `test-util` on k2so-core).
        k2so_core::db::init_for_tests();
        let signal = minimal_signal();
        let body = serde_json::to_vec(&signal).unwrap();
        let result = handle_publish(&body);
        assert_eq!(result.status, "200 OK");
        assert!(
            result.body.contains("published_to_bus"),
            "body missing field: {}",
            result.body
        );
    }

    #[test]
    fn handle_publish_bad_json_returns_400() {
        let result = handle_publish(b"{this is not json");
        assert_eq!(result.status, "400 Bad Request");
        assert!(result.body.contains("parse AgentSignal"));
    }
}

//! Daemon -> UI event channel.
//!
//! The daemon is the authoritative emitter of agent-hook events once it
//! starts serving `/hook/*` routes in its own process (corner #3). The
//! Tauri UI needs to see those emissions even though they happen in a
//! different process, so this module hosts a WebSocket endpoint at
//! `GET /events?token=<t>` that any client can subscribe to for a live
//! feed of [`k2so_core::agent_hooks::HookEvent`] frames.
//!
//! Design:
//!
//! - A single `tokio::sync::broadcast` channel fans out events from the
//!   daemon's own `AgentHookEventSink` impl to every connected WS
//!   subscriber. Slow subscribers get `Lagged` errors; we log and keep
//!   going rather than disconnect — the UI reconnects cheaply anyway.
//! - Wire format is JSON `{"event":"agent:lifecycle","payload":{...}}`.
//!   The `event` string matches `HookEvent::event_name()` so the Tauri
//!   side can loop back through its existing `AppHandle::emit(...)`
//!   path without a second mapping table.
//! - Auth reuses the daemon's per-boot 32-hex token. The WS handshake
//!   requires `?token=<t>` on the URL; mismatched tokens get a 403
//!   before any data frames flow.

use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::sync::broadcast;
use tokio_tungstenite::tungstenite::Message;

use k2so_core::agent_hooks::{AgentHookEventSink, HookEvent};
use k2so_core::log_debug;

/// One frame sent over the event WS. Matches the wire format the Tauri
/// receiver's JSON deserializer expects (see `src-tauri/src/daemon_events.rs`).
#[derive(Clone, Debug, Serialize)]
pub struct WireEvent {
    pub event: &'static str,
    pub payload: serde_json::Value,
}

/// Broadcast channel the daemon publishes into. Subscribers (WS clients)
/// hold a matching `Receiver`. Capacity is tuned for bursty hook activity:
/// a single heartbeat can fire a handful of events in quick succession,
/// and a slow UI shouldn't drop them on the floor. Lagging receivers get
/// `RecvError::Lagged` rather than a disconnect; the receive loop logs
/// and resumes.
pub const EVENT_CHANNEL_CAP: usize = 256;

/// Host-side [`AgentHookEventSink`] impl that republishes every fired
/// event onto the daemon's broadcast channel. Cloned into the per-process
/// ambient slot at startup via `k2so_core::agent_hooks::set_sink`.
pub struct DaemonBroadcastSink {
    tx: broadcast::Sender<WireEvent>,
}

impl DaemonBroadcastSink {
    pub fn new(tx: broadcast::Sender<WireEvent>) -> Self {
        Self { tx }
    }
}

impl AgentHookEventSink for DaemonBroadcastSink {
    fn emit(&self, event: HookEvent, payload: serde_json::Value) {
        let frame = WireEvent {
            event: event.event_name(),
            payload,
        };
        // Silent on "no receivers" — the daemon fires events whether or
        // not a UI is listening, and that's the whole point.
        let _ = self.tx.send(frame);
    }
}

/// Serve a single accepted TCP connection as a `/events` WebSocket.
///
/// Preconditions:
/// - The HTTP upgrade headers are still buffered in the `TcpStream` and
///   have been checked for method/path by the caller.
/// - The token query parameter has been validated by the caller.
///
/// The upgrade itself happens here because tokio-tungstenite needs to
/// consume the handshake bytes directly. Returns when the client
/// disconnects, the broadcast channel drops, or a write fails.
pub async fn serve_events_connection(
    stream: TcpStream,
    tx: Arc<broadcast::Sender<WireEvent>>,
) {
    let ws = match tokio_tungstenite::accept_async(stream).await {
        Ok(ws) => ws,
        Err(e) => {
            log_debug!("[daemon/events] ws handshake failed: {e}");
            return;
        }
    };
    let (mut write, mut read) = ws.split();
    let mut rx = tx.subscribe();
    log_debug!(
        "[daemon/events] subscriber connected (total subscribers: {})",
        tx.receiver_count()
    );

    loop {
        tokio::select! {
            recv = rx.recv() => {
                match recv {
                    Ok(frame) => {
                        let json = match serde_json::to_string(&frame) {
                            Ok(s) => s,
                            Err(e) => {
                                log_debug!("[daemon/events] serialize error: {e}");
                                continue;
                            }
                        };
                        if let Err(e) = write.send(Message::Text(json)).await {
                            log_debug!("[daemon/events] send error (client gone): {e}");
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        log_debug!("[daemon/events] subscriber lagged {n} frames");
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        log_debug!("[daemon/events] broadcast closed, disconnecting subscriber");
                        break;
                    }
                }
            }
            msg = read.next() => {
                match msg {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(Message::Ping(p))) => {
                        // Client-side heartbeat; just echo.
                        if let Err(e) = write.send(Message::Pong(p)).await {
                            log_debug!("[daemon/events] pong write failed: {e}");
                            break;
                        }
                    }
                    Some(Ok(_)) => {
                        // Subscribers are read-only from the daemon's
                        // perspective — any inbound text/binary is
                        // ignored rather than errored to keep reconnects
                        // resilient to version skew.
                    }
                    Some(Err(e)) => {
                        log_debug!("[daemon/events] read error: {e}");
                        break;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_event_json_roundtrip_matches_expected_shape() {
        let tx = broadcast::Sender::<WireEvent>::new(4);
        let sink = DaemonBroadcastSink::new(tx.clone());
        let mut rx = tx.subscribe();
        sink.emit(
            HookEvent::AgentLifecycle,
            serde_json::json!({"paneId": "p1", "eventType": "start"}),
        );
        let frame = rx.try_recv().expect("broadcast delivered");
        assert_eq!(frame.event, "agent:lifecycle");
        let json = serde_json::to_string(&frame).expect("serialize");
        assert!(json.contains(r#""event":"agent:lifecycle""#), "{json}");
        assert!(json.contains(r#""paneId":"p1""#), "{json}");
    }

    #[test]
    fn emitting_without_subscribers_is_silent() {
        // Regression guard: if no UI is connected, a fired hook event
        // must not panic or stall the daemon.
        let tx = broadcast::Sender::<WireEvent>::new(4);
        let sink = DaemonBroadcastSink::new(tx);
        sink.emit(HookEvent::SyncProjects, serde_json::json!({}));
    }
}

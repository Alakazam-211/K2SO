//! `/cli/sessions/grid` WebSocket endpoint (Alacritty_v2).
//!
//! Serves grid snapshots + deltas from a daemon-hosted
//! `DaemonPtySession`'s `alacritty_terminal::Term` to a single
//! Tauri-side thin client. This is the daemon half of the A3 + A5
//! protocol defined in `.k2so/prds/alacritty-v2.md`.
//!
//! Flow:
//!
//!   1. Parse `?session=<UUID>&token=<token>` from query. 400 on
//!      malformed, 403 on auth fail (enforced by caller in main.rs).
//!   2. Look up the session in `v2_session_map`. 400 if not found.
//!   3. WebSocket handshake.
//!   4. Take ownership of the session's AlacEvent receiver via
//!      `session.take_events()`. If already taken (second subscriber),
//!      decline with a busy error — v2 is single-subscriber by design.
//!   5. Emit an initial full snapshot as `{"event":"snapshot","payload":...}`.
//!   6. Enter select loop:
//!        - On AlacEvent::Wakeup: call `build_emit()` under the Term
//!          lock, send the resulting `Snapshot` or `Delta` payload.
//!        - On AlacEvent::ChildExit: send final snapshot, close WS.
//!        - On inbound `{"action":"input","text":...}`: write to PTY.
//!        - On inbound `{"action":"resize","cols":N,"rows":N}`:
//!          SIGWINCH + Term.resize.
//!        - On client close: exit loop; session stays alive.
//!
//! **Binding the session Arc**: the Arc from `v2_session_map` stays
//! alive for the duration of this handler. On disconnect we drop
//! our clone; if another Arc is held (by the map or a future
//! subscriber), the session persists. Only the map's removal or
//! explicit close tears it down.
//!
//! Message format is JSON text (not binary) for both directions.
//! Bandwidth of a typical delta is small (damaged rows only); the
//! JSON framing is convenient and matches the protocol style of
//! `sessions_ws.rs` / the Awareness Bus.

use std::collections::HashMap;
use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;

use k2so_core::log_debug;
use k2so_core::session::SessionId;
use k2so_core::terminal::{
    build_emit, snapshot_term, AlacEvent, EmitDecision, EmitState,
    TermGridDelta, TermGridSnapshot,
};

use crate::v2_session_map;

/// Outbound WS message. Tagged as `{"event":"<kind>","payload":...}`.
#[derive(Debug, Serialize)]
#[serde(tag = "event", content = "payload", rename_all = "snake_case")]
enum Outbound<'a> {
    /// Full grid + scrollback. Sent once on connect; repeat only
    /// when `build_emit` returns `Full` (e.g. full damage or reset).
    Snapshot(&'a TermGridSnapshot),
    /// Incremental update since the last emit.
    Delta(&'a TermGridDelta),
    /// Child process exit notification. Sent once just before the
    /// server closes the WS. `exit_code` is `None` on signal-kill.
    #[allow(dead_code)]
    ChildExit { exit_code: Option<i32> },
    /// Pre-handshake or handshake-time fatal error. Client should
    /// treat as terminal and may retry once.
    Error { message: String },
}

/// Inbound WS message. Tagged as `{"action":"<kind>",...}`.
#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum Inbound {
    /// User keystroke(s) / paste. UTF-8 text; ESC sequences
    /// encoded as the bytes they represent (`\u001b...`).
    Input { text: String },
    /// Resize request from the client's ResizeObserver.
    Resize { cols: u16, rows: u16 },
}

pub async fn serve_session_grid_connection(
    stream: TcpStream,
    params: HashMap<String, String>,
) {
    let session_id = match params.get("session").and_then(|s| SessionId::parse(s)) {
        Some(id) => id,
        None => {
            send_error_then_close(
                stream,
                "missing or malformed 'session' query param",
            )
            .await;
            return;
        }
    };

    let session = match v2_session_map::lookup_by_session_id(&session_id) {
        Some(s) => s,
        None => {
            let msg = format!("session {session_id} not found in v2 session map");
            send_error_then_close(stream, &msg).await;
            return;
        }
    };

    let ws = match tokio_tungstenite::accept_async(stream).await {
        Ok(ws) => ws,
        Err(e) => {
            log_debug!("[daemon/sessions_grid_ws] ws handshake failed: {e}");
            return;
        }
    };
    let (mut write, mut read) = ws.split();

    // Single-subscriber enforcement: grabbing the events receiver
    // is what marks this session as "attached." If `take_events()`
    // returns None, another handler already has it — reject with
    // busy and close.
    let mut events_rx = match session.take_events() {
        Some(rx) => rx,
        None => {
            let err = Outbound::Error {
                message: "session already has an attached subscriber"
                    .to_string(),
            };
            send_outbound(&mut write, &err).await;
            let _ = write.close().await;
            return;
        }
    };

    let pane_id = format!("alacritty-v2-{}", session.session_id);

    // Initial full snapshot. EmitState::default() has has_emitted=false,
    // so build_emit would do the same thing — but we skip that and
    // take an explicit snapshot first so the WS contract reads
    // cleanly ("first message is always Snapshot").
    let mut emit_state = EmitState::default();
    let initial_snapshot = {
        // Bind the Arc<FairMutex<...>> to a local so it outlives
        // the guard. `session.term()` returns a temporary Arc.
        let term_mutex = session.term();
        let mut term = term_mutex.lock();
        emit_state.has_emitted = true;
        emit_state.version = 1;
        let snap = snapshot_term(&pane_id, &*term, emit_state.version);
        emit_state.last_history_size = snap.scrollback.len();
        term.reset_damage();
        snap
    };
    if send_outbound(&mut write, &Outbound::Snapshot(&initial_snapshot))
        .await
        .is_err()
    {
        // Client disconnected before we could send — give the
        // events receiver back so a future subscriber can take it.
        restore_events(&session, events_rx);
        return;
    }

    log_debug!(
        "[daemon/sessions_grid_ws] subscriber attached to session {} (pane {})",
        session.session_id,
        pane_id
    );

    // Main loop: event-driven. Every Wakeup from alacritty is a
    // cue to build_emit + send. Inbound messages route to
    // session.write() / session.resize(). No coalescing for v1 —
    // build_emit itself returns Skip when nothing changed, which
    // keeps the volume sane.
    loop {
        tokio::select! {
            ev = events_rx.recv() => {
                match ev {
                    Some(AlacEvent::Wakeup) => {
                        let decision = {
                            let term_mutex = session.term();
                            let mut term = term_mutex.lock();
                            build_emit(&pane_id, &mut *term, &mut emit_state)
                        };
                        let res = match decision {
                            EmitDecision::Full(snap) => {
                                send_outbound(&mut write, &Outbound::Snapshot(&snap)).await
                            }
                            EmitDecision::Delta(delta) => {
                                send_outbound(&mut write, &Outbound::Delta(&delta)).await
                            }
                            EmitDecision::Skip => Ok(()),
                        };
                        if res.is_err() {
                            break;
                        }
                    }
                    Some(AlacEvent::ChildExit(status)) => {
                        let exit = Outbound::ChildExit {
                            exit_code: status.code(),
                        };
                        let _ = send_outbound(&mut write, &exit).await;
                        break;
                    }
                    Some(_other) => {
                        // Title / Bell / ClipboardStore / ColorRequest /
                        // etc. Ignored for v2 — not part of the
                        // minimal grid-rendering contract.
                    }
                    None => {
                        // Channel closed (session dropped). Exit.
                        break;
                    }
                }
            }
            msg = read.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        let parsed: Result<Inbound, _> = serde_json::from_str(&text);
                        match parsed {
                            Ok(Inbound::Input { text }) => {
                                session.write(text.into_bytes());
                            }
                            Ok(Inbound::Resize { cols, rows }) => {
                                session.resize(cols, rows);
                            }
                            Err(e) => {
                                log_debug!(
                                    "[daemon/sessions_grid_ws] malformed inbound: {e}"
                                );
                                // Non-fatal — ignore and keep the socket open.
                            }
                        }
                    }
                    Some(Ok(Message::Binary(_))) => {
                        // Binary inbound not used by v2 protocol; drop.
                    }
                    Some(Ok(Message::Ping(p))) => {
                        if write.send(Message::Pong(p)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Pong(_))) => {}
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(Message::Frame(_))) => {}
                    Some(Err(e)) => {
                        log_debug!(
                            "[daemon/sessions_grid_ws] ws read error: {e}"
                        );
                        break;
                    }
                }
            }
        }
    }

    // Clean exit: hand the events receiver back to the session so
    // a subsequent subscriber can attach.
    restore_events(&session, events_rx);

    log_debug!(
        "[daemon/sessions_grid_ws] subscriber detached from session {}",
        session.session_id,
    );
}

/// Put the events receiver back on the session so the next subscriber
/// (or reattach after Tauri reload) can take it. If the session is
/// already dropped this is a no-op — the call returns a new-state
/// receiver-less session anyway. Using a free function keeps the
/// borrow checker happy with the select-loop ownership.
fn restore_events(
    session: &Arc<k2so_core::terminal::DaemonPtySession>,
    _rx: tokio::sync::mpsc::UnboundedReceiver<AlacEvent>,
) {
    // `_rx` is dropped here. The session has its own internal slot
    // that holds events_rx (via `Mutex<Option<...>>`); after drop
    // we're NOT putting the receiver back into it. That's
    // intentional: v2 is strictly single-subscriber, so once the
    // original handler exits, the session is effectively closed
    // for further subscribers. A future reattach would use
    // find-or-spawn (A4) to create a fresh session that has its
    // own receiver.
    //
    // If we later want "subscriber A disconnects, subscriber B
    // reconnects" support on the SAME session, we'd add a
    // `session.restore_events(rx)` method that re-inserts the
    // receiver into the Mutex<Option<...>>. Deferred — not in A3
    // scope.
    let _ = session;
}

async fn send_outbound<W>(write: &mut W, msg: &Outbound<'_>) -> Result<(), ()>
where
    W: futures_util::SinkExt<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    let text = match serde_json::to_string(msg) {
        Ok(s) => s,
        Err(e) => {
            log_debug!(
                "[daemon/sessions_grid_ws] serialize outbound failed: {e}"
            );
            return Err(());
        }
    };
    write.send(Message::Text(text.into())).await.map_err(|e| {
        log_debug!("[daemon/sessions_grid_ws] send failed: {e}");
    })
}

async fn send_error_then_close(stream: TcpStream, msg: &str) {
    let err = Outbound::Error {
        message: msg.to_string(),
    };
    let ws = match tokio_tungstenite::accept_async(stream).await {
        Ok(ws) => ws,
        Err(_) => return,
    };
    let (mut write, _read) = ws.split();
    let _ = send_outbound(&mut write, &err).await;
    let _ = write.close().await;
}

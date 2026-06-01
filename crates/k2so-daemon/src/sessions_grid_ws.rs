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
    /// Terminal title change (`OSC 0/1/2`). Used by the renderer
    /// for the same fast-idle / fast-working hints legacy pulls
    /// from `terminal:title:<id>` Tauri events: Claude Code's
    /// braille-spinner prefix while working, the ✱-family glyphs
    /// the moment it goes idle, etc.
    ///
    /// **0.37.4 (Phase B):** the renderer keeps using this for
    /// activity-marker detection only — tab labels come from
    /// `LabelInitial` / `LabelChanged` instead. Title is kept on
    /// the wire so legacy idle/working hints don't regress.
    Title { title: String },
    /// Authoritative label for the session at WS-connect time
    /// (Phase B). Sent once, immediately after the initial
    /// snapshot. Renderer should display this as the tab label
    /// (or whatever surface it owns) until a `LabelChanged`
    /// supersedes it. Daemon-owned — survives renderer reload via
    /// re-fetch on the next connect.
    LabelInitial { label: String },
    /// Authoritative label updated mid-session (Phase B). Fired
    /// when the daemon's PTY-title interceptor accepts a change
    /// (label_source ∈ {Pty, Seed}) or when an explicit caller hits
    /// `set_label` via the new `/cli/sessions/<id>/label` route.
    /// Renderer just replaces its mirror — no decision-making in
    /// the client.
    LabelChanged { label: String },
    /// Bell character (`\a`, OSC 7). Mirrors how iTerm decides
    /// "agent is now waiting for input": Claude rings the bell
    /// when it's done and ready for a reply. Renderer can use
    /// this as a definitive "idle, waiting" transition signal,
    /// independent of any viewport-text scan.
    Bell,
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
    /// 0.37.11 — viewer-claim: this subscriber is becoming the
    /// active viewer (or releasing the claim). Active viewers are
    /// the only subscribers whose `Resize` frames the daemon
    /// honors. Renderer sends `true` on window focus, `false` on
    /// blur. Once the daemon-side `active_subscriber` is set,
    /// resize from non-active subscribers is ignored — eliminates
    /// the "two windows fighting over the TUI size" problem.
    SetActive { active: bool },
}

/// 0.37.11 — monotonically-increasing subscriber id generator.
/// Each WS accept claims the next value; the id is passed to the
/// session's `active_subscriber` atomic on viewer-claim. Starts at
/// 1 because 0 is the "no claim" sentinel value.
static NEXT_SUBSCRIBER_ID: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(1);

/// The state transition a `SetActive` frame implies, given the
/// session's current `active_subscriber` value and the requesting
/// subscriber's id. Extracted as a pure function so the idempotence
/// rules (Issue #8) are unit-testable without spinning up a PTY +
/// WebSocket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SetActiveOutcome {
    /// `active:true` from a subscriber that does NOT currently hold
    /// the claim — store our id (displacing any prior claimer).
    Claim,
    /// `active:false` from the subscriber that currently holds the
    /// claim — clear it (CAS guards against a concurrent takeover).
    Release,
    /// No state change: a redundant claim (we already hold it) or a
    /// redundant release (we never held it). Skip the store + skip
    /// the log so a chatty client can't flood the grid broadcast.
    NoOp,
}

/// Decide what a `SetActive { active }` frame should do given the
/// current `active_subscriber` (0 == no claim) and the requesting
/// `subscriber_id` (always nonzero — 0 is the sentinel).
fn decide_set_active(current: u64, subscriber_id: u64, active: bool) -> SetActiveOutcome {
    if active {
        // Claiming. Only a real state change if we're not already
        // the active subscriber. (current may be 0 = unclaimed, or
        // another subscriber's id = displace them — both are claims.)
        if current == subscriber_id {
            SetActiveOutcome::NoOp
        } else {
            SetActiveOutcome::Claim
        }
    } else {
        // Releasing. Only a real change if we currently hold it.
        if current == subscriber_id {
            SetActiveOutcome::Release
        } else {
            SetActiveOutcome::NoOp
        }
    }
}

pub async fn serve_session_grid_connection(
    stream: &mut TcpStream,
    params: HashMap<String, String>,
) {
    // 0.39.7: stream borrowed (was owned). See events.rs.
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

    // 0.37.11 — claim a unique subscriber id for this connection.
    // The id stays stable for the WS's lifetime; the renderer's
    // `SetActive` frame stamps this value into the session's
    // `active_subscriber`. `Resize` frames check against it.
    let subscriber_id = NEXT_SUBSCRIBER_ID.fetch_add(
        1,
        std::sync::atomic::Ordering::Relaxed,
    );

    let __t_accept = std::time::Instant::now();
    let ws = match tokio_tungstenite::accept_async(stream).await {
        Ok(ws) => ws,
        Err(e) => {
            log_debug!("[daemon/sessions_grid_ws] ws handshake failed: {e}");
            return;
        }
    };
    let ws_accept_ms = __t_accept.elapsed().as_secs_f64() * 1000.0;
    log_debug!(
        "[v2-perf] side=daemon stage=ws_accept ms={:.3} session={}",
        ws_accept_ms,
        session.session_id
    );
    let (mut write, mut read) = ws.split();

    // Subscribe to the session's event broadcast. Multiple
    // subscribers can coexist (though v2 is effectively
    // single-subscriber); remount-during-swap sequences that used
    // to fail with "busy" now subscribe fresh each time and
    // render from a new initial snapshot.
    let mut events_rx = session.subscribe_events();
    // Phase B: subscribe to out-of-band label updates. When the
    // CLI route or another process sets the session's label, we
    // need to push `LabelChanged` to this client.
    let mut labels_rx = session.subscribe_labels();

    let pane_id = format!("alacritty-v2-{}", session.session_id);

    // Initial full snapshot. EmitState::default() has has_emitted=false,
    // so build_emit would do the same thing — but we skip that and
    // take an explicit snapshot first so the WS contract reads
    // cleanly ("first message is always Snapshot").
    let mut emit_state = EmitState::default();
    let __t_first_snap = std::time::Instant::now();
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
    let snap_rows = initial_snapshot.rows;
    let snap_cols = initial_snapshot.cols;
    let snap_scrollback = initial_snapshot.scrollback.len();
    if send_outbound(&mut write, &Outbound::Snapshot(&initial_snapshot))
        .await
        .is_err()
    {
        // Client disconnected before we could send. `events_rx`
        // drops implicitly on return; broadcast subscribers don't
        // need to be restored.
        return;
    }
    // Phase B: emit the authoritative session label immediately
    // after the snapshot. Subscribers display this as the tab
    // label. If the daemon was given an empty seed (most spawn
    // paths still are during the rollout window), the renderer
    // falls back to its own display-name lookup.
    let initial_label = session.label();
    if send_outbound(
        &mut write,
        &Outbound::LabelInitial { label: initial_label },
    )
    .await
    .is_err()
    {
        return;
    }
    let first_snap_ms = __t_first_snap.elapsed().as_secs_f64() * 1000.0;
    log_debug!(
        "[v2-perf] side=daemon CONNECT-SUMMARY session={} ws_accept_ms={:.3} first_snap_ms={:.3} rows={} cols={} scrollback={}",
        session.session_id,
        ws_accept_ms,
        first_snap_ms,
        snap_rows,
        snap_cols,
        snap_scrollback
    );

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
            // Phase B: out-of-band label changes (from /cli/sessions/<id>/label
            // or a multi-window peer's set). Push to this client so
            // its tab updates without a renderer round-trip.
            label = labels_rx.recv() => {
                match label {
                    Ok(new_label) => {
                        if send_outbound(
                            &mut write,
                            &Outbound::LabelChanged { label: new_label },
                        )
                        .await
                        .is_err()
                        {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        // Subscriber fell behind on label events.
                        // Re-emit the current authoritative label so
                        // the client converges.
                        let current = session.label();
                        let _ = send_outbound(
                            &mut write,
                            &Outbound::LabelChanged { label: current },
                        )
                        .await;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        // Session dropped — main loop will see the
                        // events_rx Closed too. Don't break here;
                        // let the event loop terminate cleanly.
                    }
                }
            }
            ev = events_rx.recv() => {
                match ev {
                    Ok(AlacEvent::Wakeup) => {
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
                    Ok(AlacEvent::ChildExit(status)) => {
                        let exit = Outbound::ChildExit {
                            exit_code: status.code(),
                        };
                        let _ = send_outbound(&mut write, &exit).await;
                        // Send a Close frame before tearing down the
                        // socket so the browser sees a graceful close.
                        // Without this, WebKit fires `onerror` →
                        // frontend renders "ws error" instead of the
                        // child_exit message that just preceded it.
                        let _ = write.send(Message::Close(None)).await;
                        break;
                    }
                    Ok(AlacEvent::Title(title)) => {
                        // Forward as Outbound::Title so the renderer
                        // can use the same idle/working hints legacy
                        // pulls from `terminal:title:<id>` Tauri
                        // events (activity-marker detection only —
                        // post-Phase B, tab labels come from
                        // LabelInitial/LabelChanged instead).
                        let _ = send_outbound(
                            &mut write,
                            &Outbound::Title { title: title.clone() },
                        )
                        .await;
                        // Phase B: feed the title into the daemon's
                        // label state machine. Honors LabelSource;
                        // when not Locked, updates the label and
                        // broadcasts on `label_tx` so EVERY
                        // subscriber's labels_rx arm wakes and emits
                        // `LabelChanged` (multi-window convergence).
                        // Don't emit here — let the broadcast path
                        // handle it uniformly.
                        let _ = session.try_set_label_from_pty(title);
                    }
                    Ok(AlacEvent::ResetTitle) => {
                        // OSC 0 reset → empty title. Treated by the
                        // renderer the same as a non-marker title
                        // (no idle hint).
                        let _ = send_outbound(
                            &mut write,
                            &Outbound::Title { title: String::new() },
                        )
                        .await;
                        // Phase B: empty label → renderer falls
                        // back to its workspace-derived helper.
                        // Same broadcast path as Title.
                        let _ = session.try_set_label_from_pty(String::new());
                    }
                    Ok(AlacEvent::Bell) => {
                        // Bell — used by Claude / Codex to signal
                        // "I'm done and waiting for input." Same
                        // signal iTerm uses for its "agent waiting"
                        // notifications.
                        let _ = send_outbound(&mut write, &Outbound::Bell).await;
                    }
                    Ok(_other) => {
                        // ClipboardStore / ColorRequest / etc.
                        // Ignored for v2 — not part of the minimal
                        // grid-rendering contract.
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        // Consumer fell behind. Term state still
                        // advancing correctly on the daemon; we
                        // just missed `n` events. Issue a fresh
                        // full snapshot so client state catches up.
                        log_debug!(
                            "[daemon/sessions_grid_ws] subscriber lagged {n} events, sending fresh snapshot"
                        );
                        let snap = {
                            let term_mutex = session.term();
                            let mut term = term_mutex.lock();
                            emit_state.has_emitted = false; // force Full on next build_emit
                            let d = build_emit(&pane_id, &mut *term, &mut emit_state);
                            match d {
                                EmitDecision::Full(s) => Some(s),
                                _ => None,
                            }
                        };
                        if let Some(snap) = snap {
                            if send_outbound(&mut write, &Outbound::Snapshot(&snap))
                                .await
                                .is_err()
                            {
                                break;
                            }
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        // Session dropped (last Arc released). Exit.
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
                                // 0.37.11 — only the active subscriber
                                // can resize. `active = 0` means "no
                                // claim yet" — accept (first-resize-
                                // wins, preserves single-viewer behavior
                                // for sessions where no one ever sends
                                // SetActive). Otherwise gate on match.
                                let active = session
                                    .active_subscriber
                                    .load(std::sync::atomic::Ordering::Relaxed);
                                if active == 0 || active == subscriber_id {
                                    session.resize(cols, rows);
                                } else {
                                    log_debug!(
                                        "[v2-perf] side=daemon stage=resize_ignored \
                                         session={} from_sub={} active_sub={} \
                                         (non-active viewer resize dropped)",
                                        session.session_id,
                                        subscriber_id,
                                        active,
                                    );
                                }
                            }
                            Ok(Inbound::SetActive { active }) => {
                                // Issue #8 backstop: make the claim/release
                                // handler idempotent. A long-lived window
                                // with many mounted-but-hidden panes used
                                // to fire `set_active(true)` from every pane
                                // on each window-focus event, re-storing the
                                // same `active_subscriber` and re-logging on
                                // every redundant claim. With the renderer
                                // now keying on visible+focused, this should
                                // be quiet — but we enforce it daemon-side so
                                // a noisy/buggy client can't reintroduce the
                                // grid-broadcast flood that produced the
                                // "stall then recover" symptom. We compute the
                                // transition with a pure helper (unit-tested
                                // below), apply only real state changes, and
                                // only log when something actually changed.
                                let prev = session
                                    .active_subscriber
                                    .load(std::sync::atomic::Ordering::Relaxed);
                                match decide_set_active(prev, subscriber_id, active) {
                                    SetActiveOutcome::Claim => {
                                        // Real claim: we weren't already the
                                        // active subscriber. Previous claimer
                                        // (if any) is silently displaced —
                                        // most-recent claim wins.
                                        session.active_subscriber.store(
                                            subscriber_id,
                                            std::sync::atomic::Ordering::Relaxed,
                                        );
                                        log_debug!(
                                            "[v2-perf] side=daemon stage=active_claim \
                                             session={} sub={}",
                                            session.session_id,
                                            subscriber_id,
                                        );
                                    }
                                    SetActiveOutcome::Release => {
                                        // Real release: we held the claim and
                                        // are giving it up. CAS so a viewer
                                        // that took over before us isn't
                                        // accidentally cleared.
                                        let _ = session
                                            .active_subscriber
                                            .compare_exchange(
                                                subscriber_id,
                                                0,
                                                std::sync::atomic::Ordering::Relaxed,
                                                std::sync::atomic::Ordering::Relaxed,
                                            );
                                        log_debug!(
                                            "[v2-perf] side=daemon stage=active_release \
                                             session={} sub={}",
                                            session.session_id,
                                            subscriber_id,
                                        );
                                    }
                                    SetActiveOutcome::NoOp => {
                                        // Redundant claim/release — no state
                                        // change, no log. This is the Issue #8
                                        // backstop: silence the churn instead
                                        // of faithfully re-storing + re-logging.
                                    }
                                }
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
                    Some(Ok(Message::Close(frame))) => {
                        // Echo a Close frame so the client sees a clean
                        // graceful-close handshake. Without this, WebKit
                        // logs "The network connection was lost" because
                        // it gets TCP FIN before our Close frame, which
                        // RFC 6455 §7 calls an abnormal close. The frame
                        // payload is mirrored per spec recommendation.
                        let _ = write.send(Message::Close(frame)).await;
                        break;
                    }
                    None => break,
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

    // 0.37.11 — release the active claim on disconnect IF we still
    // hold it. CAS so a viewer that took over our claim before we
    // disconnected isn't accidentally cleared.
    let _ = session.active_subscriber.compare_exchange(
        subscriber_id,
        0,
        std::sync::atomic::Ordering::Relaxed,
        std::sync::atomic::Ordering::Relaxed,
    );

    // Drop `events_rx` implicitly on return. Broadcast subscribers
    // don't need to be "restored" — the next connection just calls
    // `subscribe_events()` for a fresh receiver.
    drop(events_rx);

    log_debug!(
        "[daemon/sessions_grid_ws] subscriber detached from session {} (sub_id={})",
        session.session_id,
        subscriber_id,
    );
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

async fn send_error_then_close(stream: &mut TcpStream, msg: &str) {
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

#[cfg(test)]
mod tests {
    use super::*;

    // Issue #8: the active-viewer claim/release handler must be
    // idempotent. These tests pin the pure decision so a future edit
    // can't silently reintroduce the redundant-store + redundant-log
    // churn that flooded the per-session grid broadcast channel.

    #[test]
    fn claiming_when_unclaimed_is_a_real_claim() {
        // active_subscriber == 0 (no claim), sub 7 claims → Claim.
        assert_eq!(decide_set_active(0, 7, true), SetActiveOutcome::Claim);
    }

    #[test]
    fn claiming_twice_with_same_subscriber_is_a_noop() {
        // We already hold the claim; re-asserting it changes nothing
        // and must not store or log again.
        assert_eq!(decide_set_active(7, 7, true), SetActiveOutcome::NoOp);
    }

    #[test]
    fn claiming_when_another_holds_it_displaces() {
        // Most-recent-claim-wins: sub 9 claims while sub 7 holds it →
        // a real Claim (the handler stores 9, displacing 7).
        assert_eq!(decide_set_active(7, 9, true), SetActiveOutcome::Claim);
    }

    #[test]
    fn releasing_our_own_claim_is_a_real_release() {
        assert_eq!(decide_set_active(7, 7, false), SetActiveOutcome::Release);
    }

    #[test]
    fn releasing_when_unclaimed_is_a_noop() {
        // Never held it (current == 0) → release is a no-op, no CAS
        // attempt, no log.
        assert_eq!(decide_set_active(0, 7, false), SetActiveOutcome::NoOp);
    }

    #[test]
    fn releasing_someone_elses_claim_is_a_noop() {
        // sub 7 sends release while sub 9 holds the claim — must NOT
        // clear sub 9's claim and must NOT log. The runtime CAS would
        // also reject this, but we short-circuit before logging.
        assert_eq!(decide_set_active(9, 7, false), SetActiveOutcome::NoOp);
    }
}

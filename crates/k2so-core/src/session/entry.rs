//! `SessionEntry` — one session's live broadcast + replay ring.
//!
//! Producer (Phase 2's `session_stream_pty` reader thread, later the
//! daemon's session spawn path) calls `publish(frame)`. Every
//! currently-subscribed receiver gets a copy via the tokio broadcast
//! channel; late subscribers can drain the replay ring to catch up
//! before hooking into live.
//!
//! Layers in the PRD at `.k2so/prds/session-stream-and-awareness-bus.md`
//! §"Session persistence model":
//!   - Live broadcast:   `tokio::sync::broadcast::Sender<Frame>`
//!   - Replay ring:      bounded `VecDeque<Frame>` (front-pop on overflow)
//!   - Archive log:      deferred to Phase 3
//!
//! `SessionEntry` holds only the first two. The archive NDJSON
//! writer task attaches as just another subscriber in Phase 3.

use std::collections::VecDeque;
use std::sync::Arc;

use parking_lot::Mutex;
use tokio::sync::broadcast;

use crate::session::Frame;

/// Replay-ring capacity — per the PRD default.
pub const REPLAY_CAP: usize = 1000;

/// Broadcast-channel capacity — matches the daemon's existing
/// `/events` endpoint (crates/k2so-daemon/src/events.rs:48). Slow
/// receivers get `RecvError::Lagged(n)` once the channel's internal
/// buffer fills; they can then catch up from the replay ring.
pub const BROADCAST_CAP: usize = 256;

/// Everything a session needs to fan frames to N consumers.
#[derive(Debug)]
pub struct SessionEntry {
    /// Live broadcast. Tokio's broadcast drops oldest for slow
    /// receivers by default — exactly the backpressure policy the
    /// PRD locks (2026-04-19 Q4 resolution).
    pub tx: broadcast::Sender<Frame>,
    /// Bounded replay buffer for late-joining subscribers.
    pub replay: Arc<Mutex<VecDeque<Frame>>>,
    /// Max size of the replay ring. Trimmed front-first on overflow.
    replay_cap: usize,
}

impl SessionEntry {
    /// Build a fresh entry with empty replay ring and default
    /// capacities.
    pub fn new() -> Self {
        Self::with_replay_cap(REPLAY_CAP)
    }

    /// Build with a custom replay cap. Mostly for tests.
    pub fn with_replay_cap(replay_cap: usize) -> Self {
        assert!(replay_cap >= 1, "SessionEntry replay_cap must be >= 1");
        let (tx, _) = broadcast::channel(BROADCAST_CAP);
        Self {
            tx,
            replay: Arc::new(Mutex::new(VecDeque::with_capacity(replay_cap))),
            replay_cap,
        }
    }

    /// Publish a frame. Durably appended to the replay ring first,
    /// then broadcast to any live subscribers. Silent if there are
    /// no receivers (same behavior as the existing `/events`
    /// endpoint — producers never care whether anyone's listening).
    pub fn publish(&self, frame: Frame) {
        {
            let mut replay = self.replay.lock();
            replay.push_back(frame.clone());
            while replay.len() > self.replay_cap {
                replay.pop_front();
            }
        }
        let _ = self.tx.send(frame);
    }

    /// Subscribe to the live stream. The returned receiver sees
    /// frames published AFTER the subscribe call — call
    /// `replay_snapshot()` to catch up on prior frames before
    /// draining this.
    pub fn subscribe(&self) -> broadcast::Receiver<Frame> {
        self.tx.subscribe()
    }

    /// Snapshot of the replay ring — clones up to `replay_cap`
    /// frames. Intended for the "subscriber connects, gets
    /// last-N, then hooks into live" flow the WS endpoint
    /// (Phase 2 D4) will implement.
    pub fn replay_snapshot(&self) -> Vec<Frame> {
        self.replay.lock().iter().cloned().collect()
    }

    /// Current receiver count — useful for the UI / monitoring.
    pub fn subscriber_count(&self) -> usize {
        self.tx.receiver_count()
    }

    /// The configured replay ring cap. Used by callers that want
    /// to pre-allocate buffers of the right size.
    pub fn replay_cap(&self) -> usize {
        self.replay_cap
    }
}

impl Default for SessionEntry {
    fn default() -> Self {
        Self::new()
    }
}

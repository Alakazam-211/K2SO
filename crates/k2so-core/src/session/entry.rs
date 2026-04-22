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
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use tokio::sync::broadcast;

use crate::session::Frame;

/// Replay-ring capacity. Originally 1000 per the PRD default, but
/// that was tuned for live-spawned sessions where subscribers attach
/// within a few hundred ms and miss at most a handful of bytes. It
/// breaks down for `claude --resume <id>` (and similar harness
/// resumes) where the child process replays the entire prior
/// conversation to the PTY in a burst of tens of thousands of bytes.
/// LineMux emits a fresh Frame::Text on every SGR change, and
/// Claude's markdown output is heavily styled — a medium
/// conversation can produce ~5-10 frames per visible line, and a
/// resume burst easily overflows a 1000-cap ring before the Tauri
/// renderer has opened its WebSocket. The user-visible symptom is
/// "resume shows only the last few lines of the conversation."
///
/// Bumped to 50_000 so typical resumes fit end-to-end. Memory cost
/// per session: ~50_000 × avg_frame_bytes (~100-200 B) ≈ 5-10 MB,
/// bounded and deallocated when the session ends.
///
/// The permanent fix is to replay the on-disk NDJSON archive
/// (`session::archive`) for late subscribers so we're not
/// memory-bound at all, but that's a larger change. This constant
/// bump is the minimal hotfix.
pub const REPLAY_CAP: usize = 50_000;

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
    /// Optional agent name the session belongs to. Set when the
    /// session is spawned for a specific agent (0.34.0 Phase 3
    /// onward). Anonymous sessions (one-off debugging, test
    /// fixtures) leave this `None`; roster queries skip them.
    agent_name: Mutex<Option<String>>,
    /// Monotonic time the entry was created. Used by the harness
    /// watchdog to avoid SIGTERM-ing sessions that just spawned
    /// (some harnesses take several seconds before emitting their
    /// first frame). Read-only after construction.
    created_at: Instant,
    /// Monotonic time of the most recent `publish()` call. Updated
    /// atomically on every frame emission. Watchdogs read this to
    /// decide whether a session is wedged.
    last_frame_at: Mutex<Instant>,
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
        let now = Instant::now();
        Self {
            tx,
            replay: Arc::new(Mutex::new(VecDeque::with_capacity(replay_cap))),
            replay_cap,
            agent_name: Mutex::new(None),
            created_at: now,
            last_frame_at: Mutex::new(now),
        }
    }

    /// Tag this session with the agent it represents. Idempotent —
    /// second call overwrites; useful if a session rebinds to a
    /// different agent (rare but not forbidden).
    pub fn set_agent_name(&self, name: impl Into<String>) {
        *self.agent_name.lock() = Some(name.into());
    }

    /// Read the current agent-name binding, if any.
    pub fn agent_name(&self) -> Option<String> {
        self.agent_name.lock().clone()
    }

    /// Publish a session-content frame. Durably appended to the
    /// replay ring, broadcast to live subscribers, and bumps
    /// `last_frame_at` so the watchdog sees this as harness
    /// activity and resets its idle timer.
    ///
    /// Use this for every frame that originates from the harness
    /// (PTY output, agent signals, semantic events lifted from
    /// harness protocols). For observer-emitted frames like
    /// watchdog escalations, use `publish_meta` so the watchdog
    /// doesn't inadvertently reset its own idle timer.
    pub fn publish(&self, frame: Frame) {
        self.publish_inner(frame, true);
    }

    /// Publish a meta frame without touching `last_frame_at`.
    /// Intended for watchdog escalations + other observer-emitted
    /// frames that describe session state rather than carry
    /// harness output. Subscribers still see the frame in the
    /// same stream; only the idle-timer bump is suppressed.
    ///
    /// Primitive rule: anything the WATCHDOG emits is meta;
    /// anything the HARNESS emits is content. Keep the two lanes
    /// separate so `idle_for(now)` always answers "how long since
    /// the harness last produced" rather than "how long since any
    /// frame was published."
    pub fn publish_meta(&self, frame: Frame) {
        self.publish_inner(frame, false);
    }

    fn publish_inner(&self, frame: Frame, bump_activity: bool) {
        {
            let mut replay = self.replay.lock();
            replay.push_back(frame.clone());
            while replay.len() > self.replay_cap {
                replay.pop_front();
            }
        }
        let _ = self.tx.send(frame);
        if bump_activity {
            *self.last_frame_at.lock() = Instant::now();
        }
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

    /// When this entry was constructed. Monotonic clock — safe to
    /// subtract from `Instant::now()`. Watchdog uses this to give
    /// freshly-spawned sessions a grace period before the idle
    /// timer starts counting (otherwise a slow-booting harness
    /// gets Ctrl-C'd before it finishes printing its first prompt).
    pub fn created_at(&self) -> Instant {
        self.created_at
    }

    /// Timestamp of the most recent `publish()`. Re-computed every
    /// time a frame is broadcast. Watchdog subtracts this from
    /// `now()` to get idle duration.
    pub fn last_frame_at(&self) -> Instant {
        *self.last_frame_at.lock()
    }

    /// How long the session has gone without emitting a frame, as
    /// of `now`. Small convenience around `now - last_frame_at()`
    /// that saturates at zero if the monotonic clock hiccups (can
    /// happen across suspend/resume on macOS).
    pub fn idle_for(&self, now: Instant) -> Duration {
        now.saturating_duration_since(self.last_frame_at())
    }
}

impl Default for SessionEntry {
    fn default() -> Self {
        Self::new()
    }
}

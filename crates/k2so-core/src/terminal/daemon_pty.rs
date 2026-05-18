//! Alacritty_v2 daemon-hosted terminal session.
//!
//! Minimum viable terminal: PTY + alacritty Term on the daemon,
//! driven by alacritty_terminal's built-in `EventLoop::spawn()`.
//! No LineMux, no byte broadcast, no ring, no APC coordination —
//! single-subscriber by design.
//!
//! Conceptually this is Alacritty_v1 with the PTY and Term moved
//! from the Tauri process into the daemon so that:
//!
//!   - Sessions survive Tauri quit (daemon owns the master FD).
//!   - Heartbeats can target the session by `agent_name` via
//!     `session_map` registration (caller's responsibility).
//!   - One Tauri-side grid-snapshot/delta client can render it.
//!
//! Follows Zed's `TerminalBuilder` pattern — uses alacritty's
//! built-in event loop instead of a custom reader thread. See
//! `.k2so/prds/alacritty-v2.md` for the product context.
//!
//! **Deliberately NOT used by** `session_stream_pty.rs` or any
//! Kessel-T0 path. Those stay on their own fork. When v2 ships and
//! retires v1, this becomes the single daemon-side terminal type.

use std::borrow::Cow;
use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use alacritty_terminal::event::{
    EventListener, Notify, OnResize, WindowSize,
};

/// Re-exported so downstream crates (the daemon, tests) can pattern-
/// match on alacritty's lifecycle events without needing their own
/// direct `alacritty_terminal` dependency. The daemon crate only
/// depends on k2so-core; this keeps that surface honest.
pub use alacritty_terminal::event::Event as AlacEvent;

use alacritty_terminal::event_loop::{EventLoop, Notifier};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::Config as TermConfig;
use alacritty_terminal::tty::{self, Options as TtyOptions, Shell};
use alacritty_terminal::Term;
use parking_lot::Mutex;
use tokio::sync::broadcast;

use crate::log_debug;
use crate::session::SessionId;

/// Scrollback depth (in rows) retained by the daemon-side Term.
/// Matches `session_stream_pty.rs`'s value so v2 sessions inherit
/// the same "how much history can I scroll back through" UX as
/// v1 sessions.
pub const SCROLLBACK_CAP: usize = 5000;

/// Thin `Dimensions` implementation for the Term. `total_lines`
/// returns rows + SCROLLBACK_CAP so the Term sizes its scrollback
/// buffer correctly at construction time.
#[derive(Clone, Copy, Debug)]
pub struct TermSize {
    pub cols: usize,
    pub rows: usize,
}

impl Dimensions for TermSize {
    fn total_lines(&self) -> usize {
        self.rows + SCROLLBACK_CAP
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

/// Minimal `EventListener` that broadcasts every alacritty lifecycle
/// event to any number of subscribers via `tokio::sync::broadcast`.
/// Consumers (A3's WS handler) subscribe fresh on attach and pull
/// from their own receiver; there's no ownership transfer, so a
/// subscriber disconnecting + reconnecting works cleanly.
///
/// `send_event` is invoked by alacritty's IO thread, which is a
/// plain `std::thread` (not a tokio context). `broadcast::Sender::send`
/// is thread-safe and non-blocking, so cross-context use is fine.
///
/// Channel capacity is 256 — enough to absorb a burst of Wakeup
/// events during a heavy PTY read without lagging subscribers.
/// Alacritty typically emits ~10-100 events/sec on active use;
/// 256 is several seconds of headroom at worst case.
pub const EVENT_CHANNEL_CAPACITY: usize = 256;

#[derive(Clone)]
pub struct DaemonEventListener {
    tx: broadcast::Sender<AlacEvent>,
}

impl EventListener for DaemonEventListener {
    fn send_event(&self, event: AlacEvent) {
        // Fire-and-forget. If no subscribers, send returns `Err`
        // and we ignore it — the daemon keeps advancing Term state
        // regardless. Subscribers that reconnect later will get the
        // current grid via an initial snapshot + subsequent live
        // events from that point forward.
        let _ = self.tx.send(event);
    }
}

/// Source-of-truth marker for `DaemonPtySession::label`. Drives
/// the title-event-vs-locked-label state machine that lives on the
/// daemon side (Phase B / `session-label-daemon-owned.md` PRD).
///
/// - `Pty` — default. PTY `WindowTitleChanged` events freely
///   overwrite the label. Right answer for ad-hoc Cmd+T tabs where
///   vim's filename or Claude's progress glyph is informative.
/// - `Seed` — caller (renderer/CLI/heartbeat fire) supplied a
///   meaningful label at spawn time but didn't lock it. PTY can
///   still update the label as the session evolves; the seed is
///   just the initial value subscribers see on connect.
/// - `Locked` — caller wants the label PINNED. PTY title events
///   are observed (still drive activity-marker detection) but
///   never mutate the label. Used by the canonical workspace+agent
///   session, heartbeat-fire sessions, and explicit user renames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelSource {
    Pty,
    Seed,
    Locked,
}

impl Default for LabelSource {
    fn default() -> Self {
        LabelSource::Pty
    }
}

/// Configuration for `DaemonPtySession::spawn`. Construct via
/// `DaemonPtyConfig::default()` and mutate fields, or use the
/// struct literal with explicit values.
#[derive(Debug, Clone)]
pub struct DaemonPtyConfig {
    pub session_id: SessionId,
    pub cols: u16,
    pub rows: u16,
    pub cwd: Option<PathBuf>,
    /// Shell program to run. `None` = alacritty's default
    /// (user's login shell).
    pub program: Option<String>,
    /// Arguments passed to `program`. Ignored if `program` is None.
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    /// If true, drain the child's pending output before tearing
    /// down the PTY on child exit. Matches Zed's default.
    pub drain_on_exit: bool,
    /// Initial label for the session (Phase B). Empty string ⇒
    /// caller had no preference; PTY title events will fill it in.
    /// Populated values are visible via `LabelInitial` to the first
    /// WS subscriber.
    pub label: String,
    /// Initial label source (Phase B). Defaults to `Pty` when the
    /// caller doesn't care; set to `Seed` when supplying an initial
    /// `label` but allowing PTY updates; set to `Locked` to pin the
    /// label so PTY events never overwrite it.
    pub label_source: LabelSource,
}

impl Default for DaemonPtyConfig {
    fn default() -> Self {
        Self {
            session_id: SessionId::new(),
            cols: 80,
            rows: 24,
            cwd: None,
            program: None,
            args: Vec::new(),
            env: HashMap::new(),
            drain_on_exit: true,
            label: String::new(),
            label_source: LabelSource::Pty,
        }
    }
}

/// A daemon-hosted terminal session.
///
/// Holds the PTY, the alacritty Term, and the PTY writer handle.
/// Typically stored inside an `Arc` so multiple subsystems
/// (session_map, registry, WS handler) can share one handle.
///
/// Dropping the last Arc closes the PTY channel, which causes
/// alacritty's IO thread to exit naturally. The thread handle is
/// NOT stored — we let it clean up itself on channel close.
pub struct DaemonPtySession {
    pub session_id: SessionId,
    pub cwd: Option<PathBuf>,
    pub program: Option<String>,
    /// Args the child was spawned with. Persisted on the session so
    /// post-spawn callers (e.g. heartbeat smart-launch's "is there a
    /// live PTY running --resume <session_id>?" check) can match by
    /// arg contents without keeping a parallel map. Empty when the
    /// shell was spawned with the user's default login args only.
    pub args: Vec<String>,

    /// The daemon-side alacritty Term. Locked briefly by the WS
    /// handler to snapshot the grid or by `resize()` to reshape it.
    /// Alacritty's `FairMutex` prevents writer starvation under
    /// heavy IO-thread contention.
    term: Arc<FairMutex<Term<DaemonEventListener>>>,

    /// Notifier for writing input bytes + signaling resize. The
    /// Notifier wraps the alacritty event loop's sender channel;
    /// dropping it closes the channel and shuts the IO thread down.
    /// Guarded by a `Mutex` so concurrent `write()` + `resize()`
    /// calls serialize (Notifier::notify needs `&self` but
    /// on_resize needs `&mut self`).
    pty_notifier: Mutex<Notifier>,

    /// Broadcast sender for alacritty lifecycle events (Wakeup,
    /// Title, Bell, ChildExit, etc.). Subscribers call
    /// `subscribe_events()` to get a fresh receiver — any number
    /// of subscribers can exist, and reconnects just subscribe
    /// again (no ownership handoff).
    events_tx: broadcast::Sender<AlacEvent>,

    /// Flipped to `true` when the alacritty `ChildExit` event has
    /// been observed. Read by `is_child_alive()` for
    /// idempotency-check liveness probes (the daemon's
    /// `agents running` reaping pass and the spawn helper's
    /// existing-session check both consult it). Set inside the
    /// event listener — fires synchronously when alacritty's IO
    /// thread sees the child exit, so the bool flips before the
    /// broadcast subscribers are woken.
    child_exited: std::sync::atomic::AtomicBool,

    /// 0.37.11 — id of the subscriber currently claiming "active
    /// viewer" status. The active viewer is the one whose grid
    /// dimensions drive the PTY's resize. Other subscribers see
    /// the daemon's snapshot/delta stream at whatever size the
    /// active viewer dictates and don't push their own resize
    /// frames.
    ///
    /// 0 means "no active claim yet" — first resize from any
    /// subscriber wins (matches pre-claim behavior for single-
    /// viewer sessions). Non-zero values are
    /// monotonically-incrementing subscriber ids generated by the
    /// WS accept handler.
    ///
    /// Implemented as AtomicU64 so claim / release / lookup are
    /// lock-free; contention is minimal (claims fire on window
    /// focus change, not per-frame).
    pub active_subscriber: std::sync::atomic::AtomicU64,

    /// Authoritative human-friendly label (Phase B / PRD
    /// `session-label-daemon-owned.md`). The daemon owns this
    /// string; every consumer (Tauri tabs, mobile companion, CLI
    /// `agents running`, future MCP) reads it from here. PTY title
    /// events are absorbed here according to `label_source` — they
    /// don't round-trip through the renderer back to the tab.
    label: std::sync::RwLock<String>,
    /// Drives the PTY-title-vs-locked-label state machine. See
    /// [`LabelSource`]. Held under its own `RwLock` so callers can
    /// flip `Pty → Locked` (user explicit rename) and have new PTY
    /// events instantly start ignoring the title surface.
    label_source: std::sync::RwLock<LabelSource>,
    /// Broadcast channel for label changes (Phase B). Out-of-band
    /// label sets — from `/cli/sessions/<id>/label` or from the
    /// agent-display-name change hook — push the new label here so
    /// every WS subscriber's select loop wakes and emits a
    /// `LabelChanged` to its client. PTY-driven label updates
    /// don't need this channel since the WS subscriber that's
    /// already processing the `Title` AlacEvent emits
    /// `LabelChanged` directly; the broadcast still fires there
    /// too so OTHER subscribers (multi-window) converge.
    label_tx: broadcast::Sender<String>,
}

impl DaemonPtySession {
    /// Spawn a child process attached to a PTY + Term pair.
    ///
    /// Synchronous to the caller. Internally starts alacritty's IO
    /// thread in the background; that thread lives until the
    /// session is dropped.
    ///
    /// Returns an `Arc<Self>` because typical use has multiple
    /// owners (e.g. `session_map` + the WS handler). Returning Arc
    /// eagerly saves every caller from having to re-wrap.
    pub fn spawn(cfg: DaemonPtyConfig) -> Result<Arc<Self>, io::Error> {
        let cols = cfg.cols.max(1);
        let rows = cfg.rows.max(1);

        let window_size = WindowSize {
            num_cols: cols,
            num_lines: rows,
            // Cell-size hints. The daemon is headless so we don't
            // have real pixel metrics; these fields are only used
            // by programs that query cell pixel size (e.g. Sixel
            // image renderers). 10x20 is a safe stand-in.
            cell_width: 10,
            cell_height: 20,
        };

        // Build alacritty's `tty::Options`. None-shell gets the
        // user's login shell, same as opening a terminal.app window
        // with no command override.
        // We clone args here because we also persist them on the
        // session below — used by smart-launch's "is there a live
        // PTY for this --resume <session_id>?" check.
        let spawn_args = cfg.args.clone();
        let shell = cfg
            .program
            .as_ref()
            .map(|prog| Shell::new(prog.clone(), spawn_args.clone()));

        // Build the env we hand to alacritty's tty::new. Without an
        // explicit TERM/COLORTERM, child processes inherit alacritty's
        // default (TERM=dumb on this version), which makes Claude Code,
        // bash prompts, ls --color, vim — basically every TUI — turn
        // OFF colors. Mirror what `alacritty_backend.rs` does for the
        // legacy renderer (TERM=xterm-256color + COLORTERM=truecolor +
        // TERM_PROGRAM=K2SO) so v2 children render the same colors as
        // legacy children.
        let mut child_env = cfg.env.clone();
        child_env
            .entry("TERM".to_string())
            .or_insert_with(|| "xterm-256color".to_string());
        child_env
            .entry("COLORTERM".to_string())
            .or_insert_with(|| "truecolor".to_string());
        child_env
            .entry("TERM_PROGRAM".to_string())
            .or_insert_with(|| "K2SO".to_string());

        let pty_options = TtyOptions {
            shell,
            working_directory: cfg.cwd.clone(),
            drain_on_exit: cfg.drain_on_exit,
            env: child_env,
            #[cfg(target_os = "windows")]
            escape_args: false,
        };

        // Window ID is used on macOS/Windows to associate the PTY
        // with a specific OS window for controlling-terminal
        // semantics. The daemon has no window, so we pass 0.
        let __t_pty = std::time::Instant::now();
        let pty = tty::new(&pty_options, window_size, 0)?;
        let pty_ms = __t_pty.elapsed().as_secs_f64() * 1000.0;
        log_debug!(
            "[v2-perf] side=daemon stage=pty_open ms={:.3} session={}",
            pty_ms,
            cfg.session_id
        );

        // Event listener + broadcast channel for alacritty's
        // lifecycle events. Subscribers attach lazily via
        // `subscribe_events()`; we keep a clone of the sender here
        // so they can all tap the same stream.
        let (events_tx, _initial_rx) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        // Drop the initial receiver — we don't keep one ourselves;
        // each subscriber calls `subscribe_events()` to get theirs.
        drop(_initial_rx);
        let listener = DaemonEventListener {
            tx: events_tx.clone(),
        };

        // Term config — scrollback + cursor + colors. Start from
        // defaults (which match Zed's behavior) and override only
        // the scrollback depth to our SCROLLBACK_CAP constant.
        let term_config = TermConfig {
            scrolling_history: SCROLLBACK_CAP,
            ..TermConfig::default()
        };

        let term_size = TermSize {
            cols: cols as usize,
            rows: rows as usize,
        };

        let __t_term = std::time::Instant::now();
        let term = Term::new(term_config, &term_size, listener.clone());
        let term_ms = __t_term.elapsed().as_secs_f64() * 1000.0;
        log_debug!(
            "[v2-perf] side=daemon stage=term_new ms={:.3} session={}",
            term_ms,
            cfg.session_id
        );
        let term = Arc::new(FairMutex::new(term));

        // Alacritty's built-in event loop drives the PTY reader +
        // Term feeding + input writer. This replaces the custom
        // reader thread the Kessel-T0 path hand-rolls.
        let __t_loop = std::time::Instant::now();
        let event_loop = EventLoop::new(
            Arc::clone(&term),
            listener,
            pty,
            cfg.drain_on_exit,
            false, // ref_test — only true for alacritty's own test harness
        )?;

        let pty_sender = event_loop.channel();

        // Spawn the IO thread. The handle is `JoinHandle<(EventLoop, State)>`
        // — we intentionally don't store it. When the last `Arc<Self>`
        // drops, `pty_notifier` drops, the EventLoopSender closes,
        // the IO thread sees the Shutdown variant and exits on its own.
        // Not joining means thread cleanup happens implicitly via
        // OS reaping; acceptable for a daemon.
        let _io_thread = event_loop.spawn();
        let event_loop_ms = __t_loop.elapsed().as_secs_f64() * 1000.0;
        log_debug!(
            "[v2-perf] side=daemon stage=event_loop_spawn ms={:.3} session={}",
            event_loop_ms,
            cfg.session_id
        );

        // Phase B: small broadcast for out-of-band label updates.
        // Cap of 16 is plenty — we only push on actual changes and
        // multi-window subscribers consume immediately.
        let (label_tx, _label_rx_drop) = broadcast::channel::<String>(16);
        drop(_label_rx_drop);

        Ok(Arc::new(Self {
            session_id: cfg.session_id,
            cwd: cfg.cwd,
            program: cfg.program,
            args: spawn_args,
            term,
            pty_notifier: Mutex::new(Notifier(pty_sender)),
            events_tx,
            child_exited: std::sync::atomic::AtomicBool::new(false),
            active_subscriber: std::sync::atomic::AtomicU64::new(0),
            label: std::sync::RwLock::new(cfg.label),
            label_source: std::sync::RwLock::new(cfg.label_source),
            label_tx,
        }))
    }

    /// Subscribe to out-of-band label change events. Receivers see
    /// every label set that happened after they subscribed.
    /// Subscribed by the WS handler so multi-window sync works:
    /// when one window sets the label, other windows' subscribers
    /// wake and emit `LabelChanged` to their clients.
    pub fn subscribe_labels(&self) -> broadcast::Receiver<String> {
        self.label_tx.subscribe()
    }

    /// Read the authoritative label. Cheap (RwLock read + clone of
    /// the inner String). Caller gets an owned String — the lock is
    /// released before return.
    pub fn label(&self) -> String {
        self.label.read().expect("label rwlock poisoned").clone()
    }

    /// Read the current label source. Cheap.
    pub fn label_source(&self) -> LabelSource {
        *self.label_source.read().expect("label_source rwlock poisoned")
    }

    /// Update the label from a PTY title event. Honors `label_source`:
    /// `Pty` and `Seed` allow the update; `Locked` ignores it.
    /// Broadcasts on `label_tx` (so every WS subscriber, not just
    /// the one whose AlacEvent loop received the Title, emits
    /// `LabelChanged` to its client). Returns the new label if it
    /// actually changed; returns `None` if locked or a no-op.
    pub fn try_set_label_from_pty(&self, title: String) -> Option<String> {
        let source = self.label_source();
        if matches!(source, LabelSource::Locked) {
            return None;
        }
        let mut guard = self.label.write().expect("label rwlock poisoned");
        if *guard == title {
            return None;
        }
        *guard = title.clone();
        drop(guard);
        // Broadcast so multi-window peers converge. Best-effort —
        // `send` returns `Err` only when there are zero subscribers.
        let _ = self.label_tx.send(title.clone());
        Some(title)
    }

    /// Explicit caller-driven label set. Used by the new
    /// `/cli/sessions/<id>/label` endpoint and any other path where
    /// a human or another process has decided what the label should
    /// be. `lock=true` flips `label_source` to `Locked` so future
    /// PTY title events can't undo this. Always succeeds, returns
    /// the new label. Broadcasts on `label_tx` so every WS
    /// subscriber emits `LabelChanged` to its client (multi-window
    /// convergence).
    pub fn set_label(&self, label: String, lock: bool) -> String {
        {
            let mut guard = self.label.write().expect("label rwlock poisoned");
            *guard = label.clone();
        }
        if lock {
            *self
                .label_source
                .write()
                .expect("label_source rwlock poisoned") = LabelSource::Locked;
        }
        // Best-effort broadcast — `send` returns `Err` only when
        // there are zero subscribers, which is fine.
        let _ = self.label_tx.send(label.clone());
        label
    }

    /// Whether the child PID is still alive. Returns `false` once
    /// the alacritty `ChildExit` event has been observed and
    /// `mark_child_exited` was called (typically from the daemon's
    /// child-exit observer task). Used by the spawn helper's
    /// idempotency check and the `agents running` reaping pass to
    /// recognize stale entries before reporting them as live.
    pub fn is_child_alive(&self) -> bool {
        !self.child_exited.load(std::sync::atomic::Ordering::Acquire)
    }

    /// Flip the `child_exited` flag. Called by the child-exit
    /// observer in the daemon (`v2_spawn::spawn_child_exit_observer`)
    /// when an `AlacEvent::ChildExit` is received. Idempotent —
    /// repeated calls are no-ops.
    pub fn mark_child_exited(&self) {
        self.child_exited
            .store(true, std::sync::atomic::Ordering::Release);
    }

    /// Write input bytes to the child's stdin. Used for user
    /// keystrokes AND heartbeat-injected signals. Non-blocking.
    pub fn write(&self, bytes: impl Into<Cow<'static, [u8]>>) {
        self.pty_notifier.lock().notify(bytes);
    }

    /// Resize the PTY (which SIGWINCHes the child) and the local
    /// Term grid. Idempotent if called with the same dimensions.
    ///
    /// 0.38.0 commits 8 + 9 — discard the visible grid *before*
    /// reflowing so full-screen TUIs that do in-place SIGWINCH redraws
    /// (notably Claude) don't leave stale chrome above the new render.
    ///
    /// Subtle: we use `goto(0,0) + ClearMode::Below` instead of the
    /// more obvious `ClearMode::All`. In non-alt-screen mode (Claude's
    /// TUI is not alt-screen), `ClearMode::All` calls
    /// `grid.clear_viewport()` which **scrolls the visible content
    /// INTO scrollback** before clearing — exactly the unwanted
    /// "every resize appends a copy of the prompt to history"
    /// behavior we just fixed in commit 9. `ClearMode::Below` uses
    /// `grid.reset_region` which discards the cells outright. The
    /// TUI's SIGWINCH-triggered repaint then fills the clean canvas.
    /// Scrollback above the viewport remains untouched: real history
    /// (heartbeat summaries, conversation, acknowledgements) is
    /// preserved and scrollable as before.
    pub fn resize(&self, cols: u16, rows: u16) {
        use alacritty_terminal::vte::ansi::{ClearMode, Handler};

        let cols = cols.max(1);
        let rows = rows.max(1);

        // SIGWINCH the PTY. The child process will re-query
        // TIOCGWINSZ and repaint for the new dimensions.
        self.pty_notifier.lock().on_resize(WindowSize {
            num_cols: cols,
            num_lines: rows,
            cell_width: 10,
            cell_height: 20,
        });

        let mut term = self.term.lock();

        // 0.38.0 commit 8/9 + 0.38.1 fix — only clear when dimensions
        // actually change. Previously we cleared unconditionally,
        // which was fine on real resizes but BROKE same-size resize
        // calls (menu opens, focus transitions, observer noise):
        // kernel doesn't SIGWINCH for a same-size resize → TUI
        // doesn't redraw → we cleared the grid → user sees a black
        // screen until the next interaction triggers Claude to
        // repaint. Gating the clear on a real dimension change
        // preserves the no-duplication win for true resizes while
        // making same-size calls a true no-op.
        let dims_changed = (term.columns() as u16) != cols
            || (term.screen_lines() as u16) != rows;

        if dims_changed {
            // Discard-then-reshape inside the same lock so the grid
            // never observes pre-clear stale chrome at new dims.
            term.goto(0, 0);
            term.clear_screen(ClearMode::Below);
            term.resize(TermSize {
                cols: cols as usize,
                rows: rows as usize,
            });
        }
    }

    /// Handle to the daemon-side alacritty Term. Locked briefly
    /// by the WS handler to serialize grid state into
    /// `TermGridSnapshot` / `TermGridDelta` payloads.
    pub fn term(&self) -> Arc<FairMutex<Term<DaemonEventListener>>> {
        Arc::clone(&self.term)
    }

    /// Subscribe to this session's alacritty event broadcast.
    /// Each call returns a fresh `Receiver`; multiple subscribers
    /// can coexist (though v2 is single-subscriber in practice).
    ///
    /// Why broadcast rather than an owned mpsc: remount scenarios
    /// (workspace swap, Tauri window reload) unmount the old
    /// subscriber while the next one is already connecting. A
    /// take-once receiver loses the race on those transitions;
    /// broadcast avoids the ownership handoff entirely.
    ///
    /// A subscriber that lags beyond the channel capacity gets
    /// `RecvError::Lagged(n)` and can either skip ahead or
    /// disconnect. Consumers should treat Wakeup as idempotent —
    /// missing one just means the next one produces an emit that
    /// covers the accumulated damage.
    pub fn subscribe_events(&self) -> broadcast::Receiver<AlacEvent> {
        self.events_tx.subscribe()
    }
}

// No explicit `Drop` impl: dropping `pty_notifier` closes the
// event-loop channel, the IO thread sees that and exits, and the
// OS reaps the thread. We don't need to join synchronously.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daemon_pty_config_default_is_80x24() {
        let cfg = DaemonPtyConfig::default();
        assert_eq!(cfg.cols, 80);
        assert_eq!(cfg.rows, 24);
        assert!(cfg.program.is_none());
        assert!(cfg.cwd.is_none());
        assert!(cfg.drain_on_exit);
    }

    #[test]
    fn daemon_pty_config_default_label_state() {
        // Phase B: defaults are empty label + Pty source. PTY title
        // events fill the label.
        let cfg = DaemonPtyConfig::default();
        assert_eq!(cfg.label, "");
        assert_eq!(cfg.label_source, LabelSource::Pty);
    }

    #[test]
    fn term_size_dimensions_include_scrollback() {
        let size = TermSize {
            cols: 120,
            rows: 40,
        };
        assert_eq!(size.columns(), 120);
        assert_eq!(size.screen_lines(), 40);
        assert_eq!(size.total_lines(), 40 + SCROLLBACK_CAP);
    }

    // Phase B label-state-machine tests. These exercise the
    // `try_set_label_from_pty` / `set_label` / accessor surface
    // without spawning a real PTY — we hand-construct a session
    // and verify the state-machine logic in isolation. Requires a
    // tokio runtime because `broadcast::channel` lives inside a
    // tokio module, and we need a Term + EventLoop to satisfy the
    // struct layout. We use the simplest valid spawn (a `cat`
    // subprocess on Unix) under #[cfg(unix)] — the test exits
    // when the Arc drops at scope end.

    #[cfg(unix)]
    #[test]
    fn label_starts_with_seed_and_source() {
        use std::path::PathBuf;
        let cfg = DaemonPtyConfig {
            session_id: SessionId::new(),
            cols: 80,
            rows: 24,
            cwd: Some(PathBuf::from("/tmp")),
            program: Some("cat".to_string()),
            args: vec![],
            env: Default::default(),
            drain_on_exit: true,
            label: "scout".to_string(),
            label_source: LabelSource::Locked,
        };
        let s = DaemonPtySession::spawn(cfg).expect("spawn cat");
        assert_eq!(s.label(), "scout");
        assert_eq!(s.label_source(), LabelSource::Locked);
    }

    #[cfg(unix)]
    #[test]
    fn try_set_label_from_pty_drops_locked() {
        use std::path::PathBuf;
        let cfg = DaemonPtyConfig {
            session_id: SessionId::new(),
            cols: 80,
            rows: 24,
            cwd: Some(PathBuf::from("/tmp")),
            program: Some("cat".to_string()),
            args: vec![],
            env: Default::default(),
            drain_on_exit: true,
            label: "scout".to_string(),
            label_source: LabelSource::Locked,
        };
        let s = DaemonPtySession::spawn(cfg).expect("spawn cat");
        // PTY would emit "Claude Code" — must be silently dropped.
        let result = s.try_set_label_from_pty("Claude Code".to_string());
        assert!(result.is_none(), "Locked label must reject PTY update");
        assert_eq!(s.label(), "scout", "label must not have changed");
    }

    #[cfg(unix)]
    #[test]
    fn try_set_label_from_pty_accepts_pty_source() {
        use std::path::PathBuf;
        let cfg = DaemonPtyConfig {
            session_id: SessionId::new(),
            cols: 80,
            rows: 24,
            cwd: Some(PathBuf::from("/tmp")),
            program: Some("cat".to_string()),
            args: vec![],
            env: Default::default(),
            drain_on_exit: true,
            label: String::new(),
            label_source: LabelSource::Pty,
        };
        let s = DaemonPtySession::spawn(cfg).expect("spawn cat");
        let r1 = s.try_set_label_from_pty("vim README.md".to_string());
        assert_eq!(r1.as_deref(), Some("vim README.md"));
        assert_eq!(s.label(), "vim README.md");
        // Same value again is a no-op (returns None).
        let r2 = s.try_set_label_from_pty("vim README.md".to_string());
        assert!(r2.is_none(), "no-op rewrite must return None");
    }

    #[cfg(unix)]
    #[test]
    fn set_label_with_lock_locks_source() {
        use std::path::PathBuf;
        let cfg = DaemonPtyConfig {
            session_id: SessionId::new(),
            cols: 80,
            rows: 24,
            cwd: Some(PathBuf::from("/tmp")),
            program: Some("cat".to_string()),
            args: vec![],
            env: Default::default(),
            drain_on_exit: true,
            ..Default::default()
        };
        let s = DaemonPtySession::spawn(cfg).expect("spawn cat");
        assert_eq!(s.label_source(), LabelSource::Pty);
        s.set_label("user-named".to_string(), true);
        assert_eq!(s.label(), "user-named");
        assert_eq!(s.label_source(), LabelSource::Locked);
        // Subsequent PTY update must be ignored.
        let r = s.try_set_label_from_pty("Claude Code".to_string());
        assert!(r.is_none());
        assert_eq!(s.label(), "user-named");
    }

    #[cfg(unix)]
    #[test]
    fn set_label_without_lock_keeps_source() {
        use std::path::PathBuf;
        let cfg = DaemonPtyConfig {
            session_id: SessionId::new(),
            cols: 80,
            rows: 24,
            cwd: Some(PathBuf::from("/tmp")),
            program: Some("cat".to_string()),
            args: vec![],
            env: Default::default(),
            drain_on_exit: true,
            label: "seed".to_string(),
            label_source: LabelSource::Seed,
        };
        let s = DaemonPtySession::spawn(cfg).expect("spawn cat");
        s.set_label("new-seed".to_string(), false);
        assert_eq!(s.label(), "new-seed");
        // Source preserved — PTY still allowed to update.
        assert_eq!(s.label_source(), LabelSource::Seed);
        let r = s.try_set_label_from_pty("vim".to_string());
        assert_eq!(r.as_deref(), Some("vim"));
    }
}

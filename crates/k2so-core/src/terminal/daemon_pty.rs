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

        Ok(Arc::new(Self {
            session_id: cfg.session_id,
            cwd: cfg.cwd,
            program: cfg.program,
            args: spawn_args,
            term,
            pty_notifier: Mutex::new(Notifier(pty_sender)),
            events_tx,
        }))
    }

    /// Write input bytes to the child's stdin. Used for user
    /// keystrokes AND heartbeat-injected signals. Non-blocking.
    pub fn write(&self, bytes: impl Into<Cow<'static, [u8]>>) {
        self.pty_notifier.lock().notify(bytes);
    }

    /// Resize the PTY (which SIGWINCHes the child) and the local
    /// Term grid. Idempotent if called with the same dimensions.
    pub fn resize(&self, cols: u16, rows: u16) {
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

        // Reshape the Term grid to match. alacritty's resize
        // handles scrollback reflow + cursor reposition naturally.
        self.term.lock().resize(TermSize {
            cols: cols as usize,
            rows: rows as usize,
        });
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

    /// Render the last `count` non-empty lines of the alacritty grid,
    /// walking scrollback history first then the visible screen. Used
    /// by `/cli/terminal/read` so v2-spawned sessions (companion
    /// background spawns, agent launches) are readable the same way
    /// legacy `SessionStreamSession`s are via the replay ring.
    ///
    /// Returns trimmed-trailing-whitespace strings, one per row.
    /// Trailing empty rows are dropped so the tail reflects actual
    /// output rather than blank cells under the cursor.
    pub fn read_lines(&self, count: usize) -> Vec<String> {
        use alacritty_terminal::index::{Column, Line};

        let term = self.term.lock();
        let grid = term.grid();
        let history = grid.history_size();
        let screen_lines = grid.screen_lines();
        let cols = grid.columns();
        let total = history + screen_lines;

        let mut lines: Vec<String> = Vec::with_capacity(total);
        for i in 0..total {
            let line_idx = i as i32 - history as i32;
            let row = &grid[Line(line_idx)];
            let mut text = String::with_capacity(cols);
            for col in 0..cols {
                text.push(row[Column(col)].c);
            }
            lines.push(text.trim_end().to_string());
        }

        // Drop trailing blanks — the visible region beneath the
        // cursor is empty cells that should not count toward `count`.
        while lines.last().map_or(false, |l| l.is_empty()) {
            lines.pop();
        }

        let start = lines.len().saturating_sub(count);
        lines.split_off(start)
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
    fn term_size_dimensions_include_scrollback() {
        let size = TermSize {
            cols: 120,
            rows: 40,
        };
        assert_eq!(size.columns(), 120);
        assert_eq!(size.screen_lines(), 40);
        assert_eq!(size.total_lines(), 40 + SCROLLBACK_CAP);
    }

    // Note: a real end-to-end spawn test requires a tokio runtime
    // (for the mpsc receiver) plus a forked shell. Deferred to the
    // A3 integration tests where the WS handler exercises the full
    // pipeline.
}

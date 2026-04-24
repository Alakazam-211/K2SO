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
use tokio::sync::mpsc;

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

/// Minimal `EventListener` that forwards every alacritty lifecycle
/// event to a tokio unbounded channel. Consumers (A3's WS handler)
/// watch the `Wakeup` variant to know when the Term has damage
/// worth serializing into a delta.
///
/// `send_event` is invoked by alacritty's IO thread, which is a
/// plain `std::thread` (not a tokio context). `mpsc::UnboundedSender::send`
/// is thread-safe and non-blocking, so cross-context use is fine.
#[derive(Clone)]
pub struct DaemonEventListener {
    tx: mpsc::UnboundedSender<AlacEvent>,
}

impl EventListener for DaemonEventListener {
    fn send_event(&self, event: AlacEvent) {
        // Fire-and-forget. If the receiver was dropped (consumer
        // went away), the send fails silently — we're in shutdown
        // and don't care.
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

    /// Event receiver for alacritty lifecycle events (Wakeup,
    /// Title, Bell, ChildExit, etc.). Taken exactly once by the
    /// A3 WS handler; subsequent calls to `take_events()` return
    /// `None`.
    events_rx: Mutex<Option<mpsc::UnboundedReceiver<AlacEvent>>>,
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
        let shell = cfg
            .program
            .as_ref()
            .map(|prog| Shell::new(prog.clone(), cfg.args.clone()));

        let pty_options = TtyOptions {
            shell,
            working_directory: cfg.cwd.clone(),
            drain_on_exit: cfg.drain_on_exit,
            env: cfg.env.clone(),
            #[cfg(target_os = "windows")]
            escape_args: false,
        };

        // Window ID is used on macOS/Windows to associate the PTY
        // with a specific OS window for controlling-terminal
        // semantics. The daemon has no window, so we pass 0.
        let pty = tty::new(&pty_options, window_size, 0)?;

        // Event listener + channel for alacritty's lifecycle events.
        let (events_tx, events_rx) = mpsc::unbounded_channel();
        let listener = DaemonEventListener { tx: events_tx };

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

        let term = Term::new(term_config, &term_size, listener.clone());
        let term = Arc::new(FairMutex::new(term));

        // Alacritty's built-in event loop drives the PTY reader +
        // Term feeding + input writer. This replaces the custom
        // reader thread the Kessel-T0 path hand-rolls.
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

        Ok(Arc::new(Self {
            session_id: cfg.session_id,
            cwd: cfg.cwd,
            program: cfg.program,
            term,
            pty_notifier: Mutex::new(Notifier(pty_sender)),
            events_rx: Mutex::new(Some(events_rx)),
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

    /// Take ownership of the event receiver. Returns `None` if
    /// already taken. The single consumer is A3's WS handler —
    /// it loops on this receiver to know when to serialize a
    /// delta (specifically on `AlacEvent::Wakeup`).
    pub fn take_events(&self) -> Option<mpsc::UnboundedReceiver<AlacEvent>> {
        self.events_rx.lock().take()
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

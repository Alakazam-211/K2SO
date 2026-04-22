//! Session Stream PTY reader (D3a — Phase 2).
//!
//! Replaces alacritty's `EventLoop::spawn()` with a reader thread we
//! own. Bytes read from the PTY master are driven byte-by-byte
//! through `vte::ansi::Processor::advance(&mut term, b)` — which
//! invokes the `vte::ansi::Handler` trait that alacritty's `Term`
//! implements, updating the grid exactly as alacritty's own
//! EventLoop would.
//!
//! D3b adds a second fork in the reader loop that also feeds
//! `LineMux` and publishes Frames to the per-session entry in
//! `session::registry`. This module is structured so the D3b diff
//! is additive — just add two function calls inside the reader
//! loop, no architectural changes.
//!
//! This module is the *proof of the Phase 2 invariant*: LineMux
//! in Phase 5 will see the exact same byte stream Term sees here,
//! because they both come from the same `reader.read(&mut buf)`
//! in the same loop. No post-parse reconstruction, no grid
//! round-tripping. The byte stream is load-bearing.
//!
//! Platform: Unix only (macOS + Linux). Windows lands post-0.34.0
//! alongside the broader portable-pty Windows story.

use std::io::Read;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use alacritty_terminal::event::{Event as AlacEvent, EventListener, WindowSize};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::Config as TermConfig;
use alacritty_terminal::Term;
use parking_lot::Mutex;
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use vte::ansi::{Processor, StdSyncHandler};

use crate::awareness::ingress;
use crate::log_debug;
use crate::session::{registry, Frame, SessionEntry, SessionId};
use crate::term::LineMux;

/// Minimal listener used by the Term we drive here. Phase 2 doesn't
/// need the Title/Bell event forwarding the legacy `K2SOListener`
/// does — no Tauri UI is consuming these events for Session Stream
/// sessions yet. Phase 3 or Phase 4 wires up real event sinks.
///
/// Exposed publicly because downstream code that inspects a
/// session's `Term<NoopListener>` grid (tests, future consumers)
/// needs to name the concrete type.
#[derive(Clone)]
pub struct NoopListener;

impl EventListener for NoopListener {
    fn send_event(&self, _event: AlacEvent) {
        // No-op. Phase 3/4 will replace with a sink that forwards
        // Title / Bell / ChildExit into the Session Stream as
        // SemanticEvent frames.
    }
}

/// Dimensions wrapper — same shape alacritty_backend uses. Scrollback
/// here matches the standard backend so existing behavior transfers.
struct TermSize {
    cols: usize,
    rows: usize,
}

impl Dimensions for TermSize {
    fn total_lines(&self) -> usize {
        self.rows + 5000
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

/// Parameters for `spawn_session_stream`. Kept in a struct so adding
/// fields in D3b / D4 (e.g. env overrides, pty name) doesn't break
/// callers.
#[derive(Debug)]
pub struct SpawnConfig {
    pub session_id: SessionId,
    pub cwd: String,
    /// Shell command to run. `None` → launch the user's default
    /// shell interactively; `Some(cmd)` → `sh -ilc <cmd <args>>`
    /// like the standard backend does.
    pub command: Option<String>,
    pub args: Option<Vec<String>>,
    pub cols: u16,
    pub rows: u16,
    /// If true, the reader thread dual-parses every PTY chunk through
    /// both alacritty's `Term` and `LineMux`. Production Kessel uses
    /// only LineMux (the Kessel renderer receives Frames, not Term
    /// grid snapshots), so this is `false` by default — halves the
    /// reader's CPU cost on the hot path and lets the PTY drain
    /// faster, which in turn lets children's stdout writes stop
    /// blocking on PTY-full back-pressure. Tests that assert on
    /// `session.term` grid state set this to `true`.
    pub track_alacritty_term: bool,
}

impl Default for SpawnConfig {
    fn default() -> Self {
        Self {
            session_id: SessionId::new(),
            cwd: String::new(),
            command: None,
            args: None,
            cols: 80,
            rows: 24,
            track_alacritty_term: false,
        }
    }
}

/// Owner-side handle to a live session_stream session. Drop the
/// handle → child is killed, reader thread joins, PTY cleaned up.
pub struct SessionStreamSession {
    pub session_id: SessionId,
    /// The alacritty grid. Consumers lock this to inspect cells —
    /// same `Term` type the standard backend uses, so reflow /
    /// snapshot helpers in `terminal::*` work unchanged.
    pub term: Arc<FairMutex<Term<NoopListener>>>,
    /// Resolved cwd the child was spawned in. Stored so the
    /// daemon's `/cli/agents/running` listing can surface it
    /// (Phase 4 H2). Matches `SpawnConfig::cwd` after the
    /// `resolve_cwd` pass in `spawn_session_stream`.
    pub cwd: String,
    /// Top-level command the child is running, if the caller
    /// supplied one. `None` = default interactive shell (same
    /// semantics as the legacy `/cli/agents/running` endpoint).
    pub command: Option<String>,
    /// Write side — clones of this are handed to callers that want
    /// to send input to the child (e.g. typed keystrokes).
    writer: Arc<Mutex<Box<dyn std::io::Write + Send>>>,
    /// The portable-pty child. Kept for kill / wait.
    child: Arc<Mutex<Box<dyn Child + Send + Sync>>>,
    /// Reader thread join handle. `take()` on drop so shutdown waits
    /// for clean exit.
    reader_handle: Option<JoinHandle<()>>,
    /// Kept to keep the PTY master alive until shutdown — dropping
    /// the master sends SIGHUP to the child.
    _master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
}

impl SessionStreamSession {
    /// Write bytes to the child's stdin.
    pub fn write(&self, bytes: &[u8]) -> std::io::Result<()> {
        self.writer.lock().write_all(bytes)
    }

    /// Resize the underlying PTY + the alacritty Term.
    pub fn resize(&self, cols: u16, rows: u16) -> Result<(), String> {
        self.term.lock().resize(TermSize {
            cols: cols as usize,
            rows: rows as usize,
        });
        let master = self._master.lock();
        master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| format!("pty resize failed: {e}"))
    }

    /// Block (with timeout) until the child exits. Returns `true`
    /// if exited, `false` if timed out. Used by tests that run a
    /// one-shot command like `echo hello`.
    pub fn wait_for_exit(&self, timeout: Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            {
                let mut child = self.child.lock();
                if let Ok(Some(_)) = child.try_wait() {
                    return true;
                }
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    /// Block (with timeout) until the reader thread has drained
    /// the PTY — i.e. it's joined. Distinct from `wait_for_exit`
    /// because a child can exit before the reader has processed
    /// its last bytes.
    pub fn wait_for_reader_drain(&mut self, timeout: Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        while self
            .reader_handle
            .as_ref()
            .map_or(false, |h| !h.is_finished())
        {
            if std::time::Instant::now() >= deadline {
                return false;
            }
            thread::sleep(Duration::from_millis(10));
        }
        if let Some(handle) = self.reader_handle.take() {
            let _ = handle.join();
        }
        true
    }

    /// Force-kill the child. Safe to call multiple times.
    pub fn kill(&self) -> Result<(), String> {
        self.child
            .lock()
            .kill()
            .map_err(|e| format!("child kill failed: {e}"))
    }
}

impl Drop for SessionStreamSession {
    fn drop(&mut self) {
        // Kill the child first — reader sees EOF on next read,
        // exits cleanly.
        let _ = self.child.lock().kill();
        if let Some(handle) = self.reader_handle.take() {
            let _ = handle.join();
        }
    }
}

/// Spawn a new Session Stream session. Returns the owner-side
/// handle; the reader thread is already running by the time this
/// returns, driving `Term` from PTY bytes.
///
/// Failure modes:
///   - PTY open fails (OS resource exhaustion) → Err with cause
///   - Child spawn fails (bad command / cwd) → Err with cause
///
/// On success, the caller owns the `SessionStreamSession` handle.
/// Drop → child SIGHUPed, reader joined.
pub fn spawn_session_stream(cfg: SpawnConfig) -> Result<SessionStreamSession, String> {
    let SpawnConfig {
        session_id,
        cwd,
        command,
        args,
        cols,
        rows,
        track_alacritty_term,
    } = cfg;

    let cols = cols.max(2);
    let rows = rows.max(1);

    // Resolve cwd the same way the standard backend does.
    let safe_cwd = crate::terminal::resolve_cwd(&cwd);
    let shell = crate::terminal::detect_shell();
    // Clone before we move `command` into the command-builder
    // branch below — the owning SessionStreamSession holds a copy
    // so `/cli/agents/running` can surface what's actually running
    // in the PTY without peeking at the live process (which
    // changes as the user `exec`s or starts sub-shells).
    let command_for_handle = command.clone();

    // Open PTY.
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| format!("openpty failed: {e}"))?;

    // Build child command. `-ilc` takes a shell COMMAND STRING
    // (a script), not a literal program word. Passing
    // `shell_escape(user_command)` used to wrap multi-word commands
    // in single quotes, which made the outer shell parse the
    // escaped form as a ONE-WORD command name — e.g. spawning
    // `sleep 300` via `--command 'sleep 300'` turned into
    // `zsh -ilc "'sleep 300'"` → `command not found: sleep 300`.
    //
    // The correct treatment: `user_command` is already a shell
    // command string; hand it through verbatim. Individual `args`
    // entries, on the other hand, ARE literal arguments and still
    // need escaping before appending.
    let mut cmd = if let Some(user_command) = command {
        let mut shell_cmd = user_command;
        if let Some(user_args) = args {
            for a in user_args {
                shell_cmd.push(' ');
                shell_cmd.push_str(&shell_escape(&a));
            }
        }
        let mut c = CommandBuilder::new(&shell);
        c.arg("-ilc");
        c.arg(&shell_cmd);
        c
    } else {
        // No command = bare interactive login shell. Pass `-il`
        // explicitly so zsh/bash activate their line editor (ZLE /
        // readline) and put the PTY into raw mode. Without these
        // flags, even when the shell sees a TTY, some shell configs
        // skip line-editor initialization — which leaves the PTY in
        // canonical mode (`icanon` + `echo`), and every visible line
        // editing operation (Backspace, Up-arrow history replace,
        // Ctrl+W word-erase) either no-ops or produces junk output
        // because the kernel handles editing but can't render back.
        //
        // Alacritty's backend does the same thing (bare shell + no
        // args) and works fine there because some of the env vars it
        // sets trip zsh into "I am definitely interactive" mode.
        // Rather than chase that env interaction, be explicit: -i
        // turns the interactive flag on unconditionally, -l makes it
        // a login shell so ~/.zprofile loads (matches Alacritty).
        let mut c = CommandBuilder::new(&shell);
        c.arg("-il");
        c
    };
    cmd.cwd(&safe_cwd);
    cmd.env("TERM", "xterm-256color");
    cmd.env("TERM_PROGRAM", "K2SO");
    cmd.env("COLORTERM", "truecolor");
    cmd.env("PROMPT_EOL_MARK", "");
    // Session-aware env so inner tools can discover their session id
    // (Phase 3+ hooks read this to route signals back through the bus).
    cmd.env("K2SO_SESSION_ID", session_id.to_string());

    // ── Env parity with alacritty_backend ──────────────────────────
    // Historical note: Kessel's daemon-side spawn used to set only
    // the five vars above. That left users' shell rc scripts running
    // `k2so ...` commands slow or broken because (a) the bundled
    // `cli/` directory wasn't on PATH, (b) K2SO_PROJECT_PATH was
    // unset so CLI commands couldn't locate the workspace, and (c)
    // hook env vars were missing so any agent-lifecycle hook calls
    // would fall through to the slower discovery path (reading
    // heartbeat.port from disk). When a shell's rc runs a slow
    // `k2so register-something` command at every startup, the net
    // effect is 1-2 seconds of added latency per session spawn —
    // exactly the gap Rosson reported vs Alacritty.

    // PATH — prepend the bundled cli/ directory so `k2so` is
    // discoverable. Same search logic alacritty_backend uses so
    // both paths resolve to the same binary. Falls back to passing
    // through the existing PATH unchanged when no cli/ is found.
    if let Ok(exe_path) = std::env::current_exe() {
        let cli_dir = if let Some(macos_dir) = exe_path.parent() {
            // Production: <app>/Contents/MacOS/<bin> → <app>/Contents/Resources/_up_/cli/
            let resources_cli = macos_dir
                .parent()
                .map(|contents| contents.join("Resources").join("_up_").join("cli"));
            if resources_cli.as_ref().map_or(false, |p| p.exists()) {
                resources_cli
            } else {
                // Dev: target/debug/<bin> → ../../cli/
                macos_dir
                    .parent()
                    .and_then(|p| p.parent())
                    .map(|repo| repo.join("cli"))
                    .filter(|p| p.exists())
            }
        } else {
            None
        };
        if let Some(cli_dir) = cli_dir {
            let existing = std::env::var("PATH").unwrap_or_default();
            cmd.env(
                "PATH",
                format!("{}:{existing}", cli_dir.to_string_lossy()),
            );
        }
    }

    // K2SO_PROJECT_PATH — walk up from cwd to find .k2so/ so the CLI
    // in the child process knows which workspace it's operating in.
    // Same 20-level safety walk alacritty_backend uses.
    {
        let mut dir = std::path::PathBuf::from(&safe_cwd);
        let mut found = None;
        for _ in 0..20 {
            if dir.join(".k2so").is_dir() {
                found = Some(dir.to_string_lossy().to_string());
                break;
            }
            if !dir.pop() {
                break;
            }
        }
        cmd.env(
            "K2SO_PROJECT_PATH",
            found.unwrap_or_else(|| safe_cwd.clone()),
        );
    }

    // Agent-hooks server env — only set when the hook server is
    // actually running in this process. In the daemon, hook_config
    // stays at 0/"" (hook server lives in src-tauri), so these are
    // skipped and the CLI falls through to heartbeat.port lookup.
    // In the Tauri process (if session_stream_pty is ever called
    // from there), the hook vars are set properly.
    let hook_port = crate::hook_config::get_port();
    if hook_port > 0 {
        cmd.env("K2SO_PORT", hook_port.to_string());
        cmd.env("K2SO_HOOK_TOKEN", crate::hook_config::get_token());
    }

    // Spawn.
    let child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| format!("spawn failed: {e}"))?;
    // Dropping the slave after spawn is standard — the child's
    // stdin/stdout/stderr are already hooked to the slave's pty
    // node. Keeping the slave open holds an extra reference that
    // prevents SIGHUP on master close.
    drop(pair.slave);

    let mut master = pair.master;
    let reader = master
        .try_clone_reader()
        .map_err(|e| format!("master.try_clone_reader: {e}"))?;
    let writer = master
        .take_writer()
        .map_err(|e| format!("master.take_writer: {e}"))?;

    // Build alacritty Term.
    let term_config = TermConfig {
        scrolling_history: 5000,
        ..TermConfig::default()
    };
    let term_size = TermSize {
        cols: cols as usize,
        rows: rows as usize,
    };
    let term = Term::new(term_config, &term_size, NoopListener);
    let term = Arc::new(FairMutex::new(term));

    // Register in SessionRegistry BEFORE the reader thread starts
    // publishing — prevents a race where the first Frame would be
    // broadcast before any subscriber has a lookup target.
    let entry = registry::register(session_id);

    // Archive writer task — opt-in based on whether we're running
    // inside a tokio runtime. Phase 2 unit tests (sync
    // `std::thread::spawn` context, no runtime) skip archiving;
    // daemon-spawned sessions (`#[tokio::main]` runtime) get it
    // for free. Uses `safe_cwd` as the project root — Phase 4
    // walks up to the real `.k2so/` ancestor.
    if tokio::runtime::Handle::try_current().is_ok() {
        let archive_root = std::path::PathBuf::from(&safe_cwd);
        let _archive_handle = crate::session::archive::spawn(
            session_id,
            Arc::clone(&entry),
            archive_root,
        );
        // Handle intentionally dropped — the task outlives this
        // function and exits when the broadcast sender closes
        // (registry unregister → last Arc drops).
    }

    // Spawn reader thread. The cyclic loop: read → drive Processor
    // against Term AND feed LineMux → publish Frames + route
    // AgentSignal frames through awareness::ingress → repeat.
    // Exits on EOF or Err, then unregisters the session from the
    // registry so stale IDs don't accumulate.
    let term_for_reader = Arc::clone(&term);
    let entry_for_reader = Arc::clone(&entry);
    let id_for_reader = session_id;
    let reader_handle = thread::Builder::new()
        .name(format!("session-stream-pty/{}", session_id))
        .spawn(move || {
            reader_loop(
                reader,
                term_for_reader,
                entry_for_reader,
                id_for_reader,
                track_alacritty_term,
            );
            // Natural shutdown — reader saw EOF or error. Drop the
            // registry entry so a future `list_ids()` doesn't
            // report this ghost session. Holders of Arc<SessionEntry>
            // (including live subscribers) keep their clones and
            // exit their receive loops when the broadcast sender's
            // last strong reference drops.
            let _ = registry::unregister(&id_for_reader);
        })
        .map_err(|e| format!("spawn reader thread: {e}"))?;

    // Wrap the master in an Arc<Mutex> so the session handle holds
    // it without blocking future resizes.
    let master = Arc::new(Mutex::new(master));
    // The implicit _ = WindowSize usage — keep the import alive via
    // a noop call path.
    let _ = WindowSize {
        num_cols: cols,
        num_lines: rows,
        cell_width: 8,
        cell_height: 16,
    };

    let writer = Arc::new(Mutex::new(writer));
    let child = Arc::new(Mutex::new(child));

    log_debug!(
        "[session_stream/pty] Spawned session {} ({}x{}) cwd={}",
        session_id,
        cols,
        rows,
        safe_cwd
    );

    Ok(SessionStreamSession {
        session_id,
        term,
        cwd: safe_cwd,
        command: command_for_handle,
        writer,
        child,
        reader_handle: Some(reader_handle),
        _master: master,
    })
}

/// The reader thread's inner loop. Reads chunks from the PTY
/// master, then in the same byte-batched pass:
///   - Drives `vte::ansi::Processor::advance(&mut term, bytes)` so
///     alacritty's `Term` grid updates exactly as its own EventLoop
///     would — desktop rendering continues to work.
///   - Feeds `LineMux::feed(bytes)` and publishes each emitted
///     `Frame` to the session's `SessionEntry` via its broadcast
///     channel + replay ring.
///
/// Both consumers see the SAME byte stream. This is the invariant
/// that makes Phase 2 testing valid for Phase 5 — when alacritty
/// goes away, deleting the `processor.advance(...)` call is the
/// only structural change; LineMux keeps seeing identical bytes.
fn reader_loop(
    mut reader: Box<dyn Read + Send>,
    term: Arc<FairMutex<Term<NoopListener>>>,
    entry: Arc<SessionEntry>,
    session_id: SessionId,
    track_alacritty_term: bool,
) {
    // `Processor` is generic over a `Timeout` type; the std-std
    // version (`StdSyncHandler`) is the default alacritty itself
    // uses internally. Be explicit so the compiler doesn't demand
    // turbofish annotations.
    let mut processor: Processor<StdSyncHandler> = Processor::new();
    let mut line_mux = LineMux::new();
    // Larger buffer than the original 4KB so each read drains more
    // of the PTY in one syscall — fewer round trips through the
    // reader's processing loop means less time the child can sit on
    // a blocked `write()`. 16KB is still well under any kernel PTY
    // buffer cap (usually 4KB × N) but reduces syscall frequency
    // for heavy bursts (shell rc, Claude UI paint).
    let mut buf = [0u8; 16384];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => {
                // EOF — child closed or master dropped.
                break;
            }
            Ok(n) => {
                let chunk = &buf[..n];
                // Fork A: alacritty Term grid. ONLY when
                // track_alacritty_term is true — production Kessel
                // consumers (SessionStreamView) render from Frames,
                // not from Term grid snapshots. Skipping this
                // double-parse halves the reader's per-chunk CPU
                // cost and lets the PTY drain faster, which in turn
                // removes the back-pressure on the child's stdout
                // writes. Root cause of the 4.6x system-time
                // penalty on `zsh -ilc` shell startup in the
                // Kessel pane.
                if track_alacritty_term {
                    let mut term_guard = term.lock();
                    processor.advance(&mut *term_guard, chunk);
                }
                // Fork B: line-mux + Frame publish. LineMux is
                // thread-local here; no other thread touches it,
                // so no locking needed.
                //
                // Additionally, when line_mux emits an AgentSignal
                // (an APC `k2so:*` escape landed inside the session
                // output), route it through awareness::ingress for
                // bus delivery. The Frame::AgentSignal itself still
                // flows through the session's Frame stream so
                // consumers can audit signals per-session; ingress
                // enriches + delivers to the appropriate egress
                // channels (PTY-inject / wake / inbox / feed).
                for frame in line_mux.feed(chunk) {
                    if let Frame::AgentSignal(ref signal) = frame {
                        let agent = entry.agent_name();
                        let _ = ingress::from_session(
                            session_id,
                            signal.clone(),
                            agent.as_deref(),
                            None,
                        );
                    }
                    entry.publish(frame);
                }
            }
            Err(e) => {
                log_debug!("[session_stream/pty] read error: {}", e);
                break;
            }
        }
    }
}

/// POSIX single-quote shell escape, same semantics as
/// `alacritty_backend::shell_escape_arg`. Duplicated here so this
/// module stays self-contained — Phase 5 deletes the duplicate when
/// alacritty_backend goes away.
fn shell_escape(s: &str) -> String {
    if s.is_empty() {
        return "''".to_string();
    }
    if s.chars()
        .all(|c| c.is_ascii_alphanumeric() || "@%+=:,./_-".contains(c))
    {
        return s.to_string();
    }
    let escaped = s.replace('\'', r#"'\''"#);
    format!("'{escaped}'")
}

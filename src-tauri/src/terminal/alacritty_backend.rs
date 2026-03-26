use std::borrow::Cow;
use std::collections::HashMap;
use std::os::unix::io::AsRawFd;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use alacritty_terminal::event::{Event as AlacEvent, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::Flags as CellFlags;
use alacritty_terminal::term::Config as TermConfig;
use alacritty_terminal::tty;
use alacritty_terminal::Term;
use alacritty_terminal::vte::ansi::{Color as AnsiColor, NamedColor, Rgb};

use base64::Engine;
use tauri::{AppHandle, Emitter};

use super::bitmap_renderer::{self, BitmapBuffer, CellInfo, CursorInfo, CursorShape};
use super::font_renderer::GlyphCache;
use super::grid_types::*;

// ── K2SO Event Listener ──────────────────────────────────────────────────

/// Event listener that forwards alacritty terminal events to a channel
/// for the grid emission thread to process.
#[derive(Clone)]
struct K2SOListener {
    wakeup_tx: mpsc::Sender<()>,
    app_handle: AppHandle,
    id: String,
}

impl EventListener for K2SOListener {
    fn send_event(&self, event: AlacEvent) {
        match event {
            AlacEvent::Wakeup => {
                let _ = self.wakeup_tx.send(());
            }
            AlacEvent::Title(title) => {
                let _ = self.app_handle.emit(
                    &format!("terminal:title:{}", self.id),
                    &title,
                );
            }
            AlacEvent::Bell => {
                let _ = self.app_handle.emit(
                    &format!("terminal:bell:{}", self.id),
                    (),
                );
            }
            AlacEvent::ChildExit(status) => {
                let code = status.code().unwrap_or(-1);
                let _ = self.app_handle.emit(
                    &format!("terminal:exit:{}", self.id),
                    serde_json::json!({ "exitCode": code }),
                );
            }
            AlacEvent::Exit => {
                let _ = self.app_handle.emit(
                    &format!("terminal:exit:{}", self.id),
                    serde_json::json!({ "exitCode": 0 }),
                );
            }
            _ => {}
        }
    }
}

// ── Terminal Size Helper ─────────────────────────────────────────────────

struct TermSize {
    cols: usize,
    rows: usize,
}

impl Dimensions for TermSize {
    fn total_lines(&self) -> usize {
        self.rows + 5000 // scrollback
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

// ── Shell escaping ──────────────────────────────────────────────────────

/// Shell-escape a single argument for use in a `-c` shell string.
/// Wraps in single quotes and escapes any embedded single quotes.
fn shell_escape_arg(arg: &str) -> String {
    if arg.is_empty() {
        return "''".to_string();
    }
    // If the arg contains no special chars, return as-is
    if arg.chars().all(|c| c.is_alphanumeric() || "-_./=:@,%+".contains(c)) {
        return arg.to_string();
    }
    // Wrap in single quotes, escaping any embedded single quotes: ' → '\''
    format!("'{}'", arg.replace('\'', "'\\''"))
}

// ── Default terminal colors (VS Code Dark+) ─────────────────────────────

fn default_color_palette() -> [Option<Rgb>; 269] {
    let mut colors = [None; 269];

    // Named colors (0-15)
    // Standard ANSI colors — matches src/shared/terminal-theme.ts
    colors[NamedColor::Black as usize] = Some(Rgb { r: 0x1e, g: 0x1e, b: 0x1e });
    colors[NamedColor::Red as usize] = Some(Rgb { r: 0xf4, g: 0x47, b: 0x47 });
    colors[NamedColor::Green as usize] = Some(Rgb { r: 0x6a, g: 0x99, b: 0x55 });
    colors[NamedColor::Yellow as usize] = Some(Rgb { r: 0xd7, g: 0xba, b: 0x7d });
    colors[NamedColor::Blue as usize] = Some(Rgb { r: 0x56, g: 0x9c, b: 0xd6 });
    colors[NamedColor::Magenta as usize] = Some(Rgb { r: 0xc5, g: 0x86, b: 0xc0 });
    colors[NamedColor::Cyan as usize] = Some(Rgb { r: 0x9c, g: 0xdc, b: 0xfe });
    colors[NamedColor::White as usize] = Some(Rgb { r: 0xd4, g: 0xd4, b: 0xd4 });
    colors[NamedColor::BrightBlack as usize] = Some(Rgb { r: 0x5a, g: 0x5a, b: 0x5a });
    colors[NamedColor::BrightRed as usize] = Some(Rgb { r: 0xf4, g: 0x47, b: 0x47 });
    colors[NamedColor::BrightGreen as usize] = Some(Rgb { r: 0x6a, g: 0x99, b: 0x55 });
    colors[NamedColor::BrightYellow as usize] = Some(Rgb { r: 0xd7, g: 0xba, b: 0x7d });
    colors[NamedColor::BrightBlue as usize] = Some(Rgb { r: 0x56, g: 0x9c, b: 0xd6 });
    colors[NamedColor::BrightMagenta as usize] = Some(Rgb { r: 0xc5, g: 0x86, b: 0xc0 });
    colors[NamedColor::BrightCyan as usize] = Some(Rgb { r: 0x9c, g: 0xdc, b: 0xfe });
    colors[NamedColor::BrightWhite as usize] = Some(Rgb { r: 0xff, g: 0xff, b: 0xff });

    // Foreground/Background — matches TERMINAL_COLORS in terminal-theme.ts
    colors[NamedColor::Foreground as usize] = Some(Rgb { r: 0xe0, g: 0xe0, b: 0xe0 });
    colors[NamedColor::Background as usize] = Some(Rgb { r: 0x0a, g: 0x0a, b: 0x0a });
    colors[NamedColor::Cursor as usize] = Some(Rgb { r: 0x52, g: 0x8b, b: 0xff });

    // Standard 256-color palette (indices 16-255)
    // Colors 16-231: 6x6x6 color cube
    for r in 0..6u8 {
        for g in 0..6u8 {
            for b in 0..6u8 {
                let idx = 16 + (r as usize * 36) + (g as usize * 6) + b as usize;
                let rv = if r == 0 { 0 } else { 55 + r * 40 };
                let gv = if g == 0 { 0 } else { 55 + g * 40 };
                let bv = if b == 0 { 0 } else { 55 + b * 40 };
                if idx < 269 {
                    colors[idx] = Some(Rgb { r: rv, g: gv, b: bv });
                }
            }
        }
    }
    // Colors 232-255: grayscale ramp
    for i in 0..24u8 {
        let v = 8 + i * 10;
        let idx = 232 + i as usize;
        if idx < 269 {
            colors[idx] = Some(Rgb { r: v, g: v, b: v });
        }
    }

    colors
}

/// Resolve an alacritty Color to an RGB value using the terminal color palette.
fn resolve_color(color: &AnsiColor, palette: &[Option<Rgb>; 269]) -> u32 {
    match color {
        AnsiColor::Spec(rgb) => ((rgb.r as u32) << 16) | ((rgb.g as u32) << 8) | rgb.b as u32,
        AnsiColor::Named(name) => {
            let idx = *name as usize;
            if let Some(rgb) = palette.get(idx).and_then(|c| *c) {
                ((rgb.r as u32) << 16) | ((rgb.g as u32) << 8) | rgb.b as u32
            } else {
                0xcccccc // fallback
            }
        }
        AnsiColor::Indexed(idx) => {
            if let Some(rgb) = palette.get(*idx as usize).and_then(|c| *c) {
                ((rgb.r as u32) << 16) | ((rgb.g as u32) << 8) | rgb.b as u32
            } else {
                0xcccccc // fallback
            }
        }
    }
}

/// Convert cell flags to our compact u8 representation.
fn flags_to_u8(flags: CellFlags) -> u8 {
    let mut out: u8 = 0;
    if flags.contains(CellFlags::BOLD) {
        out |= ATTR_BOLD;
    }
    if flags.contains(CellFlags::ITALIC) {
        out |= ATTR_ITALIC;
    }
    if flags.intersects(CellFlags::UNDERLINE | CellFlags::DOUBLE_UNDERLINE | CellFlags::UNDERCURL) {
        out |= ATTR_UNDERLINE;
    }
    if flags.contains(CellFlags::STRIKEOUT) {
        out |= ATTR_STRIKETHROUGH;
    }
    if flags.contains(CellFlags::INVERSE) {
        out |= ATTR_INVERSE;
    }
    if flags.contains(CellFlags::DIM) {
        out |= ATTR_DIM;
    }
    if flags.contains(CellFlags::HIDDEN) {
        out |= ATTR_HIDDEN;
    }
    if flags.contains(CellFlags::WIDE_CHAR) {
        out |= ATTR_WIDE;
    }
    out
}

// ── Bitmap State ────────────────────────────────────────────────────────

/// Per-terminal rendering state shared between the main thread and emission thread.
pub struct BitmapState {
    pub glyph_cache: GlyphCache,
    pub bitmap: BitmapBuffer,
    pub focused: bool,
    pub cursor_blink_visible: bool,
    pub cached_b64: Option<String>,
    /// When true, the emission loop does a full render (all rows) instead
    /// of relying on damage tracking. Set by scroll() since scroll_display
    /// doesn't reliably produce damage.
    pub force_full_render: bool,
}

// ── Terminal Instance ────────────────────────────────────────────────────

struct AlacrittyTerminalInstance {
    term: Arc<FairMutex<Term<K2SOListener>>>,
    event_loop_sender: EventLoopSender,
    app_handle: AppHandle,
    cwd: String,
    pty_raw_fd: i32,
    child_pid: Option<u32>,
    palette: [Option<Rgb>; 269],
    bitmap_state: Arc<Mutex<BitmapState>>,
    /// Atomic flag for grid emission loop — set by scroll() to force full re-snapshot.
    force_full_render: Arc<std::sync::atomic::AtomicBool>,
    /// Channel to send manual wakeups to the emission loop
    /// (e.g. after scroll_display which doesn't trigger PTY wakeups).
    wakeup_tx: Option<mpsc::Sender<()>>,
    /// Handle for the grid emission thread, joined on kill.
    grid_thread_handle: Option<thread::JoinHandle<()>>,
}

// ── Terminal Manager ─────────────────────────────────────────────────────

pub struct TerminalManager {
    terminals: HashMap<String, AlacrittyTerminalInstance>,
}

impl TerminalManager {
    pub fn new() -> Self {
        Self {
            terminals: HashMap::new(),
        }
    }

    pub fn create(
        &mut self,
        id: String,
        cwd: String,
        command: Option<String>,
        args: Option<Vec<String>>,
        cols: Option<u16>,
        rows: Option<u16>,
        app_handle: AppHandle,
    ) -> Result<(), String> {
        if self.terminals.contains_key(&id) {
            log_debug!("[terminal/alacritty] Terminal {} already exists, skipping creation", id);
            return Ok(());
        }

        let safe_cwd = super::resolve_cwd(&cwd);
        let shell = super::detect_shell();

        let c = cols.unwrap_or(80) as usize;
        let r = rows.unwrap_or(24) as usize;

        // Build terminal config
        let term_config = TermConfig {
            scrolling_history: 5000,
            ..TermConfig::default()
        };

        // Create event listener
        let (wakeup_tx, wakeup_rx) = mpsc::channel();
        // Clone wakeup_tx before it's moved into the listener.
        // Scroll uses this to inject wakeups into the same channel as PTY events.
        let scroll_wakeup_tx = wakeup_tx.clone();
        let listener = K2SOListener {
            wakeup_tx,
            app_handle: app_handle.clone(),
            id: id.clone(),
        };

        let term_size = TermSize { cols: c, rows: r };

        // Create Term
        let term = Term::new(term_config, &term_size, listener.clone());
        let term = Arc::new(FairMutex::new(term));

        // Build PTY options
        let mut pty_options = tty::Options {
            working_directory: Some(std::path::PathBuf::from(&safe_cwd)),
            drain_on_exit: true,
            ..Default::default()
        };

        // Set shell and optional command
        if let Some(ref user_command) = command {
            let mut shell_cmd = shell_escape_arg(user_command);
            if let Some(ref user_args) = args {
                for arg in user_args {
                    shell_cmd.push(' ');
                    shell_cmd.push_str(&shell_escape_arg(arg));
                }
            }
            let full_args = vec!["-ilc".to_string(), shell_cmd];
            pty_options.shell = Some(tty::Shell::new(shell, full_args));
        } else {
            pty_options.shell = Some(tty::Shell::new(shell, vec![]));
        }

        // Environment variables
        pty_options.env.insert("TERM".to_string(), "xterm-256color".to_string());
        pty_options.env.insert("TERM_PROGRAM".to_string(), "K2SO".to_string());
        pty_options.env.insert("COLORTERM".to_string(), "truecolor".to_string());
        pty_options.env.insert("PROMPT_EOL_MARK".to_string(), String::new());

        // Agent lifecycle hook env vars
        let hook_port = crate::agent_hooks::get_port();
        if hook_port > 0 {
            pty_options.env.insert("K2SO_PORT".to_string(), hook_port.to_string());
            pty_options.env.insert("K2SO_PANE_ID".to_string(), id.clone());
            pty_options.env.insert("K2SO_TAB_ID".to_string(), id.clone());
            pty_options.env.insert("K2SO_HOOK_TOKEN".to_string(), crate::agent_hooks::get_token().to_string());
        }

        // K2SO CLI: add cli/ directory to PATH so agents can call `k2so` commands
        // Search order:
        //   1. Bundled resources: K2SO.app/Contents/Resources/cli/ (production)
        //   2. Repo root: ../../cli/ relative to binary (development)
        if let Ok(exe_path) = std::env::current_exe() {
            let cli_dir = if let Some(macos_dir) = exe_path.parent() {
                // Production: K2SO.app/Contents/MacOS/k2so → Contents/Resources/cli/
                let resources_cli = macos_dir.parent()
                    .map(|contents| contents.join("Resources").join("cli"));
                if resources_cli.as_ref().map_or(false, |p| p.exists()) {
                    resources_cli
                } else {
                    // Development: target/debug/k2so → ../../cli/
                    macos_dir.parent().and_then(|p| p.parent())
                        .map(|repo| repo.join("cli"))
                        .filter(|p| p.exists())
                }
            } else {
                None
            };

            if let Some(cli_dir) = cli_dir {
                let existing_path = std::env::var("PATH").unwrap_or_default();
                pty_options.env.insert(
                    "PATH".to_string(),
                    format!("{}:{}", cli_dir.to_string_lossy(), existing_path),
                );
            }
        }

        // Set project path so the CLI knows which workspace it's operating in
        pty_options.env.insert("K2SO_PROJECT_PATH".to_string(), safe_cwd.clone());

        // Strip unwanted env vars
        for (key, _) in std::env::vars() {
            if key.starts_with("ELECTRON_") || key.starts_with("VITE_") || key.starts_with("__vite") {
                // alacritty_terminal env is additive, we can't remove from inherited env here
                // Set them to empty as a workaround
                pty_options.env.insert(key, String::new());
            }
        }

        let window_size = WindowSize {
            num_lines: r as u16,
            num_cols: c as u16,
            cell_width: 8,   // approximate, frontend will send real metrics
            cell_height: 16, // approximate
        };

        // Create PTY via alacritty's tty module
        let pty = tty::new(&pty_options, window_size, 0)
            .map_err(|e| format!("Failed to create PTY: {}", e))?;

        let raw_fd = pty.file().as_raw_fd();
        let child_pid = pty.child().id();

        // Create event loop
        let event_loop = EventLoop::new(
            Arc::clone(&term),
            listener,
            pty,
            true,  // drain_on_exit
            false, // ref_test
        )
        .map_err(|e| format!("Failed to create event loop: {}", e))?;

        let event_loop_sender = event_loop.channel();

        // Spawn the event loop thread (reads PTY, parses VT100, updates Term)
        event_loop.spawn();

        let palette = default_color_palette();

        // Initialize bitmap rendering state
        let font_size = 13.0f32; // matches TERMINAL_FONT_SIZE_DEFAULT
        let dpr = 1.0f32; // updated by frontend on first connect
        let glyph_cache = GlyphCache::new(font_size, dpr);
        let bitmap = BitmapBuffer::new(
            c as u16,
            r as u16,
            glyph_cache.cell_width,
            glyph_cache.cell_height,
        );
        let bitmap_state = Arc::new(Mutex::new(BitmapState {
            glyph_cache,
            bitmap,
            focused: true,
            cursor_blink_visible: true,
            cached_b64: None,
            force_full_render: false,
        }));

        let force_full_render = Arc::new(std::sync::atomic::AtomicBool::new(false));

        // Spawn grid (DOM text) emission thread
        let term_for_grid = Arc::clone(&term);
        let app_for_grid = app_handle.clone();
        let id_for_grid = id.clone();
        let grid_palette = palette;
        let force_full_for_grid = Arc::clone(&force_full_render);

        let grid_thread_handle = thread::spawn(move || {
            grid_emission_loop(
                &id_for_grid,
                &term_for_grid,
                &app_for_grid,
                wakeup_rx,
                grid_palette,
                force_full_for_grid,
            );
        });

        let instance = AlacrittyTerminalInstance {
            term,
            event_loop_sender,
            app_handle,
            cwd: safe_cwd,
            pty_raw_fd: raw_fd,
            child_pid: Some(child_pid),
            palette,
            bitmap_state,
            force_full_render,
            wakeup_tx: Some(scroll_wakeup_tx),
            grid_thread_handle: Some(grid_thread_handle),
        };

        self.terminals.insert(id.clone(), instance);
        log_debug!("[terminal/alacritty] Terminal {} created ({}x{}, pid={})", id, c, r, child_pid);

        Ok(())
    }

    pub fn write(&self, id: &str, data: &str) -> Result<(), String> {
        let instance = self
            .terminals
            .get(id)
            .ok_or_else(|| format!("Terminal {} not found", id))?;

        // Snap to bottom on user input — like iTerm2 / Zed.
        // If the user scrolled up into history, typing should bring
        // the viewport back to the live terminal.
        {
            let mut term = instance.term.lock_unfair();
            let display_offset = term.grid().display_offset();
            if display_offset != 0 {
                term.scroll_display(alacritty_terminal::grid::Scroll::Bottom);
            }
        }

        let bytes = data.as_bytes().to_vec();
        let _ = instance
            .event_loop_sender
            .send(Msg::Input(Cow::Owned(bytes)));

        Ok(())
    }

    pub fn resize(&self, id: &str, cols: u16, rows: u16) -> Result<(), String> {
        let instance = match self.terminals.get(id) {
            Some(i) => i,
            None => return Ok(()),
        };

        let window_size = WindowSize {
            num_lines: rows,
            num_cols: cols,
            cell_width: 8,
            cell_height: 16,
        };

        // Send resize to event loop — it handles both PTY resize and term grid resize.
        // Do NOT also call term.resize() here to avoid racing with the event loop thread.
        let _ = instance
            .event_loop_sender
            .send(Msg::Resize(window_size));

        Ok(())
    }

    pub fn kill(&mut self, id: &str) -> Result<(), String> {
        if let Some(mut instance) = self.terminals.remove(id) {
            // Send shutdown to event loop
            let _ = instance.event_loop_sender.send(Msg::Shutdown);

            // Drop the wakeup channel to unblock the grid emission thread
            instance.wakeup_tx.take();

            // Kill child process
            if let Some(pid) = instance.child_pid {
                #[cfg(unix)]
                unsafe {
                    let pgid = libc::getpgid(pid as i32);
                    if pgid > 0 {
                        libc::killpg(pgid, libc::SIGHUP);
                    } else {
                        libc::kill(pid as i32, libc::SIGHUP);
                    }
                }
                thread::sleep(Duration::from_millis(100));
                #[cfg(unix)]
                unsafe {
                    libc::kill(pid as i32, libc::SIGKILL);
                }

                // Reap the child process to prevent zombies
                #[cfg(unix)]
                unsafe {
                    let mut status: i32 = 0;
                    // WNOHANG: don't block if not yet exited (SIGKILL should handle it)
                    libc::waitpid(pid as i32, &mut status, libc::WNOHANG);
                }
            }

            // Take the grid thread handle before dropping the instance.
            // Dropping instance releases:
            //   - instance.wakeup_tx (already taken above)
            //   - instance.term (Arc refcount -1, but grid thread still holds a clone)
            //   - instance.event_loop_sender (event loop will shut down)
            // Once the event loop stops, no more wakeups are sent. The grid thread's
            // K2SOListener sender (inside Term Arc) is the last wakeup_tx clone.
            // When the grid thread's Term Arc is the sole owner and it drops,
            // recv() returns Disconnected. This is async — we don't wait for it.
            let _grid_handle = instance.grid_thread_handle.take();
            // instance is dropped here — grid thread will exit asynchronously.
        }
        Ok(())
    }

    #[cfg(unix)]
    pub fn kill_foreground(&self, id: &str) -> Result<(), String> {
        let instance = self
            .terminals
            .get(id)
            .ok_or_else(|| format!("Terminal {} not found", id))?;

        let fg_pgid = unsafe { libc::tcgetpgrp(instance.pty_raw_fd) };

        if fg_pgid > 0 {
            unsafe {
                libc::killpg(fg_pgid, libc::SIGINT);
            }
            Ok(())
        } else {
            Err("Could not determine foreground process group".to_string())
        }
    }

    #[cfg(unix)]
    pub fn get_foreground_command(&self, id: &str) -> Result<Option<String>, String> {
        let instance = match self.terminals.get(id) {
            Some(i) => i,
            None => return Ok(None),
        };

        let fg_pgid = unsafe { libc::tcgetpgrp(instance.pty_raw_fd) };
        if fg_pgid <= 0 {
            return Ok(None);
        }

        // If foreground is the shell, return None
        if let Some(pid) = instance.child_pid {
            if fg_pgid == pid as i32 {
                return Ok(None);
            }
        }

        #[cfg(target_os = "macos")]
        {
            let mut buf = [0u8; 4096];
            let ret = unsafe {
                libc::proc_pidpath(
                    fg_pgid,
                    buf.as_mut_ptr() as *mut libc::c_void,
                    buf.len() as u32,
                )
            };
            if ret > 0 {
                let path = String::from_utf8_lossy(&buf[..ret as usize]);
                let name = path.rsplit('/').next().unwrap_or("");
                if !name.is_empty() {
                    return Ok(Some(name.to_string()));
                }
            }
        }

        #[cfg(target_os = "linux")]
        {
            if let Ok(comm) = std::fs::read_to_string(format!("/proc/{}/comm", fg_pgid)) {
                let name = comm.trim();
                if !name.is_empty() {
                    return Ok(Some(name.to_string()));
                }
            }
        }

        Ok(None)
    }

    pub fn exists(&self, id: &str) -> bool {
        self.terminals.contains_key(id)
    }

    pub fn get_buffer(&self, _id: &str) -> Result<String, String> {
        // In alacritty backend, scrollback is managed by the Term.
        // Reattach uses get_grid() instead. Return empty for backward compat.
        Ok(String::new())
    }

    /// Get a full grid snapshot for the terminal.
    pub fn get_grid(&self, id: &str) -> Result<GridUpdate, String> {
        let instance = self
            .terminals
            .get(id)
            .ok_or_else(|| format!("Terminal {} not found", id))?;

        let term = instance.term.lock_unfair();
        Ok(snapshot_grid(&term, &instance.palette, true))
    }

    /// Scroll the terminal display.
    /// Renders a full bitmap directly and emits it — scroll_display doesn't
    /// reliably produce damage that the emission loop can detect.
    /// Scroll the terminal and trigger a re-render via the emission loop.
    /// We set a flag so the emission loop does a FULL render (not damage-based),
    /// since scroll_display doesn't reliably produce damage.
    pub fn scroll(&self, id: &str, delta: i32) -> Result<(), String> {
        let instance = self
            .terminals
            .get(id)
            .ok_or_else(|| format!("Terminal {} not found", id))?;

        {
            let mut term = instance.term.lock_unfair();
            term.scroll_display(alacritty_terminal::grid::Scroll::Delta(delta));
        }

        // Set the force-full-render flag so emission loop does full snapshot
        instance.force_full_render.store(true, std::sync::atomic::Ordering::Relaxed);
        // Also set on bitmap_state for backward compat (if bitmap loop is running)
        instance.bitmap_state.lock().unwrap().force_full_render = true;

        // Send wakeup through the same channel as PTY events
        // This ensures the frame goes through the emission loop → event system,
        // which is the path that reliably triggers WebKit compositing.
        if let Some(ref wakeup_tx) = instance.wakeup_tx {
            let _ = wakeup_tx.send(());
        }

        Ok(())
    }

    /// Get a full bitmap frame for tab-switch reattach.
    pub fn get_frame(&self, id: &str) -> Result<BitmapUpdate, String> {
        let instance = self
            .terminals
            .get(id)
            .ok_or_else(|| format!("Terminal {} not found", id))?;

        let term = instance.term.lock_unfair();
        let mut bstate = instance.bitmap_state.lock().unwrap();
        Ok(render_full_bitmap(&term, &instance.palette, &mut bstate))
    }

    /// Set font size and DPR — invalidates glyph cache, resizes bitmap, returns new cell metrics.
    pub fn set_font_size(&self, id: &str, font_size: f32, dpr: f32) -> Result<(u32, u32), String> {
        let instance = self
            .terminals
            .get(id)
            .ok_or_else(|| format!("Terminal {} not found", id))?;

        let mut bstate = instance.bitmap_state.lock().unwrap();
        bstate.glyph_cache.set_font_size(font_size, dpr);

        let logical_w = bstate.glyph_cache.logical_cell_width();
        let logical_h = bstate.glyph_cache.logical_cell_height();

        // Resize bitmap
        let term = instance.term.lock_unfair();
        let grid = term.grid();
        bstate.bitmap = BitmapBuffer::new(
            grid.columns() as u16,
            grid.screen_lines() as u16,
            bstate.glyph_cache.cell_width,
            bstate.glyph_cache.cell_height,
        );

        Ok((logical_w, logical_h))
    }

    /// Get logical cell metrics for mouse coordinate mapping.
    pub fn get_cell_metrics(&self, id: &str) -> Result<(u32, u32, u16, u16), String> {
        let instance = self
            .terminals
            .get(id)
            .ok_or_else(|| format!("Terminal {} not found", id))?;

        let bstate = instance.bitmap_state.lock().unwrap();
        let term = instance.term.lock_unfair();
        let grid = term.grid();

        Ok((
            bstate.glyph_cache.logical_cell_width(),
            bstate.glyph_cache.logical_cell_height(),
            grid.columns() as u16,
            grid.screen_lines() as u16,
        ))
    }

    /// Set terminal focus state (controls cursor blink).
    pub fn set_focus(&self, id: &str, focused: bool) -> Result<(), String> {
        let instance = self
            .terminals
            .get(id)
            .ok_or_else(|| format!("Terminal {} not found", id))?;

        let mut bstate = instance.bitmap_state.lock().unwrap();
        bstate.focused = focused;
        bstate.cursor_blink_visible = true; // reset blink on focus change

        Ok(())
    }

    /// Get text content from a selection range.
    pub fn get_selection_text(
        &self,
        id: &str,
        start_col: u16,
        start_row: u16,
        end_col: u16,
        end_row: u16,
    ) -> Result<String, String> {
        let instance = self
            .terminals
            .get(id)
            .ok_or_else(|| format!("Terminal {} not found", id))?;

        let term = instance.term.lock_unfair();
        let grid = term.grid();
        let cols = grid.columns();
        let mut text = String::new();

        let (sr, sc, er, ec) = if start_row < end_row || (start_row == end_row && start_col <= end_col) {
            (start_row, start_col, end_row, end_col)
        } else {
            (end_row, end_col, start_row, start_col)
        };

        for row_idx in sr..=er {
            let line = Line(row_idx as i32);
            if row_idx as usize >= grid.screen_lines() {
                break;
            }
            let row = &grid[line];

            let col_start = if row_idx == sr { sc as usize } else { 0 };
            let col_end = if row_idx == er {
                (ec as usize).min(cols - 1)
            } else {
                cols - 1
            };

            for col in col_start..=col_end {
                let cell = &row[Column(col)];
                if !cell.flags.contains(CellFlags::WIDE_CHAR_SPACER) {
                    text.push(cell.c);
                    if let Some(zws) = cell.zerowidth() {
                        for &zw in zws {
                            text.push(zw);
                        }
                    }
                }
            }

            // Add newline between rows (but not after last row)
            if row_idx < er {
                // Trim trailing spaces from line before adding newline
                let trimmed_len = text.trim_end().len();
                text.truncate(trimmed_len);
                text.push('\n');
            }
        }

        // Trim trailing whitespace from final result
        let trimmed = text.trim_end().to_string();
        Ok(trimmed)
    }

    pub fn get_count_for_path(&self, path: &str) -> i32 {
        self.terminals
            .values()
            .filter(|inst| inst.cwd.starts_with(path))
            .count() as i32
    }

    pub fn get_active_count(&self) -> i32 {
        self.terminals.len() as i32
    }

    pub fn kill_all(&mut self) {
        let ids: Vec<String> = self.terminals.keys().cloned().collect();
        for id in ids {
            let _ = self.kill(&id);
        }
    }
}

impl Drop for TerminalManager {
    fn drop(&mut self) {
        self.kill_all();
    }
}

impl Default for TerminalManager {
    fn default() -> Self {
        Self::new()
    }
}

// ── Grid Snapshot ────────────────────────────────────────────────────────

/// Default foreground/background colors (must match terminal-theme.ts TERMINAL_COLORS)
const DEFAULT_FG: u32 = 0xe0e0e0;
const DEFAULT_BG: u32 = 0x0a0a0a;

/// Convert a row of alacritty cells to a compact line (text + sparse style spans).
/// Span indices use "text position" (char index into the text string, not grid column),
/// so they stay aligned even when wide char spacers are skipped.
fn row_to_compact_line(
    row_idx: usize,
    row: &alacritty_terminal::grid::Row<alacritty_terminal::term::cell::Cell>,
    cols: usize,
    palette: &[Option<Rgb>; 269],
) -> CompactLine {
    let mut text = String::with_capacity(cols);
    let mut spans: Vec<StyleSpan> = Vec::new();

    // Current span tracking — positions are TEXT indices (not grid columns)
    let mut span_start: Option<u16> = None;
    let mut span_fg: u32 = DEFAULT_FG;
    let mut span_bg: u32 = DEFAULT_BG;
    let mut span_flags: u8 = 0;
    let mut text_pos: u16 = 0;

    // Track the rightmost styled position to avoid trimming styled trailing spaces
    let mut rightmost_styled_text_pos: Option<u16> = None;

    for col_idx in 0..cols {
        let cell = &row[Column(col_idx)];

        if cell.flags.contains(CellFlags::WIDE_CHAR_SPACER) {
            continue;
        }

        let fg = resolve_color(&cell.fg, palette);
        let bg = resolve_color(&cell.bg, palette);
        let flags = flags_to_u8(cell.flags);

        let current_text_pos = text_pos;
        text.push(cell.c);
        text_pos += 1;
        if let Some(zws) = cell.zerowidth() {
            for &zw in zws {
                text.push(zw);
                // Don't increment text_pos — zero-width chars don't take a cell
            }
        }

        let is_default = fg == DEFAULT_FG && bg == DEFAULT_BG && flags == 0;

        if !is_default {
            rightmost_styled_text_pos = Some(current_text_pos);

            // Extend or start a span
            if let Some(start) = span_start {
                if fg == span_fg && bg == span_bg && flags == span_flags {
                    // Continue current span
                } else {
                    // Flush previous span, start new one
                    spans.push(StyleSpan {
                        s: start,
                        e: current_text_pos.saturating_sub(1),
                        fg: if span_fg != DEFAULT_FG { Some(span_fg) } else { None },
                        bg: if span_bg != DEFAULT_BG { Some(span_bg) } else { None },
                        fl: if span_flags != 0 { Some(span_flags) } else { None },
                    });
                    span_start = Some(current_text_pos);
                    span_fg = fg;
                    span_bg = bg;
                    span_flags = flags;
                }
            } else {
                span_start = Some(current_text_pos);
                span_fg = fg;
                span_bg = bg;
                span_flags = flags;
            }
        } else if let Some(start) = span_start {
            // Flush span — we hit a default cell
            spans.push(StyleSpan {
                s: start,
                e: current_text_pos.saturating_sub(1),
                fg: if span_fg != DEFAULT_FG { Some(span_fg) } else { None },
                bg: if span_bg != DEFAULT_BG { Some(span_bg) } else { None },
                fl: if span_flags != 0 { Some(span_flags) } else { None },
            });
            span_start = None;
        }
    }

    // Flush final span if any
    if let Some(start) = span_start {
        spans.push(StyleSpan {
            s: start,
            e: text_pos.saturating_sub(1),
            fg: if span_fg != DEFAULT_FG { Some(span_fg) } else { None },
            bg: if span_bg != DEFAULT_BG { Some(span_bg) } else { None },
            fl: if span_flags != 0 { Some(span_flags) } else { None },
        });
    }

    // Trim trailing spaces, but only up to the rightmost styled position.
    // This preserves styled spaces (like cursor backgrounds) that would
    // otherwise be lost.
    let trimmed = if let Some(styled_pos) = rightmost_styled_text_pos {
        let min_chars = styled_pos as usize + 1;
        let t = text.trim_end();
        // Compare CHAR count, not byte length — multi-byte chars like ❯ make
        // byte length larger than char count, which would skip the fix.
        let trimmed_chars = t.chars().count();
        if trimmed_chars < min_chars {
            // Keep at least up to the rightmost styled char
            let chars: Vec<char> = text.chars().collect();
            chars[..min_chars.min(chars.len())].iter().collect()
        } else {
            t.to_string()
        }
    } else {
        text.trim_end().to_string()
    };

    // Debug: log rows with background-colored spans (like CLI cursors)
    let has_bg_spans = spans.iter().any(|s| s.bg.is_some());
    if has_bg_spans {
        log_debug!(
            "[compact] row={} text={:?} (len={}) spans={:?} rightmost_styled={:?}",
            row_idx,
            &trimmed,
            trimmed.len(),
            spans.iter().map(|s| format!("s{}..e{} fg={:?} bg={:?} fl={:?}", s.s, s.e, s.fg, s.bg, s.fl)).collect::<Vec<_>>(),
            rightmost_styled_text_pos,
        );
    }

    CompactLine {
        row: row_idx as u16,
        text: trimmed,
        spans,
    }
}

/// Take a full snapshot of the terminal grid using compact line format.
fn snapshot_grid(
    term: &Term<K2SOListener>,
    palette: &[Option<Rgb>; 269],
    full: bool,
) -> GridUpdate {
    let start = std::time::Instant::now();

    let content = term.renderable_content();
    let grid = term.grid();
    let cols = grid.columns();
    let rows = grid.screen_lines();
    let display_offset = content.display_offset;
    let cursor_point = content.cursor.point;

    // Read cursor shape from alacritty (Block, Beam, Underline, Hidden, HollowBlock)
    let (cursor_visible, cursor_shape) = match content.cursor.shape {
        alacritty_terminal::vte::ansi::CursorShape::Block => (true, "block"),
        alacritty_terminal::vte::ansi::CursorShape::Underline => (true, "underline"),
        alacritty_terminal::vte::ansi::CursorShape::Beam => (true, "bar"),
        _ => (true, "block"),
    };
    // Hide cursor when scrolled up or when SHOW_CURSOR is off
    let cursor_visible = cursor_visible
        && display_offset == 0
        && term.mode().contains(alacritty_terminal::term::TermMode::SHOW_CURSOR);

    let mut lines = Vec::with_capacity(rows);

    for row_idx in 0..rows {
        let line = Line(row_idx as i32 - display_offset as i32);
        let row = &grid[line];
        let compact = row_to_compact_line(row_idx, row, cols, palette);

        // Skip entirely empty lines to reduce payload
        if !compact.text.is_empty() || !compact.spans.is_empty() || !full {
            lines.push(compact);
        }
    }

    let elapsed = start.elapsed();
    let text_bytes: u32 = lines.iter().map(|l| l.text.len() as u32).sum();
    let span_count: u16 = lines.iter().map(|l| l.spans.len() as u16).sum();

    GridUpdate {
        cols: cols as u16,
        rows: rows as u16,
        cursor_col: cursor_point.column.0 as u16,
        cursor_row: cursor_point.line.0 as u16,
        cursor_visible,
        cursor_shape: cursor_shape.to_string(),
        lines,
        full,
        mode: term.mode().bits(),
        display_offset,
        selection: None,
        perf: Some(PerfInfo {
            snapshot_us: elapsed.as_micros() as u64,
            line_count: rows as u16,
            text_bytes,
            span_count,
        }),
    }
}

/// Take an incremental snapshot using damage tracking with compact line format.
fn snapshot_damaged(
    term: &mut Term<K2SOListener>,
    palette: &[Option<Rgb>; 269],
) -> GridUpdate {
    let start = std::time::Instant::now();

    // Get cursor info before damage() borrows term
    let content = term.renderable_content();
    let cursor_point = content.cursor.point;
    let display_offset = content.display_offset;
    let (cursor_shape_str, cursor_shape_visible) = match content.cursor.shape {
        alacritty_terminal::vte::ansi::CursorShape::Block => ("block", true),
        alacritty_terminal::vte::ansi::CursorShape::Underline => ("underline", true),
        alacritty_terminal::vte::ansi::CursorShape::Beam => ("bar", true),
        _ => ("block", true),
    };
    let cursor_visible = cursor_shape_visible
        && display_offset == 0
        && term.mode().contains(alacritty_terminal::term::TermMode::SHOW_CURSOR);
    drop(content);

    let grid = term.grid();
    let cols = grid.columns();
    let rows = grid.screen_lines();

    let damage = term.damage();
    let is_full = matches!(damage, alacritty_terminal::term::TermDamage::Full);

    let damaged_lines: Vec<usize> = match damage {
        alacritty_terminal::term::TermDamage::Full => {
            (0..rows).collect()
        }
        alacritty_terminal::term::TermDamage::Partial(iter) => {
            iter.filter(|d| d.is_damaged())
                .map(|d| d.line)
                .collect()
        }
    };

    let grid = term.grid(); // re-borrow after damage()
    let mut lines = Vec::with_capacity(damaged_lines.len());

    for &row_idx in &damaged_lines {
        if row_idx >= rows {
            continue;
        }
        let line = Line(row_idx as i32 - display_offset as i32);
        let row = &grid[line];
        lines.push(row_to_compact_line(row_idx, row, cols, palette));
    }

    term.reset_damage();

    let elapsed = start.elapsed();
    let text_bytes: u32 = lines.iter().map(|l| l.text.len() as u32).sum();
    let span_count: u16 = lines.iter().map(|l| l.spans.len() as u16).sum();
    let line_count = lines.len() as u16;

    GridUpdate {
        cols: cols as u16,
        rows: rows as u16,
        cursor_col: cursor_point.column.0 as u16,
        cursor_row: cursor_point.line.0 as u16,
        cursor_visible,
        cursor_shape: cursor_shape_str.to_string(),
        lines,
        full: is_full,
        mode: term.mode().bits(),
        display_offset,
        selection: None,
        perf: Some(PerfInfo {
            snapshot_us: elapsed.as_micros() as u64,
            line_count,
            text_bytes,
            span_count,
        }),
    }
}

// ── Bitmap Rendering Helpers ─────────────────────────────────────────────

/// Extract cell info from an alacritty row for bitmap rendering.
fn extract_row_cells(
    row: &alacritty_terminal::grid::Row<alacritty_terminal::term::cell::Cell>,
    cols: usize,
    palette: &[Option<Rgb>; 269],
) -> Vec<CellInfo> {
    let mut cells = Vec::with_capacity(cols);
    for col_idx in 0..cols {
        let cell = &row[Column(col_idx)];
        cells.push(CellInfo {
            ch: cell.c,
            fg: resolve_color(&cell.fg, palette),
            bg: resolve_color(&cell.bg, palette),
            flags: flags_to_u8(cell.flags),
        });
    }
    cells
}


/// Render the full terminal grid to bitmap and return a BitmapUpdate.
fn render_full_bitmap(
    term: &Term<K2SOListener>,
    palette: &[Option<Rgb>; 269],
    bstate: &mut BitmapState,
) -> BitmapUpdate {
    let render_start = std::time::Instant::now();

    let grid = term.grid();
    let cols = grid.columns();
    let rows = grid.screen_lines();
    let cursor = grid.cursor.point;
    let cursor_visible = term.mode().contains(alacritty_terminal::term::TermMode::SHOW_CURSOR);

    // Ensure bitmap is correctly sized
    if bstate.bitmap.needs_resize(cols as u16, rows as u16, bstate.glyph_cache.cell_width, bstate.glyph_cache.cell_height) {
        bstate.bitmap = BitmapBuffer::new(
            cols as u16,
            rows as u16,
            bstate.glyph_cache.cell_width,
            bstate.glyph_cache.cell_height,
        );
    }

    // Destructure to satisfy borrow checker
    let BitmapState { ref mut glyph_cache, ref mut bitmap, cursor_blink_visible, .. } = *bstate;

    // Render all rows — offset by display_offset to read scrolled viewport
    let display_offset = grid.display_offset();
    for row_idx in 0..rows {
        let line = Line(row_idx as i32 - display_offset as i32);
        let row = &grid[line];
        let cells = extract_row_cells(row, cols, palette);

        let cursor_info = if cursor_visible && display_offset == 0 && cursor.line.0 == row_idx as i32 && cursor_blink_visible {
            Some(CursorInfo {
                col: cursor.column.0 as u16,
                shape: CursorShape::Bar,
                visible: true,
            })
        } else {
            None
        };

        bitmap_renderer::render_row(
            bitmap,
            row_idx,
            &cells,
            glyph_cache,
            cursor_info.as_ref(),
        );
    }

    let render_elapsed = render_start.elapsed();

    // QOI encode
    let qoi_start = std::time::Instant::now();
    let qoi_bytes = match qoi::encode_to_vec(
        &bitmap.pixels,
        bitmap.width,
        bitmap.height,
    ) {
        Ok(bytes) => bytes,
        Err(e) => {
            log_debug!("[terminal/bitmap] QOI encode failed: {:?} ({}x{})", e, bitmap.width, bitmap.height);
            return BitmapUpdate::default();
        }
    };
    let qoi_elapsed = qoi_start.elapsed();

    // Base64 encode
    let image_b64 = base64::engine::general_purpose::STANDARD.encode(&qoi_bytes);

    BitmapUpdate {
        image_b64,
        width: bitmap.width,
        height: bitmap.height,
        cols: cols as u16,
        rows: rows as u16,
        cell_width: glyph_cache.logical_cell_width(),
        cell_height: glyph_cache.logical_cell_height(),
        cursor_col: cursor.column.0 as u16,
        cursor_row: cursor.line.0 as u16,
        cursor_visible,
        mode: term.mode().bits(),
        display_offset: grid.display_offset(),
        perf: Some(BitmapPerfInfo {
            render_us: render_elapsed.as_micros() as u64,
            qoi_us: qoi_elapsed.as_micros() as u64,
            qoi_bytes: qoi_bytes.len() as u32,
            damaged_rows: rows as u16,
        }),
    }
}

// ── Bitmap Emission Loop ────────────────────────────────────────────────

/// Background thread that renders terminal content to bitmaps and emits
/// them to the frontend. Replaces the old JSON grid emission loop.
///
/// Rate-limited to ~60fps. Renders only damaged rows into an existing
/// RGBA buffer, then QOI-compresses and base64-encodes for IPC.
fn bitmap_emission_loop(
    id: &str,
    term: &Arc<FairMutex<Term<K2SOListener>>>,
    app_handle: &AppHandle,
    wakeup_rx: mpsc::Receiver<()>,
    _manual_wakeup_rx: mpsc::Receiver<()>,  // unused — scroll uses same channel via K2SOListener's tx
    palette: [Option<Rgb>; 269],
    bitmap_state: Arc<Mutex<BitmapState>>,
) {
    let event_name = format!("terminal:bitmap:{}", id);
    let min_frame_interval = Duration::from_millis(16);
    let mut last_emit = std::time::Instant::now() - min_frame_interval;

    loop {
        // Block on single wakeup channel — both PTY events and scroll use this
        match wakeup_rx.recv() {
            Ok(()) => {}
            Err(_) => break,
        }

        // Rate limit: ensure at least 16ms between emissions
        let since_last = last_emit.elapsed();
        if since_last < min_frame_interval {
            let wait = min_frame_interval - since_last;
            let deadline = std::time::Instant::now() + wait;
            loop {
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                if remaining.is_zero() { break; }
                match wakeup_rx.recv_timeout(remaining) {
                    Ok(()) => continue,
                    Err(mpsc::RecvTimeoutError::Timeout) => break,
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                }
            }
        }

        // Adaptive batch window: 4ms normally, extended during high-throughput.
        // Count how many wakeups arrive during the batch to detect bursts.
        let mut wakeup_count = 0u32;
        let batch_deadline = std::time::Instant::now() + Duration::from_millis(4);
        loop {
            let remaining = batch_deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() { break; }
            match wakeup_rx.recv_timeout(remaining) {
                Ok(()) => { wakeup_count += 1; continue; }
                Err(mpsc::RecvTimeoutError::Timeout) => break,
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
        }

        // During high-throughput bursts (many wakeups in 4ms), skip this frame
        // to let the terminal accumulate more changes. This dramatically reduces
        // the number of QOI encodes during large session loads.
        if wakeup_count > 10 {
            // Drain remaining wakeups for another 30ms, then render
            let burst_deadline = std::time::Instant::now() + Duration::from_millis(30);
            loop {
                let remaining = burst_deadline.saturating_duration_since(std::time::Instant::now());
                if remaining.is_zero() { break; }
                match wakeup_rx.recv_timeout(remaining) {
                    Ok(()) => continue,
                    Err(mpsc::RecvTimeoutError::Timeout) => break,
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                }
            }
        }

        let blink_fired = false;

        // Render bitmap
        let render_start = std::time::Instant::now();

        {
            let mut term = term.lock_unfair();
            let mut bstate = bitmap_state.lock().unwrap();

            let grid = term.grid();
            let cols = grid.columns();
            let rows = grid.screen_lines();
            let display_offset = grid.display_offset();
            let cursor = grid.cursor.point;
            let cursor_visible = term.mode().contains(alacritty_terminal::term::TermMode::SHOW_CURSOR);
            let term_mode = term.mode().bits();

            // Log display offset to debug scroll
            if display_offset > 0 || bstate.force_full_render {
                log_debug!("[emit] display_offset={} force_full={} rows={}", display_offset, bstate.force_full_render, rows);
            }

            // Ensure bitmap is correctly sized
            if bstate.bitmap.needs_resize(cols as u16, rows as u16, bstate.glyph_cache.cell_width, bstate.glyph_cache.cell_height) {
                bstate.bitmap = BitmapBuffer::new(
                    cols as u16,
                    rows as u16,
                    bstate.glyph_cache.cell_width,
                    bstate.glyph_cache.cell_height,
                );
            }

            // Check if scroll requested a full render
            let force_full = bstate.force_full_render;
            if force_full {
                bstate.force_full_render = false;
            }

            // Get damaged rows (or all rows if forced by scroll)
            let all_damaged: Vec<usize> = if force_full {
                // Consume and discard damage to reset tracking
                let _ = term.damage();
                (0..rows).collect()
            } else {
                let damage = term.damage();
                match damage {
                    alacritty_terminal::term::TermDamage::Full => {
                        (0..rows).collect()
                    }
                    alacritty_terminal::term::TermDamage::Partial(iter) => {
                        iter.filter(|d| d.is_damaged())
                            .map(|d| d.line)
                            .collect()
                    }
                }
            };

            if all_damaged.is_empty() {
                term.reset_damage();
            } else {
                let damaged_count = all_damaged.len();

                // Re-borrow grid after damage()
                let grid = term.grid();
                let display_offset = grid.display_offset();

                // Destructure to satisfy borrow checker
                let BitmapState { ref mut glyph_cache, ref mut bitmap, .. } = *bstate;

                // Render damaged rows.
                // CRITICAL: When scrolled, display_offset > 0 means the viewport
                // is shifted up into history. Line(0) is always the live bottom line,
                // so we must subtract display_offset to read the scrolled viewport.
                // Line(-(display_offset as i32)) = top of visible viewport when scrolled.
                for &row_idx in &all_damaged {
                    if row_idx >= rows { continue; }
                    let line = Line(row_idx as i32 - display_offset as i32);
                    let row = &grid[line];
                    let cells = extract_row_cells(row, cols, &palette);

                    let cursor_on_row = if cursor_visible && display_offset == 0 && cursor.line.0 == row_idx as i32 {
                        Some(bitmap_renderer::CursorInfo {
                            col: cursor.column.0 as u16,
                            shape: bitmap_renderer::CursorShape::Bar,
                            visible: true,
                        })
                    } else {
                        None
                    };

                    bitmap_renderer::render_row(
                        bitmap,
                        row_idx,
                        &cells,
                        glyph_cache,
                        cursor_on_row.as_ref(),
                    );
                }

                term.reset_damage();

                let render_elapsed = render_start.elapsed();

                // QOI encode full buffer
                let qoi_start = std::time::Instant::now();
                let qoi_bytes = match qoi::encode_to_vec(
                    &bitmap.pixels,
                    bitmap.width,
                    bitmap.height,
                ) {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        log_debug!("[terminal/bitmap] QOI encode failed in emission loop: {:?}", e);
                        continue;
                    }
                };
                let qoi_elapsed = qoi_start.elapsed();

                let image_b64 = base64::engine::general_purpose::STANDARD.encode(&qoi_bytes);

                // Lightweight perf logging (Rust-side only, no JS overhead)
                let total_ms = render_start.elapsed().as_millis();
                if total_ms > 5 || damaged_count > 10 {
                    log_debug!(
                        "[perf] id={} rows={}/{} render={}us qoi={}us size={}KB total={}ms",
                        id, damaged_count, rows,
                        render_elapsed.as_micros(),
                        qoi_elapsed.as_micros(),
                        image_b64.len() / 1024,
                        total_ms,
                    );
                }

                {
                    let update = BitmapUpdate {
                        image_b64,
                        width: bitmap.width,
                        height: bitmap.height,
                        cols: cols as u16,
                        rows: rows as u16,
                        cell_width: glyph_cache.logical_cell_width(),
                        cell_height: glyph_cache.logical_cell_height(),
                        cursor_col: cursor.column.0 as u16,
                        cursor_row: cursor.line.0 as u16,
                        cursor_visible,
                        mode: term_mode,
                        display_offset,
                        perf: Some(BitmapPerfInfo {
                            render_us: render_elapsed.as_micros() as u64,
                            qoi_us: qoi_elapsed.as_micros() as u64,
                            qoi_bytes: qoi_bytes.len() as u32,
                            damaged_rows: damaged_count as u16,
                        }),
                    };

                    drop(bstate);
                    drop(term);
                    let _ = app_handle.emit(&event_name, &update);
                }
            }
        }

        last_emit = std::time::Instant::now();
    }
}

// ── Grid (DOM text) Emission Loop ───────────────────────────────────────

/// Background thread that snapshots the terminal grid as styled text runs
/// and emits GridUpdate events to the frontend for DOM rendering.
/// This replaces bitmap rendering with text-based IPC.
fn grid_emission_loop(
    id: &str,
    term: &Arc<FairMutex<Term<K2SOListener>>>,
    app_handle: &AppHandle,
    wakeup_rx: mpsc::Receiver<()>,
    palette: [Option<Rgb>; 269],
    force_full_render: Arc<std::sync::atomic::AtomicBool>,
) {
    let event_name = format!("terminal:grid:{}", id);
    let min_frame_interval = Duration::from_millis(16);
    let mut last_emit = std::time::Instant::now() - min_frame_interval;

    loop {
        // Block on wakeup channel — both PTY events and scroll use this
        match wakeup_rx.recv() {
            Ok(()) => {}
            Err(_) => break,
        }

        // Rate limit: ensure at least 16ms between emissions
        let since_last = last_emit.elapsed();
        if since_last < min_frame_interval {
            let wait = min_frame_interval - since_last;
            let deadline = std::time::Instant::now() + wait;
            loop {
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                if remaining.is_zero() { break; }
                match wakeup_rx.recv_timeout(remaining) {
                    Ok(()) => continue,
                    Err(mpsc::RecvTimeoutError::Timeout) => break,
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                }
            }
        }

        // Adaptive batch window: 4ms normally, extended during high-throughput
        let mut wakeup_count = 0u32;
        let batch_deadline = std::time::Instant::now() + Duration::from_millis(4);
        loop {
            let remaining = batch_deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() { break; }
            match wakeup_rx.recv_timeout(remaining) {
                Ok(()) => { wakeup_count += 1; continue; }
                Err(mpsc::RecvTimeoutError::Timeout) => break,
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
        }

        // During high-throughput bursts, wait longer to accumulate changes
        if wakeup_count > 10 {
            let burst_deadline = std::time::Instant::now() + Duration::from_millis(30);
            loop {
                let remaining = burst_deadline.saturating_duration_since(std::time::Instant::now());
                if remaining.is_zero() { break; }
                match wakeup_rx.recv_timeout(remaining) {
                    Ok(()) => continue,
                    Err(mpsc::RecvTimeoutError::Timeout) => break,
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                }
            }
        }

        // Snapshot the grid
        let force_full = force_full_render.swap(false, std::sync::atomic::Ordering::Relaxed);

        let update = {
            let mut term = term.lock_unfair();

            if force_full {
                // Consume and discard damage, take full snapshot
                let _ = term.damage();
                term.reset_damage();
                snapshot_grid(&term, &palette, true)
            } else {
                snapshot_damaged(&mut term, &palette)
            }
        };

        // Only emit if there are lines to send
        if !update.lines.is_empty() {
            let _ = app_handle.emit(&event_name, &update);
        }

        last_emit = std::time::Instant::now();
    }
}

use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize, Child};
use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::panic;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use tauri::{AppHandle, Emitter};

/// Ignore SIGPIPE at process startup so writing to a dead PTY returns EPIPE
/// instead of killing the entire Tauri process.
#[cfg(unix)]
pub fn ignore_sigpipe() {
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }
}

/// Grace period for SIGHUP before SIGKILL (milliseconds).
const KILL_GRACE_MS: u64 = 100;

/// Maximum scrollback buffer size (256KB). This ring buffer captures recent
/// PTY output so that when a terminal tab is re-activated, the frontend can
/// replay it and reconstruct the terminal state.
const SCROLLBACK_BUFFER_SIZE: usize = 256 * 1024;

/// Ring buffer for terminal output. Keeps the last N bytes so the frontend
/// can replay them when reattaching to a terminal after a tab switch.
#[derive(Clone)]
pub struct ScrollbackBuffer {
    data: VecDeque<u8>,
    capacity: usize,
}

impl ScrollbackBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            data: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    pub fn push(&mut self, bytes: &[u8]) {
        for &b in bytes {
            if self.data.len() >= self.capacity {
                self.data.pop_front();
            }
            self.data.push_back(b);
        }
    }

    pub fn get_contents(&self) -> Vec<u8> {
        self.data.iter().copied().collect()
    }
}

/// Holds a single PTY session.
struct TerminalInstance {
    master: Arc<Mutex<Box<dyn Write + Send>>>,
    child: Arc<Mutex<Box<dyn Child + Send + Sync>>>,
    cwd: String,
    /// Resize handle — kept so we can call resize later.
    master_pty: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    /// Scrollback buffer — captures recent output for replay on reattach.
    buffer: Arc<Mutex<ScrollbackBuffer>>,
}

/// Manages all active PTY sessions.
pub struct TerminalManager {
    terminals: HashMap<String, TerminalInstance>,
}

impl TerminalManager {
    pub fn new() -> Self {
        Self {
            terminals: HashMap::new(),
        }
    }

    /// Spawn a new PTY terminal.
    ///
    /// - `id`: unique identifier for this terminal session
    /// - `cwd`: working directory (tilde is expanded)
    /// - `command`: optional command to run (wrapped in login shell)
    /// - `args`: optional arguments for the command
    /// - `cols`/`rows`: initial PTY dimensions (defaults to 80x24)
    /// - `app_handle`: Tauri app handle for emitting events
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
            // Terminal already exists — this can happen during rapid
            // mount/unmount cycles. Return success so the frontend
            // can reattach instead of failing.
            eprintln!("[terminal] Terminal {} already exists, skipping creation", id);
            return Ok(());
        }

        // Expand tilde
        let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/"));
        let resolved_cwd = if cwd == "~" {
            home.to_string_lossy().to_string()
        } else if cwd.starts_with("~/") {
            cwd.replacen("~", &home.to_string_lossy(), 1)
        } else {
            cwd.clone()
        };

        // Ensure cwd exists, fallback to home with a warning
        let safe_cwd = if std::path::Path::new(&resolved_cwd).exists() {
            resolved_cwd.clone()
        } else {
            eprintln!(
                "[terminal] WARNING: CWD '{}' does not exist, falling back to home directory",
                resolved_cwd
            );
            home.to_string_lossy().to_string()
        };

        // Determine default shell
        let shell = std::env::var("SHELL")
            .ok()
            .filter(|s| std::path::Path::new(s).exists())
            .unwrap_or_else(|| {
                for sh in &["/bin/zsh", "/bin/bash", "/bin/sh"] {
                    if std::path::Path::new(sh).exists() {
                        return sh.to_string();
                    }
                }
                "/bin/sh".to_string()
            });

        let pty_system = native_pty_system();

        let pair = pty_system
            .openpty(PtySize {
                rows: rows.unwrap_or(24),
                cols: cols.unwrap_or(80),
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| format!("Failed to open PTY: {}", e))?;

        // Build the command
        let mut cmd = CommandBuilder::new(&shell);

        if let Some(ref user_command) = command {
            let full_command = if let Some(ref user_args) = args {
                format!("{} {}", user_command, user_args.join(" "))
            } else {
                user_command.clone()
            };
            cmd.args(["-ilc", &full_command]);
        }

        cmd.cwd(&safe_cwd);

        // Set environment variables
        cmd.env("TERM", "xterm-256color");
        cmd.env("TERM_PROGRAM", "K2SO");
        cmd.env("COLORTERM", "truecolor");
        // Suppress zsh's partial-line indicator (the highlighted % on empty lines)
        cmd.env("PROMPT_EOL_MARK", "");

        // Strip ELECTRON_* and VITE_* env vars by collecting current env
        // and selectively setting. CommandBuilder inherits the process env,
        // so we remove unwanted vars.
        for (key, _) in std::env::vars() {
            if key.starts_with("ELECTRON_") || key.starts_with("VITE_") || key.starts_with("__vite") {
                cmd.env_remove(&key);
            }
        }

        // Ensure PATH is set
        if std::env::var("PATH").is_err() {
            cmd.env("PATH", "/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin");
        }

        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| format!("Failed to spawn command: {}", e))?;

        // Get a writer for the master side
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| format!("Failed to get PTY writer: {}", e))?;

        // Get a reader for the master side
        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| format!("Failed to get PTY reader: {}", e))?;

        let writer = Arc::new(Mutex::new(writer));
        let child = Arc::new(Mutex::new(child));
        let master_pty = Arc::new(Mutex::new(pair.master));
        let buffer = Arc::new(Mutex::new(ScrollbackBuffer::new(SCROLLBACK_BUFFER_SIZE)));

        let instance = TerminalInstance {
            master: Arc::clone(&writer),
            child: Arc::clone(&child),
            cwd: safe_cwd,
            master_pty: Arc::clone(&master_pty),
            buffer: Arc::clone(&buffer),
        };

        self.terminals.insert(id.clone(), instance);

        // Spawn reader thread
        let data_event = format!("terminal:data:{}", id);
        let exit_event = format!("terminal:exit:{}", id);
        let child_for_thread = Arc::clone(&child);
        let app_for_thread = app_handle.clone();
        let buffer_for_thread = Arc::clone(&buffer);

        thread::spawn(move || {
            // Wrap entire reader thread in catch_unwind so a panic doesn't
            // silently kill the thread, leaving the UI thinking the terminal
            // is still alive. On panic, we emit an exit event with code -1.
            let data_event_clone = data_event.clone();
            let exit_event_clone = exit_event.clone();
            let app_clone = app_for_thread.clone();

            let result = panic::catch_unwind(panic::AssertUnwindSafe(move || {
                // Use a 16KB read buffer — larger reads mean fewer IPC events
                // for fast output, while immediate emission avoids latency for
                // interactive apps (claude, cursor) that use full-screen ANSI.
                let mut buf = [0u8; 16384];
                let mut first_emit = true;
                // Leftover bytes from an incomplete UTF-8 sequence at the end
                // of the previous read. Max 3 bytes (a 4-byte char split at worst).
                let mut utf8_leftover: Vec<u8> = Vec::with_capacity(4);

                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => {
                            break;
                        }
                        Ok(n) => {
                            // Buffer the raw output for replay on reattach
                            if let Ok(mut scrollback) = buffer_for_thread.lock() {
                                scrollback.push(&buf[..n]);
                            }

                            // Prepend any leftover bytes from the previous read
                            // into a combined buffer for UTF-8 boundary analysis.
                            let combined: Vec<u8>;
                            let chunk: &[u8] = if utf8_leftover.is_empty() {
                                &buf[..n]
                            } else {
                                combined = [utf8_leftover.as_slice(), &buf[..n]].concat();
                                &combined
                            };

                            // Find how many bytes at the end form an incomplete
                            // UTF-8 sequence. We check the last 1–3 bytes for a
                            // leading byte that expects more continuation bytes
                            // than are present.
                            let valid_up_to = match std::str::from_utf8(chunk) {
                                Ok(_) => chunk.len(),
                                Err(e) => {
                                    let valid = e.valid_up_to();
                                    // If there's an error_len, it's truly invalid
                                    // bytes (not just incomplete). Skip them to
                                    // avoid infinite accumulation.
                                    if e.error_len().is_some() {
                                        // Invalid byte(s) — include them (lossy)
                                        valid + e.error_len().unwrap()
                                    } else {
                                        // Incomplete sequence at end — save for next read
                                        valid
                                    }
                                }
                            };

                            // Emit the valid portion
                            if valid_up_to > 0 {
                                let data = String::from_utf8_lossy(&chunk[..valid_up_to]).to_string();
                                if first_emit {
                                    eprintln!("[terminal] First data emit to event '{}' ({} bytes)", data_event, n);
                                    first_emit = false;
                                }
                                let _ = app_for_thread.emit(&data_event, &data);
                            }

                            // Save any trailing incomplete bytes for next iteration
                            utf8_leftover = chunk[valid_up_to..].to_vec();
                        }
                        Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => {
                            continue;
                        }
                        Err(e) => {
                            eprintln!("[terminal] Reader error: {}", e);
                            break;
                        }
                    }
                }

                // Child exited — get exit status
                let exit_code = match child_for_thread.lock() {
                    Ok(mut c) => c
                        .wait()
                        .ok()
                        .map(|s| s.exit_code() as i32)
                        .unwrap_or(-1),
                    Err(e) => {
                        eprintln!("[terminal] Poisoned child mutex: {}", e);
                        -1
                    }
                };

                let _ = app_for_thread.emit(&exit_event, serde_json::json!({ "exitCode": exit_code }));
            }));

            if result.is_err() {
                eprintln!("[terminal] PANIC in reader thread for '{}' — emitting exit event", data_event_clone);
                let _ = app_clone.emit(&exit_event_clone, serde_json::json!({ "exitCode": -1, "signal": -1 }));
            }
        });

        Ok(())
    }

    /// Write data to a terminal's PTY.
    pub fn write(&self, id: &str, data: &str) -> Result<(), String> {
        let instance = self
            .terminals
            .get(id)
            .ok_or_else(|| format!("Terminal {} not found", id))?;

        instance
            .master
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?
            .write_all(data.as_bytes())
            .map_err(|e| format!("Write error: {}", e))
    }

    /// Resize a terminal's PTY.
    pub fn resize(&self, id: &str, cols: u16, rows: u16) -> Result<(), String> {
        let instance = match self.terminals.get(id) {
            Some(i) => i,
            None => return Ok(()), // Silently ignore if terminal gone
        };

        instance
            .master_pty
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| format!("Resize error: {}", e))
    }

    /// Kill a terminal and clean up.
    /// Sends SIGHUP first for graceful shutdown, then SIGKILL after a grace period.
    pub fn kill(&mut self, id: &str) -> Result<(), String> {
        if let Some(instance) = self.terminals.remove(id) {
            if let Ok(mut child) = instance.child.lock() {
                // Try graceful shutdown with SIGHUP first
                #[cfg(unix)]
                {
                    if let Some(pid) = child.process_id() {
                        unsafe {
                            libc::kill(pid as i32, libc::SIGHUP);
                        }
                    }
                    // Wait briefly for graceful exit
                    thread::sleep(Duration::from_millis(KILL_GRACE_MS));
                }
                // Force kill if still alive
                let _ = child.kill();
            }
        }
        Ok(())
    }

    /// Send SIGINT to the foreground process group of a terminal,
    /// killing just the running command without killing the shell.
    #[cfg(unix)]
    pub fn kill_foreground(&self, id: &str) -> Result<(), String> {
        let instance = self
            .terminals
            .get(id)
            .ok_or_else(|| format!("Terminal {} not found", id))?;

        let master_pty = instance
            .master_pty
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;

        let fd = master_pty
            .as_raw_fd()
            .ok_or_else(|| "PTY has no file descriptor".to_string())?;

        let fg_pgid = unsafe { libc::tcgetpgrp(fd) };

        if fg_pgid > 0 {
            unsafe {
                libc::killpg(fg_pgid, libc::SIGINT);
            }
            Ok(())
        } else {
            Err("Could not determine foreground process group".to_string())
        }
    }

    /// Get the name of the foreground process running in a terminal.
    /// Returns None if only the shell is running (no interesting command).
    #[cfg(unix)]
    pub fn get_foreground_command(&self, id: &str) -> Result<Option<String>, String> {
        let instance = match self.terminals.get(id) {
            Some(i) => i,
            None => return Ok(None),
        };

        let master_pty = instance
            .master_pty
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;

        let fd = match master_pty.as_raw_fd() {
            Some(fd) => fd,
            None => return Ok(None),
        };

        let fg_pgid = unsafe { libc::tcgetpgrp(fd) };
        if fg_pgid <= 0 {
            return Ok(None);
        }

        // Get the shell's PID to compare — if foreground IS the shell, nothing interesting is running
        let shell_pid = instance
            .child
            .lock()
            .ok()
            .and_then(|c| c.process_id());

        if let Some(spid) = shell_pid {
            if fg_pgid == spid as i32 {
                return Ok(None); // Shell is in foreground — no agent running
            }
        }

        // Get the process name via proc_pidpath (macOS)
        #[cfg(target_os = "macos")]
        {
            let mut buf = [0u8; 4096];
            let ret = unsafe {
                libc::proc_pidpath(fg_pgid, buf.as_mut_ptr() as *mut libc::c_void, buf.len() as u32)
            };
            if ret > 0 {
                let path = String::from_utf8_lossy(&buf[..ret as usize]);
                let name = path.rsplit('/').next().unwrap_or("");
                if !name.is_empty() {
                    return Ok(Some(name.to_string()));
                }
            }
        }

        // Fallback for Linux: read /proc/{pid}/comm
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

    /// Check if a terminal with the given ID exists and is still alive.
    pub fn exists(&self, id: &str) -> bool {
        self.terminals.contains_key(id)
    }

    /// Get the scrollback buffer contents for replay on reattach.
    pub fn get_buffer(&self, id: &str) -> Result<String, String> {
        let instance = self
            .terminals
            .get(id)
            .ok_or_else(|| format!("Terminal {} not found", id))?;

        let buffer = instance
            .buffer
            .lock()
            .map_err(|e| format!("Lock error: {}", e))?;

        Ok(String::from_utf8_lossy(&buffer.get_contents()).to_string())
    }

    /// Count terminals whose cwd starts with the given path.
    pub fn get_count_for_path(&self, path: &str) -> i32 {
        self.terminals
            .values()
            .filter(|inst| inst.cwd.starts_with(path))
            .count() as i32
    }

    /// Get total count of active terminals.
    pub fn get_active_count(&self) -> i32 {
        self.terminals.len() as i32
    }

    /// Kill all terminals — called on app quit.
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

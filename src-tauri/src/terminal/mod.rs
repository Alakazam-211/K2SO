// Terminal backend — Alacritty-based terminal emulation with grid-based IPC.

mod alacritty_backend;
pub mod grid_types;
mod font_renderer;
mod bitmap_renderer;

pub use alacritty_backend::TerminalManager;

// ── Shared utilities ───────────────────────────────────────────────────────

/// Ignore SIGPIPE at process startup so writing to a dead PTY returns EPIPE
/// instead of killing the entire Tauri process.
#[cfg(unix)]
pub fn ignore_sigpipe() {
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }
}

/// Expand tilde in a path and ensure the directory exists.
pub fn resolve_cwd(cwd: &str) -> String {
    let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/"));
    let resolved = if cwd == "~" {
        home.to_string_lossy().to_string()
    } else if cwd.starts_with("~/") {
        cwd.replacen("~", &home.to_string_lossy(), 1)
    } else {
        cwd.to_string()
    };

    if std::path::Path::new(&resolved).exists() {
        resolved
    } else {
        eprintln!("[terminal] WARNING: CWD '{}' does not exist, falling back to home", resolved);
        home.to_string_lossy().to_string()
    }
}

/// Detect the user's default shell.
pub fn detect_shell() -> String {
    std::env::var("SHELL")
        .ok()
        .filter(|s| std::path::Path::new(s).exists())
        .unwrap_or_else(|| {
            for sh in &["/bin/zsh", "/bin/bash", "/bin/sh"] {
                if std::path::Path::new(sh).exists() {
                    return sh.to_string();
                }
            }
            "/bin/sh".to_string()
        })
}

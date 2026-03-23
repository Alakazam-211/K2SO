// Terminal backend abstraction layer.
//
// By default, uses the legacy xterm.js + portable-pty backend.
// With the `alacritty-backend` feature, uses alacritty_terminal for
// Rust-native terminal emulation with grid-based IPC.

mod legacy;

#[cfg(feature = "alacritty-backend")]
mod alacritty_backend;

#[cfg(feature = "alacritty-backend")]
pub mod grid_types;

#[cfg(feature = "alacritty-backend")]
mod font_renderer;

#[cfg(feature = "alacritty-backend")]
mod bitmap_renderer;

// ── Re-export the active backend as `TerminalManager` ──────────────────────

#[cfg(not(feature = "alacritty-backend"))]
pub use legacy::TerminalManager;

#[cfg(feature = "alacritty-backend")]
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

/// Returns which backend is active: "legacy" or "alacritty".
pub fn backend_name() -> &'static str {
    #[cfg(feature = "alacritty-backend")]
    { "alacritty" }
    #[cfg(not(feature = "alacritty-backend"))]
    { "legacy" }
}

/// Expand tilde in a path and ensure the directory exists.
/// Falls back to home directory if the path doesn't exist.
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
        eprintln!(
            "[terminal] WARNING: CWD '{}' does not exist, falling back to home directory",
            resolved
        );
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

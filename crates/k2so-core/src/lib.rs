//! K2SO core library.
//!
//! Home of the device-local runtime that was previously embedded inside
//! `src-tauri/src/`: SQLite database, llama-cpp LLM integration, Alacritty
//! terminal backend, companion WebSocket server, heartbeat scheduler,
//! agent-lifecycle HTTP hooks, and the pluggable `PushTarget` interface for
//! notification delivery.
//!
//! Both the `k2so-daemon` binary and the `src-tauri` Tauri app link this
//! crate so the core logic executes in exactly one place — the daemon —
//! while the Tauri app stays a thin client that proxies state-mutating
//! commands over HTTP.
//!
//! Module migration from src-tauri lands incrementally.

/// Safe `eprintln!` replacement that silently ignores stderr write
/// failures. When K2SO is launched from Finder there's no tty attached and
/// the default `eprintln!` panics on broken-pipe, which then cascades into
/// abort(). This macro swallows the write result instead.
///
/// `#[macro_export]` so both k2so-core-internal modules and downstream
/// crates (src-tauri, k2so-daemon) can share one definition.
#[macro_export]
macro_rules! log_debug {
    ($($arg:tt)*) => {{
        use std::io::Write;
        let _ = writeln!(std::io::stderr(), $($arg)*);
    }};
}

pub mod db;
pub mod perf;
pub mod push;

#[doc(hidden)]
pub fn __scaffolding_marker() {}

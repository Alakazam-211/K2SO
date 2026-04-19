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

pub mod perf;

#[doc(hidden)]
pub fn __scaffolding_marker() {}

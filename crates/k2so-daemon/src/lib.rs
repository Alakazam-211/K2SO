//! Thin library surface for the k2so-daemon binary.
//!
//! The crate's primary artifact remains the `k2so-daemon` binary
//! (see `src/main.rs`). This lib exists so integration tests in
//! `crates/k2so-daemon/tests/*.rs` can reach internal modules like
//! `sessions_ws` without duplicating the code — the binary's own
//! `mod` declarations are unchanged and sit above `main.rs`.

pub mod agents_routes;
pub mod awareness_ws;
pub mod cli_response;
pub mod companion_routes;
pub mod events;
pub mod pending_live;
pub mod providers;
pub mod session_map;
pub mod sessions_ws;
pub mod signal_format;
pub mod spawn;
pub mod terminal_routes;
pub mod triage;
pub mod watchdog;

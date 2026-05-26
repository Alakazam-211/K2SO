//! Thin library surface for the k2so-daemon binary.
//!
//! The crate's primary artifact remains the `k2so-daemon` binary
//! (see `src/main.rs`). This lib exists so integration tests in
//! `crates/k2so-daemon/tests/*.rs` can reach internal modules like
//! `sessions_ws` without duplicating the code — the binary's own
//! `mod` declarations are unchanged and sit above `main.rs`.

pub mod agents_routes;
pub mod awareness_ws;
pub mod canonical_session;
pub mod chat_routes;
pub mod claude_auth_host;
pub mod cli_response;
pub mod companion_host;
pub mod companion_routes;
pub mod events;
pub mod fs_routes;
pub mod heartbeat_launch;
pub mod heartbeat_routes;
pub mod inbox_routes;
pub mod llm_host;
pub mod llm_routes;
pub mod pending_live;
pub mod project_config_routes;
pub mod providers;
pub mod review_checklist_routes;
pub mod session_events;
pub mod session_events_ws;
pub mod session_lookup;
pub mod sessions_grid_ws;
pub mod sessions_ws;
pub mod signal_format;
pub mod skill_layers_routes;
pub mod spawn;
pub mod terminal_routes;
pub mod themes_routes;
pub mod triage;
pub mod v2_session_map;
pub mod v2_spawn;
pub mod wake_headless;
pub mod watchdog;
pub mod workspace_layouts_dedup;
pub mod workspace_msg;

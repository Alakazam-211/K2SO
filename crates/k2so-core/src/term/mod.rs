//! Line-oriented terminal producer (Phase 1 scaffolding).
//!
//! Sibling to `terminal/` (alacritty-backed single-grid path). The `term/`
//! tree holds the WezTerm-style line-mux pipeline that emits client-
//! agnostic `Line` and `Frame` events. Populated by C4 (`line_mux`),
//! C5 (`apc`), C6 (`recognizers/`).
//!
//! Feature-gated via `#[cfg(feature = "session_stream")]` in `lib.rs`.

/// Module presence marker; deleted in C4 once `LineMux` lands.
pub const VERSION: &str = "0.34.0-phase1";

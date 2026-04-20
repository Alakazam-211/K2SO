//! Line-oriented terminal producer.
//!
//! Sibling to `terminal/` (alacritty-backed single-grid path). The
//! `term/` tree holds the WezTerm-style line-mux pipeline that emits
//! client-agnostic `Line` and `Frame` events — the Producer A of the
//! Session Stream PRD.
//!
//! Feature-gated via `#[cfg(feature = "session_stream")]` in `lib.rs`.

pub mod line_mux;

pub use line_mux::LineMux;

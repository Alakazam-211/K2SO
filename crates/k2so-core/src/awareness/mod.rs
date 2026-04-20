//! Awareness Bus primitive (Phase 1 scaffolding).
//!
//! This module hosts the primitive-B types from the Session Stream PRD:
//! `AgentSignal`, `SignalKind`, `AgentAddress`. Types land in C3;
//! routing / egress / filesystem-inbox integration is Phase 3.
//!
//! Feature-gated via `#[cfg(feature = "session_stream")]` in `lib.rs`.

/// Module presence marker; deleted in C3 once real public items exist.
pub const VERSION: &str = "0.34.0-phase1";

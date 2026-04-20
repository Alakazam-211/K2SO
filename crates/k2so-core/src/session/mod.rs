//! Session primitive (Phase 1 scaffolding).
//!
//! This module hosts the primitive-A types from the Session Stream PRD
//! (`.k2so/prds/session-stream-and-awareness-bus.md`): `Frame`, `Line`,
//! `SemanticKind`, `Session`. Types land in C2; this file is just the
//! module root so downstream commits have a home to add to.
//!
//! The whole module sits behind `#[cfg(feature = "session_stream")]` in
//! `lib.rs`. Flag off → module doesn't compile in; zero impact on the
//! alacritty-backed terminal path.

/// Module presence marker. Used by the scaffold test to prove the
/// feature-gated module is reachable. Later commits will delete this
/// once real public items exist.
pub const VERSION: &str = "0.34.0-phase1";

//! Historical migration helpers.
//!
//! Each submodule here is a one-shot migration tied to a specific
//! K2SO release. They get scheduled once at daemon boot, write a
//! sentinel file when they finish, and no-op on subsequent boots.
//!
//! Older modules are kept around (rather than deleted post-release)
//! so freshly-upgraded workspaces from a pre-migration version still
//! pick up the fix. Each module's name encodes the release it shipped
//! in (e.g. `unification_0_37_0`).

pub mod unification_0_37_0;

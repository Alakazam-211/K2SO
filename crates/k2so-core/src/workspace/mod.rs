//! Workspace — lifecycle, state, identity, and launch helpers.
//!
//! Phase 2.5c relocation home. Before Phase 2.5c, most of these
//! responsibilities lived under `agents/` (e.g. `agents/scheduler.rs`,
//! `agents/checkin.rs`, `agents/wake.rs`), but post-Phase-2.1 the
//! "workspace == agent" 1:1 invariant means these are workspace-scoped
//! concerns, not agent-scoped. The folder name now matches what the
//! code does.
//!
//! See `.k2so/prds/phase-2.5c-core-rename.md` for the relocation
//! catalogue and rationale.

pub mod agent_identity;
pub mod agent_launch;
pub mod checkin;
pub mod launch_profile;
pub mod onboarding;
pub mod scheduler;
pub mod session;
pub mod settings;
pub mod terminal_id;
pub mod triage;
pub mod wake_prompts;


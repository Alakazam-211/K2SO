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
pub mod display;
pub mod events;
pub mod launch_profile;
// Phase 2.5d: boot-time idempotent migration helpers, extracted from
// the monolithic `agents/workspace.rs`.
pub mod migrations;
pub mod onboarding;
pub mod scheduler;
// Phase 2.5d: SKILL.md regen + scaffolding (write_workspace_skill_file*
// + regenerate_workspace_skill cluster). Extracted from
// `agents/workspace.rs`.
pub mod skill_writer;
// Phase 2.5d: workspace-root harness file-discovery cluster
// (symlink/scaffold helpers + preview + ingest + disable). Extracted
// from `agents/workspace.rs`.
pub mod harness;
// Phase 2.5d: workspace teardown (disconnect) — freeze or restore the
// symlinks K2SO scaffolded. Extracted from `agents/workspace.rs`.
pub mod teardown;
// Phase 2.5d: Agent CRUD commands. Extracted from `agents/commands.rs`.
pub mod agent;
pub mod session;
pub mod settings;
pub mod terminal_id;
pub mod triage;
pub mod wake_prompts;
pub mod work_item;


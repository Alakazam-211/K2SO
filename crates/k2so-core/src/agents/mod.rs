//! K2SO Agent system — the heartbeat scheduler, primary-agent
//! resolution, and project/filesystem bookkeeping that the Tauri app
//! and the k2so-daemon both need.
//!
//! Home for the slice of `src-tauri/src/commands/k2so_agents.rs` that
//! has to run inside the daemon so agents keep firing while the
//! laptop lid is closed. Each submodule carries a narrow, testable
//! responsibility:
//!
//! - [`heartbeat`] — multi-heartbeat CRUD + tick evaluation + audit
//!   stamping. The piece that turns a launchd wake into actual fired
//!   `heartbeat_fires` rows.
//!
//! Helpers at this level are small, pure-ish utilities that multiple
//! submodules (and the in-progress route migration) depend on. They
//! stay public so src-tauri's existing call sites can re-export them
//! via `pub use k2so_core::agents::*` without churning 170+ lines of
//! renames.

// Phase 2.5c: identity helpers (`resolve_project_id`, `agents_dir`,
// `workspace_agent_path`, `agent_template_dir`,
// `workspace_heartbeats_dir`, `skills_dir`, `skill_dir`, `agent_dir`,
// `parse_frontmatter`, `agent_type_for`, `find_primary_agent`,
// `workspace_agent_md_path`) extracted to
// [`crate::workspace::agent_identity`]. Re-exported via the glob
// `pub use` at the bottom of this file so every existing
// `crate::agents::find_primary_agent` / `crate::agents::agent_dir` /
// etc. call site keeps resolving without changes.

/// Phase 2.5c: `build_launch` relocated to
/// [`crate::workspace::agent_launch`]. The rename clarifies that this
/// composes a launch from workspace context, not a generic build step.
pub use crate::workspace::agent_launch as build_launch;
pub mod channel;
/// Phase 2.5c: `checkin` relocated to [`crate::workspace::checkin`].
/// Back-compat alias.
pub use crate::workspace::checkin;
pub mod commands;
pub mod connections;
/// Phase 2.5c: `cron_schedule` relocated to
/// [`crate::heartbeats::cron`]. Back-compat alias.
pub use crate::heartbeats::cron as cron_schedule;
/// Phase 2.5c: `delegate` relocated to [`crate::deprecated::delegate`]
/// with `#[deprecated]` annotations on public functions. Back-compat
/// alias keeps `agents::delegate::*` resolving for existing callers
/// during the deprecation window.
pub use crate::deprecated::delegate;
pub mod display;
pub mod events;
/// Phase 2.5c: `heartbeat` relocated to top-level [`crate::heartbeats`].
/// Back-compat alias for callers that still reference
/// `agents::heartbeat::*`.
pub use crate::heartbeats as heartbeat;
/// Phase 2.5c: `heartbeat_install` relocated to
/// [`crate::heartbeats::install`]. Back-compat alias.
pub use crate::heartbeats::install as heartbeat_install;
/// Phase 2.5c: `launch_profile` relocated to
/// [`crate::workspace::launch_profile`]. Back-compat alias.
pub use crate::workspace::launch_profile;
/// Phase 2.5c: `onboarding` relocated to [`crate::workspace::onboarding`].
pub use crate::workspace::onboarding;
pub mod resume_chat;
pub mod reviews;
/// Phase 2.5c: `scheduler` relocated to [`crate::workspace::scheduler`].
/// Back-compat alias for callers that still reference
/// `agents::scheduler::*`.
pub use crate::workspace::scheduler;
/// Phase 2.5c: `session` relocated to [`crate::workspace::session`].
/// Back-compat alias.
pub use crate::workspace::session;
/// Phase 2.5c: `settings` relocated to [`crate::workspace::settings`].
/// Back-compat alias.
pub use crate::workspace::settings;
/// Phase 2.5c: `skill` (versioning + upgrade protocol) relocated to
/// [`crate::skills::version`]. Back-compat alias.
pub use crate::skills::version as skill;
/// Phase 2.5c: `skill_content` relocated to [`crate::skills::content`].
/// Back-compat alias for callers that still reference
/// `agents::skill_content::*`.
pub use crate::skills::content as skill_content;
/// Phase 2.5c: `skill_writer` relocated to [`crate::skills::writer`].
/// Back-compat alias for callers that still reference
/// `agents::skill_writer::*`.
pub use crate::skills::writer as skill_writer;
/// Phase 2.5c: `terminal_id` relocated to
/// [`crate::workspace::terminal_id`]. Back-compat alias.
pub use crate::workspace::terminal_id;
/// Phase 2.5c: `triage_summary` relocated to [`crate::workspace::triage`].
pub use crate::workspace::triage as triage_summary;
/// Phase 2.5c: `unification` relocated to
/// [`crate::migrations::unification_0_37_0`]. Back-compat alias.
pub use crate::migrations::unification_0_37_0 as unification;
/// Phase 2.5c: `wake` relocated to [`crate::workspace::wake_prompts`]
/// (renamed to avoid collision with the top-level `crate::wake` module).
/// Back-compat alias keeps `agents::wake::*` resolving.
pub use crate::workspace::wake_prompts as wake;
pub mod work_item;
pub mod workspace;
pub mod workspaces;

// Phase 2.5c: identity helpers extracted to
// [`crate::workspace::agent_identity`]. The re-exports below preserve
// every public name at `crate::agents::<name>` so existing call sites
// (and the `src-tauri` re-export bundle) keep resolving unchanged.
pub use crate::workspace::agent_identity::{
    agent_dir, agent_template_dir, agent_type_for, agents_dir, find_primary_agent,
    parse_frontmatter, resolve_project_id, skill_dir, skills_dir, workspace_agent_md_path,
    workspace_agent_path, workspace_heartbeats_dir,
};

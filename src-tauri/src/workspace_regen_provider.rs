//! Tauri-side impl of [`k2so_core::agents::workspace_regen::
//! WorkspaceRegenProvider`].
//!
//! Wraps `commands::k2so_agents::k2so_agents_generate_workspace_claude_md`
//! — the full `.k2so/` scaffolding + workspace SKILL.md regen
//! orchestrator that still lives in src-tauri because it depends on
//! src-tauri-only helpers (`generate_default_agent_body`,
//! `write_agent_skill_file`, etc.). Registered in `lib.rs::setup()`
//! so `k2so_core::agents::build_launch::k2so_agents_build_launch`'s
//! no-work case can invoke the regen without core taking a hard dep
//! on the scaffolding code.
//!
//! Daemon + test contexts don't register a provider; they get a
//! silent no-op + rely on Tauri's next-startup regen for workspace
//! SKILL.md freshness.

use k2so_core::agents::workspace_regen::WorkspaceRegenProvider;

pub struct TauriWorkspaceRegenProvider;

impl WorkspaceRegenProvider for TauriWorkspaceRegenProvider {
    fn regen(&self, project_path: &str) -> Result<String, String> {
        crate::commands::k2so_agents::k2so_agents_generate_workspace_claude_md(
            project_path.to_string(),
        )
    }
}

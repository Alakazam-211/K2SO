//! Workspace SKILL.md regen bridge.
//!
//! `k2so_agents_build_launch`'s no-work case eagerly regenerates the
//! workspace-root SKILL.md (`./CLAUDE.md` symlink target) before the
//! agent wakes, so the wake session sees fresh PROJECT.md / agent
//! roster / workspace-state content. The full regen orchestrator
//! (`k2so_agents_generate_workspace_claude_md`) lives in src-tauri
//! today — it pulls in ~1000 lines of `.k2so/` scaffolding + auto-
//! creation of manager/k2so-agent directories that depend on
//! src-tauri-only helpers like `generate_default_agent_body` and
//! `write_agent_skill_file`.
//!
//! This bridge lets `build_launch` (now in core) invoke the regen
//! without core having to depend on the scaffolding surface.
//! Follows the same ambient-singleton pattern as
//! `agent_hooks::AgentHookEventSink` and the companion bridges: src-
//! tauri registers a provider at startup; core calls through it; no-op
//! when unregistered (daemon / test contexts get skipped regen, which
//! is safe — the workspace SKILL is merely slightly stale until the
//! next Tauri-side refresh).

use parking_lot::Mutex;
use std::sync::OnceLock;

/// What the host provides: a single synchronous function that
/// regenerates the workspace root SKILL.md for a given project path.
/// Return values are intentionally opaque-as-`Result<String, String>`
/// to match the original `#[tauri::command]` signature.
pub trait WorkspaceRegenProvider: Send + Sync {
    fn regen(&self, project_path: &str) -> Result<String, String>;
}

static PROVIDER: OnceLock<Mutex<Option<Box<dyn WorkspaceRegenProvider>>>> = OnceLock::new();

fn slot() -> &'static Mutex<Option<Box<dyn WorkspaceRegenProvider>>> {
    PROVIDER.get_or_init(|| Mutex::new(None))
}

/// Register the host's regen impl. Idempotent; last writer wins.
pub fn set_provider(p: Box<dyn WorkspaceRegenProvider>) {
    *slot().lock() = Some(p);
}

/// Call the registered regen (best-effort — returns silently if no
/// provider is installed, which is the correct daemon-only / test
/// context fallback).
pub fn regen_workspace_skill(project_path: &str) -> Result<String, String> {
    if let Some(p) = slot().lock().as_ref() {
        p.regen(project_path)
    } else {
        // No host registered — daemon context. The Tauri app's
        // startup regen covers this file on its next launch.
        Ok(String::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    // Serialize these tests against each other since PROVIDER is a
    // process-global singleton.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn regen_without_provider_is_silent_ok() {
        let _g = TEST_LOCK.lock();
        *slot().lock() = None;
        assert!(regen_workspace_skill("/tmp/nope").is_ok());
    }

    #[test]
    fn registered_provider_is_called() {
        let _g = TEST_LOCK.lock();
        let hits = Arc::new(AtomicUsize::new(0));
        struct Fake(Arc<AtomicUsize>);
        impl WorkspaceRegenProvider for Fake {
            fn regen(&self, _project_path: &str) -> Result<String, String> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Ok("fresh skill".to_string())
            }
        }
        set_provider(Box::new(Fake(hits.clone())));
        let body = regen_workspace_skill("/r").unwrap();
        assert_eq!(body, "fresh skill");
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        *slot().lock() = None;
    }
}

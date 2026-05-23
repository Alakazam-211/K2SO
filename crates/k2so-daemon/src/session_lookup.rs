//! Unified lookup over the daemon's session map.
//!
//! K2SO holds one agent → session map:
//!
//!   - [`crate::v2_session_map`] — Alacritty_v2's `DaemonPtySession`
//!     (alacritty Term + grid WS pipeline).
//!
//! Historically there was a parallel `session_map` for the Kessel-T0
//! `SessionStreamSession` path; that was retired in 0.39.0 along with
//! the legacy renderer. [`LiveSession`] is preserved as a thin wrapper
//! around `Arc<DaemonPtySession>` so call sites that previously branched
//! on Legacy/V2 keep their signatures stable; new code can use the
//! per-map helpers directly.

use std::path::PathBuf;
use std::sync::Arc;

use k2so_core::session::SessionId;
use k2so_core::terminal::DaemonPtySession;

use crate::v2_session_map;

/// One live session — always backed by a v2 `DaemonPtySession`.
///
/// Holds the same `Arc<...>` the underlying map holds, so cloning
/// a `LiveSession` is cheap and dropping it doesn't tear down the
/// session unless it's the last reference.
#[derive(Clone)]
pub struct LiveSession(pub Arc<DaemonPtySession>);

impl LiveSession {
    pub fn session_id(&self) -> SessionId {
        self.0.session_id
    }

    /// Write bytes to the child's stdin.
    pub fn write(&self, bytes: &[u8]) -> std::io::Result<()> {
        // DaemonPtySession::write takes Cow<'static, [u8]>.
        // We have a borrowed slice with no lifetime extension
        // available, so copy. Hot path is single-keystroke
        // injection (~tens of bytes); the alloc is fine.
        self.0.write(bytes.to_vec());
        Ok(())
    }

    /// Resize the underlying PTY + Term grid. Errors are stringly-
    /// typed for call-site parity with prior `LiveSession::resize`;
    /// v2's resize is infallible at the daemon API surface so we
    /// always return `Ok(())`.
    pub fn resize(&self, cols: u16, rows: u16) -> Result<(), String> {
        self.0.resize(cols, rows);
        Ok(())
    }

    /// Resolved cwd the child was spawned in. Companion endpoints
    /// match this against `projects.path` to attribute sessions to
    /// workspaces.
    pub fn cwd(&self) -> String {
        self.0
            .cwd
            .as_ref()
            .map(PathBuf::as_path)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// Top-level command string the child runs, if specified.
    pub fn command(&self) -> Option<String> {
        self.0.program.clone()
    }

    /// Args the child was spawned with — used by smart-launch to
    /// find a live PTY running `--resume <session_id>` for a given
    /// heartbeat.
    pub fn args(&self) -> Vec<String> {
        self.0.args.clone()
    }

    /// True if this is a v2 (DaemonPtySession) session. Always true
    /// now that legacy is retired; preserved so legacy callers that
    /// branched on the renderer compile without edits.
    pub fn is_v2(&self) -> bool {
        true
    }

    /// Whether the session's child PID is still alive.
    pub fn is_child_alive(&self) -> bool {
        self.0.is_child_alive()
    }
}

/// Look up an agent in the v2 session map.
pub fn lookup_any(agent: &str) -> Option<LiveSession> {
    v2_session_map::lookup_by_agent_name(agent).map(LiveSession)
}

/// Look up by `SessionId` rather than agent name. Used by the
/// `/cli/terminal/{write,resize}` HTTP routes — those callers know
/// the session uuid but not the agent name behind it.
pub fn lookup_by_session_id(id: &SessionId) -> Option<LiveSession> {
    v2_session_map::lookup_by_session_id(id).map(LiveSession)
}

/// Every (agent_name, session) pair registered in the v2 map.
/// Returning owned `LiveSession`s lets the caller drop the
/// underlying mutex before doing expensive per-session work
/// (PTY writes, child kills, frame emits).
pub fn snapshot_all() -> Vec<(String, LiveSession)> {
    v2_session_map::snapshot()
        .into_iter()
        .map(|(name, session)| (name, LiveSession(session)))
        .collect()
}

/// All registered agent names in the v2 map.
#[allow(dead_code)]
pub fn list_agents() -> Vec<String> {
    let mut names = v2_session_map::list_agents();
    names.sort();
    names.dedup();
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `lookup_any` returns None when the map has no entry. Sanity
    /// check that the lookup path doesn't error.
    #[test]
    fn lookup_any_returns_none_for_unknown_agent() {
        let result = lookup_any("nonexistent-agent-xyz-test-only");
        assert!(result.is_none());
    }

    /// `snapshot_all` returns an empty vec when the map is empty.
    /// Tests are isolated per binary run, so the global map may
    /// hold entries from other tests in the same process — we don't
    /// assert emptiness, just that the call succeeds without panic.
    #[test]
    fn snapshot_all_does_not_panic() {
        let _ = snapshot_all();
    }
}

//! Unified lookup across the daemon's two session maps.
//!
//! K2SO holds two parallel agent → session maps:
//!
//!   - [`crate::session_map`] — Kessel-T0's `SessionStreamSession`
//!     (legacy renderer, frame-broadcast pipeline).
//!   - [`crate::v2_session_map`] — Alacritty_v2's `DaemonPtySession`
//!     (alacritty Term + grid WS pipeline).
//!
//! Every consumer that asks "is agent X live?" or "give me the
//! session for agent X" needs to check BOTH maps to be correct,
//! otherwise it's blind to whichever map didn't ship first. Before
//! this module, ~15 daemon call sites asked only `session_map::*`
//! and silently missed every v2 session — see audit findings in
//! `~/.claude/plans/happy-hatching-locket.md`.
//!
//! [`LiveSession`] hides the choice of map behind an enum so call
//! sites can call `write` / `resize` / `cwd` / etc. polymorphically.
//! New code should always reach for `lookup_any` / `snapshot_all`
//! instead of the per-map helpers; the per-map helpers stay around
//! for the spawn handlers that genuinely need to know which map
//! they're inserting into.

use std::path::PathBuf;
use std::sync::Arc;

use k2so_core::session::SessionId;
use k2so_core::terminal::{DaemonPtySession, SessionStreamSession};

use crate::{session_map, v2_session_map};

/// One live session — either Kessel-T0 (legacy) or Alacritty_v2.
///
/// Holds the same `Arc<...>` the underlying map holds, so cloning
/// a `LiveSession` is cheap and dropping it doesn't tear down the
/// session unless it's the last reference.
#[derive(Clone)]
pub enum LiveSession {
    Legacy(Arc<SessionStreamSession>),
    V2(Arc<DaemonPtySession>),
}

impl LiveSession {
    pub fn session_id(&self) -> SessionId {
        match self {
            LiveSession::Legacy(s) => s.session_id,
            LiveSession::V2(s) => s.session_id,
        }
    }

    /// Write bytes to the child's stdin. Both renderers accept the
    /// same byte stream — no transformation is applied here, the
    /// caller is responsible for whatever encoding (UTF-8, control
    /// chars, etc.) the target expects.
    pub fn write(&self, bytes: &[u8]) -> std::io::Result<()> {
        match self {
            LiveSession::Legacy(s) => s.write(bytes),
            LiveSession::V2(s) => {
                // DaemonPtySession::write takes Cow<'static, [u8]>.
                // We have a borrowed slice with no lifetime extension
                // available, so copy. Hot path is single-keystroke
                // injection (~tens of bytes); the alloc is fine.
                s.write(bytes.to_vec());
                Ok(())
            }
        }
    }

    /// Resize the underlying PTY + Term grid. Errors are stringly-
    /// typed because that's how `SessionStreamSession::resize`
    /// already reports them; v2's resize is infallible at the
    /// daemon API surface so we wrap `Ok(())` for parity.
    pub fn resize(&self, cols: u16, rows: u16) -> Result<(), String> {
        match self {
            LiveSession::Legacy(s) => s.resize(cols, rows),
            LiveSession::V2(s) => {
                s.resize(cols, rows);
                Ok(())
            }
        }
    }

    /// Resolved cwd the child was spawned in. Companion endpoints
    /// match this against `projects.path` to attribute sessions to
    /// workspaces. Returns `String` (not `&str`) so v2's PathBuf
    /// can be lossy-converted without borrowing inside the enum.
    pub fn cwd(&self) -> String {
        match self {
            LiveSession::Legacy(s) => s.cwd.clone(),
            LiveSession::V2(s) => s
                .cwd
                .as_ref()
                .map(PathBuf::as_path)
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default(),
        }
    }

    /// Top-level command string the child runs, if specified.
    /// Legacy stores this verbatim; v2 stores it as `program`.
    pub fn command(&self) -> Option<String> {
        match self {
            LiveSession::Legacy(s) => s.command.clone(),
            LiveSession::V2(s) => s.program.clone(),
        }
    }

    /// Args the child was spawned with. Empty for legacy sessions
    /// (the legacy struct never persisted args; not relevant for the
    /// post-A9 heartbeat path which is v2-only). v2 returns the
    /// stored arg vector — used by smart-launch to find a live PTY
    /// running `--resume <session_id>` for a given heartbeat.
    pub fn args(&self) -> Vec<String> {
        match self {
            LiveSession::Legacy(_) => Vec::new(),
            LiveSession::V2(s) => s.args.clone(),
        }
    }

    /// True if this is a v2 (DaemonPtySession) session. Used by
    /// callers that need to branch on per-renderer behavior — e.g.
    /// kill semantics differ (legacy has `child.kill()`, v2 relies
    /// on dropping the registry Arc to trigger SIGHUP).
    pub fn is_v2(&self) -> bool {
        matches!(self, LiveSession::V2(_))
    }

    /// Whether the session's child PID is still alive.
    ///
    /// Legacy `SessionStreamSession` doesn't yet expose an analogous
    /// flag, so it always returns `true` for now — Kessel-T0 sessions
    /// are explicit user opt-in and the canonical-session reaping
    /// pass for 0.37.0 only needs to handle v2. If reaping logic
    /// later wants to extend to legacy, add the equivalent
    /// `child_exited` AtomicBool to `SessionStreamSession` and route
    /// through here.
    pub fn is_child_alive(&self) -> bool {
        match self {
            LiveSession::Legacy(_) => true,
            LiveSession::V2(s) => s.is_child_alive(),
        }
    }
}

/// Look up an agent across both maps. Legacy first because it's
/// where explicitly-spawned Kessel sessions land; v2 is where every
/// system-driven (Tauri tab, agent panel, background, heartbeat)
/// session lands. The two maps never share an agent_name in steady
/// state (Tauri tabs use `tab-<terminalId>` UUIDs; legacy spawns
/// use the bare agent name), so checking-legacy-first costs nothing.
pub fn lookup_any(agent: &str) -> Option<LiveSession> {
    if let Some(s) = session_map::lookup(agent) {
        return Some(LiveSession::Legacy(s));
    }
    v2_session_map::lookup_by_agent_name(agent).map(LiveSession::V2)
}

/// Look up by `SessionId` rather than agent name. Used by the
/// `/cli/terminal/{write,resize}` HTTP routes — those callers know
/// the session uuid but not the agent name behind it.
pub fn lookup_by_session_id(id: &SessionId) -> Option<LiveSession> {
    if let Some(s) = session_map::lookup_by_session_id(id) {
        return Some(LiveSession::Legacy(s));
    }
    v2_session_map::lookup_by_session_id(id).map(LiveSession::V2)
}

/// Every (agent_name, session) pair across both maps. Order is
/// legacy-first then v2-first; within each map, ordering is
/// unspecified. Returning owned `LiveSession`s lets the caller
/// drop the underlying mutexes before doing expensive per-session
/// work (PTY writes, child kills, frame emits).
pub fn snapshot_all() -> Vec<(String, LiveSession)> {
    let mut out: Vec<(String, LiveSession)> = session_map::snapshot()
        .into_iter()
        .map(|(name, session)| (name, LiveSession::Legacy(session)))
        .collect();
    out.extend(
        v2_session_map::snapshot()
            .into_iter()
            .map(|(name, session)| (name, LiveSession::V2(session))),
    );
    out
}

/// All registered agent names across both maps. De-duped just in
/// case (shouldn't happen in steady state, but harmless).
#[allow(dead_code)]
pub fn list_agents() -> Vec<String> {
    let mut names = session_map::list_agents();
    names.extend(v2_session_map::list_agents());
    names.sort();
    names.dedup();
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `lookup_any` returns None when neither map has the agent.
    /// Sanity check that the cross-map fallthrough doesn't error.
    #[test]
    fn lookup_any_returns_none_for_unknown_agent() {
        let result = lookup_any("nonexistent-agent-xyz-test-only");
        assert!(result.is_none());
    }

    /// `snapshot_all` returns an empty vec when both maps are empty.
    /// Tests are isolated per binary run, so the global maps may
    /// hold entries from other tests in the same process — we don't
    /// assert emptiness, just that the call succeeds without panic.
    #[test]
    fn snapshot_all_does_not_panic() {
        let _ = snapshot_all();
    }
}

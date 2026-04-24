//! Daemon-owned map of `agent_name → Arc<DaemonPtySession>` for
//! Alacritty_v2 sessions.
//!
//! Parallel to `session_map.rs` (which holds Kessel-T0's
//! `SessionStreamSession`). They're kept separate so v1 / Kessel-T0
//! and v2 can coexist during the transition without sharing a
//! heterogeneous map. Post-cleanup (`.k2so/prds/post-landing-cleanup.md`),
//! this may become the only daemon session map.
//!
//! Lifecycle:
//!   - Inserted by `/cli/sessions/v2/spawn` (added in A4).
//!   - Looked up by `/cli/sessions/grid` WS (added in A3) to find
//!     the session a client is trying to attach to.
//!   - Removed on deliberate tab close (via A6 wiring).
//!
//! `DaemonPtySession` is held inside an `Arc` so the WS handler and
//! the map can each retain a handle independently — dropping the
//! last Arc triggers the IO-thread shutdown naturally.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use k2so_core::session::SessionId;
use k2so_core::terminal::DaemonPtySession;

type AgentMap = Arc<Mutex<HashMap<String, Arc<DaemonPtySession>>>>;

static MAP: OnceLock<AgentMap> = OnceLock::new();

fn shared() -> AgentMap {
    MAP.get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
        .clone()
}

/// Register a live v2 session under `agent_name`. If an entry
/// already exists for that name, it's replaced — old holders of
/// the Arc keep working until they drop. Useful for session
/// rebinds (child exits, user re-opens same tab).
pub fn register(agent_name: impl Into<String>, session: Arc<DaemonPtySession>) {
    shared().lock().unwrap().insert(agent_name.into(), session);
}

/// Remove the map entry. Returns the Arc if one was present;
/// subsequent drops of all holders tear the session down.
pub fn unregister(agent_name: &str) -> Option<Arc<DaemonPtySession>> {
    shared().lock().unwrap().remove(agent_name)
}

/// Lookup by agent name. Called on find-or-spawn to decide
/// whether to reuse an existing session.
pub fn lookup_by_agent_name(agent_name: &str) -> Option<Arc<DaemonPtySession>> {
    shared().lock().unwrap().get(agent_name).cloned()
}

/// Lookup by `SessionId`. Iterates the map — O(N) where N is the
/// number of live v2 sessions. Called on every WS grid attach to
/// resolve the requested session. N is expected to stay small
/// (a handful of open Tauri tabs at most).
pub fn lookup_by_session_id(id: &SessionId) -> Option<Arc<DaemonPtySession>> {
    shared()
        .lock()
        .unwrap()
        .values()
        .find(|s| s.session_id == *id)
        .cloned()
}

/// Every registered (agent_name, session) pair. Returning owned
/// Arcs lets the caller drop the map lock before doing expensive
/// work against the sessions. Ordering is unspecified.
pub fn snapshot() -> Vec<(String, Arc<DaemonPtySession>)> {
    shared()
        .lock()
        .unwrap()
        .iter()
        .map(|(name, session)| (name.clone(), Arc::clone(session)))
        .collect()
}

/// All registered agent names. Used by diagnostic endpoints.
#[allow(dead_code)]
pub fn list_agents() -> Vec<String> {
    shared().lock().unwrap().keys().cloned().collect()
}

/// Test helper — drop every registered entry. Keeps tests that
/// share the global map from contaminating each other.
#[cfg(test)]
pub fn clear_for_tests() {
    shared().lock().unwrap().clear();
}

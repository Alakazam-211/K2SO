//! Daemon-owned map of agent name → running session.
//!
//! F1 of Phase 3.1. When the daemon spawns a Session Stream session
//! on behalf of an agent (via F2's `/cli/sessions/spawn` endpoint),
//! it inserts the `SessionStreamSession` handle here keyed by
//! agent name. The daemon-side `InjectProvider` (in `providers.rs`)
//! looks agents up in this map to resolve "write to bar's PTY" into
//! an actual `session.write(bytes)` call.
//!
//! **Why not in k2so-core?** The core's `session::registry` maps
//! SessionId → SessionEntry (broadcast channel metadata). This map
//! is agent-name → PTY writer — a daemon-specific concern that's
//! the inverse lookup, and conflating them would leak daemon
//! session-ownership concepts into the library.
//!
//! **Lifecycle.** Insert on daemon session spawn; remove on session
//! exit. Holders of the `Arc<SessionStreamSession>` (e.g. the
//! inject path mid-write) keep the session alive until they drop
//! their clone — matches how `session::registry` handles unregister.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use k2so_core::terminal::SessionStreamSession;

type AgentMap = Arc<Mutex<HashMap<String, Arc<SessionStreamSession>>>>;

static MAP: OnceLock<AgentMap> = OnceLock::new();

fn shared() -> AgentMap {
    MAP.get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
        .clone()
}

/// Register a live session under `agent_name`. If an entry already
/// exists for this name, it's replaced — old holders of the Arc
/// keep working until they drop. Useful for session rebinds (agent
/// restarts its session; new one takes over the inject target).
pub fn register(agent_name: impl Into<String>, session: Arc<SessionStreamSession>) {
    shared().lock().unwrap().insert(agent_name.into(), session);
}

/// Remove the map entry for `agent_name`. Returns the Arc if one
/// was present; subsequent drop by the caller + any concurrent
/// holders cleans up the session when the last reference goes
/// away.
pub fn unregister(agent_name: &str) -> Option<Arc<SessionStreamSession>> {
    shared().lock().unwrap().remove(agent_name)
}

/// Lookup by agent name. Returns `None` if no session is registered
/// for that agent. Called on every `DaemonInjectProvider::inject`.
pub fn lookup(agent_name: &str) -> Option<Arc<SessionStreamSession>> {
    shared().lock().unwrap().get(agent_name).cloned()
}

/// Every registered agent name. Sorting is the caller's problem.
/// Used by the roster-query path and by diagnostic CLI output.
pub fn list_agents() -> Vec<String> {
    shared().lock().unwrap().keys().cloned().collect()
}

/// Every registered (agent_name, session) pair. The watchdog
/// iterates this every poll tick; returning owned Arcs lets the
/// watchdog drop the map lock immediately and do expensive work
/// (PTY writes, child kills, SemanticEvent emission) without
/// holding it. Ordering is unspecified.
pub fn snapshot() -> Vec<(String, Arc<SessionStreamSession>)> {
    shared()
        .lock()
        .unwrap()
        .iter()
        .map(|(name, session)| (name.clone(), Arc::clone(session)))
        .collect()
}

/// Test helper — drop every registered entry. Keeps tests that
/// share the global map from contaminating each other. Only
/// compiled in test / with `test-util` feature on k2so-core.
#[cfg(test)]
pub fn clear_for_tests() {
    shared().lock().unwrap().clear();
}

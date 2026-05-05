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

/// Register a live v2 session under `agent_name`.
///
/// **Project-namespaced keys (0.36.14+).** When `agent_name` is in the
/// form `<project_id>:<bare_name>` (chat tab spawn from a workspace),
/// we register under the prefixed key AND mirror to the bare name for
/// back-compat with bare-keyed lookups (awareness bus inject by agent
/// name, e.g. `k2so msg --wake manager`). The bare slot is
/// last-write-wins — the most recently registered "manager" is what
/// `lookup_by_agent_name("manager")` returns. Workspace-specific
/// lookups via the prefixed key always resolve to the right session,
/// preventing the cross-workspace pinned-chat collision where two
/// workspaces both running `manager` mode would share one PTY.
///
/// Bare-form `agent_name` (no `:`) registers normally — used by
/// heartbeat-surfaced sessions, worktree chats, and other code paths
/// that don't carry a workspace context.
pub fn register(agent_name: impl Into<String>, session: Arc<DaemonPtySession>) {
    let key = agent_name.into();
    let map_arc = shared();
    let mut map = map_arc.lock().unwrap();
    map.insert(key.clone(), Arc::clone(&session));
    // Mirror to bare key when prefixed, so legacy lookups keep working.
    if let Some((_pid, bare)) = key.split_once(':') {
        if !bare.is_empty() {
            map.insert(bare.to_string(), session);
        }
    }
}

/// Remove the map entry. Returns the Arc if one was present;
/// subsequent drops of all holders tear the session down.
///
/// Also runs the heartbeat-active-session cleanup path: any
/// `agent_heartbeats` row whose `active_terminal_id` matches the
/// removed session's id is nulled, and the matching `agent_sessions`
/// row gets `surfaced=0` + `status='sleeping'`. This is the single
/// chokepoint for "v2 session goes away" — child-exit observer in
/// v2_spawn invokes us, the explicit /v2/close route invokes us, the
/// watchdog escalation path invokes us. See the
/// `heartbeat-active-session-tracking` PRD.
///
/// **Project-namespaced cleanup (0.36.14+).** When the key is the
/// prefixed form `<project_id>:<bare_name>`, we (a) remove the prefixed
/// entry, (b) remove the bare-name mirror only if it still points at
/// the same session (otherwise another workspace's session has taken
/// the bare slot — leave it alone), and (c) scope the
/// `agent_sessions` UPDATE by `project_id` so closing one workspace's
/// chat doesn't sleep another workspace's row.
pub fn unregister(agent_name: &str) -> Option<Arc<DaemonPtySession>> {
    let map_arc = shared();
    let mut map = map_arc.lock().unwrap();
    let removed = map.remove(agent_name);
    // If this was a prefixed key and the bare-name mirror still points
    // at this exact session, remove it too. If it points at a different
    // session (a newer workspace took the bare slot), leave it alone.
    if let Some(ref session) = removed {
        if let Some((_pid, bare)) = agent_name.split_once(':') {
            if !bare.is_empty() {
                let same = map
                    .get(bare)
                    .map(|s| Arc::ptr_eq(s, session))
                    .unwrap_or(false);
                if same {
                    map.remove(bare);
                }
            }
        }
    }
    drop(map);

    if let Some(ref session) = removed {
        let terminal_id = session.session_id.to_string();
        let db = k2so_core::db::shared();
        let conn = db.lock();
        let _ = k2so_core::db::schema::AgentHeartbeat::clear_active_terminal_id_by_terminal(
            &conn,
            &terminal_id,
        );
        // Mirror of the heartbeat cleanup above (migration 0037): the
        // chat tab's pinned `agent_sessions` row stamps its own
        // `active_terminal_id` on v2 spawn under the workspace agent's
        // canonical name. PTY exit nulls it here so the next mount's
        // `/cli/sessions/lookup-by-agent` lookup sees the truth.
        let _ = k2so_core::db::schema::AgentSession::clear_active_terminal_id_by_terminal(
            &conn,
            &terminal_id,
        );
        // Flip the `agent_sessions` row for this agent. When the key is
        // the prefixed `<project_id>:<bare>` form, scope by project_id
        // so we don't sleep rows belonging to other workspaces that
        // share the same agent name (e.g., multiple workspaces in
        // Workspace Manager mode all using `agent_name='manager'`).
        if let Some((pid, bare)) = agent_name.split_once(':') {
            if !pid.is_empty() && !bare.is_empty() {
                let _ = conn.execute(
                    "UPDATE agent_sessions SET surfaced = 0, status = 'sleeping' \
                     WHERE project_id = ?1 AND agent_name = ?2",
                    rusqlite::params![pid, bare],
                );
                return removed;
            }
        }
        // Legacy bare-form (heartbeat-surfaced, worktree chat, etc.) —
        // keep prior behavior. The (project_id, agent_name) UNIQUE
        // constraint ensures at most one row per (workspace, agent),
        // but with multiple workspaces this UPDATE can hit several
        // rows; that's the legacy contract for non-chat-tab callers
        // and is preserved for back-compat.
        let _ = conn.execute(
            "UPDATE agent_sessions SET surfaced = 0, status = 'sleeping' \
             WHERE agent_name = ?1",
            rusqlite::params![agent_name],
        );
    }
    removed
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

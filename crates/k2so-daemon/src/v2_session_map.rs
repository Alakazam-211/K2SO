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
/// 0.37.0 retired the 0.36.14 bare-name mirror: workspace-agent
/// sessions are keyed on `<project_id>:<bare>` exclusively, so the
/// awareness bus and CLI lookups always carry workspace context.
/// Worktree chats and ad-hoc Cmd+T tabs register under their own
/// terminal-id-shaped keys; nothing depends on a bare-name slot.
pub fn register(agent_name: impl Into<String>, session: Arc<DaemonPtySession>) {
    let key = agent_name.into();
    let map_arc = shared();
    let mut map = map_arc.lock().unwrap();
    map.insert(key, session);
}

/// Remove the map entry. Returns the Arc if one was present;
/// subsequent drops of all holders tear the session down.
///
/// Runs the active-session cleanup path: any `agent_heartbeats` or
/// `workspace_sessions` row whose `active_terminal_id` matches the
/// removed session's id is nulled, and the matching workspace's row
/// gets `surfaced=0` + `status='sleeping'`. This is the single
/// chokepoint for "v2 session goes away" — child-exit observer in
/// v2_spawn invokes us, the explicit /v2/close route invokes us, the
/// watchdog escalation path invokes us. See the
/// `heartbeat-active-session-tracking` PRD.
///
/// 0.37.0: with `workspace_sessions` keyed on `project_id` and the
/// `agent_name` column gone, the cleanup is keyed entirely on the
/// terminal_id we just stopped. The pre-0.37.0 dual-cleanup logic
/// (prefix split → scoped UPDATE by `(project_id, agent_name)`) is
/// retired.
pub fn unregister(agent_name: &str) -> Option<Arc<DaemonPtySession>> {
    let map_arc = shared();
    let removed = {
        let mut map = map_arc.lock().unwrap();
        map.remove(agent_name)
    };

    if let Some(ref session) = removed {
        let terminal_id = session.session_id.to_string();
        let db = k2so_core::db::shared();
        let conn = db.lock();
        let _ = k2so_core::db::schema::AgentHeartbeat::clear_active_terminal_id_by_terminal(
            &conn,
            &terminal_id,
        );
        // Mirror of the heartbeat cleanup above (migration 0037): the
        // chat tab's pinned workspace_sessions row stamps its own
        // active_terminal_id on v2 spawn. PTY exit nulls it here so
        // the next mount's `/cli/sessions/lookup-by-agent` sees the
        // truth.
        let _ = k2so_core::db::schema::WorkspaceSession::clear_active_terminal_id_by_terminal(
            &conn,
            &terminal_id,
        );
        // Flip surfaced=0 + status=sleeping for the workspace whose
        // active_terminal_id matched. Targeting by terminal_id (rather
        // than (project_id, agent_name)) means this single UPDATE
        // covers every code path — chat tab, heartbeat headless wake,
        // worktree chat — without needing to know which kind of
        // session this was.
        let _ = conn.execute(
            "UPDATE workspace_sessions SET surfaced = 0, status = 'sleeping' \
             WHERE terminal_id = ?1 OR active_terminal_id = ?1",
            rusqlite::params![terminal_id],
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
/// Drop every registered entry. Available to both unit tests
/// (in this module) and integration tests (in `tests/*.rs`) so
/// shared global state doesn't leak between cases.
pub fn clear_for_tests() {
    shared().lock().unwrap().clear();
}

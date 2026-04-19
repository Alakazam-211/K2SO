use parking_lot::{Mutex, ReentrantMutex};
use std::collections::HashMap;
use std::sync::Arc;

/// Shared process-wide application state injected into every Tauri
/// command as `State<AppState>`. The `db` handle is an `Arc` wrapper so
/// it can be cloned into the module-level `crate::db::SHARED` static —
/// guaranteeing ad-hoc CLI/HTTP code paths operate on the same physical
/// SQLite connection (and therefore the same in-memory write queue) as
/// Tauri commands. Prior to this refactor, 60+ call sites each opened
/// their own transient connection, defeating WAL write serialization
/// and producing silent `SQLITE_BUSY` drops under parallel delegations.
///
/// The connection sits behind a `ReentrantMutex` because the helper-
/// calls-helper pattern is pervasive in k2so_agents: a Tauri command
/// takes the lock, calls `find_primary_agent()`, which takes the lock
/// again. A plain `Mutex` would deadlock the UI thread on first such
/// call (observed as a macOS beachball). Re-entrant semantics let the
/// same thread re-acquire without blocking itself while still
/// serializing across threads. rusqlite methods only need `&Connection`
/// so `ReentrantMutex`'s read-only guard suffices.
pub struct AppState {
    pub db: Arc<ReentrantMutex<rusqlite::Connection>>,
    pub terminal_manager: Mutex<crate::terminal::TerminalManager>,
    pub llm_manager: Mutex<crate::llm::LlmManager>,
    pub watchers: Mutex<HashMap<String, notify::RecommendedWatcher>>,
}

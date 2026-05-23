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
///
/// **Post-Phase-2 shrinkage**: Units 1/2/3 removed the companion,
/// LLM, and terminal_manager fields respectively. AppState now
/// carries only `db` + `watchers`. Unit 4 removes `db`; once it
/// lands, AppState should collapse to just `watchers` (and could
/// be inlined into `watcher.rs` as a module static). Don't delete
/// `state.rs` until Unit 4 finishes — the `db` field is still
/// load-bearing for every other command surface.
pub struct AppState {
    pub db: Arc<ReentrantMutex<rusqlite::Connection>>,
    // Phase 2 Unit 2 — `llm_manager: Arc<Mutex<llm::LlmManager>>`
    // removed. LLM inference + model lifecycle moved to k2so-daemon
    // (see `crates/k2so-daemon/src/llm_host.rs`); Tauri no longer
    // holds a handle.
    // Phase 2 Unit 3 — `terminal_manager: Arc<Mutex<TerminalManager>>`
    // removed. The daemon's `terminal_event_sink::register()` +
    // `/cli/terminal/*` routes own PTY lifecycle now. The
    // TerminalManager itself is still a k2so-core singleton
    // (`k2so_core::terminal::shared()`) accessed by daemon route
    // handlers — Tauri just no longer holds a handle.
    pub watchers: Mutex<HashMap<String, notify::RecommendedWatcher>>,
}

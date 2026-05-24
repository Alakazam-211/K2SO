use parking_lot::Mutex;
use std::collections::HashMap;

/// Shared process-wide application state injected into every Tauri
/// command as `State<AppState>`.
///
/// **Post-Phase-2 shrinkage**: Units 1/2/3 removed the companion,
/// LLM, and terminal_manager fields respectively. Unit 4 removed the
/// `db` field — every DB-writing surface in src-tauri now routes
/// through `crate::daemon_client::DaemonClient` and the daemon's
/// `/cli/*` HTTP routes (the daemon is the sole SQLite writer post
/// Phase 2). The struct is reduced to a single `watchers` field
/// because `watcher.rs` registers `notify::RecommendedWatcher`
/// handles per filesystem path and tears them down on `fs_unwatch_dir`.
/// Inlining `watchers` into `watcher.rs` as a module-level static is
/// an option but kept here so existing `app.state::<AppState>()`
/// lookups in `watcher.rs` keep working without churn.
pub struct AppState {
    pub watchers: Mutex<HashMap<String, notify::RecommendedWatcher>>,
}

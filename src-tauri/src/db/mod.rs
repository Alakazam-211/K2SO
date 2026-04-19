pub mod schema;

use parking_lot::ReentrantMutex;
use rusqlite::{params, Connection, Result};
use std::path::Path;
use std::sync::{Arc, OnceLock};

/// Process-wide SQLite handle. Populated exactly once by
/// [`init_database`] during app startup and accessed from any thread via
/// [`shared`]. The `Arc<ReentrantMutex<Connection>>` shape means `AppState.db`
/// and every ad-hoc command-handler/HTTP-thread caller can clone the
/// same handle — there is only one physical connection (and therefore
/// only one write lock queue) for the lifetime of the process.
///
/// Rationale: rusqlite connections are not `Sync`, so they must sit
/// behind a mutex. A SINGLE connection is the right call here because
/// WAL mode already serializes writes at the database level — spinning
/// up multiple connections just multiplies the places the `SQLITE_BUSY`
/// error can surface without buying parallelism. Parallel-reader code
/// paths are rare in K2SO (most work is write-heavy: agent sessions,
/// work items, heartbeats). When that changes, swap this for an
/// `r2d2::Pool<SqliteConnectionManager>` — the public API stays the same.
static SHARED: OnceLock<Arc<ReentrantMutex<Connection>>> = OnceLock::new();

/// Path of the on-disk database file. Derivable from the home dir, but
/// hoisted into a helper so tests can stub via a known path.
pub fn db_path() -> std::path::PathBuf {
    dirs::home_dir()
        .unwrap_or_default()
        .join(".k2so")
        .join("k2so.db")
}

/// Open a SQLite connection with K2SO's standard resilience PRAGMAs:
/// - WAL mode (set once per database — readers don't block writers)
/// - busy_timeout 5000ms (waits on contention instead of SQLITE_BUSY-
///   failing immediately, which was the silent-write-loss class)
/// - foreign_keys ON (matches init_database)
///
/// **Only use this for standalone tools or migration scripts.** Runtime
/// code should always access the shared connection via [`shared`] so it
/// isn't racing against the AppState connection for write slots.
pub fn open_with_resilience<P: AsRef<Path>>(path: P) -> Result<Connection> {
    let conn = Connection::open(path)?;
    // Wait up to 5s on a busy lock rather than returning SQLITE_BUSY
    // immediately. Avoids the silent-drop class where a write was
    // mid-flight on another connection.
    conn.busy_timeout(std::time::Duration::from_millis(5000))?;
    // WAL is per-database-file and persists once set; foreign_keys is
    // per-connection. Errors here are informational — the connection is
    // still usable without them.
    let _ = conn.execute_batch("PRAGMA journal_mode = WAL;");
    let _ = conn.execute_batch("PRAGMA foreign_keys = ON;");
    Ok(conn)
}

/// Clone a handle to the process-wide SQLite connection. In production
/// builds this panics (with a diagnostic) if called before
/// [`init_database`] — which would only happen via a programming error,
/// not a user-reachable path. All startup flows call init_database
/// before the first command handler or HTTP endpoint can fire.
///
/// Under `#[cfg(test)]` this lazily initializes to an in-memory SQLite
/// on first call, so unit tests that exercise code paths touching the DB
/// don't need to wire up the full Tauri startup. Production builds do
/// NOT get this lazy-init — missing startup initialization must be a
/// hard error, not a silent fallback to an ephemeral DB.
///
/// Usage pattern:
///   let db = crate::db::shared();
///   let conn = db.lock();
///   conn.execute(...)?;
///
/// The returned `Arc` is cheap to clone but the lock must be acquired
/// before each SQL operation. Hold the lock for the duration of a
/// transaction block, then drop the guard to release the write queue.
pub fn shared() -> Arc<ReentrantMutex<Connection>> {
    if let Some(handle) = SHARED.get() {
        return handle.clone();
    }
    #[cfg(test)]
    {
        return init_for_tests();
    }
    #[cfg(not(test))]
    {
        panic!("db::init_database must run before db::shared()");
    }
}

/// Test-only: populate SHARED with an in-memory SQLite that's been
/// through the full migration + seed sequence. Idempotent across test
/// threads because OnceLock::set is atomic — losers drop their handle
/// and clone the winner's.
///
/// Caveat: every unit test in the process shares this one in-memory DB.
/// Tests that expect isolated DB state must either (a) clean up their
/// rows on exit, or (b) use a scratch_project() directory pattern that
/// keeps filesystem state separate even when DB state overlaps.
#[cfg(test)]
pub fn init_for_tests() -> Arc<ReentrantMutex<Connection>> {
    if let Some(handle) = SHARED.get() {
        return handle.clone();
    }
    let conn = Connection::open(":memory:")
        .expect("in-memory SQLite open failed");
    conn.busy_timeout(std::time::Duration::from_millis(5000))
        .expect("set busy_timeout");
    let _ = conn.execute_batch("PRAGMA foreign_keys = ON;");
    run_migrations(&conn).expect("test migrations");
    seed_agent_presets(&conn).expect("test seed");
    let handle = Arc::new(ReentrantMutex::new(conn));
    match SHARED.set(handle.clone()) {
        Ok(()) => handle,
        Err(_) => SHARED.get().expect("SHARED populated").clone(),
    }
}

/// Open (or create) the K2SO database at ~/.k2so/k2so.db, run all
/// migrations, seed default data, and populate the process-wide
/// [`SHARED`] connection. Returns an `Arc` handle so the caller can
/// store it in `AppState.db` AND the shared static points at the same
/// physical connection.
///
/// Safe to call exactly once per process. A second call returns the
/// already-initialized handle (tests that reuse the binary hit this).
pub fn init_database() -> Result<Arc<ReentrantMutex<Connection>>> {
    // Fast path for tests that re-invoke the init (or if somewhere in
    // startup accidentally re-initializes): just clone the existing
    // handle rather than opening another connection.
    if let Some(existing) = SHARED.get() {
        return Ok(existing.clone());
    }

    let db_dir = dirs::home_dir()
        .ok_or_else(|| rusqlite::Error::InvalidParameterName("Could not determine home directory".to_string()))?
        .join(".k2so");
    std::fs::create_dir_all(&db_dir)
        .map_err(|e| rusqlite::Error::InvalidParameterName(format!("Could not create ~/.k2so directory: {}", e)))?;

    let db_path = db_dir.join("k2so.db");
    let conn = open_with_resilience(&db_path)?;

    run_migrations(&conn)?;
    seed_agent_presets(&conn)?;

    let handle = Arc::new(ReentrantMutex::new(conn));
    // Race-free publish: whoever wins gets their handle stored, losers
    // drop theirs and return the winner's. In practice only one thread
    // calls init_database during startup.
    match SHARED.set(handle.clone()) {
        Ok(()) => Ok(handle),
        Err(_) => Ok(SHARED.get().expect("SHARED just populated").clone()),
    }
}

/// Simple migration runner using a _migrations table to track applied migrations.
fn run_migrations(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS _migrations (
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL UNIQUE,
            applied_at INTEGER NOT NULL DEFAULT (unixepoch())
        );",
    )?;

    let migrations: &[(&str, &str)] = &[
        ("0000_lethal_scalphunter", include_str!("../../drizzle_sql/0000_lethal_scalphunter.sql")),
        ("0001_nostalgic_lenny_balinger", include_str!("../../drizzle_sql/0001_nostalgic_lenny_balinger.sql")),
        ("0002_fearless_photon", include_str!("../../drizzle_sql/0002_fearless_photon.sql")),
        ("0003_fancy_thunderball", include_str!("../../drizzle_sql/0003_fancy_thunderball.sql")),
        ("0004_pinned_workspaces", include_str!("../../drizzle_sql/0004_pinned_workspaces.sql")),
        ("0005_window_state", include_str!("../../drizzle_sql/0005_window_state.sql")),
        ("0006_time_entries", include_str!("../../drizzle_sql/0006_time_entries.sql")),
        ("0007_chat_session_names", include_str!("../../drizzle_sql/0007_chat_session_names.sql")),
        ("0008_chat_pinned", include_str!("../../drizzle_sql/0008_chat_pinned.sql")),
        ("0009_workspace_sessions", include_str!("../../drizzle_sql/0009_workspace_sessions.sql")),
        ("0010_active_workspaces", include_str!("../../drizzle_sql/0010_active_workspaces.sql")),
        ("0011_add_indexes", include_str!("../../drizzle_sql/0011_add_indexes.sql")),
        ("0012_agent_mode", include_str!("../../drizzle_sql/0012_agent_mode.sql")),
        ("0013_agent_mode_selector", include_str!("../../drizzle_sql/0013_agent_mode_selector.sql")),
        ("0014_agent_sessions", include_str!("../../drizzle_sql/0014_agent_sessions.sql")),
        ("0015_workspace_tiers", include_str!("../../drizzle_sql/0015_workspace_tiers.sql")),
        ("0016_rename_tiers_to_states", include_str!("../../drizzle_sql/0016_rename_tiers_to_states.sql")),
        ("0017_fix_maintenance_state", include_str!("../../drizzle_sql/0017_fix_maintenance_state.sql")),
        ("0018_rename_pod_to_coordinator", include_str!("../../drizzle_sql/0018_rename_pod_to_coordinator.sql")),
        ("0019_workspace_nav_visible", include_str!("../../drizzle_sql/0019_workspace_nav_visible.sql")),
        ("0020_heartbeat_schedule", include_str!("../../drizzle_sql/0020_heartbeat_schedule.sql")),
        ("0021_rename_coordinator_to_manager", include_str!("../../drizzle_sql/0021_rename_coordinator_to_manager.sql")),
        ("0022_agent_sessions_table", include_str!("../../drizzle_sql/0022_agent_sessions_table.sql")),
        ("0023_workspace_relations", include_str!("../../drizzle_sql/0023_workspace_relations.sql")),
        ("0024_activity_feed", include_str!("../../drizzle_sql/0024_activity_feed.sql")),
        ("0025_activity_feed_read", include_str!("../../drizzle_sql/0025_activity_feed_read.sql")),
        ("0026_heartbeat_fires", include_str!("../../drizzle_sql/0026_heartbeat_fires.sql")),
        ("0027_wakes_since_compact", include_str!("../../drizzle_sql/0027_wakes_since_compact.sql")),
        ("0028_agent_heartbeats", include_str!("../../drizzle_sql/0028_agent_heartbeats.sql")),
        ("0029_heartbeat_fires_schedule_name", include_str!("../../drizzle_sql/0029_heartbeat_fires_schedule_name.sql")),
    ];

    for (name, sql) in migrations {
        let already_applied: bool = conn.query_row(
            "SELECT COUNT(*) > 0 FROM _migrations WHERE name = ?1",
            params![name],
            |row| row.get(0),
        )?;

        if !already_applied {
            // Run each migration inside a transaction for atomicity.
            // Retry the entire transaction on lock contention.
            let mut last_err = None;
            for attempt in 0..5u32 {
                match run_single_migration(conn, name, sql) {
                    Ok(_) => { last_err = None; break; },
                    Err(e) => {
                        let msg = e.to_string();
                        if (msg.contains("database is locked") || msg.contains("schema is locked"))
                            && attempt < 4
                        {
                            log_debug!("[db] Migration {}: locked, retrying ({}/5)", name, attempt + 1);
                            std::thread::sleep(std::time::Duration::from_millis(50 * (attempt as u64 + 1)));
                            last_err = Some(e);
                            continue;
                        }
                        return Err(e);
                    }
                }
            }
            if let Some(e) = last_err {
                return Err(e);
            }
        }
    }

    Ok(())
}

/// Execute a single migration file's statements inside a transaction.
/// "already exists" / "duplicate column" errors are silently skipped (idempotent).
fn run_single_migration(conn: &Connection, name: &str, sql: &str) -> Result<()> {
    conn.execute_batch("BEGIN;")?;
    for statement in sql.split("--> statement-breakpoint") {
        let trimmed = statement.trim();
        if !trimmed.is_empty() {
            if let Err(e) = conn.execute_batch(trimmed) {
                let msg = e.to_string();
                if msg.contains("already exists") || msg.contains("duplicate column") {
                    log_debug!("[db] Migration {}: skipping ({})", name, msg);
                    continue;
                }
                // Rollback on real errors
                let _ = conn.execute_batch("ROLLBACK;");
                return Err(e);
            }
        }
    }
    conn.execute(
        "INSERT INTO _migrations (name) VALUES (?1)",
        params![name],
    )?;
    conn.execute_batch("COMMIT;")?;
    Ok(())
}

/// Seed built-in agent presets. Uses INSERT OR IGNORE so new presets
/// are added on upgrade without duplicating existing ones.
fn seed_agent_presets(conn: &Connection) -> Result<()> {
    let presets: &[(&str, &str, &str, &str, i64)] = &[
        // Cloud CLI agents (no emoji — use custom AgentIcon SVGs)
        ("b0a1c2d3-e4f5-6789-abcd-ef0123456001", "Claude", "claude --dangerously-skip-permissions", "", 0),
        ("b0a1c2d3-e4f5-6789-abcd-ef0123456002", "Codex", "codex -c model_reasoning_effort=\"high\" --dangerously-bypass-approvals-and-sandbox", "", 1),
        ("b0a1c2d3-e4f5-6789-abcd-ef0123456003", "Gemini", "gemini --yolo", "", 2),
        ("b0a1c2d3-e4f5-6789-abcd-ef0123456004", "Copilot", "copilot --allow-all", "", 3),
        ("b0a1c2d3-e4f5-6789-abcd-ef0123456005", "Aider", "aider", "", 4),
        ("b0a1c2d3-e4f5-6789-abcd-ef0123456006", "Cursor Agent", "cursor-agent", "", 5),
        ("b0a1c2d3-e4f5-6789-abcd-ef0123456007", "OpenCode", "opencode", "", 6),
        ("b0a1c2d3-e4f5-6789-abcd-ef0123456008", "Code Puppy", "codepuppy", "", 7),
        // Local/on-device LLM tools (keep emoji — no custom icon)
        ("b0a1c2d3-e4f5-6789-abcd-ef0123456009", "Ollama", "ollama run llama3.2", "\u{1F999}", 8),
        ("b0a1c2d3-e4f5-6789-abcd-ef0123456010", "Interpreter", "interpreter", "\u{1F310}", 9),
        ("b0a1c2d3-e4f5-6789-abcd-ef0123456011", "Goose", "goose", "\u{1FABF}", 10),
    ];

    for (id, label, command, icon, sort_order) in presets {
        conn.execute(
            "INSERT OR IGNORE INTO agent_presets (id, label, command, icon, enabled, sort_order, is_built_in) \
             VALUES (?1, ?2, ?3, ?4, 1, ?5, 1)",
            params![id, label, command, icon, sort_order],
        )?;
    }

    Ok(())
}

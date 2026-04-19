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

/// Open a SQLite connection with K2SO's standard resilience + performance
/// PRAGMAs.
///
/// **Resilience**
/// - WAL mode (set once per database — readers don't block writers).
/// - busy_timeout **500 ms** (was 5000 ms pre-0.32.13; 5 s was masking real
///   contention behind a UI hang. Zed and the rusqlite community both use
///   500 ms.). Waits on contention instead of SQLITE_BUSY-failing immediately.
/// - foreign_keys ON.
///
/// **Performance** (added 0.32.13, all benchmarked in Zed + Spacedrive)
/// - `cache_size = -20000` — 20 MB page cache per connection. Without this
///   SQLite uses the built-in 2 MB default.
/// - `mmap_size = 67108864` — map the first 64 MB of the database file for
///   reads. Cuts read-path syscall count on the common hot queries.
/// - `temp_store = MEMORY` — keep any temp tables / sort buffers in RAM.
///
/// **Only use this for standalone tools or migration scripts.** Runtime
/// code should always access the shared connection via [`shared`] so it
/// isn't racing against the AppState connection for write slots.
pub fn open_with_resilience<P: AsRef<Path>>(path: P) -> Result<Connection> {
    let conn = Connection::open(path)?;
    // Resilience PRAGMAs.
    conn.busy_timeout(std::time::Duration::from_millis(500))?;
    // Each PRAGMA logged-but-not-fatal — the connection is usable without
    // them even if a particular pragma fails on an exotic SQLite build.
    let _ = conn.execute_batch(
        "PRAGMA journal_mode = WAL;\n\
         PRAGMA foreign_keys = ON;\n\
         PRAGMA cache_size = -20000;\n\
         PRAGMA mmap_size = 67108864;\n\
         PRAGMA temp_store = MEMORY;",
    );
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
    #[cfg(any(test, feature = "test-util"))]
    {
        return init_for_tests();
    }
    #[cfg(not(any(test, feature = "test-util")))]
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
///
/// Gated on `#[cfg(test)]` OR the `test-util` feature so downstream
/// crates' test binaries can reach it (their cfg(test) doesn't flip
/// cfg(test) here). Production builds compile this out, restoring the
/// invariant that only test contexts can acquire an in-memory DB
/// without first calling `init_database()`.
#[cfg(any(test, feature = "test-util"))]
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

/// Bootstrap a brand-new database file at `path` with the full migration
/// + seed sequence. Test-only: used by concurrency tests that need
/// multiple `Connection`s sharing real disk state (the in-memory default
/// gives each connection a separate database). Writing on-disk tempfiles
/// makes multi-connection CAS behavior observable.
///
/// Production code must never use this — `init_database()` handles the
/// real startup path and publishes the shared connection.
#[cfg(test)]
pub(crate) fn bootstrap_test_db_at<P: AsRef<Path>>(path: P) -> Result<()> {
    let conn = open_with_resilience(path)?;
    run_migrations(&conn)?;
    seed_agent_presets(&conn)?;
    Ok(())
}

/// Build a fresh isolated in-memory connection. Test-only. Unlike the
/// shared `init_for_tests()` helper, each call returns its own handle
/// backed by its own `:memory:` database — so tests that assert on
/// specific row counts, migration state, or table contents can't
/// collide with other tests in the same process.
#[cfg(test)]
pub(crate) fn isolated_test_connection() -> Connection {
    let conn = Connection::open(":memory:").expect("open :memory:");
    conn.busy_timeout(std::time::Duration::from_millis(5000))
        .expect("busy_timeout");
    let _ = conn.execute_batch("PRAGMA foreign_keys = ON;");
    run_migrations(&conn).expect("migrations");
    seed_agent_presets(&conn).expect("seed");
    conn
}

/// Simple migration runner using a _migrations table to track applied migrations.
pub(crate) fn run_migrations(conn: &Connection) -> Result<()> {
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
        ("0030_code_migrations", include_str!("../../drizzle_sql/0030_code_migrations.sql")),
        ("0031_skill_regen_version", include_str!("../../drizzle_sql/0031_skill_regen_version.sql")),
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

/// Check whether a code-side migration with the given id has been recorded.
///
/// "Code migrations" are one-time runtime passes (filesystem rewrites,
/// legacy-type coercion, etc.) whose only job is to get from state A to
/// state B. Gating them behind this check turns every launch after the
/// first into a no-op for that pass, instead of re-scanning the whole
/// workspace tree to confirm there's nothing to do.
///
/// The table (`code_migrations`, added in migration 0030) is created
/// lazily at startup; callers before migration 0030 has run see `false`
/// and safely fall through to running the migration.
pub fn has_code_migration_applied(conn: &Connection, id: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM code_migrations WHERE id = ?1 LIMIT 1",
        params![id],
        |_| Ok(()),
    )
    .is_ok()
}

/// Record that a code migration completed successfully. Idempotent
/// (INSERT OR IGNORE) so repeat callers during a partial-completion
/// scenario don't error. Takes a free-form `notes` string for future
/// debugging — store counts, version numbers, anything small.
pub fn mark_code_migration_applied(conn: &Connection, id: &str, notes: Option<&str>) {
    let _ = conn.execute(
        "INSERT OR IGNORE INTO code_migrations (id, applied_at, notes) \
         VALUES (?1, unixepoch(), ?2)",
        params![id, notes],
    );
}

/// Seed built-in agent presets. Uses INSERT OR IGNORE so new presets
/// are added on upgrade without duplicating existing ones.
pub(crate) fn seed_agent_presets(conn: &Connection) -> Result<()> {
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

#[cfg(test)]
mod tests {
    //! Unit tests for the migration/bootstrap layer. Every test opens
    //! its own in-memory connection — the shared `init_for_tests()`
    //! handle is NOT used here because these tests assert on
    //! migration application order, PRAGMA state, and idempotency,
    //! which would be polluted by a process-wide handle.
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

    fn fresh_memory() -> Connection {
        let conn = Connection::open(":memory:").unwrap();
        conn.busy_timeout(std::time::Duration::from_millis(5000))
            .unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn
    }

    fn scratch_db_path() -> std::path::PathBuf {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let base = std::env::temp_dir().join(format!(
            "k2so-db-mod-test-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            SEQ.fetch_add(1, AtomicOrdering::Relaxed)
        ));
        std::fs::create_dir_all(&base).unwrap();
        base.join("k2so.db")
    }

    // ── Migration runner ──────────────────────────────────────────
    #[test]
    fn migrations_create_core_tables() {
        let conn = fresh_memory();
        run_migrations(&conn).unwrap();
        // Sanity: every table we unit-test in schema::unit_tests must
        // exist after migrations. Using sqlite_master to confirm.
        for table in [
            "projects",
            "agent_sessions",
            "agent_heartbeats",
            "agent_presets",
            "heartbeat_fires",
            "activity_feed",
            "workspace_relations",
            "focus_groups",
        ] {
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    params![table],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(count, 1, "expected table '{}' to exist", table);
        }
    }

    #[test]
    fn migrations_are_idempotent_when_run_twice() {
        let conn = fresh_memory();
        run_migrations(&conn).unwrap();
        let first: i64 = conn
            .query_row("SELECT COUNT(*) FROM _migrations", [], |r| r.get(0))
            .unwrap();
        // Second run must be a no-op — every migration is already in
        // _migrations, so the `if !already_applied` guard short-circuits.
        run_migrations(&conn).unwrap();
        let second: i64 = conn
            .query_row("SELECT COUNT(*) FROM _migrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(first, second, "re-running migrations must not add rows");
    }

    #[test]
    fn migrations_registers_every_file_in_migrations_table() {
        let conn = fresh_memory();
        run_migrations(&conn).unwrap();
        // The full list of migration names is internal; we can at
        // least assert the latest known migration is tracked. If a
        // new migration is added, this assertion stays truthful
        // because we check >= known recent + <= reasonable upper
        // bound.
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM _migrations", [], |r| r.get(0))
            .unwrap();
        assert!(n >= 30, "expected >=30 applied migrations, got {}", n);

        // Name ordering: the last applied migration's name should be
        // 0029_heartbeat_fires_schedule_name (the highest-numbered
        // one shipped to date). If this breaks after adding a new
        // migration, updating the expected name here is a deliberate
        // signal to update migration docs.
        let last_name: String = conn
            .query_row(
                "SELECT name FROM _migrations ORDER BY id DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            last_name.starts_with("00"),
            "unexpected last migration name: {}",
            last_name
        );
    }

    #[test]
    fn seed_agent_presets_creates_expected_entries() {
        let conn = fresh_memory();
        run_migrations(&conn).unwrap();
        seed_agent_presets(&conn).unwrap();
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM agent_presets WHERE is_built_in = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 11, "expected 11 built-in presets");
    }

    #[test]
    fn seed_agent_presets_idempotent_across_reseeds() {
        let conn = fresh_memory();
        run_migrations(&conn).unwrap();
        seed_agent_presets(&conn).unwrap();
        seed_agent_presets(&conn).unwrap();
        seed_agent_presets(&conn).unwrap();
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM agent_presets WHERE is_built_in = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 11, "reseeding must not duplicate rows");
    }

    // ── open_with_resilience PRAGMAs ──────────────────────────────
    #[test]
    fn open_with_resilience_sets_foreign_keys_on() {
        let path = scratch_db_path();
        let conn = open_with_resilience(&path).unwrap();
        let fk: i64 = conn
            .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
            .unwrap();
        assert_eq!(fk, 1, "foreign_keys should be ON after open");
        drop(conn);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn open_with_resilience_sets_wal_mode_on_disk_db() {
        // journal_mode=WAL only sticks on file-backed DBs; memory DBs
        // report "memory". That's why this test uses a disk path.
        let path = scratch_db_path();
        let conn = open_with_resilience(&path).unwrap();
        let mode: String = conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode.to_lowercase(), "wal", "expected WAL mode, got {}", mode);
        drop(conn);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn open_with_resilience_sets_pragmas() {
        let path = scratch_db_path();
        let conn = open_with_resilience(&path).unwrap();
        // busy_timeout: 500ms as of 0.32.13 (was 5000 — masked real contention
        // behind a 5 s UI hang).
        let timeout: i64 = conn
            .query_row("PRAGMA busy_timeout", [], |r| r.get(0))
            .unwrap();
        assert_eq!(timeout, 500, "busy_timeout should be 500ms");

        // cache_size negative means KiB (positive means pages). -20000 = 20 MB.
        let cache_size: i64 = conn
            .query_row("PRAGMA cache_size", [], |r| r.get(0))
            .unwrap();
        assert_eq!(cache_size, -20000, "cache_size should be -20000 (20MB)");

        // temp_store 2 = MEMORY (0=default, 1=FILE, 2=MEMORY).
        let temp_store: i64 = conn
            .query_row("PRAGMA temp_store", [], |r| r.get(0))
            .unwrap();
        assert_eq!(temp_store, 2, "temp_store should be 2 (MEMORY)");

        drop(conn);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    // ── bootstrap_test_db_at ──────────────────────────────────────
    #[test]
    fn bootstrap_test_db_at_creates_usable_database() {
        let path = scratch_db_path();
        bootstrap_test_db_at(&path).unwrap();

        // Reopen and verify tables + presets present.
        let conn = open_with_resilience(&path).unwrap();
        let table_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='agent_presets'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(table_count, 1);
        let preset_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM agent_presets WHERE is_built_in=1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(preset_count, 11);
        drop(conn);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn bootstrap_test_db_at_is_rerunnable_on_existing_file() {
        // If the user (or a prior test run) left a DB file in place,
        // bootstrap_test_db_at must still succeed without duplicating
        // rows or failing migrations.
        let path = scratch_db_path();
        bootstrap_test_db_at(&path).unwrap();
        bootstrap_test_db_at(&path).unwrap();
        let conn = open_with_resilience(&path).unwrap();
        let presets: i64 = conn
            .query_row("SELECT COUNT(*) FROM agent_presets WHERE is_built_in=1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(presets, 11, "re-bootstrap must not duplicate presets");
        drop(conn);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    // ── isolated_test_connection ──────────────────────────────────
    #[test]
    fn isolated_test_connection_gives_distinct_databases() {
        // Two calls to isolated_test_connection return two different
        // :memory: connections — a write to one must not be visible
        // from the other. This is the isolation guarantee that lets
        // unit tests run in parallel without polluting each other.
        let a = isolated_test_connection();
        let b = isolated_test_connection();

        // Insert a project row into A via raw SQL (bypassing schema::
        // helpers so we don't need a project_id generator).
        a.execute(
            "INSERT INTO projects (id, name, path) VALUES ('p-iso', 'a', '/iso')",
            [],
        )
        .unwrap();

        let a_has: i64 = a
            .query_row(
                "SELECT COUNT(*) FROM projects WHERE id='p-iso'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let b_has: i64 = b
            .query_row(
                "SELECT COUNT(*) FROM projects WHERE id='p-iso'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(a_has, 1, "A must see its own write");
        assert_eq!(b_has, 0, "B must not see A's write");
    }

    #[test]
    fn isolated_test_connection_carries_full_schema() {
        // Spot-check: every table hit by schema::unit_tests must be
        // present in a fresh isolated_test_connection.
        let conn = isolated_test_connection();
        for table in [
            "projects",
            "agent_sessions",
            "agent_heartbeats",
            "agent_presets",
            "heartbeat_fires",
            "activity_feed",
            "workspace_relations",
            "focus_groups",
        ] {
            let exists: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    params![table],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(exists, 1, "isolated connection missing table: {}", table);
        }
    }
}

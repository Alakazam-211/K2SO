//! Per-project settings accessors — thin DB wrappers.
//!
//! The CLI's `k2so mode`, `k2so heartbeat on/off`, `k2so worktree`,
//! `k2so settings` commands all land here. Each is a read-or-write
//! against the `projects` table filtered by path. Kept separate from
//! the broader `AppSettings` (in `src-tauri/src/commands/settings.rs`)
//! because that struct is mostly UI preferences; these are per-project
//! mode flags that affect agent behavior.
//!
//! Moved to core so the daemon can serve `/cli/mode`, `/cli/worktree`,
//! `/cli/settings` headlessly.

/// Update a single project setting. Field names are allowlisted —
/// the SQL interpolates the column name directly so any arbitrary
/// string from query params would be an injection vector without
/// this check.
pub fn update_project_setting(
    project_path: &str,
    field: &str,
    value: &str,
) -> Result<(), String> {
    let db = crate::db::shared();
    let conn = db.lock();

    let allowed = [
        "agent_mode",
        "worktree_mode",
        "heartbeat_enabled",
        "agent_enabled",
        "pinned",
        "tier_id",
        // 0.34.0 Session Stream opt-in (Phase 2). Values: 'on' | 'off'.
        "use_session_stream",
    ];
    if !allowed.contains(&field) {
        return Err(format!("Unknown setting: {}", field));
    }
    // Validate value for the new enum-like setting so a typo doesn't
    // silently leave a project in a broken half-state. Existing fields
    // keep their bare string/int semantics for back-compat.
    if field == "use_session_stream" && value != "on" && value != "off" {
        return Err(format!(
            "use_session_stream must be 'on' or 'off', got {value:?}"
        ));
    }

    let sql = format!("UPDATE projects SET {} = ?1 WHERE path = ?2", field);
    let rows = conn
        .execute(&sql, rusqlite::params![value, project_path])
        .map_err(|e| format!("DB update failed: {}", e))?;

    if rows == 0 {
        return Err(format!("Project not found in DB: {}", project_path));
    }

    // Keep agent_enabled in sync with agent_mode — the UI derives one
    // from the other and the CLI expects them coherent.
    if field == "agent_mode" {
        let enabled = if value == "off" { "0" } else { "1" };
        let _ = conn.execute(
            "UPDATE projects SET agent_enabled = ?1 WHERE path = ?2",
            rusqlite::params![enabled, project_path],
        );
    }

    Ok(())
}

/// Read every exposed per-project setting as a JSON blob. Shape
/// matches what the React frontend expects from
/// `invoke('projects_get_settings', ...)`.
pub fn get_project_settings(project_path: &str) -> Result<serde_json::Value, String> {
    let db = crate::db::shared();
    let conn = db.lock();

    conn.query_row(
        "SELECT agent_mode, worktree_mode, heartbeat_enabled, agent_enabled, pinned, name, tier_id FROM projects WHERE path = ?1",
        rusqlite::params![project_path],
        |row| {
            Ok(serde_json::json!({
                "mode": row.get::<_, String>(0).unwrap_or_else(|_| "off".to_string()),
                "worktreeMode": row.get::<_, i64>(1).unwrap_or(0) == 1,
                "heartbeatEnabled": row.get::<_, i64>(2).unwrap_or(0) == 1,
                "agentEnabled": row.get::<_, i64>(3).unwrap_or(0) == 1,
                "pinned": row.get::<_, i64>(4).unwrap_or(0) == 1,
                "name": row.get::<_, String>(5).unwrap_or_default(),
                "stateId": row.get::<_, Option<String>>(6).unwrap_or(None),
            }))
        },
    )
    .map_err(|e| format!("Project not found: {}", e))
}

/// Read the global agentic-systems toggle from the `app_settings`
/// key/value table. Defaults to `false` if the row isn't present.
pub fn get_agentic_enabled() -> bool {
    let db = crate::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT value FROM app_settings WHERE key = 'agentic_systems_enabled'",
        [],
        |row| row.get::<_, String>(0),
    )
    .map(|v| v == "1")
    .unwrap_or(false)
}

/// Set the global agentic-systems toggle. UPSERTs the
/// `agentic_systems_enabled` key in `app_settings`.
pub fn set_agentic_enabled(enabled: bool) -> Result<(), String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let value = if enabled { "1" } else { "0" };
    conn.execute(
        "INSERT OR REPLACE INTO app_settings (key, value) VALUES ('agentic_systems_enabled', ?1)",
        rusqlite::params![value],
    )
    .map(|_| ())
    .map_err(|e| format!("DB update failed: {}", e))
}

/// Read the "keep daemon running when K2SO quits" preference from
/// `app_settings`. Defaults to `true` — matches the persistent-agents
/// flagship: if the user installed K2SO and opted into heartbeats,
/// they presumably want them to keep firing when the window closes.
/// The menubar icon provides visibility into what's running, so
/// defaulting ON doesn't leave the user wondering.
pub fn get_keep_daemon_on_quit() -> bool {
    let db = crate::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT value FROM app_settings WHERE key = 'keep_daemon_on_quit'",
        [],
        |row| row.get::<_, String>(0),
    )
    .map(|v| v == "1")
    .unwrap_or(true) // default ON
}

/// Set the "keep daemon running when K2SO quits" preference. UPSERTs
/// the `keep_daemon_on_quit` key in `app_settings`.
pub fn set_keep_daemon_on_quit(keep: bool) -> Result<(), String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let value = if keep { "1" } else { "0" };
    conn.execute(
        "INSERT OR REPLACE INTO app_settings (key, value) VALUES ('keep_daemon_on_quit', ?1)",
        rusqlite::params![value],
    )
    .map(|_| ())
    .map_err(|e| format!("DB update failed: {}", e))
}

/// Return `true` if the given project has opted into the 0.34.0
/// Session Stream pipeline (Phase 2). Defaults to `false` when the
/// project doesn't exist or the column reads NULL (rows inserted
/// before migration 0032 applied — the ALTER default backfills to
/// 'off', so NULL here means "unknown project").
///
/// Callers pair this with the compile-time `session_stream` feature
/// flag: both must be true for the dual-emit reader to kick in.
pub fn get_use_session_stream(project_path: &str) -> bool {
    let db = crate::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT use_session_stream FROM projects WHERE path = ?1",
        rusqlite::params![project_path],
        |row| row.get::<_, Option<String>>(0),
    )
    .map(|v| v.as_deref() == Some("on"))
    .unwrap_or(false)
}

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
    ];
    if !allowed.contains(&field) {
        return Err(format!("Unknown setting: {}", field));
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

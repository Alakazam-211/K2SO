//! DB-backed sub-handlers for the `/cli/companion/*` route family.
//!
//! The mobile companion issues these to discover globally-scoped
//! state (agent presets, registered workspaces, etc.) before the
//! project-specific calls. Both the daemon and the Tauri app can
//! answer; the DB is the source of truth.
//!
//! Scope here: `presets`, `projects`. The richer `sessions` and
//! `projects-summary` endpoints require a live terminal-manager
//! handle to count running agents, which is process-local (Tauri
//! and daemon each have their own). Those stay split until the
//! daemon becomes the sole PTY owner.

/// Global list of user-enabled agent presets. Mirrors the existing
/// `/cli/companion/presets` JSON shape the companion app expects.
pub fn list_presets() -> Result<String, String> {
    let db = crate::db::shared();
    let conn = db.lock();

    let mut stmt = conn
        .prepare(
            "SELECT id, label, command, icon FROM agent_presets WHERE enabled = 1 ORDER BY sort_order ASC, label ASC",
        )
        .map_err(|e| e.to_string())?;

    let presets: Vec<serde_json::Value> = stmt
        .query_map([], |row| {
            Ok(serde_json::json!({
                "id": row.get::<_, String>(0)?,
                "name": row.get::<_, String>(1)?,
                "command": row.get::<_, String>(2)?,
                "icon": row.get::<_, Option<String>>(3)?,
            }))
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();

    Ok(serde_json::to_string(&presets).unwrap_or_else(|_| "[]".to_string()))
}

/// All registered workspaces (projects), joined with focus-group
/// metadata. Global — doesn't filter by project_path. Shape matches
/// what the existing `/cli/companion/projects` emits.
pub fn list_projects() -> Result<String, String> {
    let db = crate::db::shared();
    let conn = db.lock();

    let mut stmt = conn
        .prepare(
            "SELECT p.id, p.name, p.path, p.color, p.icon_url, p.agent_mode, p.pinned, \
             p.tab_order, p.focus_group_id, fg.name, fg.color \
             FROM projects p \
             LEFT JOIN focus_groups fg ON p.focus_group_id = fg.id \
             ORDER BY p.pinned DESC, p.tab_order ASC, p.name ASC",
        )
        .map_err(|e| e.to_string())?;

    let projects: Vec<serde_json::Value> = stmt
        .query_map([], |row| {
            let fg_id: Option<String> = row.get(8)?;
            let fg_name: Option<String> = row.get(9)?;
            let fg_color: Option<String> = row.get(10)?;
            let focus_group = if let (Some(id), Some(name)) = (&fg_id, &fg_name) {
                serde_json::json!({ "id": id, "name": name, "color": fg_color })
            } else {
                serde_json::Value::Null
            };

            Ok(serde_json::json!({
                "id": row.get::<_, String>(0)?,
                "name": row.get::<_, String>(1)?,
                "path": row.get::<_, String>(2)?,
                "color": row.get::<_, String>(3)?,
                "iconUrl": row.get::<_, Option<String>>(4)?,
                "agentMode": row.get::<_, String>(5)?,
                "pinned": row.get::<_, bool>(6)?,
                "tabOrder": row.get::<_, i32>(7)?,
                "focusGroup": focus_group,
            }))
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();

    Ok(serde_json::to_string(&projects).unwrap_or_else(|_| "[]".to_string()))
}

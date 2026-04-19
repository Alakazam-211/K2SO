//! Workspace connections — thin dispatch over `WorkspaceRelation` +
//! `log_activity`.
//!
//! Powers `k2so connections list/add/remove`. A connection is a row
//! in `workspace_relations` linking two projects (`source_project_id`
//! → `target_project_id`) so cross-workspace `k2so msg` can verify
//! the sender is actually allowed to post.
//!
//! Moved to core so the daemon can serve `/cli/connections`
//! headlessly. Same three verbs src-tauri had: `list` / `add` /
//! `remove`.

use crate::agents::resolve_project_id;
use crate::db::schema::{log_activity, WorkspaceRelation};

/// Dispatch by `action`. Returns a JSON-serialized string matching
/// the shapes the CLI has emitted since 0.32.x.
pub fn connections(
    project_path: &str,
    action: &str,
    target: Option<&str>,
    rel_type: Option<&str>,
) -> Result<String, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let project_id = resolve_project_id(&conn, project_path)
        .ok_or_else(|| format!("Project not found: {}", project_path))?;

    match action {
        "list" => {
            let outgoing = WorkspaceRelation::list_for_source(&conn, &project_id)
                .map_err(|e| e.to_string())?;
            let incoming = WorkspaceRelation::list_for_target(&conn, &project_id)
                .map_err(|e| e.to_string())?;

            let mut connections = Vec::new();
            for rel in &outgoing {
                let name: String = conn
                    .query_row(
                        "SELECT name FROM projects WHERE id = ?1",
                        rusqlite::params![rel.target_project_id],
                        |row| row.get(0),
                    )
                    .unwrap_or_else(|_| "Unknown".to_string());
                connections.push(serde_json::json!({
                    "id": rel.id,
                    "direction": "outgoing",
                    "type": rel.relation_type,
                    "projectId": rel.target_project_id,
                    "projectName": name,
                }));
            }
            for rel in &incoming {
                let name: String = conn
                    .query_row(
                        "SELECT name FROM projects WHERE id = ?1",
                        rusqlite::params![rel.source_project_id],
                        |row| row.get(0),
                    )
                    .unwrap_or_else(|_| "Unknown".to_string());
                connections.push(serde_json::json!({
                    "id": rel.id,
                    "direction": "incoming",
                    "type": rel.relation_type,
                    "projectId": rel.source_project_id,
                    "projectName": name,
                }));
            }
            Ok(serde_json::json!({ "connections": connections }).to_string())
        }
        "add" => {
            let target_name = target
                .filter(|s| !s.is_empty())
                .ok_or_else(|| "Missing 'target' parameter (workspace name or path)".to_string())?;
            let target_id: String = conn
                .query_row(
                    "SELECT id FROM projects WHERE name = ?1 OR path = ?1",
                    rusqlite::params![target_name],
                    |row| row.get(0),
                )
                .map_err(|_| format!("Workspace '{}' not found", target_name))?;

            let id = uuid::Uuid::new_v4().to_string();
            let rel_type = rel_type.unwrap_or("oversees");
            WorkspaceRelation::create(&conn, &id, &project_id, &target_id, rel_type)
                .map_err(|e| e.to_string())?;

            let target_display: String = conn
                .query_row(
                    "SELECT name FROM projects WHERE id = ?1",
                    rusqlite::params![target_id],
                    |row| row.get(0),
                )
                .unwrap_or_else(|_| target_name.to_string());

            log_activity(
                &conn,
                &project_id,
                None,
                "connection.created",
                None,
                None,
                Some(&target_id),
                Some(&format!("Connected to {}", target_display)),
            );

            Ok(serde_json::json!({
                "success": true,
                "id": id,
                "target": target_display,
            })
            .to_string())
        }
        "remove" => {
            let target_name = target
                .filter(|s| !s.is_empty())
                .ok_or_else(|| "Missing 'target' parameter".to_string())?;
            let target_id: String = conn
                .query_row(
                    "SELECT id FROM projects WHERE name = ?1 OR path = ?1",
                    rusqlite::params![target_name],
                    |row| row.get(0),
                )
                .map_err(|_| format!("Workspace '{}' not found", target_name))?;

            let rel_id: Result<String, _> = conn.query_row(
                "SELECT id FROM workspace_relations WHERE source_project_id = ?1 AND target_project_id = ?2",
                rusqlite::params![project_id, target_id],
                |row| row.get(0),
            );
            match rel_id {
                Ok(id) => {
                    WorkspaceRelation::delete(&conn, &id).map_err(|e| e.to_string())?;
                    log_activity(
                        &conn,
                        &project_id,
                        None,
                        "connection.removed",
                        None,
                        None,
                        Some(&target_id),
                        Some(&format!("Disconnected from {}", target_name)),
                    );
                    Ok(serde_json::json!({"success": true}).to_string())
                }
                Err(_) => Err(format!("No connection to '{}' found", target_name)),
            }
        }
        other => Err(format!(
            "Unknown action '{}'. Use: list, add, remove",
            other
        )),
    }
}

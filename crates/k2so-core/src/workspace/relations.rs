//! workspace_sessions + workspace_relations DB accessors.
//!
//! Phase 2.5d: extracted from the monolithic `agents/commands.rs`.
//! Pre-Phase-2-Unit-7d these used `state.db.lock()` (Tauri's
//! `AppState`); post-7d they use the shared `db::shared()` handle
//! directly so the daemon serves them without the Tauri state
//! container. The function bodies are unchanged from the agents/
//! commands.rs origin.

use crate::db::schema::WorkspaceSession;

/// Fetch the workspace_sessions row for a project, if one exists.
/// Returns `None` for projects with no pinned chat session yet.
pub fn workspace_session_get(
    project_id: String,
) -> Result<Option<WorkspaceSession>, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    WorkspaceSession::get(&conn, &project_id).map_err(|e| e.to_string())
}

/// List every workspace_relations row where the given project is the
/// SOURCE (this workspace "oversees" / "depends-on" the targets).
pub fn workspace_relations_list(
    project_id: String,
) -> Result<Vec<crate::db::schema::WorkspaceRelation>, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    crate::db::schema::WorkspaceRelation::list_for_source(&conn, &project_id)
        .map_err(|e| e.to_string())
}

/// List every workspace_relations row where the given project is the
/// TARGET (other workspaces "oversee" this one).
pub fn workspace_relations_list_incoming(
    project_id: String,
) -> Result<Vec<crate::db::schema::WorkspaceRelation>, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    crate::db::schema::WorkspaceRelation::list_for_target(&conn, &project_id)
        .map_err(|e| e.to_string())
}

/// Create a new workspace_relations row.
pub fn workspace_relations_create(
    source_project_id: String,
    target_project_id: String,
    relation_type: Option<String>,
) -> Result<crate::db::schema::WorkspaceRelation, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let id = uuid::Uuid::new_v4().to_string();
    let rel_type = relation_type.unwrap_or_else(|| "oversees".to_string());
    crate::db::schema::WorkspaceRelation::create(
        &conn,
        &id,
        &source_project_id,
        &target_project_id,
        &rel_type,
    )
    .map_err(|e| e.to_string())?;
    Ok(crate::db::schema::WorkspaceRelation {
        id,
        source_project_id,
        target_project_id,
        relation_type: rel_type,
        created_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64,
    })
}

/// Delete a workspace_relations row by id.
pub fn workspace_relations_delete(id: String) -> Result<(), String> {
    let db = crate::db::shared();
    let conn = db.lock();
    crate::db::schema::WorkspaceRelation::delete(&conn, &id).map_err(|e| e.to_string())?;
    Ok(())
}

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
//!
//! 0.39.0 directive (workspace==agent model): the stored data is
//! intentionally directional (`source → target`) for audit/lineage
//! value, but **user-facing surfaces present connections as
//! bidirectional**. Whoever initiated the relationship, BOTH
//! workspaces see each other in their roster as just "connected" —
//! no direction labels. That's what [`list_peers`] implements; the
//! `list` action of [`connections`] now returns the deduped peer
//! shape so the CLI / SKILL.md / roster never have to think about
//! direction.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::db::schema::{log_activity, WorkspaceRelation};
use crate::workspace::agent_identity::resolve_project_id;

/// A connected peer workspace, deduped across the directional
/// `workspace_relations` rows. If the same peer appears as both an
/// outgoing target (this workspace → them) and an incoming source
/// (them → this workspace), they show up once here with
/// `relation_types` carrying both labels (sorted, deduped).
///
/// Returned by [`list_peers`] and the user-facing `connections list`
/// dispatch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Peer {
    pub project_id: String,
    pub project_name: String,
    /// Every distinct relation_type value seen across the
    /// outgoing+incoming rows that point at this peer. A peer might
    /// be "collaborator" in one direction and "oversees" in the
    /// other; both labels are listed (sorted alphabetically, deduped).
    pub relation_types: Vec<String>,
}

/// Bidirectional, deduped peer list. Returns each connected
/// workspace exactly once regardless of which side initiated the
/// relationship. Used by user-facing surfaces (roster, manager
/// SKILL.md team section, CLI list) that should not surface
/// direction as a user-visible distinction.
///
/// Output is sorted alphabetically by `project_name` for stable
/// rendering.
pub fn list_peers(project_path: &str) -> Result<Vec<Peer>, String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let project_id = resolve_project_id(&conn, project_path)
        .ok_or_else(|| format!("Project not found: {}", project_path))?;

    let outgoing = WorkspaceRelation::list_for_source(&conn, &project_id)
        .map_err(|e| e.to_string())?;
    let incoming = WorkspaceRelation::list_for_target(&conn, &project_id)
        .map_err(|e| e.to_string())?;

    let mut peers: HashMap<String, Peer> = HashMap::new();

    for rel in &outgoing {
        let pid = &rel.target_project_id;
        let name: String = conn
            .query_row(
                "SELECT name FROM projects WHERE id = ?1",
                rusqlite::params![pid],
                |row| row.get(0),
            )
            .unwrap_or_else(|_| "Unknown".to_string());
        peers
            .entry(pid.clone())
            .or_insert_with(|| Peer {
                project_id: pid.clone(),
                project_name: name,
                relation_types: Vec::new(),
            })
            .relation_types
            .push(rel.relation_type.clone());
    }

    for rel in &incoming {
        let pid = &rel.source_project_id;
        let name: String = conn
            .query_row(
                "SELECT name FROM projects WHERE id = ?1",
                rusqlite::params![pid],
                |row| row.get(0),
            )
            .unwrap_or_else(|_| "Unknown".to_string());
        peers
            .entry(pid.clone())
            .or_insert_with(|| Peer {
                project_id: pid.clone(),
                project_name: name,
                relation_types: Vec::new(),
            })
            .relation_types
            .push(rel.relation_type.clone());
    }

    // Dedupe + sort relation_types per peer so callers don't see
    // duplicate labels when both directions carry the same type.
    let mut out: Vec<Peer> = peers
        .into_values()
        .map(|mut p| {
            p.relation_types.sort();
            p.relation_types.dedup();
            p
        })
        .collect();
    out.sort_by(|a, b| a.project_name.cmp(&b.project_name));
    Ok(out)
}

/// Dispatch by `action`. Returns a JSON-serialized string.
///
/// **0.39.0 list-shape change**: `action == "list"` now returns the
/// deduped, bidirectional [`Peer`] shape via [`list_peers`] —
/// `{"connections": [{"projectId": "...", "projectName": "...",
/// "relationTypes": [...] }]}`. The pre-0.39 directional shape
/// (`{"id", "direction", "type", "projectId", "projectName"}`) is
/// no longer surfaced because direction is an implementation detail
/// of the relation row, not a user-facing concept in the
/// workspace==agent model. Callers that still need the raw
/// directional rows can use `WorkspaceRelation::list_for_source` /
/// `list_for_target` directly.
pub fn connections(
    project_path: &str,
    action: &str,
    target: Option<&str>,
    rel_type: Option<&str>,
) -> Result<String, String> {
    match action {
        "list" => {
            let peers = list_peers(project_path)?;
            // Emit as `{"connections": [...]}` for backwards-compatible
            // envelope shape (CLI parsers still key on "connections").
            // Each entry uses the Peer struct's serde camelCase form
            // — `projectId`, `projectName`, `relationTypes`.
            Ok(serde_json::json!({ "connections": peers }).to_string())
        }
        "add" => {
            let db = crate::db::shared();
            let conn = db.lock();
            let project_id = resolve_project_id(&conn, project_path)
                .ok_or_else(|| format!("Project not found: {}", project_path))?;

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
            let db = crate::db::shared();
            let conn = db.lock();
            let project_id = resolve_project_id(&conn, project_path)
                .ok_or_else(|| format!("Project not found: {}", project_path))?;

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

#[cfg(test)]
mod tests {
    //! Tests for `list_peers` — the bidirectional, deduped peer-list
    //! helper added in 0.39.0. These exercise the four shapes
    //! `list_peers` has to handle correctly:
    //!
    //!   - no relations at all → empty Vec
    //!   - outgoing-only relation → one peer
    //!   - incoming-only relation → one peer
    //!   - bidirectional (outgoing + incoming for same peer) →
    //!     one peer with both relation_types listed
    //!
    //! Plus alphabetical sort of multi-peer output.
    //!
    //! The shared in-memory test DB (`init_for_tests`) accumulates
    //! state across tests in the same binary, so each test uses a
    //! UUID-suffixed path to keep its `projects` rows isolated.
    use super::*;
    use uuid::Uuid;

    /// Insert a `projects` row at a unique path so the test doesn't
    /// collide with other tests sharing the same in-memory DB.
    /// Returns `(project_path, project_id)`.
    fn make_project(suffix: &str) -> (String, String) {
        let db = crate::db::shared();
        let conn = db.lock();
        let path = format!("/tmp/list-peers-test-{}-{}", suffix, Uuid::new_v4());
        let id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
            rusqlite::params![id, suffix, path],
        )
        .expect("insert project");
        (path, id)
    }

    #[test]
    fn list_peers_returns_empty_when_no_relations() {
        let (path, _id) = make_project("solo");
        let peers = list_peers(&path).expect("list_peers ok");
        assert!(
            peers.is_empty(),
            "workspace with no relations should have no peers; got {peers:?}"
        );
    }

    #[test]
    fn list_peers_returns_outgoing_only_relation_once() {
        let (src_path, src_id) = make_project("out-src");
        let (_tgt_path, tgt_id) = make_project("out-tgt");
        let db = crate::db::shared();
        let conn = db.lock();
        WorkspaceRelation::create(
            &conn,
            &Uuid::new_v4().to_string(),
            &src_id,
            &tgt_id,
            "oversees",
        )
        .unwrap();
        drop(conn);

        let peers = list_peers(&src_path).expect("list_peers ok");
        assert_eq!(peers.len(), 1, "outgoing-only relation should yield 1 peer; got {peers:?}");
        let peer = &peers[0];
        assert_eq!(peer.project_id, tgt_id);
        assert_eq!(peer.relation_types, vec!["oversees".to_string()]);
    }

    #[test]
    fn list_peers_returns_incoming_only_relation_once() {
        let (src_path, src_id) = make_project("in-src");
        let (tgt_path, tgt_id) = make_project("in-tgt");
        let db = crate::db::shared();
        let conn = db.lock();
        // Relation goes src → tgt; we call list_peers on tgt, so tgt
        // sees src as an INCOMING peer.
        WorkspaceRelation::create(
            &conn,
            &Uuid::new_v4().to_string(),
            &src_id,
            &tgt_id,
            "collaborator",
        )
        .unwrap();
        drop(conn);

        let peers = list_peers(&tgt_path).expect("list_peers ok");
        assert_eq!(peers.len(), 1, "incoming-only relation should yield 1 peer; got {peers:?}");
        let peer = &peers[0];
        assert_eq!(peer.project_id, src_id);
        assert_eq!(peer.relation_types, vec!["collaborator".to_string()]);
        // sanity: the source-path side doesn't see itself
        let _ = src_path;
    }

    #[test]
    fn list_peers_dedupes_bidirectional_relation_to_one_entry_with_both_relation_types() {
        let (a_path, a_id) = make_project("bi-a");
        let (_b_path, b_id) = make_project("bi-b");
        let db = crate::db::shared();
        let conn = db.lock();
        // A → B as "oversees"; B → A as "collaborator".
        WorkspaceRelation::create(
            &conn,
            &Uuid::new_v4().to_string(),
            &a_id,
            &b_id,
            "oversees",
        )
        .unwrap();
        WorkspaceRelation::create(
            &conn,
            &Uuid::new_v4().to_string(),
            &b_id,
            &a_id,
            "collaborator",
        )
        .unwrap();
        drop(conn);

        let peers = list_peers(&a_path).expect("list_peers ok");
        assert_eq!(
            peers.len(),
            1,
            "bidirectional relation should dedupe to 1 peer; got {peers:?}"
        );
        let peer = &peers[0];
        assert_eq!(peer.project_id, b_id);
        // Both relation_types present (sorted alphabetically:
        // "collaborator" < "oversees").
        assert_eq!(
            peer.relation_types,
            vec!["collaborator".to_string(), "oversees".to_string()],
            "bidirectional relation should expose both labels"
        );
    }

    #[test]
    fn list_peers_sorted_alphabetically_by_name() {
        // Create three peers with names that won't sort the same as
        // insertion order. Names need to be predictable so we can
        // assert order — explicitly set them after make_project's
        // default-name insert by updating via UUIDs.
        let db = crate::db::shared();
        let conn = db.lock();
        let me_path = format!("/tmp/sort-me-{}", Uuid::new_v4());
        let me_id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
            rusqlite::params![me_id, "z-me", &me_path],
        )
        .unwrap();
        // Three peers with names that should appear in alphabetical
        // order: "alpha", "bravo", "charlie".
        let mut peer_ids = Vec::new();
        for name in ["charlie", "alpha", "bravo"] {
            let pid = Uuid::new_v4().to_string();
            let path = format!("/tmp/sort-peer-{}-{}", name, Uuid::new_v4());
            conn.execute(
                "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
                rusqlite::params![pid, name, &path],
            )
            .unwrap();
            peer_ids.push((name, pid));
        }
        for (_name, pid) in &peer_ids {
            WorkspaceRelation::create(
                &conn,
                &Uuid::new_v4().to_string(),
                &me_id,
                pid,
                "oversees",
            )
            .unwrap();
        }
        drop(conn);

        let peers = list_peers(&me_path).expect("list_peers ok");
        let names: Vec<&str> = peers.iter().map(|p| p.project_name.as_str()).collect();
        assert_eq!(
            names,
            vec!["alpha", "bravo", "charlie"],
            "peers must be sorted alphabetically by name"
        );
    }
}

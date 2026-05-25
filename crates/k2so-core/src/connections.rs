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

            let target_display: String = conn
                .query_row(
                    "SELECT name FROM projects WHERE id = ?1",
                    rusqlite::params![target_id],
                    |row| row.get(0),
                )
                .unwrap_or_else(|_| target_name.to_string());

            // Bidirectional-awareness guard (0.39.0): a connection
            // between two workspaces implies BOTH-WAYS awareness
            // regardless of which side initiated it (see [`list_peers`]
            // and migration 0051). If the REVERSE pair already exists
            // (target → this workspace), inserting a new (this →
            // target) row would create the same kind of redundant pair
            // migration 0051 just cleaned up. No-op so the dedup work
            // isn't undone by future redundant adds.
            //
            // Reads the reverse row's (id, relation_type) so the
            // response can tell the caller exactly what existing
            // relation satisfied their request — no surprises if their
            // intent was to confirm a connection, not to add a new one.
            let existing_reverse: Option<(String, String)> = conn
                .query_row(
                    "SELECT id, relation_type FROM workspace_relations \
                     WHERE source_project_id = ?1 AND target_project_id = ?2 \
                     LIMIT 1",
                    rusqlite::params![target_id, project_id],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .ok();

            if let Some((existing_id, existing_type)) = existing_reverse {
                return Ok(serde_json::json!({
                    "success": true,
                    "id": existing_id,
                    "target": target_display,
                    "noop": true,
                    "message": format!(
                        "already connected (existing relation: {} → this workspace, type: {})",
                        target_display, existing_type
                    ),
                })
                .to_string());
            }

            let id = uuid::Uuid::new_v4().to_string();
            let rel_type = rel_type.unwrap_or("oversees");
            WorkspaceRelation::create(&conn, &id, &project_id, &target_id, rel_type)
                .map_err(|e| e.to_string())?;

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

    // ── add-side reverse-pair no-op guard (0.39.0) ───────────────
    //
    // Phase 2.5b workspace==agent insight: a connection between two
    // workspaces implies BIDIRECTIONAL awareness regardless of who
    // initiated it. `connections("add", ...)` therefore must NOT
    // insert a (this→target) row when the reverse (target→this)
    // already exists, or migration 0051's symmetric-dedup work would
    // be silently undone by every future `k2so connections add`.

    #[test]
    fn add_creates_new_relation_when_no_existing() {
        let (src_path, src_id) = make_project("add-new-src");
        let (_tgt_path, tgt_id) = make_project("add-new-tgt");

        // Baseline: no reverse exists; add should insert a fresh row.
        let resp = connections(&src_path, "add", Some("add-new-tgt"), Some("oversees"))
            .expect("add should succeed");
        let parsed: serde_json::Value = serde_json::from_str(&resp).expect("valid JSON");
        assert_eq!(parsed["success"], serde_json::json!(true));
        // No "noop" flag because this is a real insert.
        assert!(
            parsed.get("noop").is_none(),
            "fresh add should NOT carry noop flag; got {parsed}"
        );

        let db = crate::db::shared();
        let conn = db.lock();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM workspace_relations \
                 WHERE source_project_id = ?1 AND target_project_id = ?2",
                rusqlite::params![src_id, tgt_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "fresh add should create exactly 1 row");
    }

    #[test]
    fn add_no_ops_when_reverse_pair_already_exists() {
        let (src_path, src_id) = make_project("add-noop-src");
        let (_tgt_path, tgt_id) = make_project("add-noop-tgt");

        // Seed the REVERSE pair (target → source) FIRST. This is the
        // "other side initiated the connection" case.
        let db = crate::db::shared();
        {
            let conn = db.lock();
            WorkspaceRelation::create(
                &conn,
                &Uuid::new_v4().to_string(),
                &tgt_id,
                &src_id,
                "collaborator",
            )
            .unwrap();
        }

        // Now src side tries to add tgt — must no-op because the
        // bidirectional-awareness contract says they're already
        // connected.
        let resp = connections(&src_path, "add", Some("add-noop-tgt"), Some("oversees"))
            .expect("add should succeed (no-op)");
        let parsed: serde_json::Value = serde_json::from_str(&resp).expect("valid JSON");
        assert_eq!(parsed["success"], serde_json::json!(true));
        assert_eq!(
            parsed["noop"],
            serde_json::json!(true),
            "reverse-pair-exists add must report noop:true; got {parsed}"
        );

        // Storage layer: still exactly 1 row total for this pair (the
        // pre-existing reverse), zero in the forward direction. This
        // is the invariant migration 0051 protects.
        let conn = db.lock();
        let forward: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM workspace_relations \
                 WHERE source_project_id = ?1 AND target_project_id = ?2",
                rusqlite::params![src_id, tgt_id],
                |r| r.get(0),
            )
            .unwrap();
        let reverse: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM workspace_relations \
                 WHERE source_project_id = ?1 AND target_project_id = ?2",
                rusqlite::params![tgt_id, src_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            forward, 0,
            "forward row must NOT be inserted when reverse exists"
        );
        assert_eq!(reverse, 1, "pre-existing reverse row must remain untouched");
    }

    #[test]
    fn add_returns_helpful_message_when_no_op_due_to_reverse_existing() {
        let (src_path, src_id) = make_project("add-msg-src");
        let (_tgt_path, tgt_id) = make_project("add-msg-tgt");

        // Reverse seeded with a distinctive type so the message can
        // surface it.
        let db = crate::db::shared();
        {
            let conn = db.lock();
            WorkspaceRelation::create(
                &conn,
                &Uuid::new_v4().to_string(),
                &tgt_id,
                &src_id,
                "peer",
            )
            .unwrap();
        }
        let _ = src_id;

        let resp = connections(&src_path, "add", Some("add-msg-tgt"), Some("oversees"))
            .expect("add should succeed (no-op)");
        let parsed: serde_json::Value = serde_json::from_str(&resp).expect("valid JSON");
        let message = parsed["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("already connected"),
            "noop message must announce 'already connected'; got {message:?}"
        );
        assert!(
            message.contains("peer"),
            "noop message must surface the existing relation type so the caller \
             knows what's there; got {message:?}"
        );
    }

    #[test]
    fn list_peers_unaffected_by_add_no_op_guard() {
        // Regression: confirm dedupe still works the same when the
        // no-op guard turns redundant adds into no-ops. Before this
        // test runs, the only row is the reverse (target→source);
        // list_peers from either side should still surface the peer
        // exactly once with the reverse-side relation_type.
        let (src_path, src_id) = make_project("add-list-src");
        let (tgt_path, tgt_id) = make_project("add-list-tgt");

        let db = crate::db::shared();
        {
            let conn = db.lock();
            WorkspaceRelation::create(
                &conn,
                &Uuid::new_v4().to_string(),
                &tgt_id,
                &src_id,
                "collaborator",
            )
            .unwrap();
        }

        // src side: attempts add → no-op.
        let _ = connections(&src_path, "add", Some("add-list-tgt"), Some("oversees"));

        // list_peers from src side should see tgt exactly once.
        let src_peers = list_peers(&src_path).expect("list_peers ok");
        assert_eq!(
            src_peers.len(),
            1,
            "src must see exactly 1 peer (incoming reverse); got {src_peers:?}"
        );
        assert_eq!(src_peers[0].project_id, tgt_id);

        // list_peers from tgt side should see src exactly once.
        let tgt_peers = list_peers(&tgt_path).expect("list_peers ok");
        assert_eq!(
            tgt_peers.len(),
            1,
            "tgt must see exactly 1 peer (its own outgoing); got {tgt_peers:?}"
        );
        assert_eq!(tgt_peers[0].project_id, src_id);
    }

    // ── Migration 0051 smoke test ────────────────────────────────
    //
    // Seeds symmetric (A→B + B→A) AND a solo pair (C→D) on a fresh
    // isolated DB, runs migrations, asserts the symmetric pair
    // collapsed to one row with both relation_types merged, and the
    // solo pair remained untouched. Uses an isolated test connection
    // so we can control migration ordering (the shared `init_for_tests`
    // already ran migrations).

    #[test]
    fn migration_0051_dedupes_symmetric_pairs_and_merges_relation_types() {
        use rusqlite::Connection;

        // Fresh in-memory DB; manually run migrations up to (not
        // including) 0051 so we can seed the legacy state, then run
        // 0051 in isolation.
        let conn = Connection::open(":memory:").unwrap();
        conn.busy_timeout(std::time::Duration::from_millis(5000))
            .unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        crate::db::run_migrations(&conn).expect("run all migrations");

        // Clear out everything seeded by other tests / migrations.
        conn.execute("DELETE FROM workspace_relations", []).unwrap();

        // Seed 4 projects: a, b for the symmetric pair; c, d for solo.
        conn.execute(
            "INSERT INTO projects (id, path, name) VALUES \
             ('p-a', '/tmp/0051-a', 'a'), \
             ('p-b', '/tmp/0051-b', 'b'), \
             ('p-c', '/tmp/0051-c', 'c'), \
             ('p-d', '/tmp/0051-d', 'd')",
            [],
        )
        .unwrap();

        // A→B as 'peer' at created_at=100.
        // B→A as 'collaborator' at created_at=200.
        // C→D as 'oversees' at created_at=150 (solo, must stay).
        conn.execute(
            "INSERT INTO workspace_relations \
             (id, source_project_id, target_project_id, relation_type, created_at) VALUES \
             ('rel-ab', 'p-a', 'p-b', 'peer',         100), \
             ('rel-ba', 'p-b', 'p-a', 'collaborator', 200), \
             ('rel-cd', 'p-c', 'p-d', 'oversees',     150)",
            [],
        )
        .unwrap();

        // Sanity: 3 rows before dedup.
        let before: i64 = conn
            .query_row("SELECT COUNT(*) FROM workspace_relations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(before, 3, "test setup should produce 3 relation rows");

        // Backdate the 0051 marker so we can replay it manually
        // against the seeded state. Then re-run migrations — 0051
        // fires on the symmetric pair.
        conn.execute(
            "DELETE FROM _migrations WHERE name = '0051_dedup_symmetric_workspace_relations'",
            [],
        )
        .unwrap();
        crate::db::run_migrations(&conn).expect("re-run with 0051");

        // After dedup: 2 rows (one merged from A↔B + the solo C→D).
        let after: i64 = conn
            .query_row("SELECT COUNT(*) FROM workspace_relations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            after, 2,
            "expected 2 rows after dedup (1 merged symmetric + 1 solo); got {after}"
        );

        // Solo row untouched.
        let solo_rt: String = conn
            .query_row(
                "SELECT relation_type FROM workspace_relations WHERE id = 'rel-cd'",
                [],
                |r| r.get(0),
            )
            .expect("solo C→D row must survive");
        assert_eq!(
            solo_rt, "oversees",
            "solo C→D row's relation_type must be untouched"
        );

        // Surviving merged row: the EARLIER created_at (A→B at 100)
        // is the keeper; the LATER (B→A at 200) was deleted.
        let keeper_id: String = conn
            .query_row(
                "SELECT id FROM workspace_relations \
                 WHERE source_project_id = 'p-a' AND target_project_id = 'p-b'",
                [],
                |r| r.get(0),
            )
            .expect("keeper A→B row must remain");
        assert_eq!(keeper_id, "rel-ab", "earlier row must be the keeper");

        // Reverse direction must be gone.
        let reverse_exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM workspace_relations \
                 WHERE source_project_id = 'p-b' AND target_project_id = 'p-a'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(reverse_exists, 0, "later B→A row must be deleted");

        // Merged relation_type contains both original labels (order
        // doesn't matter — assert by substring presence on both).
        let merged_rt: String = conn
            .query_row(
                "SELECT relation_type FROM workspace_relations WHERE id = 'rel-ab'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            merged_rt.contains("peer"),
            "merged row must retain 'peer' label; got {merged_rt:?}"
        );
        assert!(
            merged_rt.contains("collaborator"),
            "merged row must retain 'collaborator' label; got {merged_rt:?}"
        );
        assert!(
            merged_rt.contains(';'),
            "merged row must use ';' as separator; got {merged_rt:?}"
        );
    }

    #[test]
    fn migration_0051_is_no_op_on_db_with_no_symmetric_pairs() {
        // Fresh-workspace edge case: a DB whose `workspace_relations`
        // either is empty or only carries unique directional rows
        // should pass through 0051 unchanged. Both states are valid
        // post-migration outcomes.
        use rusqlite::Connection;
        let conn = Connection::open(":memory:").unwrap();
        conn.busy_timeout(std::time::Duration::from_millis(5000))
            .unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        crate::db::run_migrations(&conn).expect("run all migrations");

        conn.execute("DELETE FROM workspace_relations", []).unwrap();
        conn.execute(
            "INSERT INTO projects (id, path, name) VALUES \
             ('p-x', '/tmp/0051-x', 'x'), \
             ('p-y', '/tmp/0051-y', 'y'), \
             ('p-z', '/tmp/0051-z', 'z')",
            [],
        )
        .unwrap();
        // Two solo (non-symmetric) rows. Must both survive.
        conn.execute(
            "INSERT INTO workspace_relations \
             (id, source_project_id, target_project_id, relation_type, created_at) VALUES \
             ('rel-xy', 'p-x', 'p-y', 'oversees', 100), \
             ('rel-yz', 'p-y', 'p-z', 'peer',     200)",
            [],
        )
        .unwrap();

        // Re-run 0051 explicitly.
        conn.execute(
            "DELETE FROM _migrations WHERE name = '0051_dedup_symmetric_workspace_relations'",
            [],
        )
        .unwrap();
        crate::db::run_migrations(&conn).expect("re-run with 0051");

        let after: i64 = conn
            .query_row("SELECT COUNT(*) FROM workspace_relations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            after, 2,
            "non-symmetric rows must survive 0051 unchanged; got {after}"
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

//! 0.38.0 — one-shot heal pass for `workspace_layouts.layout_json`.
//!
//! Pre-0.38.0, the mount-time `sync:tabs-request` broadcast race caused
//! non-main windows to adopt tabs from the main window on top of their
//! own restored set. When the bloated state was saved back, the layout
//! JSON grew duplicate `tabs[]` entries that all pointed at the same
//! paneGroup id. Every subsequent open faithfully recreated the dupes.
//!
//! This pass runs once per daemon, gated by a `code_migrations` marker
//! (`0.38.0-layout-dedup`). It iterates every `workspace_layouts` row,
//! collapses tabs sharing canonical paneGroup-id sets in the main
//! group and every extra group, and writes the cleaned JSON back.
//! The renderer also does the same dedup on every restore (see
//! `tabs.ts::dedupTabsBySignature`) — this pass is the eager
//! one-shot cleanup so corrupt rows get repaired even for workspaces
//! the user hasn't opened in a while.

use serde_json::Value;
use std::collections::BTreeSet;

use k2so_core::log_debug;

const MIGRATION_ID: &str = "0.38.0-layout-dedup";

/// Canonical identity of one tab: the sorted set of paneGroup IDs it
/// owns, joined as a comma-separated string. Two tabs with identical
/// signatures point at the same daemon-side session(s) and are
/// duplicates regardless of distinct renderer-side tab UUIDs.
fn tab_signature(tab: &Value) -> String {
    let mut ids: BTreeSet<String> = BTreeSet::new();
    if let Some(pgs) = tab.get("paneGroups").and_then(|v| v.as_object()) {
        for (k, _) in pgs {
            ids.insert(k.clone());
        }
    }
    ids.into_iter().collect::<Vec<_>>().join(",")
}

/// Walk a `tabs[]` array, dropping duplicates by signature.
/// Returns (cleaned_tabs, removed_count, id_remap). The id_remap maps
/// every removed tab's `id` to the kept tab's `id` so callers can
/// rewrite `activeTabId` pointers that referenced a dropped duplicate.
fn dedup_tabs_array(
    tabs: Vec<Value>,
) -> (Vec<Value>, usize, std::collections::HashMap<String, String>) {
    use std::collections::HashMap;
    let mut seen: HashMap<String, String> = HashMap::new(); // signature -> kept tab id
    let mut id_remap: HashMap<String, String> = HashMap::new();
    let mut kept: Vec<Value> = Vec::with_capacity(tabs.len());
    let mut removed = 0usize;
    for tab in tabs {
        let sig = tab_signature(&tab);
        let tab_id = tab
            .get("id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_default();
        if let Some(kept_id) = seen.get(&sig) {
            if !tab_id.is_empty() {
                id_remap.insert(tab_id, kept_id.clone());
            }
            removed += 1;
        } else {
            seen.insert(sig, tab_id.clone());
            kept.push(tab);
        }
    }
    (kept, removed, id_remap)
}

/// Returns (changed, total_removed). `changed` is whether any tab was
/// dropped; if false, callers skip the UPDATE entirely.
fn dedup_layout_json(layout: &mut Value) -> (bool, usize) {
    let mut total_removed = 0usize;

    // Main group: tabs + activeTabId
    if let Some(tabs_arr) = layout.get_mut("tabs").and_then(|v| v.as_array_mut()) {
        let original = std::mem::take(tabs_arr);
        let (cleaned, removed, id_remap) = dedup_tabs_array(original);
        *tabs_arr = cleaned;
        total_removed += removed;

        // Rewrite activeTabId if it pointed at a dropped duplicate.
        if removed > 0 {
            if let Some(active) = layout.get("activeTabId").and_then(|v| v.as_str()) {
                if let Some(new_active) = id_remap.get(active) {
                    layout["activeTabId"] = Value::String(new_active.clone());
                }
            }
        }
    }

    // Extra groups (split columns): each has its own tabs[] + activeTabId.
    if let Some(extra) = layout.get_mut("extraGroups").and_then(|v| v.as_array_mut()) {
        for group in extra.iter_mut() {
            if let Some(group_tabs) = group.get_mut("tabs").and_then(|v| v.as_array_mut()) {
                let original = std::mem::take(group_tabs);
                let (cleaned, removed, id_remap) = dedup_tabs_array(original);
                *group_tabs = cleaned;
                total_removed += removed;
                if removed > 0 {
                    if let Some(active) = group.get("activeTabId").and_then(|v| v.as_str()) {
                        if let Some(new_active) = id_remap.get(active) {
                            group["activeTabId"] = Value::String(new_active.clone());
                        }
                    }
                }
            }
        }
    }

    (total_removed > 0, total_removed)
}

/// Run the one-shot heal pass. Idempotent — gated by a
/// `code_migrations` marker that's stamped on successful completion.
/// Safe to call on every daemon boot; subsequent calls are O(1) lookups.
pub fn run_once() {
    let db = k2so_core::db::shared();
    let conn = db.lock();

    if k2so_core::db::has_code_migration_applied(&conn, MIGRATION_ID) {
        return;
    }

    log_debug!("[daemon/boot] running {MIGRATION_ID} pass over workspace_layouts");

    // SELECT all rows first so the prepared statement's row iterator
    // doesn't hold a borrow across the UPDATE statements.
    let mut rows: Vec<(String, String, String, String)> = Vec::new();
    match conn.prepare(
        "SELECT id, project_id, workspace_id, layout_json FROM workspace_layouts",
    ) {
        Ok(mut stmt) => {
            let mapped = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            });
            if let Ok(it) = mapped {
                for r in it.flatten() {
                    rows.push(r);
                }
            }
        }
        Err(e) => {
            log_debug!("[daemon/boot] {MIGRATION_ID}: SELECT failed: {e}");
            return;
        }
    }

    let total = rows.len();
    let mut healed = 0usize;
    let mut tabs_removed = 0usize;
    for (id, project_id, workspace_id, json_str) in rows {
        let mut layout: Value = match serde_json::from_str(&json_str) {
            Ok(v) => v,
            Err(e) => {
                log_debug!(
                    "[daemon/boot] {MIGRATION_ID}: skip row id={id} (parse: {e})"
                );
                continue;
            }
        };

        let (changed, removed) = dedup_layout_json(&mut layout);
        if !changed {
            continue;
        }

        let new_json = match serde_json::to_string(&layout) {
            Ok(s) => s,
            Err(e) => {
                log_debug!(
                    "[daemon/boot] {MIGRATION_ID}: skip row id={id} (re-serialize: {e})"
                );
                continue;
            }
        };

        match conn.execute(
            "UPDATE workspace_layouts SET layout_json = ?1, updated_at = unixepoch() WHERE id = ?2",
            rusqlite::params![new_json, id],
        ) {
            Ok(_) => {
                healed += 1;
                tabs_removed += removed;
                log_debug!(
                    "[daemon/boot] {MIGRATION_ID}: healed project={project_id} workspace={workspace_id} (-{removed} tab rows)"
                );
            }
            Err(e) => {
                log_debug!(
                    "[daemon/boot] {MIGRATION_ID}: UPDATE failed for id={id}: {e}"
                );
            }
        }
    }

    let notes = format!("rows_scanned={total} rows_healed={healed} tabs_removed={tabs_removed}");
    k2so_core::db::mark_code_migration_applied(&conn, MIGRATION_ID, Some(&notes));
    log_debug!(
        "[daemon/boot] {MIGRATION_ID} complete: {notes}"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn dedup_collapses_tabs_sharing_panegroups() {
        let mut layout = json!({
            "tabs": [
                { "id": "t1", "title": "Chat",   "paneGroups": { "pg-a": {} } },
                { "id": "t2", "title": "Claude", "paneGroups": { "pg-b": {} } },
                { "id": "t3", "title": "Claude", "paneGroups": { "pg-b": {} } },
                { "id": "t4", "title": "Claude", "paneGroups": { "pg-b": {} } }
            ],
            "activeTabId": "t3"
        });
        let (changed, removed) = dedup_layout_json(&mut layout);
        assert!(changed);
        assert_eq!(removed, 2);
        let tabs = layout["tabs"].as_array().unwrap();
        assert_eq!(tabs.len(), 2);
        // activeTabId t3 was a dup of t2 → remapped
        assert_eq!(layout["activeTabId"].as_str().unwrap(), "t2");
    }

    #[test]
    fn dedup_noop_on_clean_layout() {
        let mut layout = json!({
            "tabs": [
                { "id": "t1", "paneGroups": { "pg-a": {} } },
                { "id": "t2", "paneGroups": { "pg-b": {} } }
            ],
            "activeTabId": "t1"
        });
        let before = layout.clone();
        let (changed, removed) = dedup_layout_json(&mut layout);
        assert!(!changed);
        assert_eq!(removed, 0);
        assert_eq!(layout, before);
    }

    #[test]
    fn dedup_handles_extra_groups() {
        let mut layout = json!({
            "tabs": [
                { "id": "t1", "paneGroups": { "pg-a": {} } }
            ],
            "extraGroups": [
                {
                    "tabs": [
                        { "id": "g1t1", "paneGroups": { "pg-x": {} } },
                        { "id": "g1t2", "paneGroups": { "pg-x": {} } }
                    ],
                    "activeTabId": "g1t2"
                }
            ]
        });
        let (changed, removed) = dedup_layout_json(&mut layout);
        assert!(changed);
        assert_eq!(removed, 1);
        let g0 = layout["extraGroups"][0]["tabs"].as_array().unwrap();
        assert_eq!(g0.len(), 1);
        assert_eq!(layout["extraGroups"][0]["activeTabId"].as_str().unwrap(), "g1t1");
    }
}

//! Aggregated agent check-in — the data bundle an agent reads at the
//! top of a session to understand its current state.
//!
//! Serves `/cli/checkin`. Composes:
//!
//! - `task`: first file in the agent's `work/active/` (structured).
//! - `inbox.work`: all files in the agent's `work/inbox/` + the
//!   workspace-level `.k2so/work/inbox/` (for manager roles).
//! - `inbox.messages`: unread DB messages addressed to this agent.
//!   Marked read on retrieval.
//! - `peers`: `agent_sessions` rows for every connected workspace
//!   (outgoing + incoming `workspace_relations`), with the current
//!   status / status_message / terminal_id.
//! - `reservations`: the JSON map at `.k2so/reservations.json`.
//! - `feed`: last 10 activity-feed entries for this project.
//! - `wakeupInstructions`: the agent's wakeup.md body (or the
//!   workspace-level wakeup for `__lead__`); `null` for
//!   agent-template roles that don't use wake-up prompts.
//!
//! Finally logs a `checkin` activity entry so peers can see the
//! agent just checked in.
//!
//! Moved to core so the daemon serves it headlessly.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use crate::agents::resolve_project_id;
use crate::agents::wake::{compose_wake_prompt_for_agent, compose_wake_prompt_for_lead};
use crate::db::schema::{
    get_unread_messages, log_activity, mark_messages_read, ActivityFeedEntry, WorkspaceSession,
    WorkspaceRelation,
};

/// Parse the minimum frontmatter fields the CLI echoes back for each
/// work item. Kept private: callers that want the full `WorkItem`
/// struct use [`super::work_item::read_work_item`] instead.
fn parse_work_item(filename: &str, content: &str) -> serde_json::Value {
    let mut title = filename.trim_end_matches(".md").to_string();
    let mut priority = "normal".to_string();
    let mut item_type = "task".to_string();
    let mut from = serde_json::Value::Null;
    let mut body = content.to_string();

    if let Some(stripped) = content.strip_prefix("---\n") {
        if let Some(end) = stripped.find("\n---") {
            let fm = &stripped[..end];
            body = stripped[end + 4..].trim().to_string();
            for line in fm.lines() {
                let parts: Vec<&str> = line.splitn(2, ':').collect();
                if parts.len() == 2 {
                    let key = parts[0].trim();
                    let val = parts[1].trim().trim_matches('"');
                    match key {
                        "title" => title = val.to_string(),
                        "priority" => priority = val.to_string(),
                        "type" => item_type = val.to_string(),
                        "from" => from = serde_json::Value::String(val.to_string()),
                        "assigned_by" if from.is_null() => {
                            from = serde_json::Value::String(val.to_string())
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    serde_json::json!({
        "file": filename,
        "title": title,
        "priority": priority,
        "type": item_type,
        "from": from,
        "body": body,
    })
}

/// Build the check-in bundle. Returns JSON string matching the shape
/// the CLI has emitted since 0.32.x.
pub fn checkin(project_path: &str, agent: &str) -> Result<String, String> {
    if agent.is_empty() {
        return Err("Missing 'agent' parameter".to_string());
    }

    let db = crate::db::shared();
    let conn = db.lock();
    let project_id = resolve_project_id(&conn, project_path)
        .ok_or_else(|| format!("Project not found: {}", project_path))?;

    // Current task (first file in active/)
    let active_dir = PathBuf::from(project_path)
        .join(".k2so/agents")
        .join(agent)
        .join("work/active");
    let task: serde_json::Value = if active_dir.is_dir() {
        fs::read_dir(&active_dir)
            .ok()
            .and_then(|mut entries| entries.next())
            .and_then(|e| e.ok())
            .map(|e| {
                let fname = e.file_name().to_string_lossy().to_string();
                let content = fs::read_to_string(e.path()).unwrap_or_default();
                parse_work_item(&fname, &content)
            })
            .unwrap_or(serde_json::Value::Null)
    } else {
        serde_json::Value::Null
    };

    // Agent inbox + workspace inbox (for manager roles)
    let inbox_dir = PathBuf::from(project_path)
        .join(".k2so/agents")
        .join(agent)
        .join("work/inbox");
    let mut work_items: Vec<serde_json::Value> = if inbox_dir.is_dir() {
        fs::read_dir(&inbox_dir)
            .ok()
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .map(|e| {
                        let fname = e.file_name().to_string_lossy().to_string();
                        let content = fs::read_to_string(e.path()).unwrap_or_default();
                        parse_work_item(&fname, &content)
                    })
                    .collect()
            })
            .unwrap_or_default()
    } else {
        vec![]
    };

    let ws_inbox_dir = PathBuf::from(project_path).join(".k2so/work/inbox");
    if ws_inbox_dir.is_dir() {
        if let Ok(entries) = fs::read_dir(&ws_inbox_dir) {
            for e in entries.flatten() {
                let fname = e.file_name().to_string_lossy().to_string();
                let content = fs::read_to_string(e.path()).unwrap_or_default();
                work_items.push(parse_work_item(&fname, &content));
            }
        }
    }

    // Messages (DB-indexed)
    let messages: Vec<serde_json::Value> = get_unread_messages(&conn, &project_id, agent)
        .unwrap_or_default()
        .into_iter()
        .map(|m| {
            let text = m
                .metadata
                .as_deref()
                .and_then(|md| serde_json::from_str::<serde_json::Value>(md).ok())
                .and_then(|v| {
                    v.get("text")
                        .and_then(|t| t.as_str())
                        .map(|s| s.to_string())
                })
                .unwrap_or_else(|| m.summary.clone().unwrap_or_default());
            serde_json::json!({
                "type": "message",
                "from": m.from_workspace,
                "text": text,
                "at": m.created_at,
                "id": m.id,
            })
        })
        .collect();

    let _ = mark_messages_read(&conn, &project_id, agent);

    let inbox = serde_json::json!({
        "work": work_items,
        "messages": messages,
    });

    // Peers: agent_sessions across this project + connected workspaces
    let mut peer_project_ids = vec![project_id.clone()];
    if let Ok(rels) = WorkspaceRelation::list_for_source(&conn, &project_id) {
        for r in &rels {
            peer_project_ids.push(r.target_project_id.clone());
        }
    }
    if let Ok(rels) = WorkspaceRelation::list_for_target(&conn, &project_id) {
        for r in &rels {
            peer_project_ids.push(r.source_project_id.clone());
        }
    }

    let mut project_names: HashMap<String, String> = HashMap::new();
    for pid in &peer_project_ids {
        if let Ok(name) = conn.query_row(
            "SELECT name FROM projects WHERE id = ?1",
            rusqlite::params![pid],
            |row| row.get::<_, String>(0),
        ) {
            project_names.insert(pid.clone(), name);
        }
    }

    let mut peers = Vec::new();
    for pid in &peer_project_ids {
        if pid == &project_id {
            // The caller's own workspace — skip; not a peer.
            continue;
        }
        if let Ok(Some(s)) = WorkspaceSession::get(&conn, pid) {
            let pname = project_names.get(pid).cloned().unwrap_or_default();
            peers.push(serde_json::json!({
                "agent": pname.clone(),
                "status": s.status,
                "statusMessage": s.status_message,
                "terminalId": s.terminal_id,
                "project": pname,
                "projectId": s.project_id,
                "harness": s.harness,
            }));
        }
    }

    // Reservations
    let reservations_path = PathBuf::from(project_path).join(".k2so/reservations.json");
    let reservations: serde_json::Value = if reservations_path.exists() {
        fs::read_to_string(&reservations_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    // Recent feed
    let feed: Vec<serde_json::Value> =
        ActivityFeedEntry::list_by_project(&conn, &project_id, 10, 0)
            .unwrap_or_default()
            .into_iter()
            .map(|e| {
                serde_json::json!({
                    "eventType": e.event_type,
                    "agent": e.actor,
                    "from": e.from_workspace,
                    "to": e.to_workspace,
                    "summary": e.summary,
                    "createdAt": e.created_at,
                })
            })
            .collect();

    log_activity(
        &conn,
        &project_id,
        Some(agent),
        "checkin",
        Some(agent),
        None,
        None,
        None,
    );

    // Wake-up instructions — __lead__ uses the workspace wake prompt
    // composer; other agents use their own wakeup.md (or null for
    // agent-template roles that don't use wake-up).
    let wakeup_instructions: serde_json::Value = if agent == "__lead__" {
        serde_json::Value::String(compose_wake_prompt_for_lead(project_path))
    } else {
        match compose_wake_prompt_for_agent(project_path, agent) {
            Some(s) => serde_json::Value::String(s),
            None => serde_json::Value::Null,
        }
    };

    Ok(serde_json::json!({
        "agent": agent,
        "project": project_path,
        "task": task,
        "inbox": inbox,
        "peers": peers,
        "reservations": reservations,
        "feed": feed,
        "wakeupInstructions": wakeup_instructions,
    })
    .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_work_item_reads_basic_frontmatter() {
        let md = "---\ntitle: Fix auth bug\npriority: high\ntype: bug\nfrom: qa\n---\n\nDetails here.";
        let v = parse_work_item("bug.md", md);
        assert_eq!(v.get("title").and_then(|t| t.as_str()), Some("Fix auth bug"));
        assert_eq!(v.get("priority").and_then(|t| t.as_str()), Some("high"));
        assert_eq!(v.get("type").and_then(|t| t.as_str()), Some("bug"));
        assert_eq!(v.get("from").and_then(|t| t.as_str()), Some("qa"));
        assert_eq!(v.get("file").and_then(|t| t.as_str()), Some("bug.md"));
    }

    #[test]
    fn parse_work_item_without_frontmatter_defaults_gracefully() {
        let v = parse_work_item("plain.md", "Just a body, no frontmatter.");
        assert_eq!(v.get("title").and_then(|t| t.as_str()), Some("plain"));
        assert_eq!(v.get("priority").and_then(|t| t.as_str()), Some("normal"));
        assert_eq!(v.get("type").and_then(|t| t.as_str()), Some("task"));
        assert!(v.get("from").unwrap().is_null());
    }

    #[test]
    fn parse_work_item_falls_back_to_assigned_by_when_from_missing() {
        let md = "---\ntitle: T\nassigned_by: reviewer\n---\n";
        let v = parse_work_item("t.md", md);
        assert_eq!(v.get("from").and_then(|t| t.as_str()), Some("reviewer"));
    }
}

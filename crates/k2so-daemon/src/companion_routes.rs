//! H4 of Phase 4 — daemon-side `/cli/companion/sessions` +
//! `/cli/companion/projects-summary`.
//!
//! Both endpoints enumerate live sessions across every workspace
//! and project them against the `projects` table so companion
//! clients (mobile, web viewer, desktop sidebar) can show a
//! cross-workspace summary without reaching into the Tauri
//! process.
//!
//! Before Phase 4, these lived in Tauri's `agent_hooks.rs` and
//! walked `AppState.terminal_manager.list_terminal_ids()` — which
//! only knew about Tauri-spawned terminals. With session ownership
//! now daemon-side (Phase 3.1 onward), those walks miss every
//! daemon-owned session. H4 rewrites the logic to source live
//! sessions from `session_map::snapshot()`.
//!
//! **Response shapes match the legacy Tauri endpoints** so
//! existing companion clients don't need to branch:
//!
//! - `sessions` → array of per-session records with workspace
//!   attribution (matched by longest cwd-prefix).
//! - `projects-summary` → one record per project with counts,
//!   focus-group info, and pending-review tallies.

use std::collections::HashMap;

use crate::cli_response::CliResponse;
use crate::session_map;

/// Minimal projects row shape the companion routes need.
struct ProjectRow {
    id: String,
    name: String,
    path: String,
    color: String,
}

fn list_projects() -> Vec<ProjectRow> {
    let db = k2so_core::db::shared();
    let conn = db.lock();
    let Ok(mut stmt) = conn.prepare(
        "SELECT id, name, path, color FROM projects ORDER BY name ASC",
    ) else {
        return Vec::new();
    };
    let rows = stmt
        .query_map([], |row| {
            Ok(ProjectRow {
                id: row.get(0)?,
                name: row.get(1)?,
                path: row.get(2)?,
                color: row.get(3).unwrap_or_default(),
            })
        })
        .ok();
    let Some(rows) = rows else { return Vec::new() };
    rows.filter_map(Result::ok).collect()
}

/// Projects row + focus-group fields for the summary endpoint.
struct ProjectSummaryRow {
    id: String,
    name: String,
    path: String,
    color: String,
    agent_mode: String,
    pinned: bool,
    tab_order: i32,
    fg_id: Option<String>,
    fg_name: Option<String>,
    fg_color: Option<String>,
}

fn list_projects_for_summary() -> Vec<ProjectSummaryRow> {
    let db = k2so_core::db::shared();
    let conn = db.lock();
    let Ok(mut stmt) = conn.prepare(
        "SELECT p.id, p.name, p.path, p.color, p.agent_mode, p.pinned, p.tab_order, \
                p.focus_group_id, fg.name, fg.color \
         FROM projects p \
         LEFT JOIN focus_groups fg ON p.focus_group_id = fg.id \
         ORDER BY p.pinned DESC, p.tab_order ASC, p.name ASC",
    ) else {
        return Vec::new();
    };
    let rows = stmt
        .query_map([], |row| {
            Ok(ProjectSummaryRow {
                id: row.get(0)?,
                name: row.get(1)?,
                path: row.get(2)?,
                color: row.get(3).unwrap_or_default(),
                agent_mode: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
                pinned: row.get::<_, i64>(5).unwrap_or(0) == 1,
                tab_order: row.get::<_, i64>(6).unwrap_or(0) as i32,
                fg_id: row.get(7)?,
                fg_name: row.get(8)?,
                fg_color: row.get(9)?,
            })
        })
        .ok();
    let Some(rows) = rows else { return Vec::new() };
    rows.filter_map(Result::ok).collect()
}

/// Match a session's cwd to the workspace whose path is its
/// longest matching prefix. Returns `None` if no project
/// contains the cwd. "Longest prefix wins" covers the common
/// case of nested projects (e.g. `/foo` and `/foo/bar` both
/// registered; a session in `/foo/bar/deep` belongs to
/// `/foo/bar`).
fn match_workspace<'a>(
    cwd: &str,
    projects: &'a [ProjectRow],
) -> Option<&'a ProjectRow> {
    projects
        .iter()
        .filter(|p| cwd.starts_with(&p.path))
        .max_by_key(|p| p.path.len())
}

/// Derive (agent_name, label) for a session's display. The
/// legacy rules:
///
/// 1. If cwd is under `<ws_path>/.k2so/worktrees/<name>/...` →
///    the first path component after `worktrees/` is both agent
///    and label. Matches the delegate-creates-worktree pattern.
/// 2. Otherwise use `<cwd>`'s final path component as the label
///    and `"shell"` as the agent name.
///
/// H4 adds a third rule: if the daemon's session_map already
/// tagged the session with a real agent_name (not the
/// `terminal-<hex>` synthesized form from H3's background
/// spawns), prefer that over cwd-derivation. Gives named
/// sessions a stable label even when they run in the workspace
/// root.
fn derive_agent_and_label(
    session_agent_name: &str,
    cwd: &str,
    ws_path: &str,
    ws_name: &str,
) -> (String, String) {
    let worktree_prefix = format!("{ws_path}/.k2so/worktrees/");
    if let Some(rest) = cwd.strip_prefix(&worktree_prefix) {
        let name = rest.split('/').next().unwrap_or("agent");
        return (name.to_string(), name.to_string());
    }
    // Prefer the session_map-registered agent name when it's a
    // real agent (not a background-spawn synthetic like
    // "terminal-abcd1234").
    if !session_agent_name.is_empty() && !session_agent_name.starts_with("terminal-") {
        return (session_agent_name.to_string(), session_agent_name.to_string());
    }
    let folder = std::path::Path::new(cwd)
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_else(|| ws_name.to_string());
    ("shell".to_string(), folder)
}

/// Handler for `GET /cli/companion/sessions`.
///
/// Global session enumeration — every live session in the
/// daemon, joined against `projects` by longest cwd-prefix.
/// Sessions whose cwd doesn't match any registered project are
/// dropped (same behavior as the legacy Tauri endpoint).
pub fn handle_companion_sessions(_params: &HashMap<String, String>) -> CliResponse {
    let projects = list_projects();
    let live = session_map::snapshot();
    let mut out: Vec<serde_json::Value> = Vec::with_capacity(live.len());
    for (agent_name, session) in live {
        let Some(ws) = match_workspace(&session.cwd, &projects) else {
            continue;
        };
        let (agent, label) =
            derive_agent_and_label(&agent_name, &session.cwd, &ws.path, &ws.name);
        out.push(serde_json::json!({
            "workspaceName": ws.name,
            "workspaceId": ws.id,
            "workspaceColor": ws.color,
            "agentName": agent,
            "label": label,
            "terminalId": session.session_id.to_string(),
            "command": session.command,
            "cwd": session.cwd,
        }));
    }
    CliResponse::ok_json(serde_json::to_string(&out).unwrap_or_else(|_| "[]".into()))
}

/// Handler for `GET /cli/companion/projects-summary`.
///
/// One record per registered project with live-session counts
/// and filesystem-derived pending-review counts. Matches the
/// pre-Phase-4 Tauri shape byte-for-byte so the companion UI
/// can swap endpoints without change.
///
/// Review-pending count walks `<project>/.k2so/agents/*/work/done/`
/// filesystem — same heuristic the legacy route used. Future
/// commit can switch this to a DB query once work-item state
/// lives there.
pub fn handle_companion_projects_summary(
    _params: &HashMap<String, String>,
) -> CliResponse {
    let workspaces = list_projects_for_summary();
    let live_sessions = session_map::snapshot();

    // Tally live sessions per workspace via longest-prefix match,
    // identical to the /cli/companion/sessions grouping rule.
    let mut counts: HashMap<String, usize> = HashMap::new();
    for (_, session) in &live_sessions {
        let matched = workspaces
            .iter()
            .filter(|p| session.cwd.starts_with(&p.path))
            .max_by_key(|p| p.path.len());
        if let Some(ws) = matched {
            *counts.entry(ws.id.clone()).or_insert(0) += 1;
        }
    }

    let mut summaries: Vec<serde_json::Value> = Vec::with_capacity(workspaces.len());
    for ws in &workspaces {
        let review_count = count_pending_reviews(&ws.path);
        let focus_group = match (&ws.fg_id, &ws.fg_name) {
            (Some(id), Some(name)) => serde_json::json!({
                "id": id,
                "name": name,
                "color": ws.fg_color,
            }),
            _ => serde_json::Value::Null,
        };
        summaries.push(serde_json::json!({
            "id": ws.id,
            "name": ws.name,
            "path": ws.path,
            "color": ws.color,
            "agentMode": ws.agent_mode,
            "pinned": ws.pinned,
            "tabOrder": ws.tab_order,
            "focusGroup": focus_group,
            "agentsRunning": counts.get(&ws.id).copied().unwrap_or(0),
            "reviewsPending": review_count,
        }));
    }
    CliResponse::ok_json(
        serde_json::to_string(&summaries).unwrap_or_else(|_| "[]".into()),
    )
}

/// Count `.md` files in `<project>/.k2so/agents/*/work/done/`.
/// Matches the legacy Tauri heuristic. Returns 0 on any IO
/// error so a broken filesystem doesn't 500 the endpoint.
fn count_pending_reviews(project_path: &str) -> usize {
    let agents_dir = std::path::Path::new(project_path).join(".k2so/agents");
    let Ok(entries) = std::fs::read_dir(&agents_dir) else {
        return 0;
    };
    let mut count = 0;
    for entry in entries.flatten() {
        let done_dir = entry.path().join("work/done");
        let Ok(files) = std::fs::read_dir(&done_dir) else {
            continue;
        };
        count += files
            .filter_map(Result::ok)
            .filter(|f| f.path().extension().is_some_and(|ext| ext == "md"))
            .count();
    }
    count
}


//! Phase 2 Unit 4 — daemon-side operations for the SQLite domains
//! previously written by Tauri commands.
//!
//! Each `pub fn` in this module is the canonical implementation of a
//! state-mutating (or non-trivial query) operation against
//! `k2so_core::db::shared()`. The daemon's `/cli/*` route handlers
//! (and the Tauri commands during the transition) call these.
//!
//! Conventions
//! - All returns are `Result<T, String>` so the caller can hand the
//!   error straight to a JSON `{"error": "..."}` body.
//! - Inputs use `Option<T>` for "leave unchanged on update" semantics
//!   that match the pre-Unit-4 Tauri command signatures.
//! - No file I/O outside the DB unless explicitly noted (project_config
//!   reads / git scans).

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::db;
use crate::db::schema::{
    AgentPreset, FocusGroup, Project, TimeEntry, Workspace, WorkspaceSection, WorkspaceState,
};
use crate::project_config;

// ── Built-in agent presets ──────────────────────────────────────────────
//
// Lives here (instead of the daemon-side route module) because it's
// pure data — both the Tauri shim and the daemon route call the same
// `presets_reset_built_ins` here. Originally lived in
// `src-tauri/src/commands/agents.rs`; moved verbatim so the IDs +
// order still match `db::seed_agent_presets` exactly.
const BUILT_IN_PRESETS: &[(&str, &str, &str, &str, i64)] = &[
    ("b0a1c2d3-e4f5-6789-abcd-ef0123456001", "Claude", "claude --dangerously-skip-permissions", "", 0),
    ("b0a1c2d3-e4f5-6789-abcd-ef0123456002", "Codex", "codex -c model_reasoning_effort=\"high\" --dangerously-bypass-approvals-and-sandbox", "", 1),
    ("b0a1c2d3-e4f5-6789-abcd-ef0123456003", "Gemini", "gemini --yolo", "", 2),
    ("b0a1c2d3-e4f5-6789-abcd-ef0123456006", "Cursor Agent", "cursor-agent", "", 3),
    ("b0a1c2d3-e4f5-6789-abcd-ef0123456012", "Pi", "pi", "", 4),
    ("b0a1c2d3-e4f5-6789-abcd-ef0123456007", "OpenCode", "opencode", "", 5),
    ("b0a1c2d3-e4f5-6789-abcd-ef0123456011", "Goose", "goose", "", 6),
    ("b0a1c2d3-e4f5-6789-abcd-ef0123456005", "Aider", "aider", "", 7),
    ("b0a1c2d3-e4f5-6789-abcd-ef0123456009", "Ollama", "ollama run llama3.2", "", 8),
    ("b0a1c2d3-e4f5-6789-abcd-ef0123456004", "Copilot", "copilot --allow-all", "", 9),
    ("b0a1c2d3-e4f5-6789-abcd-ef0123456010", "Interpreter", "interpreter", "", 10),
];

// ── States (workspace states) ──────────────────────────────────────────

pub fn states_list() -> Result<Vec<WorkspaceState>, String> {
    let db = db::shared();
    let conn = db.lock();
    WorkspaceState::list(&conn).map_err(|e| e.to_string())
}

pub fn states_get(id: &str) -> Result<WorkspaceState, String> {
    let db = db::shared();
    let conn = db.lock();
    WorkspaceState::get(&conn, id).map_err(|e| e.to_string())
}

pub fn states_create(
    name: &str,
    description: Option<&str>,
    cap_features: &str,
    cap_issues: &str,
    cap_crashes: &str,
    cap_security: &str,
    cap_audits: &str,
    heartbeat: bool,
) -> Result<WorkspaceState, String> {
    let db = db::shared();
    let conn = db.lock();
    let id = format!(
        "state-{}",
        Uuid::new_v4().to_string().split('-').next().unwrap_or("custom")
    );
    WorkspaceState::create(
        &conn,
        &id,
        name,
        description,
        cap_features,
        cap_issues,
        cap_crashes,
        cap_security,
        cap_audits,
        heartbeat,
    )
    .map_err(|e| e.to_string())?;
    WorkspaceState::get(&conn, &id).map_err(|e| e.to_string())
}

#[allow(clippy::too_many_arguments)]
pub fn states_update(
    id: &str,
    name: Option<&str>,
    description: Option<&str>,
    cap_features: Option<&str>,
    cap_issues: Option<&str>,
    cap_crashes: Option<&str>,
    cap_security: Option<&str>,
    cap_audits: Option<&str>,
    heartbeat: Option<bool>,
) -> Result<WorkspaceState, String> {
    let db = db::shared();
    let conn = db.lock();
    WorkspaceState::update(
        &conn,
        id,
        name,
        description,
        cap_features,
        cap_issues,
        cap_crashes,
        cap_security,
        cap_audits,
        heartbeat,
    )
    .map_err(|e| e.to_string())?;
    WorkspaceState::get(&conn, id).map_err(|e| e.to_string())
}

pub fn states_delete(id: &str) -> Result<(), String> {
    let db = db::shared();
    let conn = db.lock();
    WorkspaceState::delete(&conn, id).map_err(|e| e.to_string())
}

// ── Workspaces ─────────────────────────────────────────────────────────

pub fn workspaces_list(project_id: &str) -> Result<Vec<Workspace>, String> {
    let db = db::shared();
    let conn = db.lock();
    Workspace::list(&conn, project_id).map_err(|e| e.to_string())
}

pub fn workspaces_create(
    project_id: &str,
    name: &str,
    type_: Option<&str>,
    branch: Option<&str>,
    worktree_path: Option<&str>,
) -> Result<Workspace, String> {
    let db = db::shared();
    let conn = db.lock();
    let id = Uuid::new_v4().to_string();
    let type_val = type_.unwrap_or("branch");

    let existing = Workspace::list(&conn, project_id).unwrap_or_default();
    let max_order = existing.iter().map(|w| w.tab_order).max().unwrap_or(-1) + 1;

    Workspace::create(
        &conn,
        &id,
        project_id,
        None,
        type_val,
        branch,
        name,
        max_order,
        worktree_path,
    )
    .map_err(|e| e.to_string())?;

    Workspace::get(&conn, &id).map_err(|e| e.to_string())
}

pub fn workspaces_delete(id: &str) -> Result<(), String> {
    let db = db::shared();
    let conn = db.lock();
    Workspace::delete(&conn, id).map_err(|e| e.to_string())
}

pub fn workspace_set_nav_visible(id: &str, visible: bool) -> Result<(), String> {
    let db = db::shared();
    let conn = db.lock();
    conn.execute(
        "UPDATE workspaces SET nav_visible = ?1 WHERE id = ?2",
        rusqlite::params![if visible { 1 } else { 0 }, id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

// ── Focus groups ───────────────────────────────────────────────────────

pub fn focus_groups_list() -> Result<Vec<FocusGroup>, String> {
    let db = db::shared();
    let conn = db.lock();
    FocusGroup::list(&conn).map_err(|e| e.to_string())
}

pub fn focus_groups_create(name: &str, color: Option<&str>) -> Result<FocusGroup, String> {
    let db = db::shared();
    let conn = db.lock();
    let id = Uuid::new_v4().to_string();

    let existing = FocusGroup::list(&conn).unwrap_or_default();
    let max_order = existing.iter().map(|g| g.tab_order).max().unwrap_or(-1) + 1;

    FocusGroup::create(&conn, &id, name, color, max_order).map_err(|e| e.to_string())?;
    FocusGroup::get(&conn, &id).map_err(|e| e.to_string())
}

pub fn focus_groups_update(
    id: &str,
    name: Option<&str>,
    color: Option<&str>,
    tab_order: Option<i64>,
) -> Result<FocusGroup, String> {
    let db = db::shared();
    let conn = db.lock();
    FocusGroup::update(&conn, id, name, color, tab_order).map_err(|e| e.to_string())?;
    FocusGroup::get(&conn, id).map_err(|e| e.to_string())
}

pub fn focus_groups_delete(id: &str) -> Result<(), String> {
    let db = db::shared();
    let conn = db.lock();
    FocusGroup::delete(&conn, id).map_err(|e| e.to_string())
}

pub fn focus_groups_assign_project(
    project_id: &str,
    focus_group_id: Option<&str>,
) -> Result<Project, String> {
    let db = db::shared();
    let conn = db.lock();

    Project::update(
        &conn,
        project_id,
        None, None, None, None, None, None,
        Some(focus_group_id),
        None, None, None, None, None, None, None, None,
    )
    .map_err(|e| e.to_string())?;

    // Write the focus group name to .k2so/config.json
    let project = Project::get(&conn, project_id).map_err(|e| e.to_string())?;
    let group_name = match focus_group_id {
        Some(gid) => FocusGroup::get(&conn, gid).ok().map(|g| g.name),
        None => None,
    };

    project_config::set_project_config_value(
        &project.path,
        "focusGroupName",
        group_name.as_deref(),
    )
    .ok();

    Project::get(&conn, project_id).map_err(|e| e.to_string())
}

pub fn focus_groups_reconcile_project(project_id: &str) -> Result<Project, String> {
    let db = db::shared();
    let conn = db.lock();
    let project = Project::get(&conn, project_id).map_err(|e| e.to_string())?;
    let config = project_config::get_project_config(&project.path);
    let config_group_name = config.focus_group_name;

    match config_group_name {
        None => {
            if project.focus_group_id.is_some() {
                Project::update(
                    &conn,
                    project_id,
                    None, None, None, None, None, None,
                    Some(None),
                    None, None, None, None, None, None, None, None,
                )
                .map_err(|e| e.to_string())?;
            }
        }
        Some(ref group_name) => {
            let groups = FocusGroup::list(&conn).map_err(|e| e.to_string())?;
            let existing = groups.iter().find(|g| &g.name == group_name);

            let group_id = if let Some(g) = existing {
                g.id.clone()
            } else {
                let new_id = Uuid::new_v4().to_string();
                let max_order = groups.iter().map(|g| g.tab_order).max().unwrap_or(-1) + 1;
                FocusGroup::create(&conn, &new_id, group_name, None, max_order)
                    .map_err(|e| e.to_string())?;
                new_id
            };

            if project.focus_group_id.as_deref() != Some(&group_id) {
                Project::update(
                    &conn,
                    project_id,
                    None, None, None, None, None, None,
                    Some(Some(group_id.as_str())),
                    None, None, None, None, None, None, None, None,
                )
                .map_err(|e| e.to_string())?;
            }
        }
    }

    Project::get(&conn, project_id).map_err(|e| e.to_string())
}

// ── Workspace sections ─────────────────────────────────────────────────

pub fn sections_list(project_id: &str) -> Result<Vec<WorkspaceSection>, String> {
    let db = db::shared();
    let conn = db.lock();
    WorkspaceSection::list(&conn, project_id).map_err(|e| e.to_string())
}

pub fn sections_create(
    project_id: &str,
    name: &str,
    color: Option<&str>,
) -> Result<WorkspaceSection, String> {
    let db = db::shared();
    let conn = db.lock();
    let id = Uuid::new_v4().to_string();
    let existing = WorkspaceSection::list(&conn, project_id).unwrap_or_default();
    let max_order = existing.iter().map(|s| s.tab_order).max().unwrap_or(-1) + 1;

    WorkspaceSection::create(&conn, &id, project_id, name, color, max_order)
        .map_err(|e| e.to_string())?;
    WorkspaceSection::get(&conn, &id).map_err(|e| e.to_string())
}

pub fn sections_update(
    id: &str,
    name: Option<&str>,
    color: Option<&str>,
    is_collapsed: Option<i64>,
    tab_order: Option<i64>,
) -> Result<WorkspaceSection, String> {
    let db = db::shared();
    let conn = db.lock();
    WorkspaceSection::update(&conn, id, name, color, is_collapsed, tab_order)
        .map_err(|e| e.to_string())?;
    WorkspaceSection::get(&conn, id).map_err(|e| e.to_string())
}

pub fn sections_delete(id: &str) -> Result<(), String> {
    let db = db::shared();
    let conn = db.lock();
    WorkspaceSection::delete(&conn, id).map_err(|e| e.to_string())
}

pub fn sections_reorder(ids: &[String]) -> Result<(), String> {
    let db = db::shared();
    let conn = db.lock();
    for (i, id) in ids.iter().enumerate() {
        WorkspaceSection::update(&conn, id, None, None, None, Some(i as i64))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub fn sections_assign_workspace(
    workspace_id: &str,
    section_id: Option<&str>,
) -> Result<Workspace, String> {
    let db = db::shared();
    let conn = db.lock();
    Workspace::update(
        &conn,
        workspace_id,
        Some(section_id),
        None,
        None,
        None,
        None,
        None,
    )
    .map_err(|e| e.to_string())?;
    Workspace::get(&conn, workspace_id).map_err(|e| e.to_string())
}

// ── Workspace layouts ─────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceLayout {
    pub project_id: String,
    pub workspace_id: String,
    pub layout_json: String,
}

pub fn workspace_layout_save(
    project_id: &str,
    workspace_id: &str,
    layout_json: &str,
) -> Result<(), String> {
    let db = db::shared();
    let conn = db.lock();
    let id = format!("{}:{}", project_id, workspace_id);

    conn.execute(
        "INSERT INTO workspace_layouts (id, project_id, workspace_id, layout_json, updated_at)
         VALUES (?1, ?2, ?3, ?4, unixepoch())
         ON CONFLICT(project_id, workspace_id)
         DO UPDATE SET layout_json = excluded.layout_json, updated_at = unixepoch()",
        rusqlite::params![id, project_id, workspace_id, layout_json],
    )
    .map_err(|e| e.to_string())?;

    Ok(())
}

pub fn workspace_layout_load(
    project_id: &str,
    workspace_id: &str,
) -> Result<Option<String>, String> {
    let db = db::shared();
    let conn = db.lock();
    let result = conn.query_row(
        "SELECT layout_json FROM workspace_layouts WHERE project_id = ?1 AND workspace_id = ?2",
        rusqlite::params![project_id, workspace_id],
        |row| row.get::<_, String>(0),
    );
    match result {
        Ok(json) => Ok(Some(json)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

pub fn workspace_layout_load_all() -> Result<Vec<WorkspaceLayout>, String> {
    let db = db::shared();
    let conn = db.lock();
    let mut stmt = conn
        .prepare("SELECT project_id, workspace_id, layout_json FROM workspace_layouts")
        .map_err(|e| e.to_string())?;
    let layouts = stmt
        .query_map([], |row| {
            Ok(WorkspaceLayout {
                project_id: row.get(0)?,
                workspace_id: row.get(1)?,
                layout_json: row.get(2)?,
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    Ok(layouts)
}

pub fn workspace_layout_delete(
    project_id: &str,
    workspace_id: Option<&str>,
) -> Result<(), String> {
    let db = db::shared();
    let conn = db.lock();
    if let Some(ws_id) = workspace_id {
        conn.execute(
            "DELETE FROM workspace_layouts WHERE project_id = ?1 AND workspace_id = ?2",
            rusqlite::params![project_id, ws_id],
        )
        .map_err(|e| e.to_string())?;
    } else {
        conn.execute(
            "DELETE FROM workspace_layouts WHERE project_id = ?1",
            rusqlite::params![project_id],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

// ── Time entries (timer) ───────────────────────────────────────────────

pub fn timer_entries_list(
    start: Option<i64>,
    end: Option<i64>,
    project_id: Option<&str>,
) -> Result<Vec<TimeEntry>, String> {
    let db = db::shared();
    let conn = db.lock();
    TimeEntry::list(&conn, start, end, project_id).map_err(|e| e.to_string())
}

pub fn timer_entry_create(
    id: &str,
    project_id: Option<&str>,
    start_time: i64,
    end_time: i64,
    duration_seconds: i64,
    memo: Option<&str>,
) -> Result<(), String> {
    let db = db::shared();
    let conn = db.lock();
    TimeEntry::create(
        &conn,
        id,
        project_id,
        start_time,
        end_time,
        duration_seconds,
        memo,
    )
    .map_err(|e| e.to_string())
}

pub fn timer_entry_delete(id: &str) -> Result<(), String> {
    let db = db::shared();
    let conn = db.lock();
    TimeEntry::delete(&conn, id).map_err(|e| e.to_string())
}

pub fn timer_entries_export(
    format: &str,
    start: Option<i64>,
    end: Option<i64>,
    project_id: Option<&str>,
) -> Result<String, String> {
    let entries = timer_entries_list(start, end, project_id)?;
    match format {
        "csv" => {
            let mut csv =
                String::from("id,project_id,start_time,end_time,duration_seconds,memo,created_at\n");
            for e in &entries {
                csv.push_str(&format!(
                    "{},{},{},{},{},{},{}\n",
                    e.id,
                    e.project_id.as_deref().unwrap_or(""),
                    e.start_time,
                    e.end_time,
                    e.duration_seconds,
                    csv_escape(e.memo.as_deref().unwrap_or("")),
                    e.created_at,
                ));
            }
            Ok(csv)
        }
        _ => serde_json::to_string_pretty(&entries).map_err(|e| e.to_string()),
    }
}

fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

// ── Agent presets ──────────────────────────────────────────────────────

pub fn presets_list() -> Result<Vec<AgentPreset>, String> {
    let db = db::shared();
    let conn = db.lock();
    AgentPreset::list(&conn).map_err(|e| e.to_string())
}

pub fn presets_create(
    label: &str,
    command: &str,
    icon: Option<&str>,
) -> Result<AgentPreset, String> {
    let db = db::shared();
    let conn = db.lock();
    let id = Uuid::new_v4().to_string();
    let existing = AgentPreset::list(&conn).unwrap_or_default();
    let max_order = existing.iter().map(|p| p.sort_order).max().unwrap_or(-1) + 1;

    AgentPreset::create(&conn, &id, label, command, icon, 1, max_order, 0)
        .map_err(|e| e.to_string())?;
    AgentPreset::get(&conn, &id).map_err(|e| e.to_string())
}

pub fn presets_update(
    id: &str,
    label: Option<&str>,
    command: Option<&str>,
    icon: Option<Option<&str>>,
    enabled: Option<i64>,
    sort_order: Option<i64>,
) -> Result<AgentPreset, String> {
    let db = db::shared();
    let conn = db.lock();
    AgentPreset::update(&conn, id, label, command, icon, enabled, sort_order)
        .map_err(|e| e.to_string())?;
    AgentPreset::get(&conn, id).map_err(|e| e.to_string())
}

pub fn presets_delete(id: &str) -> Result<(), String> {
    let db = db::shared();
    let conn = db.lock();
    let preset = AgentPreset::get(&conn, id).map_err(|e| e.to_string())?;
    if preset.is_built_in != 0 {
        return Err("Cannot delete built-in presets. Disable them instead.".to_string());
    }
    AgentPreset::delete(&conn, id).map_err(|e| e.to_string())
}

pub fn presets_reorder(ids: &[String]) -> Result<(), String> {
    let db = db::shared();
    let conn = db.lock();
    for (i, id) in ids.iter().enumerate() {
        AgentPreset::update(&conn, id, None, None, None, None, Some(i as i64))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub fn presets_reset_built_ins() -> Result<Vec<AgentPreset>, String> {
    let db = db::shared();
    let conn = db.lock();

    conn.execute("DELETE FROM agent_presets WHERE is_built_in = 1", [])
        .map_err(|e| e.to_string())?;

    for (id, label, command, icon, sort_order) in BUILT_IN_PRESETS {
        AgentPreset::create(&conn, id, label, command, Some(icon), 1, *sort_order, 1)
            .map_err(|e| e.to_string())?;
    }
    AgentPreset::list(&conn).map_err(|e| e.to_string())
}

// ── Window state ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowState {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub is_maximized: bool,
}

pub fn window_state_get() -> Result<Option<WindowState>, String> {
    let db = db::shared();
    let conn = db.lock();
    let r = conn.query_row(
        "SELECT x, y, width, height, is_maximized FROM window_state WHERE id = 1",
        [],
        |row| {
            Ok(WindowState {
                x: row.get(0)?,
                y: row.get(1)?,
                width: row.get(2)?,
                height: row.get(3)?,
                is_maximized: row.get::<_, i32>(4)? != 0,
            })
        },
    );
    match r {
        Ok(s) => Ok(Some(s)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

pub fn window_state_set(state: &WindowState, only_maximized_flag: bool) -> Result<(), String> {
    let db = db::shared();
    let conn = db.lock();
    if only_maximized_flag {
        conn.execute(
            "UPDATE window_state SET is_maximized = 1, updated_at = unixepoch() WHERE id = 1",
            [],
        )
        .map_err(|e| e.to_string())?;
        return Ok(());
    }
    conn.execute(
        "INSERT INTO window_state (id, x, y, width, height, is_maximized, updated_at)
         VALUES (1, ?1, ?2, ?3, ?4, ?5, unixepoch())
         ON CONFLICT(id) DO UPDATE SET
           x = excluded.x, y = excluded.y,
           width = excluded.width, height = excluded.height,
           is_maximized = excluded.is_maximized,
           updated_at = unixepoch()",
        rusqlite::params![
            state.x,
            state.y,
            state.width,
            state.height,
            state.is_maximized as i32
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

// ── Workspace layouts migration (one-shot, from lib.rs) ────────────────

/// Phase 2 Unit 4: relocated from `src-tauri/src/lib.rs`. Migrates
/// legacy `workspaceLayouts` map in `~/.k2so/settings.json` into the
/// `workspace_layouts` SQLite table. Idempotent — safe to call on
/// every boot. After moving, removes `workspaceLayouts` from
/// settings.json so the read side stops fighting the DB.
pub fn migrate_workspace_layouts_to_db() {
    let Some(home) = dirs::home_dir() else { return };
    let settings_path = home.join(".k2so").join("settings.json");
    if !settings_path.exists() {
        return;
    }
    let raw = match std::fs::read_to_string(&settings_path) {
        Ok(r) => r,
        Err(_) => return,
    };
    let mut parsed: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(_) => return,
    };
    let layouts = match parsed.get("workspaceLayouts") {
        Some(v) if v.is_object() && !v.as_object().unwrap().is_empty() => {
            v.as_object().unwrap().clone()
        }
        _ => return,
    };

    let db = db::shared();
    let conn = db.lock();

    let mut migrated = 0usize;
    for (key, layout_val) in &layouts {
        let parts: Vec<&str> = key.splitn(2, ':').collect();
        if parts.len() != 2 {
            continue;
        }
        let project_id = parts[0];
        let workspace_id = parts[1];
        let layout_json = match serde_json::to_string(layout_val) {
            Ok(j) => j,
            Err(_) => continue,
        };
        let id = key.clone();
        if conn
            .execute(
                "INSERT OR IGNORE INTO workspace_layouts (id, project_id, workspace_id, layout_json, updated_at) VALUES (?1, ?2, ?3, ?4, unixepoch())",
                rusqlite::params![id, project_id, workspace_id, layout_json],
            )
            .is_ok()
        {
            migrated += 1;
        }
    }
    drop(conn);

    if migrated > 0 {
        crate::log_debug!(
            "[daemon/migrations] migrated {migrated} workspace_layouts row(s) from settings.json"
        );
        if let Some(obj) = parsed.as_object_mut() {
            obj.remove("workspaceLayouts");
        }
        if let Ok(json) = serde_json::to_string_pretty(&parsed) {
            let tmp = settings_path.with_extension("json.tmp");
            if std::fs::write(&tmp, &json).is_ok() {
                let _ = std::fs::rename(&tmp, &settings_path);
            }
        }
    }
}

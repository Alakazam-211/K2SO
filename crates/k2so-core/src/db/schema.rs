use rusqlite::{params, Connection, Result};
use serde::{Deserialize, Serialize};

// ── Focus Groups ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FocusGroup {
    pub id: String,
    pub name: String,
    pub color: Option<String>,
    pub tab_order: i64,
    pub created_at: i64,
}

impl FocusGroup {
    pub fn list(conn: &Connection) -> Result<Vec<FocusGroup>> {
        let mut stmt = conn.prepare(
            "SELECT id, name, color, tab_order, created_at FROM focus_groups ORDER BY tab_order",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(FocusGroup {
                id: row.get(0)?,
                name: row.get(1)?,
                color: row.get(2)?,
                tab_order: row.get(3)?,
                created_at: row.get(4)?,
            })
        })?;
        rows.collect()
    }

    pub fn get(conn: &Connection, id: &str) -> Result<FocusGroup> {
        conn.query_row(
            "SELECT id, name, color, tab_order, created_at FROM focus_groups WHERE id = ?1",
            params![id],
            |row| {
                Ok(FocusGroup {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    color: row.get(2)?,
                    tab_order: row.get(3)?,
                    created_at: row.get(4)?,
                })
            },
        )
    }

    pub fn create(conn: &Connection, id: &str, name: &str, color: Option<&str>, tab_order: i64) -> Result<()> {
        conn.execute(
            "INSERT INTO focus_groups (id, name, color, tab_order) VALUES (?1, ?2, ?3, ?4)",
            params![id, name, color, tab_order],
        )?;
        Ok(())
    }

    pub fn update(conn: &Connection, id: &str, name: Option<&str>, color: Option<&str>, tab_order: Option<i64>) -> Result<()> {
        if let Some(n) = name {
            conn.execute("UPDATE focus_groups SET name = ?1 WHERE id = ?2", params![n, id])?;
        }
        if let Some(c) = color {
            conn.execute("UPDATE focus_groups SET color = ?1 WHERE id = ?2", params![c, id])?;
        }
        if let Some(t) = tab_order {
            conn.execute("UPDATE focus_groups SET tab_order = ?1 WHERE id = ?2", params![t, id])?;
        }
        Ok(())
    }

    pub fn delete(conn: &Connection, id: &str) -> Result<()> {
        conn.execute("DELETE FROM focus_groups WHERE id = ?1", params![id])?;
        Ok(())
    }
}

// ── Projects ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub name: String,
    pub path: String,
    pub color: String,
    pub tab_order: i64,
    pub last_opened_at: Option<i64>,
    pub worktree_mode: i64,
    pub icon_url: Option<String>,
    pub focus_group_id: Option<String>,
    pub pinned: i64,
    pub manually_active: i64,
    pub last_interaction_at: Option<i64>,
    pub created_at: i64,
    pub agent_enabled: i64,
    pub heartbeat_enabled: i64,
    pub agent_mode: String,
    pub state_id: Option<String>,
    pub heartbeat_mode: String,
    pub heartbeat_schedule: Option<String>,
    pub heartbeat_last_fire: Option<String>,
}

impl Project {
    pub fn list(conn: &Connection) -> Result<Vec<Project>> {
        let mut stmt = conn.prepare(
            "SELECT id, name, path, color, tab_order, last_opened_at, worktree_mode, icon_url, focus_group_id, pinned, manually_active, last_interaction_at, created_at, agent_enabled, heartbeat_enabled, agent_mode, tier_id, heartbeat_mode, heartbeat_schedule, heartbeat_last_fire \
             FROM projects ORDER BY tab_order",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Project {
                id: row.get(0)?,
                name: row.get(1)?,
                path: row.get(2)?,
                color: row.get(3)?,
                tab_order: row.get(4)?,
                last_opened_at: row.get(5)?,
                worktree_mode: row.get(6)?,
                icon_url: row.get(7)?,
                focus_group_id: row.get(8)?,
                pinned: row.get(9)?,
                manually_active: row.get(10)?,
                last_interaction_at: row.get(11)?,
                created_at: row.get(12)?,
                agent_enabled: row.get(13)?,
                heartbeat_enabled: row.get(14)?,
                agent_mode: row.get::<_, String>(15).unwrap_or_else(|_| "off".to_string()),
                state_id: row.get(16).ok(),
                heartbeat_mode: row.get::<_, String>(17).unwrap_or_else(|_| "off".to_string()),
                heartbeat_schedule: row.get(18).ok().flatten(),
                heartbeat_last_fire: row.get(19).ok().flatten(),
            })
        })?;
        rows.collect()
    }

    pub fn get(conn: &Connection, id: &str) -> Result<Project> {
        conn.query_row(
            "SELECT id, name, path, color, tab_order, last_opened_at, worktree_mode, icon_url, focus_group_id, pinned, manually_active, last_interaction_at, created_at, agent_enabled, heartbeat_enabled, agent_mode, tier_id, heartbeat_mode, heartbeat_schedule, heartbeat_last_fire \
             FROM projects WHERE id = ?1",
            params![id],
            |row| {
                Ok(Project {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    path: row.get(2)?,
                    color: row.get(3)?,
                    tab_order: row.get(4)?,
                    last_opened_at: row.get(5)?,
                    worktree_mode: row.get(6)?,
                    icon_url: row.get(7)?,
                    focus_group_id: row.get(8)?,
                    pinned: row.get(9)?,
                    manually_active: row.get(10)?,
                    last_interaction_at: row.get(11)?,
                    created_at: row.get(12)?,
                    agent_enabled: row.get(13)?,
                    heartbeat_enabled: row.get(14)?,
                    agent_mode: row.get::<_, String>(15).unwrap_or_else(|_| "off".to_string()),
                    state_id: row.get(16).ok(),
                    heartbeat_mode: row.get::<_, String>(17).unwrap_or_else(|_| "off".to_string()),
                    heartbeat_schedule: row.get(18).ok().flatten(),
                    heartbeat_last_fire: row.get(19).ok().flatten(),
                })
            },
        )
    }

    pub fn create(
        conn: &Connection,
        id: &str,
        name: &str,
        path: &str,
        color: &str,
        tab_order: i64,
        worktree_mode: i64,
        icon_url: Option<&str>,
        focus_group_id: Option<&str>,
    ) -> Result<()> {
        conn.execute(
            "INSERT INTO projects (id, name, path, color, tab_order, worktree_mode, icon_url, focus_group_id) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![id, name, path, color, tab_order, worktree_mode, icon_url, focus_group_id],
        )?;
        Ok(())
    }

    pub fn update(
        conn: &Connection,
        id: &str,
        name: Option<&str>,
        path: Option<&str>,
        color: Option<&str>,
        tab_order: Option<i64>,
        worktree_mode: Option<i64>,
        icon_url: Option<Option<&str>>,
        focus_group_id: Option<Option<&str>>,
        pinned: Option<i64>,
        manually_active: Option<i64>,
        agent_enabled: Option<i64>,
        heartbeat_enabled: Option<i64>,
        agent_mode: Option<String>,
        state_id: Option<Option<&str>>,
        heartbeat_mode: Option<String>,
        heartbeat_schedule: Option<Option<&str>>,
    ) -> Result<()> {
        // Wrap in transaction so all field updates succeed or fail atomically.
        // Without this, agent_mode and agent_enabled can diverge if the process crashes mid-update.
        let tx = conn.unchecked_transaction()?;
        if let Some(v) = name {
            tx.execute("UPDATE projects SET name = ?1 WHERE id = ?2", params![v, id])?;
        }
        if let Some(v) = path {
            tx.execute("UPDATE projects SET path = ?1 WHERE id = ?2", params![v, id])?;
        }
        if let Some(v) = color {
            tx.execute("UPDATE projects SET color = ?1 WHERE id = ?2", params![v, id])?;
        }
        if let Some(v) = tab_order {
            tx.execute("UPDATE projects SET tab_order = ?1 WHERE id = ?2", params![v, id])?;
        }
        if let Some(v) = worktree_mode {
            tx.execute("UPDATE projects SET worktree_mode = ?1 WHERE id = ?2", params![v, id])?;
        }
        if let Some(v) = icon_url {
            tx.execute("UPDATE projects SET icon_url = ?1 WHERE id = ?2", params![v, id])?;
        }
        if let Some(v) = focus_group_id {
            tx.execute("UPDATE projects SET focus_group_id = ?1 WHERE id = ?2", params![v, id])?;
        }
        if let Some(v) = pinned {
            tx.execute("UPDATE projects SET pinned = ?1 WHERE id = ?2", params![v, id])?;
        }
        if let Some(v) = manually_active {
            tx.execute("UPDATE projects SET manually_active = ?1 WHERE id = ?2", params![v, id])?;
        }
        if let Some(v) = agent_enabled {
            tx.execute("UPDATE projects SET agent_enabled = ?1 WHERE id = ?2", params![v, id])?;
        }
        if let Some(v) = heartbeat_enabled {
            tx.execute("UPDATE projects SET heartbeat_enabled = ?1 WHERE id = ?2", params![v, id])?;
        }
        if let Some(ref v) = agent_mode {
            tx.execute("UPDATE projects SET agent_mode = ?1 WHERE id = ?2", params![v, id])?;
            // Keep agent_enabled in sync for backward compat
            let enabled = if v == "off" { 0i64 } else { 1i64 };
            tx.execute("UPDATE projects SET agent_enabled = ?1 WHERE id = ?2", params![enabled, id])?;
        }
        if let Some(v) = state_id {
            match v {
                Some(sid) => tx.execute("UPDATE projects SET tier_id = ?1 WHERE id = ?2", params![sid, id])?,
                None => tx.execute("UPDATE projects SET tier_id = NULL WHERE id = ?1", params![id])?,
            };
        }
        if let Some(ref v) = heartbeat_mode {
            tx.execute("UPDATE projects SET heartbeat_mode = ?1 WHERE id = ?2", params![v, id])?;
            // Keep heartbeat_enabled in sync for backward compat
            let enabled = if v == "off" { 0i64 } else { 1i64 };
            tx.execute("UPDATE projects SET heartbeat_enabled = ?1 WHERE id = ?2", params![enabled, id])?;
        }
        if let Some(v) = heartbeat_schedule {
            tx.execute("UPDATE projects SET heartbeat_schedule = ?1 WHERE id = ?2", params![v, id])?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn delete(conn: &Connection, id: &str) -> Result<()> {
        conn.execute("DELETE FROM projects WHERE id = ?1", params![id])?;
        Ok(())
    }

    #[allow(dead_code)] // API surface — covered by tests, not yet wired from UI
    pub fn update_last_opened(conn: &Connection, id: &str) -> Result<()> {
        conn.execute(
            "UPDATE projects SET last_opened_at = unixepoch() WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }

    pub fn touch_interaction(conn: &Connection, id: &str) -> Result<()> {
        conn.execute(
            "UPDATE projects SET last_interaction_at = unixepoch() WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }

    pub fn clear_interaction(conn: &Connection, id: &str) -> Result<()> {
        conn.execute(
            "UPDATE projects SET last_interaction_at = NULL WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }
}

// ── Workspace Sections ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSection {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub color: Option<String>,
    pub is_collapsed: i64,
    pub tab_order: i64,
    pub created_at: i64,
}

impl WorkspaceSection {
    pub fn list(conn: &Connection, project_id: &str) -> Result<Vec<WorkspaceSection>> {
        let mut stmt = conn.prepare(
            "SELECT id, project_id, name, color, is_collapsed, tab_order, created_at \
             FROM workspace_sections WHERE project_id = ?1 ORDER BY tab_order",
        )?;
        let rows = stmt.query_map(params![project_id], |row| {
            Ok(WorkspaceSection {
                id: row.get(0)?,
                project_id: row.get(1)?,
                name: row.get(2)?,
                color: row.get(3)?,
                is_collapsed: row.get(4)?,
                tab_order: row.get(5)?,
                created_at: row.get(6)?,
            })
        })?;
        rows.collect()
    }

    pub fn get(conn: &Connection, id: &str) -> Result<WorkspaceSection> {
        conn.query_row(
            "SELECT id, project_id, name, color, is_collapsed, tab_order, created_at \
             FROM workspace_sections WHERE id = ?1",
            params![id],
            |row| {
                Ok(WorkspaceSection {
                    id: row.get(0)?,
                    project_id: row.get(1)?,
                    name: row.get(2)?,
                    color: row.get(3)?,
                    is_collapsed: row.get(4)?,
                    tab_order: row.get(5)?,
                    created_at: row.get(6)?,
                })
            },
        )
    }

    pub fn create(
        conn: &Connection,
        id: &str,
        project_id: &str,
        name: &str,
        color: Option<&str>,
        tab_order: i64,
    ) -> Result<()> {
        conn.execute(
            "INSERT INTO workspace_sections (id, project_id, name, color, tab_order) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, project_id, name, color, tab_order],
        )?;
        Ok(())
    }

    pub fn update(
        conn: &Connection,
        id: &str,
        name: Option<&str>,
        color: Option<&str>,
        is_collapsed: Option<i64>,
        tab_order: Option<i64>,
    ) -> Result<()> {
        if let Some(v) = name {
            conn.execute("UPDATE workspace_sections SET name = ?1 WHERE id = ?2", params![v, id])?;
        }
        if let Some(v) = color {
            conn.execute("UPDATE workspace_sections SET color = ?1 WHERE id = ?2", params![v, id])?;
        }
        if let Some(v) = is_collapsed {
            conn.execute("UPDATE workspace_sections SET is_collapsed = ?1 WHERE id = ?2", params![v, id])?;
        }
        if let Some(v) = tab_order {
            conn.execute("UPDATE workspace_sections SET tab_order = ?1 WHERE id = ?2", params![v, id])?;
        }
        Ok(())
    }

    pub fn delete(conn: &Connection, id: &str) -> Result<()> {
        conn.execute("DELETE FROM workspace_sections WHERE id = ?1", params![id])?;
        Ok(())
    }
}

// ── Workspaces ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    pub id: String,
    pub project_id: String,
    pub section_id: Option<String>,
    #[serde(rename = "type")]
    pub type_: String,
    pub branch: Option<String>,
    pub name: String,
    pub tab_order: i64,
    pub worktree_path: Option<String>,
    pub nav_visible: i64,
    pub created_at: i64,
}

impl Workspace {
    pub fn list(conn: &Connection, project_id: &str) -> Result<Vec<Workspace>> {
        let mut stmt = conn.prepare(
            "SELECT id, project_id, section_id, type, branch, name, tab_order, worktree_path, nav_visible, created_at \
             FROM workspaces WHERE project_id = ?1 ORDER BY tab_order",
        )?;
        let rows = stmt.query_map(params![project_id], |row| {
            Ok(Workspace {
                id: row.get(0)?,
                project_id: row.get(1)?,
                section_id: row.get(2)?,
                type_: row.get(3)?,
                branch: row.get(4)?,
                name: row.get(5)?,
                tab_order: row.get(6)?,
                worktree_path: row.get(7)?,
                nav_visible: row.get(8)?,
                created_at: row.get(9)?,
            })
        })?;
        rows.collect()
    }

    pub fn get(conn: &Connection, id: &str) -> Result<Workspace> {
        conn.query_row(
            "SELECT id, project_id, section_id, type, branch, name, tab_order, worktree_path, nav_visible, created_at \
             FROM workspaces WHERE id = ?1",
            params![id],
            |row| {
                Ok(Workspace {
                    id: row.get(0)?,
                    project_id: row.get(1)?,
                    section_id: row.get(2)?,
                    type_: row.get(3)?,
                    branch: row.get(4)?,
                    name: row.get(5)?,
                    tab_order: row.get(6)?,
                    worktree_path: row.get(7)?,
                    nav_visible: row.get(8)?,
                    created_at: row.get(9)?,
                })
            },
        )
    }

    pub fn create(
        conn: &Connection,
        id: &str,
        project_id: &str,
        section_id: Option<&str>,
        type_: &str,
        branch: Option<&str>,
        name: &str,
        tab_order: i64,
        worktree_path: Option<&str>,
    ) -> Result<()> {
        conn.execute(
            "INSERT INTO workspaces (id, project_id, section_id, type, branch, name, tab_order, worktree_path) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![id, project_id, section_id, type_, branch, name, tab_order, worktree_path],
        )?;
        Ok(())
    }

    pub fn update(
        conn: &Connection,
        id: &str,
        section_id: Option<Option<&str>>,
        type_: Option<&str>,
        branch: Option<Option<&str>>,
        name: Option<&str>,
        tab_order: Option<i64>,
        worktree_path: Option<Option<&str>>,
    ) -> Result<()> {
        if let Some(v) = section_id {
            conn.execute("UPDATE workspaces SET section_id = ?1 WHERE id = ?2", params![v, id])?;
        }
        if let Some(v) = type_ {
            conn.execute("UPDATE workspaces SET type = ?1 WHERE id = ?2", params![v, id])?;
        }
        if let Some(v) = branch {
            conn.execute("UPDATE workspaces SET branch = ?1 WHERE id = ?2", params![v, id])?;
        }
        if let Some(v) = name {
            conn.execute("UPDATE workspaces SET name = ?1 WHERE id = ?2", params![v, id])?;
        }
        if let Some(v) = tab_order {
            conn.execute("UPDATE workspaces SET tab_order = ?1 WHERE id = ?2", params![v, id])?;
        }
        if let Some(v) = worktree_path {
            conn.execute("UPDATE workspaces SET worktree_path = ?1 WHERE id = ?2", params![v, id])?;
        }
        Ok(())
    }

    pub fn delete(conn: &Connection, id: &str) -> Result<()> {
        conn.execute("DELETE FROM workspaces WHERE id = ?1", params![id])?;
        Ok(())
    }
}

// ── Agent Presets ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentPreset {
    pub id: String,
    pub label: String,
    pub command: String,
    pub icon: Option<String>,
    pub enabled: i64,
    pub sort_order: i64,
    pub is_built_in: i64,
    pub created_at: i64,
}

impl AgentPreset {
    pub fn list(conn: &Connection) -> Result<Vec<AgentPreset>> {
        let mut stmt = conn.prepare(
            "SELECT id, label, command, icon, enabled, sort_order, is_built_in, created_at \
             FROM agent_presets ORDER BY sort_order",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(AgentPreset {
                id: row.get(0)?,
                label: row.get(1)?,
                command: row.get(2)?,
                icon: row.get(3)?,
                enabled: row.get(4)?,
                sort_order: row.get(5)?,
                is_built_in: row.get(6)?,
                created_at: row.get(7)?,
            })
        })?;
        rows.collect()
    }

    pub fn get(conn: &Connection, id: &str) -> Result<AgentPreset> {
        conn.query_row(
            "SELECT id, label, command, icon, enabled, sort_order, is_built_in, created_at \
             FROM agent_presets WHERE id = ?1",
            params![id],
            |row| {
                Ok(AgentPreset {
                    id: row.get(0)?,
                    label: row.get(1)?,
                    command: row.get(2)?,
                    icon: row.get(3)?,
                    enabled: row.get(4)?,
                    sort_order: row.get(5)?,
                    is_built_in: row.get(6)?,
                    created_at: row.get(7)?,
                })
            },
        )
    }

    pub fn create(
        conn: &Connection,
        id: &str,
        label: &str,
        command: &str,
        icon: Option<&str>,
        enabled: i64,
        sort_order: i64,
        is_built_in: i64,
    ) -> Result<()> {
        conn.execute(
            "INSERT INTO agent_presets (id, label, command, icon, enabled, sort_order, is_built_in) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![id, label, command, icon, enabled, sort_order, is_built_in],
        )?;
        Ok(())
    }

    pub fn update(
        conn: &Connection,
        id: &str,
        label: Option<&str>,
        command: Option<&str>,
        icon: Option<Option<&str>>,
        enabled: Option<i64>,
        sort_order: Option<i64>,
    ) -> Result<()> {
        if let Some(v) = label {
            conn.execute("UPDATE agent_presets SET label = ?1 WHERE id = ?2", params![v, id])?;
        }
        if let Some(v) = command {
            conn.execute("UPDATE agent_presets SET command = ?1 WHERE id = ?2", params![v, id])?;
        }
        if let Some(v) = icon {
            conn.execute("UPDATE agent_presets SET icon = ?1 WHERE id = ?2", params![v, id])?;
        }
        if let Some(v) = enabled {
            conn.execute("UPDATE agent_presets SET enabled = ?1 WHERE id = ?2", params![v, id])?;
        }
        if let Some(v) = sort_order {
            conn.execute("UPDATE agent_presets SET sort_order = ?1 WHERE id = ?2", params![v, id])?;
        }
        Ok(())
    }

    pub fn delete(conn: &Connection, id: &str) -> Result<()> {
        conn.execute("DELETE FROM agent_presets WHERE id = ?1", params![id])?;
        Ok(())
    }
}

// ── Time Entries ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimeEntry {
    pub id: String,
    pub project_id: Option<String>,
    pub start_time: i64,
    pub end_time: i64,
    pub duration_seconds: i64,
    pub memo: Option<String>,
    pub created_at: i64,
}

impl TimeEntry {
    pub fn list(
        conn: &Connection,
        start: Option<i64>,
        end: Option<i64>,
        project_id: Option<&str>,
    ) -> Result<Vec<TimeEntry>> {
        let mut sql = String::from(
            "SELECT id, project_id, start_time, end_time, duration_seconds, memo, created_at \
             FROM time_entries WHERE 1=1",
        );
        let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        let mut idx = 1;

        if let Some(s) = start {
            sql.push_str(&format!(" AND start_time >= ?{}", idx));
            param_values.push(Box::new(s));
            idx += 1;
        }
        if let Some(e) = end {
            sql.push_str(&format!(" AND start_time <= ?{}", idx));
            param_values.push(Box::new(e));
            idx += 1;
        }
        if let Some(pid) = project_id {
            sql.push_str(&format!(" AND project_id = ?{}", idx));
            param_values.push(Box::new(pid.to_string()));
        }
        sql.push_str(" ORDER BY start_time DESC");

        let mut stmt = conn.prepare(&sql)?;
        let params_ref: Vec<&dyn rusqlite::types::ToSql> = param_values.iter().map(|p| p.as_ref()).collect();
        let rows = stmt.query_map(params_ref.as_slice(), |row| {
            Ok(TimeEntry {
                id: row.get(0)?,
                project_id: row.get(1)?,
                start_time: row.get(2)?,
                end_time: row.get(3)?,
                duration_seconds: row.get(4)?,
                memo: row.get(5)?,
                created_at: row.get(6)?,
            })
        })?;
        rows.collect()
    }

    pub fn create(
        conn: &Connection,
        id: &str,
        project_id: Option<&str>,
        start_time: i64,
        end_time: i64,
        duration_seconds: i64,
        memo: Option<&str>,
    ) -> Result<()> {
        conn.execute(
            "INSERT INTO time_entries (id, project_id, start_time, end_time, duration_seconds, memo) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, project_id, start_time, end_time, duration_seconds, memo],
        )?;
        Ok(())
    }

    pub fn delete(conn: &Connection, id: &str) -> Result<()> {
        conn.execute("DELETE FROM time_entries WHERE id = ?1", params![id])?;
        Ok(())
    }
}

// ── Terminal Tabs (stub) ────────────────────────────────────────────────────

#[allow(dead_code)] // Scaffold for the persisted-terminal-tabs feature —
                    // schema shape is agreed but CRUD hasn't shipped yet.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalTab {
    pub id: String,
    pub workspace_id: String,
    pub title: String,
    pub tab_order: i64,
    pub created_at: i64,
}

// ── Terminal Panes (stub) ───────────────────────────────────────────────────

#[allow(dead_code)] // Scaffold for persisted pane splits — same deal as TerminalTab.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalPane {
    pub id: String,
    pub tab_id: String,
    pub split_direction: Option<String>,
    pub split_ratio: Option<f64>,
    pub pane_order: i64,
    pub created_at: i64,
}

// ── Workspace States ─────────────────────────────────────────────────────

/// A workspace state defines what agents are allowed to do automatically.
/// Each capability has three levels: "auto", "gated", "off".
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceState {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub is_built_in: i64,
    /// Features: new functionality, enhancements
    pub cap_features: String,
    /// Issues: bug fixes from submitted issues
    pub cap_issues: String,
    /// Crashes: automatic crash report fixes
    pub cap_crashes: String,
    /// Security: automatic security patches
    pub cap_security: String,
    /// Audits: scheduled code reviews
    pub cap_audits: String,
    /// Whether the heartbeat scheduler is active
    pub heartbeat: i64,
    pub sort_order: i64,
}

impl WorkspaceState {
    pub fn list(conn: &Connection) -> Result<Vec<WorkspaceState>> {
        let mut stmt = conn.prepare(
            "SELECT id, name, description, is_built_in, cap_features, cap_issues, cap_crashes, cap_security, cap_audits, heartbeat, sort_order \
             FROM workspace_states ORDER BY sort_order"
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(WorkspaceState {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                is_built_in: row.get(3)?,
                cap_features: row.get(4)?,
                cap_issues: row.get(5)?,
                cap_crashes: row.get(6)?,
                cap_security: row.get(7)?,
                cap_audits: row.get(8)?,
                heartbeat: row.get(9)?,
                sort_order: row.get(10)?,
            })
        })?;
        rows.collect()
    }

    pub fn get(conn: &Connection, id: &str) -> Result<WorkspaceState> {
        conn.query_row(
            "SELECT id, name, description, is_built_in, cap_features, cap_issues, cap_crashes, cap_security, cap_audits, heartbeat, sort_order \
             FROM workspace_states WHERE id = ?1",
            params![id],
            |row| {
                Ok(WorkspaceState {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    description: row.get(2)?,
                    is_built_in: row.get(3)?,
                    cap_features: row.get(4)?,
                    cap_issues: row.get(5)?,
                    cap_crashes: row.get(6)?,
                    cap_security: row.get(7)?,
                    cap_audits: row.get(8)?,
                    heartbeat: row.get(9)?,
                    sort_order: row.get(10)?,
                })
            },
        )
    }

    pub fn create(
        conn: &Connection,
        id: &str,
        name: &str,
        description: Option<&str>,
        cap_features: &str,
        cap_issues: &str,
        cap_crashes: &str,
        cap_security: &str,
        cap_audits: &str,
        heartbeat: bool,
    ) -> Result<()> {
        // Wrap in transaction to prevent race condition on sort_order
        // (Zed pattern: savepoint-wrapped mutations for atomicity)
        let tx = conn.unchecked_transaction()?;
        let max_order: i64 = tx.query_row(
            "SELECT COALESCE(MAX(sort_order), -1) + 1 FROM workspace_states", [], |r| r.get(0)
        )?;
        tx.execute(
            "INSERT INTO workspace_states (id, name, description, is_built_in, cap_features, cap_issues, cap_crashes, cap_security, cap_audits, heartbeat, sort_order) \
             VALUES (?1, ?2, ?3, 0, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![id, name, description, cap_features, cap_issues, cap_crashes, cap_security, cap_audits, heartbeat as i64, max_order],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn update(
        conn: &Connection,
        id: &str,
        name: Option<&str>,
        description: Option<&str>,
        cap_features: Option<&str>,
        cap_issues: Option<&str>,
        cap_crashes: Option<&str>,
        cap_security: Option<&str>,
        cap_audits: Option<&str>,
        heartbeat: Option<bool>,
    ) -> Result<()> {
        // Wrap in transaction so all updates succeed or fail together
        // (Zed pattern: atomic multi-field updates prevent partial state)
        let tx = conn.unchecked_transaction()?;
        if let Some(v) = name { tx.execute("UPDATE workspace_states SET name = ?1 WHERE id = ?2", params![v, id])?; }
        if let Some(v) = description { tx.execute("UPDATE workspace_states SET description = ?1 WHERE id = ?2", params![v, id])?; }
        if let Some(v) = cap_features { tx.execute("UPDATE workspace_states SET cap_features = ?1 WHERE id = ?2", params![v, id])?; }
        if let Some(v) = cap_issues { tx.execute("UPDATE workspace_states SET cap_issues = ?1 WHERE id = ?2", params![v, id])?; }
        if let Some(v) = cap_crashes { tx.execute("UPDATE workspace_states SET cap_crashes = ?1 WHERE id = ?2", params![v, id])?; }
        if let Some(v) = cap_security { tx.execute("UPDATE workspace_states SET cap_security = ?1 WHERE id = ?2", params![v, id])?; }
        if let Some(v) = cap_audits { tx.execute("UPDATE workspace_states SET cap_audits = ?1 WHERE id = ?2", params![v, id])?; }
        if let Some(v) = heartbeat { tx.execute("UPDATE workspace_states SET heartbeat = ?1 WHERE id = ?2", params![v as i64, id])?; }
        tx.commit()?;
        Ok(())
    }

    pub fn delete(conn: &Connection, id: &str) -> Result<()> {
        // Don't delete built-in states — explicit check instead of unwrap_or(1) which
        // silently treats "not found" as "built-in"
        let is_built_in = conn.query_row(
            "SELECT is_built_in FROM workspace_states WHERE id = ?1", params![id], |r| r.get::<_, i64>(0)
        );
        match is_built_in {
            Ok(1) => return Ok(()), // Built-in — don't delete
            Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(()), // Not found — nothing to delete
            Err(e) => return Err(e),
            Ok(_) => {} // Custom state — proceed with delete
        }
        // Wrap cascade + delete in transaction for atomicity
        let tx = conn.unchecked_transaction()?;
        // Clear tier_id on projects using this state
        tx.execute("UPDATE projects SET tier_id = NULL WHERE tier_id = ?1", params![id])?;
        tx.execute("DELETE FROM workspace_states WHERE id = ?1", params![id])?;
        tx.commit()?;
        Ok(())
    }

    /// Get the capability state for a given work item source type.
    /// Returns "auto", "gated", or "off".
    pub fn capability_for_source(&self, source: &str) -> &str {
        match source {
            "feature" => &self.cap_features,
            "issue" => &self.cap_issues,
            "crash" => &self.cap_crashes,
            "security" => &self.cap_security,
            "audit" => &self.cap_audits,
            _ => "gated", // Unknown source → require approval
        }
    }
}

// ── Agent Sessions ──────────────────────────────────────────────────────

/// DB-tracked agent session. Single source of truth — the legacy
/// `.lock` and `.last_session` filesystem tracking was retired.
/// `owner` distinguishes system-managed sessions (safe to inject) from user
/// interactive sessions (never inject).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSession {
    pub id: String,
    pub project_id: String,
    pub agent_name: String,
    pub terminal_id: Option<String>,
    pub session_id: Option<String>,
    pub harness: String,
    pub owner: String,
    pub status: String,
    pub status_message: Option<String>,
    pub last_activity_at: Option<i64>,
    pub created_at: i64,
}

impl AgentSession {
    /// Insert or replace session keyed on (project_id, agent_name).
    pub fn upsert(
        conn: &Connection,
        id: &str,
        project_id: &str,
        agent_name: &str,
        terminal_id: Option<&str>,
        session_id: Option<&str>,
        harness: &str,
        owner: &str,
        status: &str,
    ) -> Result<()> {
        conn.execute(
            "INSERT INTO agent_sessions (id, project_id, agent_name, terminal_id, session_id, harness, owner, status, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, unixepoch()) \
             ON CONFLICT(project_id, agent_name) DO UPDATE SET \
               terminal_id = ?4, session_id = COALESCE(?5, agent_sessions.session_id), \
               harness = ?6, owner = ?7, status = ?8, last_activity_at = unixepoch()",
            params![id, project_id, agent_name, terminal_id, session_id, harness, owner, status],
        )?;
        Ok(())
    }

    /// Find the session row whose `terminal_id` matches — used by the
    /// hook handler to resolve which AgentSession row a fired event
    /// belongs to without the caller needing to know project/agent.
    pub fn get_by_terminal_id(conn: &Connection, terminal_id: &str) -> Result<Option<AgentSession>> {
        let mut stmt = conn.prepare(
            "SELECT id, project_id, agent_name, terminal_id, session_id, harness, owner, status, status_message, last_activity_at, created_at \
             FROM agent_sessions WHERE terminal_id = ?1 LIMIT 1"
        )?;
        let mut rows = stmt.query_map(params![terminal_id], |row| {
            Ok(AgentSession {
                id: row.get(0)?,
                project_id: row.get(1)?,
                agent_name: row.get(2)?,
                terminal_id: row.get(3)?,
                session_id: row.get(4)?,
                harness: row.get(5)?,
                owner: row.get(6)?,
                status: row.get(7)?,
                status_message: row.get(8)?,
                last_activity_at: row.get(9)?,
                created_at: row.get(10)?,
            })
        })?;
        match rows.next() {
            Some(Ok(s)) => Ok(Some(s)),
            Some(Err(e)) => Err(e),
            None => Ok(None),
        }
    }

    pub fn get_by_agent(conn: &Connection, project_id: &str, agent_name: &str) -> Result<Option<AgentSession>> {
        let mut stmt = conn.prepare(
            "SELECT id, project_id, agent_name, terminal_id, session_id, harness, owner, status, status_message, last_activity_at, created_at \
             FROM agent_sessions WHERE project_id = ?1 AND agent_name = ?2"
        )?;
        let mut rows = stmt.query_map(params![project_id, agent_name], |row| {
            Ok(AgentSession {
                id: row.get(0)?,
                project_id: row.get(1)?,
                agent_name: row.get(2)?,
                terminal_id: row.get(3)?,
                session_id: row.get(4)?,
                harness: row.get(5)?,
                owner: row.get(6)?,
                status: row.get(7)?,
                status_message: row.get(8)?,
                last_activity_at: row.get(9)?,
                created_at: row.get(10)?,
            })
        })?;
        match rows.next() {
            Some(Ok(s)) => Ok(Some(s)),
            Some(Err(e)) => Err(e),
            None => Ok(None),
        }
    }

    pub fn list_by_project(conn: &Connection, project_id: &str) -> Result<Vec<AgentSession>> {
        let mut stmt = conn.prepare(
            "SELECT id, project_id, agent_name, terminal_id, session_id, harness, owner, status, status_message, last_activity_at, created_at \
             FROM agent_sessions WHERE project_id = ?1 ORDER BY agent_name"
        )?;
        let rows = stmt.query_map(params![project_id], |row| {
            Ok(AgentSession {
                id: row.get(0)?,
                project_id: row.get(1)?,
                agent_name: row.get(2)?,
                terminal_id: row.get(3)?,
                session_id: row.get(4)?,
                harness: row.get(5)?,
                owner: row.get(6)?,
                status: row.get(7)?,
                status_message: row.get(8)?,
                last_activity_at: row.get(9)?,
                created_at: row.get(10)?,
            })
        })?;
        rows.collect()
    }

    pub fn update_status(conn: &Connection, project_id: &str, agent_name: &str, status: &str) -> Result<usize> {
        // Fires on every agent state transition — cached.
        let mut stmt = conn.prepare_cached(
            "UPDATE agent_sessions SET status = ?1, last_activity_at = unixepoch() WHERE project_id = ?2 AND agent_name = ?3",
        )?;
        stmt.execute(params![status, project_id, agent_name])
    }

    /// Try to atomically acquire the "running" lock for an agent. Returns
    /// `Ok(true)` if this call was the one that took the lock (and thus
    /// the caller should proceed to spawn the PTY). Returns `Ok(false)`
    /// if the agent was already running (caller must NOT spawn — the
    /// existing session owns the terminal).
    ///
    /// This replaces the pre-0.32.9 `is_agent_locked() → spawn → upsert`
    /// sequence, which had a TOCTOU race: two heartbeats firing at
    /// roughly the same time could both observe `is_locked=false` and
    /// both spawn, producing duplicate PTYs and a stale row.
    ///
    /// Implementation: BEGIN IMMEDIATE takes the database write lock
    /// before any reads, so concurrent callers serialize. Inside the
    /// transaction we check for an existing running session; if present
    /// we rollback (no change) and return false. If not, we INSERT/
    /// UPDATE the session row with status='running' and COMMIT.
    // TODO(resilience-followup): 0.32.9 introduced this CAS helper
    // but the production spawn path in `commands/k2so_agents.rs` still
    // uses the pre-CAS `is_agent_locked → spawn → upsert` sequence.
    // Wire this in before advertising the TOCTOU fix as live.
    #[allow(dead_code)]
    pub fn try_acquire_running(
        conn: &Connection,
        session_id: &str,
        project_id: &str,
        agent_name: &str,
        terminal_id: Option<&str>,
        harness: &str,
        owner: &str,
    ) -> Result<bool> {
        // IMMEDIATE upgrades the connection to a write lock at BEGIN
        // time rather than deferring to the first write — this prevents
        // two concurrent readers from both thinking they can proceed.
        conn.execute_batch("BEGIN IMMEDIATE;")?;

        // Check existing status.
        let existing: Option<String> = conn.query_row(
            "SELECT status FROM agent_sessions WHERE project_id = ?1 AND agent_name = ?2",
            params![project_id, agent_name],
            |row| row.get::<_, String>(0),
        ).ok();

        if matches!(existing.as_deref(), Some("running")) {
            conn.execute_batch("ROLLBACK;")?;
            return Ok(false);
        }

        // Acquire: upsert with status='running'. Mirrors Self::upsert's
        // schema so downstream reads see the same shape.
        let result = conn.execute(
            "INSERT INTO agent_sessions (id, project_id, agent_name, terminal_id, session_id, harness, owner, status, created_at) \
             VALUES (?1, ?2, ?3, ?4, NULL, ?5, ?6, 'running', unixepoch()) \
             ON CONFLICT(project_id, agent_name) DO UPDATE SET \
               terminal_id = excluded.terminal_id, \
               harness = excluded.harness, \
               owner = excluded.owner, \
               status = 'running', \
               last_activity_at = unixepoch()",
            params![session_id, project_id, agent_name, terminal_id, harness, owner],
        );

        match result {
            Ok(_) => {
                conn.execute_batch("COMMIT;")?;
                Ok(true)
            }
            Err(e) => {
                let _ = conn.execute_batch("ROLLBACK;");
                Err(e)
            }
        }
    }

    pub fn update_status_message(conn: &Connection, project_id: &str, agent_name: &str, message: &str) -> Result<usize> {
        conn.execute(
            "UPDATE agent_sessions SET status_message = ?1, last_activity_at = unixepoch() WHERE project_id = ?2 AND agent_name = ?3",
            params![message, project_id, agent_name],
        )
    }

    pub fn update_session_id(conn: &Connection, project_id: &str, agent_name: &str, session_id: &str) -> Result<usize> {
        conn.execute(
            "UPDATE agent_sessions SET session_id = ?1, last_activity_at = unixepoch() WHERE project_id = ?2 AND agent_name = ?3",
            params![session_id, project_id, agent_name],
        )
    }

    pub fn clear_session_id(conn: &Connection, project_id: &str, agent_name: &str) -> Result<usize> {
        conn.execute(
            "UPDATE agent_sessions SET session_id = NULL WHERE project_id = ?1 AND agent_name = ?2",
            params![project_id, agent_name],
        )
    }

    pub fn delete(conn: &Connection, project_id: &str, agent_name: &str) -> Result<usize> {
        conn.execute(
            "DELETE FROM agent_sessions WHERE project_id = ?1 AND agent_name = ?2",
            params![project_id, agent_name],
        )
    }

    /// Atomically increment the "wakes since last /compact" counter and
    /// return the new value. Used by the heartbeat wake path to decide
    /// whether to prepend `/compact` to the wake message every N wakes.
    ///
    /// Returns 1 on first wake after upsert, increments from there. Row
    /// is auto-created with count=0 → increment → returns 1 if missing.
    pub fn bump_wake_counter(conn: &Connection, project_id: &str, agent_name: &str) -> Result<i64> {
        conn.execute(
            "UPDATE agent_sessions SET wakes_since_compact = wakes_since_compact + 1 \
             WHERE project_id = ?1 AND agent_name = ?2",
            params![project_id, agent_name],
        )?;
        let val: i64 = conn.query_row(
            "SELECT wakes_since_compact FROM agent_sessions WHERE project_id = ?1 AND agent_name = ?2",
            params![project_id, agent_name],
            |row| row.get(0),
        ).unwrap_or(0);
        Ok(val)
    }

    pub fn reset_wake_counter(conn: &Connection, project_id: &str, agent_name: &str) -> Result<usize> {
        conn.execute(
            "UPDATE agent_sessions SET wakes_since_compact = 0 WHERE project_id = ?1 AND agent_name = ?2",
            params![project_id, agent_name],
        )
    }
}

// ── Agent Heartbeats (multi-heartbeat architecture) ────────────────────
//
// Replaces the legacy single-slot projects.heartbeat_schedule. Each row
// is one named heartbeat with its own frequency + wakeup path. Scheduler
// loop iterates enabled rows per workspace, evaluates fire eligibility,
// spawns using the row's wakeup_path. See
// .k2so/prds/multi-schedule-heartbeat.md for full design.

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentHeartbeat {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub frequency: String,
    pub spec_json: String,
    pub wakeup_path: String,
    pub enabled: bool,
    pub last_fired: Option<String>,
    /// Claude session id from the most recent successful spawn for this
    /// heartbeat. The next fire passes this to `--resume` so the
    /// heartbeat keeps its own dedicated chat thread instead of
    /// reusing the agent's global session.
    pub last_session_id: Option<String>,
    /// RFC3339 timestamp set when the user archives the heartbeat from
    /// Settings. NULL = active. Archived rows are hidden from the
    /// Settings list but appear in the sidebar's collapsed Archived
    /// section so chat history stays auditable.
    pub archived_at: Option<String>,
    pub created_at: i64,
}

impl AgentHeartbeat {
    /// Validate a heartbeat name. Enforced at every insert/write path so
    /// users can't get into a weird state. See PRD § Name validation.
    pub fn validate_name(name: &str) -> Result<()> {
        if name.is_empty() {
            return Err(rusqlite::Error::InvalidParameterName(
                "heartbeat name cannot be empty".into(),
            ));
        }
        let reserved = ["default", "legacy"];
        if reserved.contains(&name) {
            return Err(rusqlite::Error::InvalidParameterName(format!(
                "heartbeat name '{}' is reserved",
                name
            )));
        }
        if !name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
            return Err(rusqlite::Error::InvalidParameterName(
                "heartbeat name must be lowercase letters, digits, and hyphens only".into(),
            ));
        }
        if name.starts_with('-') || name.ends_with('-') {
            return Err(rusqlite::Error::InvalidParameterName(
                "heartbeat name cannot start or end with a hyphen".into(),
            ));
        }
        Ok(())
    }

    pub fn insert(
        conn: &Connection,
        id: &str,
        project_id: &str,
        name: &str,
        frequency: &str,
        spec_json: &str,
        wakeup_path: &str,
        enabled: bool,
    ) -> Result<()> {
        conn.execute(
            "INSERT INTO agent_heartbeats \
             (id, project_id, name, frequency, spec_json, wakeup_path, enabled, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, unixepoch())",
            params![id, project_id, name, frequency, spec_json, wakeup_path, enabled as i64],
        )?;
        Ok(())
    }

    /// Column list for SELECTs. Centralised so adding a new column means
    /// updating one constant + `from_row`, not five query strings.
    const COLS: &'static str = "id, project_id, name, frequency, spec_json, wakeup_path, enabled, last_fired, last_session_id, archived_at, created_at";

    pub fn get_by_name(conn: &Connection, project_id: &str, name: &str) -> Result<Option<AgentHeartbeat>> {
        let sql = format!(
            "SELECT {} FROM agent_heartbeats WHERE project_id = ?1 AND name = ?2",
            Self::COLS,
        );
        let mut stmt = conn.prepare(&sql)?;
        let mut rows = stmt.query_map(params![project_id, name], Self::from_row)?;
        match rows.next() {
            Some(Ok(h)) => Ok(Some(h)),
            Some(Err(e)) => Err(e),
            None => Ok(None),
        }
    }

    pub fn list_by_project(conn: &Connection, project_id: &str) -> Result<Vec<AgentHeartbeat>> {
        let sql = format!(
            "SELECT {} FROM agent_heartbeats WHERE project_id = ?1 ORDER BY name",
            Self::COLS,
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![project_id], Self::from_row)?;
        rows.collect()
    }

    /// Active (non-archived) rows for a project. The Settings list and the
    /// sidebar's Live/Resumable/Scheduled sections both use this.
    pub fn list_active(conn: &Connection, project_id: &str) -> Result<Vec<AgentHeartbeat>> {
        let sql = format!(
            "SELECT {} FROM agent_heartbeats \
             WHERE project_id = ?1 AND archived_at IS NULL ORDER BY name",
            Self::COLS,
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![project_id], Self::from_row)?;
        rows.collect()
    }

    /// Archived rows for a project. The sidebar's collapsed Archived
    /// section uses this; ordered by archive recency (newest first).
    pub fn list_archived(conn: &Connection, project_id: &str) -> Result<Vec<AgentHeartbeat>> {
        let sql = format!(
            "SELECT {} FROM agent_heartbeats \
             WHERE project_id = ?1 AND archived_at IS NOT NULL \
             ORDER BY archived_at DESC",
            Self::COLS,
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![project_id], Self::from_row)?;
        rows.collect()
    }

    pub fn list_enabled(conn: &Connection, project_id: &str) -> Result<Vec<AgentHeartbeat>> {
        // Tick-time evaluator. Skip archived heartbeats — they no longer
        // fire on schedule even if `enabled` was never flipped before
        // archiving.
        let sql = format!(
            "SELECT {} FROM agent_heartbeats \
             WHERE project_id = ?1 AND enabled = 1 AND archived_at IS NULL \
             ORDER BY name",
            Self::COLS,
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![project_id], Self::from_row)?;
        rows.collect()
    }

    pub fn set_enabled(conn: &Connection, project_id: &str, name: &str, enabled: bool) -> Result<usize> {
        conn.execute(
            "UPDATE agent_heartbeats SET enabled = ?1 WHERE project_id = ?2 AND name = ?3",
            params![enabled as i64, project_id, name],
        )
    }

    pub fn update_schedule(
        conn: &Connection,
        project_id: &str,
        name: &str,
        frequency: &str,
        spec_json: &str,
    ) -> Result<usize> {
        conn.execute(
            "UPDATE agent_heartbeats SET frequency = ?1, spec_json = ?2 \
             WHERE project_id = ?3 AND name = ?4",
            params![frequency, spec_json, project_id, name],
        )
    }

    pub fn update_wakeup_path(
        conn: &Connection,
        project_id: &str,
        name: &str,
        wakeup_path: &str,
    ) -> Result<usize> {
        conn.execute(
            "UPDATE agent_heartbeats SET wakeup_path = ?1 WHERE project_id = ?2 AND name = ?3",
            params![wakeup_path, project_id, name],
        )
    }

    /// Stamp last_fired. Only called on *successful* spawn — lock-skips
    /// deliberately do NOT stamp, so the heartbeat stays eligible for
    /// the next tick. See PRD § last_fired semantics.
    pub fn stamp_last_fired(conn: &Connection, project_id: &str, name: &str) -> Result<usize> {
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE agent_heartbeats SET last_fired = ?1 WHERE project_id = ?2 AND name = ?3",
            params![now, project_id, name],
        )
    }

    /// Record the Claude session id from a successful heartbeat spawn.
    /// The next fire's `--resume` target. Called by `spawn_wake_headless`
    /// alongside the existing `agent_sessions::save_session_id` write.
    pub fn save_session_id(
        conn: &Connection,
        project_id: &str,
        name: &str,
        session_id: &str,
    ) -> Result<usize> {
        conn.execute(
            "UPDATE agent_heartbeats SET last_session_id = ?1 \
             WHERE project_id = ?2 AND name = ?3",
            params![session_id, project_id, name],
        )
    }

    /// Soft-archive: set archived_at to now. Idempotent — re-archiving an
    /// already-archived row is a no-op (timestamp unchanged). Called by
    /// the Settings "Archive" button (replaced the previous hard-delete
    /// "Remove" behaviour in 0.36.0).
    pub fn archive(conn: &Connection, project_id: &str, name: &str) -> Result<usize> {
        conn.execute(
            "UPDATE agent_heartbeats SET archived_at = ?1 \
             WHERE project_id = ?2 AND name = ?3 AND archived_at IS NULL",
            params![chrono::Utc::now().to_rfc3339(), project_id, name],
        )
    }

    /// Restore a soft-archived heartbeat. Reserved for a future "Restore
    /// from Archive" UI affordance — no caller in 0.36.0.
    pub fn unarchive(conn: &Connection, project_id: &str, name: &str) -> Result<usize> {
        conn.execute(
            "UPDATE agent_heartbeats SET archived_at = NULL \
             WHERE project_id = ?1 AND name = ?2",
            params![project_id, name],
        )
    }

    pub fn delete(conn: &Connection, project_id: &str, name: &str) -> Result<usize> {
        conn.execute(
            "DELETE FROM agent_heartbeats WHERE project_id = ?1 AND name = ?2",
            params![project_id, name],
        )
    }

    fn from_row(row: &rusqlite::Row<'_>) -> Result<AgentHeartbeat> {
        Ok(AgentHeartbeat {
            id: row.get(0)?,
            project_id: row.get(1)?,
            name: row.get(2)?,
            frequency: row.get(3)?,
            spec_json: row.get(4)?,
            wakeup_path: row.get(5)?,
            enabled: row.get::<_, i64>(6)? == 1,
            last_fired: row.get(7)?,
            last_session_id: row.get(8)?,
            archived_at: row.get(9)?,
            created_at: row.get(10)?,
        })
    }
}

// ── Workspace Relations ─────────────────────────────────────────────────

/// Cross-workspace relationship. A custom agent workspace can oversee
/// one or more workspace manager workspaces.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceRelation {
    pub id: String,
    pub source_project_id: String,
    pub target_project_id: String,
    pub relation_type: String,
    pub created_at: i64,
}

impl WorkspaceRelation {
    pub fn create(
        conn: &Connection,
        id: &str,
        source_project_id: &str,
        target_project_id: &str,
        relation_type: &str,
    ) -> Result<()> {
        conn.execute(
            "INSERT INTO workspace_relations (id, source_project_id, target_project_id, relation_type, created_at) \
             VALUES (?1, ?2, ?3, ?4, unixepoch())",
            params![id, source_project_id, target_project_id, relation_type],
        )?;
        Ok(())
    }

    /// Workspaces that this project oversees (source → targets).
    pub fn list_for_source(conn: &Connection, project_id: &str) -> Result<Vec<WorkspaceRelation>> {
        let mut stmt = conn.prepare(
            "SELECT id, source_project_id, target_project_id, relation_type, created_at \
             FROM workspace_relations WHERE source_project_id = ?1 ORDER BY created_at"
        )?;
        let rows = stmt.query_map(params![project_id], |row| {
            Ok(WorkspaceRelation {
                id: row.get(0)?,
                source_project_id: row.get(1)?,
                target_project_id: row.get(2)?,
                relation_type: row.get(3)?,
                created_at: row.get(4)?,
            })
        })?;
        rows.collect()
    }

    /// Custom agents that oversee this project (target ← sources).
    pub fn list_for_target(conn: &Connection, project_id: &str) -> Result<Vec<WorkspaceRelation>> {
        let mut stmt = conn.prepare(
            "SELECT id, source_project_id, target_project_id, relation_type, created_at \
             FROM workspace_relations WHERE target_project_id = ?1 ORDER BY created_at"
        )?;
        let rows = stmt.query_map(params![project_id], |row| {
            Ok(WorkspaceRelation {
                id: row.get(0)?,
                source_project_id: row.get(1)?,
                target_project_id: row.get(2)?,
                relation_type: row.get(3)?,
                created_at: row.get(4)?,
            })
        })?;
        rows.collect()
    }

    pub fn delete(conn: &Connection, id: &str) -> Result<usize> {
        conn.execute("DELETE FROM workspace_relations WHERE id = ?1", params![id])
    }
}

// ── Activity Feed ───────────────────────────────────────────────────────

/// Audit trail entry for agent communications and lifecycle events.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityFeedEntry {
    pub id: i64,
    pub project_id: String,
    pub agent_name: Option<String>,
    pub event_type: String,
    pub from_agent: Option<String>,
    pub to_agent: Option<String>,
    pub to_project_id: Option<String>,
    pub summary: Option<String>,
    pub metadata: Option<String>,
    pub created_at: i64,
}

impl ActivityFeedEntry {
    pub fn insert(
        conn: &Connection,
        project_id: &str,
        agent_name: Option<&str>,
        event_type: &str,
        from_agent: Option<&str>,
        to_agent: Option<&str>,
        to_project_id: Option<&str>,
        summary: Option<&str>,
        metadata: Option<&str>,
    ) -> Result<i64> {
        // prepare_cached keeps the compiled statement in rusqlite's per-
        // connection LRU cache (default 16 slots). activity_feed appends
        // fire on every agent event; criterion bench at P1.3 showed ~25%
        // speedup vs rebuilding the statement each call.
        let mut stmt = conn.prepare_cached(
            "INSERT INTO activity_feed (project_id, agent_name, event_type, from_agent, to_agent, to_project_id, summary, metadata, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, unixepoch())",
        )?;
        stmt.execute(params![project_id, agent_name, event_type, from_agent, to_agent, to_project_id, summary, metadata])?;
        Ok(conn.last_insert_rowid())
    }

    pub fn list_by_project(conn: &Connection, project_id: &str, limit: i64, offset: i64) -> Result<Vec<ActivityFeedEntry>> {
        let mut stmt = conn.prepare(
            "SELECT id, project_id, agent_name, event_type, from_agent, to_agent, to_project_id, summary, metadata, created_at \
             FROM activity_feed WHERE project_id = ?1 ORDER BY created_at DESC LIMIT ?2 OFFSET ?3"
        )?;
        let rows = stmt.query_map(params![project_id, limit, offset], |row| {
            Ok(ActivityFeedEntry {
                id: row.get(0)?,
                project_id: row.get(1)?,
                agent_name: row.get(2)?,
                event_type: row.get(3)?,
                from_agent: row.get(4)?,
                to_agent: row.get(5)?,
                to_project_id: row.get(6)?,
                summary: row.get(7)?,
                metadata: row.get(8)?,
                created_at: row.get(9)?,
            })
        })?;
        rows.collect()
    }

    pub fn list_by_agent(conn: &Connection, project_id: &str, agent_name: &str, limit: i64) -> Result<Vec<ActivityFeedEntry>> {
        let mut stmt = conn.prepare(
            "SELECT id, project_id, agent_name, event_type, from_agent, to_agent, to_project_id, summary, metadata, created_at \
             FROM activity_feed WHERE project_id = ?1 AND (agent_name = ?2 OR from_agent = ?2 OR to_agent = ?2) \
             ORDER BY created_at DESC LIMIT ?3"
        )?;
        let rows = stmt.query_map(params![project_id, agent_name, limit], |row| {
            Ok(ActivityFeedEntry {
                id: row.get(0)?,
                project_id: row.get(1)?,
                agent_name: row.get(2)?,
                event_type: row.get(3)?,
                from_agent: row.get(4)?,
                to_agent: row.get(5)?,
                to_project_id: row.get(6)?,
                summary: row.get(7)?,
                metadata: row.get(8)?,
                created_at: row.get(9)?,
            })
        })?;
        rows.collect()
    }
}

/// Convenience function to log an activity feed entry.
/// Used by CLI route handlers in agent_hooks.rs.
pub fn log_activity(
    conn: &Connection,
    project_id: &str,
    agent_name: Option<&str>,
    event_type: &str,
    from_agent: Option<&str>,
    to_agent: Option<&str>,
    to_project_id: Option<&str>,
    summary: Option<&str>,
) {
    let _ = ActivityFeedEntry::insert(conn, project_id, agent_name, event_type, from_agent, to_agent, to_project_id, summary, None);
}

/// Get unread messages for a specific agent in a project.
pub fn get_unread_messages(
    conn: &Connection,
    project_id: &str,
    agent_name: &str,
) -> Result<Vec<ActivityFeedEntry>> {
    let mut stmt = conn.prepare(
        "SELECT id, project_id, agent_name, event_type, from_agent, to_agent, to_project_id, summary, metadata, created_at \
         FROM activity_feed \
         WHERE (to_agent = ?1 OR (to_agent IS NULL AND ?1 = '__lead__')) \
         AND (project_id = ?2 OR to_project_id = ?2) \
         AND event_type IN ('message.sent', 'message.delivered') \
         AND read = 0 \
         ORDER BY created_at ASC"
    )?;
    let rows = stmt.query_map(params![agent_name, project_id], |row| {
        Ok(ActivityFeedEntry {
            id: row.get(0)?,
            project_id: row.get(1)?,
            agent_name: row.get(2)?,
            event_type: row.get(3)?,
            from_agent: row.get(4)?,
            to_agent: row.get(5)?,
            to_project_id: row.get(6)?,
            summary: row.get(7)?,
            metadata: row.get(8)?,
            created_at: row.get(9)?,
        })
    })?;
    rows.collect()
}

/// Mark messages as read for an agent.
pub fn mark_messages_read(
    conn: &Connection,
    project_id: &str,
    agent_name: &str,
) -> Result<usize> {
    conn.execute(
        "UPDATE activity_feed SET read = 1 \
         WHERE (to_agent = ?1 OR (to_agent IS NULL AND ?1 = '__lead__')) \
         AND (project_id = ?2 OR to_project_id = ?2) \
         AND event_type IN ('message.sent', 'message.delivered') \
         AND read = 0",
        params![agent_name, project_id],
    )
}

// ── Heartbeat audit log ────────────────────────────────────────────────

/// One row per scheduler decision. Written on every tick — both for
/// agents that were launched and for agents that were skipped — so users
/// can see exactly why each agent did or didn't wake.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HeartbeatFire {
    pub id: i64,
    pub project_id: String,
    pub agent_name: Option<String>,
    pub schedule_name: Option<String>,
    pub fired_at: String,
    pub mode: String,
    pub decision: String,
    pub reason: Option<String>,
    pub inbox_priority: Option<String>,
    pub inbox_count: Option<i64>,
    pub duration_ms: Option<i64>,
}

impl HeartbeatFire {
    /// Insert an audit row. `schedule_name` is the multi-heartbeat name
    /// (the `agent_heartbeats.name`); None for legacy fires that predate
    /// the multi-heartbeat system or aren't tied to a specific heartbeat.
    pub fn insert(
        conn: &Connection,
        project_id: &str,
        agent_name: Option<&str>,
        mode: &str,
        decision: &str,
        reason: Option<&str>,
        inbox_priority: Option<&str>,
        inbox_count: Option<i64>,
        duration_ms: Option<i64>,
    ) -> Result<i64> {
        Self::insert_with_schedule(
            conn, project_id, agent_name, None,
            mode, decision, reason, inbox_priority, inbox_count, duration_ms,
        )
    }

    /// Insert an audit row with an explicit schedule_name — used by the
    /// multi-heartbeat tick so `k2so heartbeat status <name>` can filter
    /// cleanly. schedule_name is denormalized TEXT (NOT a FK to
    /// agent_heartbeats.name) so audit rows survive heartbeat deletion.
    pub fn insert_with_schedule(
        conn: &Connection,
        project_id: &str,
        agent_name: Option<&str>,
        schedule_name: Option<&str>,
        mode: &str,
        decision: &str,
        reason: Option<&str>,
        inbox_priority: Option<&str>,
        inbox_count: Option<i64>,
        duration_ms: Option<i64>,
    ) -> Result<i64> {
        // Fires on every heartbeat tick — high-volume INSERT, cached.
        let mut stmt = conn.prepare_cached(
            "INSERT INTO heartbeat_fires \
             (project_id, agent_name, fired_at, mode, decision, reason, inbox_priority, inbox_count, duration_ms, schedule_name) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        )?;
        stmt.execute(params![
            project_id,
            agent_name,
            chrono::Local::now().to_rfc3339(),
            mode,
            decision,
            reason,
            inbox_priority,
            inbox_count,
            duration_ms,
            schedule_name,
        ])?;
        Ok(conn.last_insert_rowid())
    }

    /// Return the most recent `limit` fire rows for a project.
    pub fn list_by_project(
        conn: &Connection,
        project_id: &str,
        limit: i64,
    ) -> Result<Vec<HeartbeatFire>> {
        let mut stmt = conn.prepare(
            "SELECT id, project_id, agent_name, schedule_name, fired_at, mode, decision, reason, \
                    inbox_priority, inbox_count, duration_ms \
             FROM heartbeat_fires WHERE project_id = ?1 \
             ORDER BY fired_at DESC LIMIT ?2"
        )?;
        let rows = stmt.query_map(params![project_id, limit], |row| {
            Ok(HeartbeatFire {
                id: row.get(0)?,
                project_id: row.get(1)?,
                agent_name: row.get(2)?,
                schedule_name: row.get(3)?,
                fired_at: row.get(4)?,
                mode: row.get(5)?,
                decision: row.get(6)?,
                reason: row.get(7)?,
                inbox_priority: row.get(8)?,
                inbox_count: row.get(9)?,
                duration_ms: row.get(10)?,
            })
        })?;
        rows.collect()
    }

    /// Filter fire rows by schedule_name — powers `k2so heartbeat status <name>`.
    pub fn list_by_schedule_name(
        conn: &Connection,
        project_id: &str,
        schedule_name: &str,
        limit: i64,
    ) -> Result<Vec<HeartbeatFire>> {
        let mut stmt = conn.prepare(
            "SELECT id, project_id, agent_name, schedule_name, fired_at, mode, decision, reason, \
                    inbox_priority, inbox_count, duration_ms \
             FROM heartbeat_fires WHERE project_id = ?1 AND schedule_name = ?2 \
             ORDER BY fired_at DESC LIMIT ?3"
        )?;
        let rows = stmt.query_map(params![project_id, schedule_name, limit], |row| {
            Ok(HeartbeatFire {
                id: row.get(0)?,
                project_id: row.get(1)?,
                agent_name: row.get(2)?,
                schedule_name: row.get(3)?,
                fired_at: row.get(4)?,
                mode: row.get(5)?,
                decision: row.get(6)?,
                reason: row.get(7)?,
                inbox_priority: row.get(8)?,
                inbox_count: row.get(9)?,
                duration_ms: row.get(10)?,
            })
        })?;
        rows.collect()
    }

    /// Delete fire rows older than the given RFC3339 timestamp. Returns
    /// the number of rows removed. Used by retention pruning (e.g., a
    /// user-triggered "clear old heartbeats" action).
    #[allow(dead_code)] // Retention helper — called from tests; the UI
                        // currently has no "clear old heartbeats" action.
    pub fn prune_before(conn: &Connection, cutoff: &str) -> Result<usize> {
        conn.execute(
            "DELETE FROM heartbeat_fires WHERE fired_at < ?1",
            params![cutoff],
        )
    }
}

#[cfg(test)]
mod unit_tests {
    //! Per-struct CRUD + invariant coverage for schema.rs. Each test
    //! uses `crate::db::isolated_test_connection()` to get a fresh
    //! in-memory SQLite with the full migration + seed sequence
    //! applied — so tests can assert on specific row counts or state
    //! transitions without worrying about pollution from sibling
    //! tests.
    //!
    //! Coverage target: every public method on every schema struct
    //! has at least a round-trip test (write → read → assert). Edge
    //! cases (unique constraint violations, name validation, enabled
    //! filter semantics) have dedicated tests.
    use super::*;
    use rusqlite::Connection;

    fn fresh() -> Connection {
        crate::db::isolated_test_connection()
    }

    fn make_project_row(conn: &Connection, path: &str) -> String {
        // Every test that touches session/heartbeat/fire tables needs
        // a projects row because of the FK. This matches make_project
        // in concurrency_tests but is duplicated here to keep the two
        // test modules independent.
        let id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT OR IGNORE INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
            params![id, "test", path],
        )
        .expect("insert project");
        id
    }

    // ── FocusGroup ────────────────────────────────────────────────
    #[test]
    fn focus_group_create_list_get_update_delete() {
        let conn = fresh();
        FocusGroup::create(&conn, "fg1", "Work", Some("#ff0000"), 0).unwrap();
        FocusGroup::create(&conn, "fg2", "Personal", None, 1).unwrap();

        let list = FocusGroup::list(&conn).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].name, "Work");
        assert_eq!(list[1].name, "Personal");

        let fg = FocusGroup::get(&conn, "fg1").unwrap();
        assert_eq!(fg.color.as_deref(), Some("#ff0000"));

        FocusGroup::update(&conn, "fg1", Some("Work Rebranded"), None, None).unwrap();
        let fg = FocusGroup::get(&conn, "fg1").unwrap();
        assert_eq!(fg.name, "Work Rebranded");

        FocusGroup::delete(&conn, "fg2").unwrap();
        assert_eq!(FocusGroup::list(&conn).unwrap().len(), 1);
    }

    // ── Project ───────────────────────────────────────────────────
    #[test]
    fn project_create_list_get_delete_roundtrip() {
        let conn = fresh();
        // Baseline accounts for the `_orphan` + `_broadcast`
        // sentinel rows seeded by `db::seed_audit_sentinels` — they
        // exist so egress audit never fails FK when a signal's
        // workspace id doesn't match a real project.
        let baseline = Project::list(&conn).unwrap().len();

        let id = make_project_row(&conn, "/tmp/proj-cr");
        let all = Project::list(&conn).unwrap();
        assert_eq!(all.len(), baseline + 1);
        assert!(
            all.iter().any(|p| p.path == "/tmp/proj-cr"),
            "inserted project should appear in list"
        );

        let p = Project::get(&conn, &id).unwrap();
        assert_eq!(p.id, id);
        assert_eq!(p.name, "test");

        Project::delete(&conn, &id).unwrap();
        assert_eq!(Project::list(&conn).unwrap().len(), baseline);
    }

    #[test]
    fn project_path_unique_constraint_rejects_duplicate() {
        let conn = fresh();
        let path = "/tmp/proj-dup";
        make_project_row(&conn, path);
        // Second insert with same path must fail.
        let id2 = uuid::Uuid::new_v4().to_string();
        let err = conn.execute(
            "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
            params![id2, "other", path],
        );
        assert!(err.is_err(), "duplicate path should violate unique index");
    }

    #[test]
    fn project_touch_and_clear_interaction() {
        let conn = fresh();
        let id = make_project_row(&conn, "/tmp/proj-touch");
        Project::touch_interaction(&conn, &id).unwrap();
        let p = Project::get(&conn, &id).unwrap();
        assert!(p.last_interaction_at.is_some(), "touch should set timestamp");

        Project::clear_interaction(&conn, &id).unwrap();
        let p = Project::get(&conn, &id).unwrap();
        assert!(p.last_interaction_at.is_none(), "clear should null timestamp");
    }

    #[test]
    fn project_update_last_opened_sets_timestamp() {
        let conn = fresh();
        let id = make_project_row(&conn, "/tmp/proj-opened");
        Project::update_last_opened(&conn, &id).unwrap();
        let p = Project::get(&conn, &id).unwrap();
        assert!(p.last_opened_at.is_some());
    }

    // ── AgentSession ──────────────────────────────────────────────
    #[test]
    fn agent_session_upsert_then_get_by_agent() {
        let conn = fresh();
        let pid = make_project_row(&conn, "/tmp/as1");
        AgentSession::upsert(
            &conn,
            "sess-1",
            &pid,
            "alice",
            Some("term-7"),
            None,
            "claude",
            "manager",
            "sleeping",
        )
        .unwrap();

        let s = AgentSession::get_by_agent(&conn, &pid, "alice")
            .unwrap()
            .expect("session exists");
        assert_eq!(s.id, "sess-1");
        assert_eq!(s.terminal_id.as_deref(), Some("term-7"));
        assert_eq!(s.status, "sleeping");
    }

    #[test]
    fn agent_session_upsert_updates_existing_row() {
        let conn = fresh();
        let pid = make_project_row(&conn, "/tmp/as2");
        AgentSession::upsert(
            &conn, "s1", &pid, "bob", Some("t1"), None, "claude", "manager", "sleeping",
        )
        .unwrap();
        AgentSession::upsert(
            &conn, "s2", &pid, "bob", Some("t2"), Some("scid"), "codex", "user", "running",
        )
        .unwrap();

        // Same (project, agent) — second upsert replaces the row's
        // payload but the conflict resolution uses the original id.
        let s = AgentSession::get_by_agent(&conn, &pid, "bob").unwrap().unwrap();
        assert_eq!(s.terminal_id.as_deref(), Some("t2"));
        assert_eq!(s.harness, "codex");
        assert_eq!(s.status, "running");
        assert_eq!(s.session_id.as_deref(), Some("scid"));
    }

    #[test]
    fn agent_session_unique_constraint_on_project_agent() {
        let conn = fresh();
        let pid = make_project_row(&conn, "/tmp/as3");
        AgentSession::upsert(
            &conn, "s1", &pid, "carol", None, None, "claude", "manager", "sleeping",
        )
        .unwrap();
        // Raw insert with different id but same (project, agent) must
        // violate the UNIQUE constraint.
        let err = conn.execute(
            "INSERT INTO agent_sessions (id, project_id, agent_name, harness, owner, status) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params!["sX", &pid, "carol", "claude", "manager", "sleeping"],
        );
        assert!(err.is_err(), "UNIQUE(project_id, agent_name) must reject");
    }

    #[test]
    fn agent_session_list_by_project_ordered() {
        let conn = fresh();
        let pid = make_project_row(&conn, "/tmp/as-list");
        for name in ["charlie", "alice", "bob"] {
            AgentSession::upsert(
                &conn, &format!("s-{}", name), &pid, name, None, None, "claude", "manager", "sleeping",
            )
            .unwrap();
        }
        let rows = AgentSession::list_by_project(&conn, &pid).unwrap();
        let names: Vec<&str> = rows.iter().map(|r| r.agent_name.as_str()).collect();
        assert_eq!(names, ["alice", "bob", "charlie"]);
    }

    #[test]
    fn agent_session_get_by_terminal_id() {
        let conn = fresh();
        let pid = make_project_row(&conn, "/tmp/as-term");
        AgentSession::upsert(
            &conn, "sa", &pid, "dana", Some("terminal-99"), None, "claude", "manager", "running",
        )
        .unwrap();
        let s = AgentSession::get_by_terminal_id(&conn, "terminal-99")
            .unwrap()
            .unwrap();
        assert_eq!(s.agent_name, "dana");
        let none = AgentSession::get_by_terminal_id(&conn, "no-such").unwrap();
        assert!(none.is_none());
    }

    #[test]
    fn agent_session_update_status_and_message() {
        let conn = fresh();
        let pid = make_project_row(&conn, "/tmp/as-sm");
        AgentSession::upsert(
            &conn, "s", &pid, "eve", None, None, "claude", "manager", "sleeping",
        )
        .unwrap();

        let n = AgentSession::update_status(&conn, &pid, "eve", "running").unwrap();
        assert_eq!(n, 1);
        AgentSession::update_status_message(&conn, &pid, "eve", "spawning PTY").unwrap();
        let s = AgentSession::get_by_agent(&conn, &pid, "eve").unwrap().unwrap();
        assert_eq!(s.status, "running");
        assert_eq!(s.status_message.as_deref(), Some("spawning PTY"));
    }

    #[test]
    fn agent_session_session_id_set_and_clear() {
        let conn = fresh();
        let pid = make_project_row(&conn, "/tmp/as-sid");
        AgentSession::upsert(
            &conn, "s", &pid, "frank", None, None, "claude", "manager", "sleeping",
        )
        .unwrap();
        AgentSession::update_session_id(&conn, &pid, "frank", "claude-abcd").unwrap();
        assert_eq!(
            AgentSession::get_by_agent(&conn, &pid, "frank").unwrap().unwrap().session_id.as_deref(),
            Some("claude-abcd")
        );
        AgentSession::clear_session_id(&conn, &pid, "frank").unwrap();
        assert!(AgentSession::get_by_agent(&conn, &pid, "frank").unwrap().unwrap().session_id.is_none());
    }

    #[test]
    fn agent_session_wake_counter_increments_and_resets() {
        let conn = fresh();
        let pid = make_project_row(&conn, "/tmp/as-wc");
        AgentSession::upsert(
            &conn, "s", &pid, "grace", None, None, "claude", "manager", "sleeping",
        )
        .unwrap();
        assert_eq!(AgentSession::bump_wake_counter(&conn, &pid, "grace").unwrap(), 1);
        assert_eq!(AgentSession::bump_wake_counter(&conn, &pid, "grace").unwrap(), 2);
        assert_eq!(AgentSession::bump_wake_counter(&conn, &pid, "grace").unwrap(), 3);
        AgentSession::reset_wake_counter(&conn, &pid, "grace").unwrap();
        assert_eq!(AgentSession::bump_wake_counter(&conn, &pid, "grace").unwrap(), 1);
    }

    #[test]
    fn agent_session_delete_removes_row() {
        let conn = fresh();
        let pid = make_project_row(&conn, "/tmp/as-del");
        AgentSession::upsert(
            &conn, "s", &pid, "henry", None, None, "claude", "manager", "sleeping",
        )
        .unwrap();
        assert!(AgentSession::get_by_agent(&conn, &pid, "henry").unwrap().is_some());
        let n = AgentSession::delete(&conn, &pid, "henry").unwrap();
        assert_eq!(n, 1);
        assert!(AgentSession::get_by_agent(&conn, &pid, "henry").unwrap().is_none());
    }

    // ── AgentHeartbeat ────────────────────────────────────────────
    #[test]
    fn heartbeat_validate_name_rejects_empty() {
        assert!(AgentHeartbeat::validate_name("").is_err());
    }

    #[test]
    fn heartbeat_validate_name_rejects_reserved() {
        assert!(AgentHeartbeat::validate_name("default").is_err());
        assert!(AgentHeartbeat::validate_name("legacy").is_err());
    }

    #[test]
    fn heartbeat_validate_name_rejects_uppercase() {
        assert!(AgentHeartbeat::validate_name("MyHeartbeat").is_err());
    }

    #[test]
    fn heartbeat_validate_name_rejects_leading_trailing_hyphen() {
        assert!(AgentHeartbeat::validate_name("-foo").is_err());
        assert!(AgentHeartbeat::validate_name("foo-").is_err());
    }

    #[test]
    fn heartbeat_validate_name_accepts_valid() {
        assert!(AgentHeartbeat::validate_name("nightly").is_ok());
        assert!(AgentHeartbeat::validate_name("morning-1").is_ok());
        assert!(AgentHeartbeat::validate_name("h1").is_ok());
    }

    #[test]
    fn heartbeat_insert_list_get_delete() {
        let conn = fresh();
        let pid = make_project_row(&conn, "/tmp/hb-c");
        AgentHeartbeat::insert(
            &conn, "hb1", &pid, "nightly", "60m", "{}", "agents/foo/heartbeats/nightly/WAKEUP.md", true,
        )
        .unwrap();
        AgentHeartbeat::insert(
            &conn, "hb2", &pid, "morning", "30m", "{}", "agents/foo/heartbeats/morning/WAKEUP.md", false,
        )
        .unwrap();

        let list = AgentHeartbeat::list_by_project(&conn, &pid).unwrap();
        assert_eq!(list.len(), 2);
        // list is ORDER BY name — morning < nightly
        assert_eq!(list[0].name, "morning");
        assert_eq!(list[1].name, "nightly");

        let enabled_only = AgentHeartbeat::list_enabled(&conn, &pid).unwrap();
        assert_eq!(enabled_only.len(), 1);
        assert_eq!(enabled_only[0].name, "nightly");

        let h = AgentHeartbeat::get_by_name(&conn, &pid, "nightly").unwrap().unwrap();
        assert_eq!(h.frequency, "60m");

        let n = AgentHeartbeat::delete(&conn, &pid, "morning").unwrap();
        assert_eq!(n, 1);
        assert_eq!(AgentHeartbeat::list_by_project(&conn, &pid).unwrap().len(), 1);
    }

    #[test]
    fn heartbeat_set_enabled_toggles() {
        let conn = fresh();
        let pid = make_project_row(&conn, "/tmp/hb-en");
        AgentHeartbeat::insert(
            &conn, "hb1", &pid, "weekly", "7d", "{}", "agents/foo/heartbeats/weekly/WAKEUP.md", false,
        )
        .unwrap();
        AgentHeartbeat::set_enabled(&conn, &pid, "weekly", true).unwrap();
        let h = AgentHeartbeat::get_by_name(&conn, &pid, "weekly").unwrap().unwrap();
        assert!(h.enabled);
        AgentHeartbeat::set_enabled(&conn, &pid, "weekly", false).unwrap();
        assert!(!AgentHeartbeat::get_by_name(&conn, &pid, "weekly").unwrap().unwrap().enabled);
    }

    #[test]
    fn heartbeat_update_schedule_and_wakeup_path() {
        let conn = fresh();
        let pid = make_project_row(&conn, "/tmp/hb-upd");
        AgentHeartbeat::insert(
            &conn, "hb1", &pid, "pulse", "60m", "{\"x\":1}", "path1", true,
        )
        .unwrap();
        AgentHeartbeat::update_schedule(&conn, &pid, "pulse", "30m", "{\"x\":2}").unwrap();
        let h = AgentHeartbeat::get_by_name(&conn, &pid, "pulse").unwrap().unwrap();
        assert_eq!(h.frequency, "30m");
        assert_eq!(h.spec_json, "{\"x\":2}");

        AgentHeartbeat::update_wakeup_path(&conn, &pid, "pulse", "new/path").unwrap();
        let h = AgentHeartbeat::get_by_name(&conn, &pid, "pulse").unwrap().unwrap();
        assert_eq!(h.wakeup_path, "new/path");
    }

    #[test]
    fn heartbeat_stamp_last_fired_sets_rfc3339() {
        let conn = fresh();
        let pid = make_project_row(&conn, "/tmp/hb-fire");
        AgentHeartbeat::insert(
            &conn, "hb1", &pid, "hb", "60m", "{}", "p", true,
        )
        .unwrap();
        AgentHeartbeat::stamp_last_fired(&conn, &pid, "hb").unwrap();
        let h = AgentHeartbeat::get_by_name(&conn, &pid, "hb").unwrap().unwrap();
        let ts = h.last_fired.expect("last_fired set");
        // RFC3339 sanity — "YYYY-MM-DDTHH:MM:SS..."
        assert!(ts.contains('T'), "expected RFC3339 timestamp, got: {}", ts);
        assert!(chrono::DateTime::parse_from_rfc3339(&ts).is_ok(), "parseable RFC3339: {}", ts);
    }

    // ── 0.36.0 fields: last_session_id + archived_at ──────────────

    #[test]
    fn heartbeat_save_session_id_writes_value() {
        let conn = fresh();
        let pid = make_project_row(&conn, "/tmp/hb-sid");
        AgentHeartbeat::insert(
            &conn, "hb1", &pid, "triage", "60m", "{}", "p", true,
        )
        .unwrap();
        let pre = AgentHeartbeat::get_by_name(&conn, &pid, "triage").unwrap().unwrap();
        assert!(pre.last_session_id.is_none(), "fresh row has no session id");

        let n = AgentHeartbeat::save_session_id(&conn, &pid, "triage", "claude-xyz").unwrap();
        assert_eq!(n, 1);

        let h = AgentHeartbeat::get_by_name(&conn, &pid, "triage").unwrap().unwrap();
        assert_eq!(h.last_session_id.as_deref(), Some("claude-xyz"));
    }

    #[test]
    fn heartbeat_archive_sets_timestamp_and_is_idempotent() {
        let conn = fresh();
        let pid = make_project_row(&conn, "/tmp/hb-arch");
        AgentHeartbeat::insert(
            &conn, "hb1", &pid, "weekly", "7d", "{}", "p", true,
        )
        .unwrap();

        // First archive — sets the timestamp.
        let n1 = AgentHeartbeat::archive(&conn, &pid, "weekly").unwrap();
        assert_eq!(n1, 1, "first archive updates one row");
        let archived_first = AgentHeartbeat::get_by_name(&conn, &pid, "weekly")
            .unwrap()
            .unwrap()
            .archived_at
            .expect("archived_at set after archive");

        // Second archive — no-op (the WHERE clause excludes already-archived rows).
        let n2 = AgentHeartbeat::archive(&conn, &pid, "weekly").unwrap();
        assert_eq!(n2, 0, "re-archive of an archived row is a no-op");

        let archived_second = AgentHeartbeat::get_by_name(&conn, &pid, "weekly")
            .unwrap()
            .unwrap()
            .archived_at
            .expect("archived_at preserved after no-op re-archive");
        assert_eq!(
            archived_first, archived_second,
            "archived_at timestamp must NOT change on re-archive"
        );
    }

    #[test]
    fn heartbeat_unarchive_clears_timestamp() {
        let conn = fresh();
        let pid = make_project_row(&conn, "/tmp/hb-un");
        AgentHeartbeat::insert(
            &conn, "hb1", &pid, "monthly", "30d", "{}", "p", true,
        )
        .unwrap();
        AgentHeartbeat::archive(&conn, &pid, "monthly").unwrap();
        assert!(AgentHeartbeat::get_by_name(&conn, &pid, "monthly").unwrap().unwrap().archived_at.is_some());

        let n = AgentHeartbeat::unarchive(&conn, &pid, "monthly").unwrap();
        assert_eq!(n, 1);
        assert!(AgentHeartbeat::get_by_name(&conn, &pid, "monthly").unwrap().unwrap().archived_at.is_none());
    }

    #[test]
    fn heartbeat_list_active_excludes_archived() {
        let conn = fresh();
        let pid = make_project_row(&conn, "/tmp/hb-lact");
        AgentHeartbeat::insert(&conn, "hb1", &pid, "alpha", "60m", "{}", "p", true).unwrap();
        AgentHeartbeat::insert(&conn, "hb2", &pid, "beta",  "60m", "{}", "p", true).unwrap();
        AgentHeartbeat::insert(&conn, "hb3", &pid, "gamma", "60m", "{}", "p", true).unwrap();
        AgentHeartbeat::archive(&conn, &pid, "beta").unwrap();

        let active = AgentHeartbeat::list_active(&conn, &pid).unwrap();
        let names: Vec<&str> = active.iter().map(|h| h.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "gamma"], "archived row must not appear in list_active");
    }

    #[test]
    fn heartbeat_list_archived_returns_only_archived_rows() {
        let conn = fresh();
        let pid = make_project_row(&conn, "/tmp/hb-larc");
        AgentHeartbeat::insert(&conn, "hb1", &pid, "alpha", "60m", "{}", "p", true).unwrap();
        AgentHeartbeat::insert(&conn, "hb2", &pid, "beta",  "60m", "{}", "p", true).unwrap();
        AgentHeartbeat::archive(&conn, &pid, "beta").unwrap();

        let archived = AgentHeartbeat::list_archived(&conn, &pid).unwrap();
        assert_eq!(archived.len(), 1);
        assert_eq!(archived[0].name, "beta");
        assert!(archived[0].archived_at.is_some());
    }

    #[test]
    fn heartbeat_list_enabled_excludes_archived_even_when_enabled() {
        // The scheduler-tick path uses list_enabled; archiving must
        // stop a heartbeat from firing even if enabled was never
        // toggled off before archive.
        let conn = fresh();
        let pid = make_project_row(&conn, "/tmp/hb-len");
        AgentHeartbeat::insert(&conn, "hb1", &pid, "live",     "60m", "{}", "p", true).unwrap();
        AgentHeartbeat::insert(&conn, "hb2", &pid, "retired",  "60m", "{}", "p", true).unwrap();
        AgentHeartbeat::archive(&conn, &pid, "retired").unwrap();

        let enabled = AgentHeartbeat::list_enabled(&conn, &pid).unwrap();
        let names: Vec<&str> = enabled.iter().map(|h| h.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["live"],
            "archived heartbeat must be skipped by the tick evaluator"
        );
    }

    #[test]
    fn migration_0034_default_show_heartbeat_sessions_is_zero() {
        // Bare projects row — no explicit show_heartbeat_sessions value.
        // Migration 0034 sets DEFAULT 0, so freshly-inserted rows must
        // have it as 0 (silent autonomous mode is the v2-headless default).
        let conn = fresh();
        let pid = make_project_row(&conn, "/tmp/proj-0034");
        let v: i64 = conn
            .query_row(
                "SELECT show_heartbeat_sessions FROM projects WHERE id = ?1",
                params![pid],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(v, 0, "show_heartbeat_sessions must default to 0 (off)");
    }

    // ── ActivityFeedEntry ─────────────────────────────────────────
    #[test]
    fn activity_feed_insert_and_list_by_project() {
        let conn = fresh();
        let pid = make_project_row(&conn, "/tmp/af");
        let id1 = ActivityFeedEntry::insert(
            &conn, &pid, Some("alice"), "wake.start", None, None, None, Some("kick"), None,
        )
        .unwrap();
        let id2 = ActivityFeedEntry::insert(
            &conn, &pid, Some("alice"), "wake.end", None, None, None, Some("done"), None,
        )
        .unwrap();
        assert!(id2 > id1);

        let rows = ActivityFeedEntry::list_by_project(&conn, &pid, 10, 0).unwrap();
        assert_eq!(rows.len(), 2);
        // ORDER BY created_at DESC — newest first. Matching timestamps
        // (unixepoch() resolves to seconds) can tie; we only assert
        // both rows came back.
        assert!(rows.iter().any(|r| r.event_type == "wake.start"));
        assert!(rows.iter().any(|r| r.event_type == "wake.end"));
    }

    #[test]
    fn activity_feed_list_by_agent_matches_agent_from_or_to() {
        let conn = fresh();
        let pid = make_project_row(&conn, "/tmp/af-b");
        ActivityFeedEntry::insert(&conn, &pid, Some("alice"), "x", None, None, None, None, None).unwrap();
        ActivityFeedEntry::insert(&conn, &pid, None, "y", Some("alice"), Some("bob"), None, None, None).unwrap();
        ActivityFeedEntry::insert(&conn, &pid, None, "z", Some("carol"), Some("alice"), None, None, None).unwrap();
        ActivityFeedEntry::insert(&conn, &pid, None, "w", Some("bob"), Some("carol"), None, None, None).unwrap();

        let alice_rows = ActivityFeedEntry::list_by_agent(&conn, &pid, "alice", 10).unwrap();
        // 3 rows: agent_name=alice, from=alice, to=alice.
        assert_eq!(alice_rows.len(), 3);
    }

    #[test]
    fn activity_feed_unread_messages_filtered_and_marked() {
        let conn = fresh();
        let pid = make_project_row(&conn, "/tmp/af-unread");
        ActivityFeedEntry::insert(
            &conn, &pid, None, "message.sent", Some("alice"), Some("bob"), None, Some("hello"), None,
        )
        .unwrap();
        ActivityFeedEntry::insert(
            &conn, &pid, None, "wake.start", Some("alice"), Some("bob"), None, None, None,
        )
        .unwrap();
        // wake.start should NOT show up in unread messages (filter
        // limits to message.sent/message.delivered).
        let unread = super::get_unread_messages(&conn, &pid, "bob").unwrap();
        assert_eq!(unread.len(), 1);
        assert_eq!(unread[0].event_type, "message.sent");

        let n = super::mark_messages_read(&conn, &pid, "bob").unwrap();
        assert_eq!(n, 1);
        let unread_after = super::get_unread_messages(&conn, &pid, "bob").unwrap();
        assert!(unread_after.is_empty(), "after mark_read, no unread should remain");
    }

    // ── HeartbeatFire ─────────────────────────────────────────────
    #[test]
    fn heartbeat_fire_insert_and_list_by_project() {
        let conn = fresh();
        let pid = make_project_row(&conn, "/tmp/hf");
        HeartbeatFire::insert(
            &conn, &pid, Some("alice"), "agent", "fired", Some("inbox has work"), Some("normal"), Some(2), Some(150),
        )
        .unwrap();
        HeartbeatFire::insert(
            &conn, &pid, Some("bob"), "agent", "skipped", Some("already running"), None, None, None,
        )
        .unwrap();

        let rows = HeartbeatFire::list_by_project(&conn, &pid, 10).unwrap();
        assert_eq!(rows.len(), 2);
        // DESC by fired_at — may or may not tie. Just assert presence.
        let decisions: Vec<&str> = rows.iter().map(|r| r.decision.as_str()).collect();
        assert!(decisions.contains(&"fired"));
        assert!(decisions.contains(&"skipped"));
    }

    #[test]
    fn heartbeat_fire_insert_with_schedule_persists_schedule_name() {
        let conn = fresh();
        let pid = make_project_row(&conn, "/tmp/hf-sched");
        HeartbeatFire::insert_with_schedule(
            &conn, &pid, Some("alice"), Some("nightly"),
            "agent", "fired", Some("nightly tick"), None, None, Some(42),
        )
        .unwrap();
        let rows = HeartbeatFire::list_by_schedule_name(&conn, &pid, "nightly", 10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].schedule_name.as_deref(), Some("nightly"));
        assert_eq!(rows[0].duration_ms, Some(42));
    }

    #[test]
    fn heartbeat_fire_prune_before_removes_old_rows() {
        let conn = fresh();
        let pid = make_project_row(&conn, "/tmp/hf-prune");
        // Insert one row with a known-old timestamp and one fresh.
        conn.execute(
            "INSERT INTO heartbeat_fires (project_id, mode, decision, fired_at) VALUES (?1, 'agent', 'fired', '2020-01-01T00:00:00Z')",
            params![pid],
        )
        .unwrap();
        HeartbeatFire::insert(&conn, &pid, None, "agent", "fired", None, None, None, None).unwrap();

        let removed = HeartbeatFire::prune_before(&conn, "2021-01-01T00:00:00Z").unwrap();
        assert_eq!(removed, 1, "prune should remove 1 old row");
        let remaining = HeartbeatFire::list_by_project(&conn, &pid, 10).unwrap();
        assert_eq!(remaining.len(), 1, "fresh row remains");
    }

    // ── WorkspaceRelation ─────────────────────────────────────────
    #[test]
    fn workspace_relation_create_list_for_source_and_target_delete() {
        let conn = fresh();
        let src = make_project_row(&conn, "/tmp/ws-src");
        let tgt = make_project_row(&conn, "/tmp/ws-tgt");

        WorkspaceRelation::create(&conn, "rel-1", &src, &tgt, "manages").unwrap();

        let from_src = WorkspaceRelation::list_for_source(&conn, &src).unwrap();
        assert_eq!(from_src.len(), 1);
        assert_eq!(from_src[0].target_project_id, tgt);

        let from_tgt = WorkspaceRelation::list_for_target(&conn, &tgt).unwrap();
        assert_eq!(from_tgt.len(), 1);
        assert_eq!(from_tgt[0].source_project_id, src);

        let n = WorkspaceRelation::delete(&conn, "rel-1").unwrap();
        assert_eq!(n, 1);
        assert!(WorkspaceRelation::list_for_source(&conn, &src).unwrap().is_empty());
    }

    // ── AgentPreset seed ──────────────────────────────────────────
    #[test]
    fn agent_preset_seed_populates_built_ins() {
        // isolated_test_connection runs seed_agent_presets — so the
        // built-in presets should be present. Spot-check that Claude
        // and at least one local LLM are there.
        let conn = fresh();
        let presets = AgentPreset::list(&conn).unwrap();
        let labels: Vec<&str> = presets.iter().map(|p| p.label.as_str()).collect();
        assert!(labels.contains(&"Claude"), "Claude preset missing: {:?}", labels);
        assert!(labels.contains(&"Ollama"), "Ollama preset missing: {:?}", labels);
        assert!(presets.len() >= 11, "expected >=11 built-in presets, got {}", presets.len());
    }

    #[test]
    fn agent_preset_seed_is_idempotent_on_reapply() {
        let conn = fresh();
        let before = AgentPreset::list(&conn).unwrap().len();
        // The isolated_test_connection already seeded. A second seed
        // must be a no-op (INSERT OR IGNORE).
        crate::db::seed_agent_presets(&conn).unwrap();
        let after = AgentPreset::list(&conn).unwrap().len();
        assert_eq!(before, after, "re-seed must not duplicate rows");
    }
}

#[cfg(test)]
mod concurrency_tests {
    //! CAS and multi-connection concurrency tests for schema-level
    //! operations. These tests use file-backed SQLite (via a temp
    //! directory) because in-memory `:memory:` databases are not
    //! shared across `Connection` handles — to actually race
    //! connections we need real disk state.
    //!
    //! The resilience review introduced `try_acquire_running` with
    //! `BEGIN IMMEDIATE` specifically to avoid a TOCTOU race between
    //! two heartbeats firing at the same time. These tests PROVE the
    //! claim by spawning N threads, each opening their own
    //! connection, and asserting exactly one wins the acquisition.
    //! Without these, "concurrency-safe" is just an assertion in the
    //! doc comment.
    use super::*;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Build a unique on-disk DB path and bootstrap it through the
    /// full migration + seed sequence. Caller is responsible for
    /// cleanup (directory removal after the test).
    fn scratch_db() -> (PathBuf, PathBuf) {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "k2so-schema-test-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let db_path = dir.join("k2so.db");
        crate::db::bootstrap_test_db_at(&db_path).expect("bootstrap");
        (dir, db_path)
    }

    fn open_conn(path: &std::path::Path) -> Connection {
        crate::db::open_with_resilience(path).expect("open connection")
    }

    fn make_project(conn: &Connection, project_path: &str) -> String {
        // Schema requires a projects row: agent_sessions.project_id is
        // a FK to projects.id, and PRAGMA foreign_keys is ON. Returns
        // the generated UUID — callers pass that as the project_id
        // arg to try_acquire_running.
        let id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT OR IGNORE INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
            params![id, "test", project_path],
        )
        .expect("insert project");
        id
    }

    #[test]
    fn try_acquire_running_exactly_one_winner_under_parallel_contention() {
        // 20 threads race to acquire the same (project, agent) lock.
        // Exactly one must return Ok(true); all others Ok(false).
        // The pre-0.32.9 is_locked() → spawn → upsert sequence had a
        // TOCTOU here; BEGIN IMMEDIATE closes it. This test is the
        // proof.
        let (dir, db_path) = scratch_db();
        let project_path = {
            let conn = open_conn(&db_path);
            make_project(&conn, "/tmp/proj-a")
        };

        let db_path = Arc::new(db_path);
        let project = Arc::new(project_path);
        let n_threads = 20usize;

        let handles: Vec<_> = (0..n_threads)
            .map(|tid| {
                let db_path = db_path.clone();
                let project = project.clone();
                std::thread::spawn(move || -> bool {
                    let conn = open_conn(&db_path);
                    AgentSession::try_acquire_running(
                        &conn,
                        &format!("session-{}", tid),
                        &project,
                        "agent-x",
                        None,
                        "claude",
                        "manager",
                    )
                    .expect("try_acquire_running")
                })
            })
            .collect();

        let results: Vec<bool> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let winners = results.iter().filter(|&&r| r).count();
        assert_eq!(
            winners, 1,
            "expected exactly 1 winner under contention, got {}: results={:?}",
            winners, results
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn try_acquire_running_different_agents_all_succeed() {
        // Sanity check: the CAS is per-agent, not global. 8 different
        // agents in the same project, each acquired by its own
        // thread, all should win.
        let (dir, db_path) = scratch_db();
        let project_path = {
            let conn = open_conn(&db_path);
            make_project(&conn, "/tmp/proj-b")
        };

        let db_path = Arc::new(db_path);
        let project = Arc::new(project_path);
        let n_agents = 8usize;

        let handles: Vec<_> = (0..n_agents)
            .map(|i| {
                let db_path = db_path.clone();
                let project = project.clone();
                std::thread::spawn(move || -> bool {
                    let conn = open_conn(&db_path);
                    AgentSession::try_acquire_running(
                        &conn,
                        &format!("session-a{}", i),
                        &project,
                        &format!("agent-{}", i),
                        None,
                        "claude",
                        "manager",
                    )
                    .expect("try_acquire_running")
                })
            })
            .collect();

        let results: Vec<bool> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let winners = results.iter().filter(|&&r| r).count();
        assert_eq!(winners, n_agents, "different agents should all win: {:?}", results);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn try_acquire_running_serializes_without_busy_errors() {
        // 5 rounds of 10 threads each. After each round the winner
        // releases the lock (status='stopped'). Next round must also
        // produce exactly one winner. Verifies that BEGIN IMMEDIATE
        // + busy_timeout doesn't surface SQLITE_BUSY as an error —
        // instead callers block on the write queue until they get
        // their turn.
        let (dir, db_path) = scratch_db();
        let project_path = {
            let conn = open_conn(&db_path);
            make_project(&conn, "/tmp/proj-c")
        };

        for round in 0..5 {
            let db_path = Arc::new(db_path.clone());
            let project = Arc::new(project_path.clone());

            let handles: Vec<_> = (0..10)
                .map(|tid| {
                    let db_path = db_path.clone();
                    let project = project.clone();
                    std::thread::spawn(move || -> bool {
                        let conn = open_conn(&db_path);
                        AgentSession::try_acquire_running(
                            &conn,
                            &format!("session-r{}-t{}", round, tid),
                            &project,
                            "agent-y",
                            None,
                            "claude",
                            "manager",
                        )
                        .expect("try_acquire_running should never error, only return false")
                    })
                })
                .collect();

            let winners: usize = handles
                .into_iter()
                .map(|h| h.join().unwrap() as usize)
                .sum();
            assert_eq!(winners, 1, "round {}: expected 1 winner, got {}", round, winners);

            // Release the lock so the next round has something to acquire.
            let conn = open_conn(&db_path);
            conn.execute(
                "UPDATE agent_sessions SET status='stopped' WHERE project_id=?1 AND agent_name=?2",
                params![project_path, "agent-y"],
            ).expect("release lock");
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn try_acquire_running_reacquires_after_release() {
        // Single-threaded correctness check: acquire → release →
        // re-acquire must all return Ok(true). Catches a regression
        // where the CAS could treat a released row as still held.
        let (dir, db_path) = scratch_db();
        let conn = open_conn(&db_path);
        let project = make_project(&conn, "/tmp/proj-d");

        let first = AgentSession::try_acquire_running(
            &conn, "s1", &project, "a1", None, "claude", "manager",
        )
        .unwrap();
        assert!(first, "first acquire should win");

        let second = AgentSession::try_acquire_running(
            &conn, "s2", &project, "a1", None, "claude", "manager",
        )
        .unwrap();
        assert!(!second, "second acquire (already held) should lose");

        // Release (status != 'running').
        conn.execute(
            "UPDATE agent_sessions SET status='stopped' WHERE project_id=?1 AND agent_name=?2",
            params![project, "a1"],
        )
        .unwrap();

        let third = AgentSession::try_acquire_running(
            &conn, "s3", &project, "a1", None, "claude", "manager",
        )
        .unwrap();
        assert!(third, "re-acquire after release should win");

        let _ = std::fs::remove_dir_all(&dir);
    }
}

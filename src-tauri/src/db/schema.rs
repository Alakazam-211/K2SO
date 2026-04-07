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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalTab {
    pub id: String,
    pub workspace_id: String,
    pub title: String,
    pub tab_order: i64,
    pub created_at: i64,
}

// ── Terminal Panes (stub) ───────────────────────────────────────────────────

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

/// DB-tracked agent session. Replaces .lock/.last_session filesystem tracking.
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
        conn.execute(
            "UPDATE agent_sessions SET status = ?1, last_activity_at = unixepoch() WHERE project_id = ?2 AND agent_name = ?3",
            params![status, project_id, agent_name],
        )
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
        conn.execute(
            "INSERT INTO activity_feed (project_id, agent_name, event_type, from_agent, to_agent, to_project_id, summary, metadata, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, unixepoch())",
            params![project_id, agent_name, event_type, from_agent, to_agent, to_project_id, summary, metadata],
        )?;
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

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
}

impl Project {
    pub fn list(conn: &Connection) -> Result<Vec<Project>> {
        let mut stmt = conn.prepare(
            "SELECT id, name, path, color, tab_order, last_opened_at, worktree_mode, icon_url, focus_group_id, pinned, manually_active, last_interaction_at, created_at \
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
            })
        })?;
        rows.collect()
    }

    pub fn get(conn: &Connection, id: &str) -> Result<Project> {
        conn.query_row(
            "SELECT id, name, path, color, tab_order, last_opened_at, worktree_mode, icon_url, focus_group_id, pinned, manually_active, last_interaction_at, created_at \
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
    ) -> Result<()> {
        if let Some(v) = name {
            conn.execute("UPDATE projects SET name = ?1 WHERE id = ?2", params![v, id])?;
        }
        if let Some(v) = path {
            conn.execute("UPDATE projects SET path = ?1 WHERE id = ?2", params![v, id])?;
        }
        if let Some(v) = color {
            conn.execute("UPDATE projects SET color = ?1 WHERE id = ?2", params![v, id])?;
        }
        if let Some(v) = tab_order {
            conn.execute("UPDATE projects SET tab_order = ?1 WHERE id = ?2", params![v, id])?;
        }
        if let Some(v) = worktree_mode {
            conn.execute("UPDATE projects SET worktree_mode = ?1 WHERE id = ?2", params![v, id])?;
        }
        if let Some(v) = icon_url {
            conn.execute("UPDATE projects SET icon_url = ?1 WHERE id = ?2", params![v, id])?;
        }
        if let Some(v) = focus_group_id {
            conn.execute("UPDATE projects SET focus_group_id = ?1 WHERE id = ?2", params![v, id])?;
        }
        if let Some(v) = pinned {
            conn.execute("UPDATE projects SET pinned = ?1 WHERE id = ?2", params![v, id])?;
        }
        if let Some(v) = manually_active {
            conn.execute("UPDATE projects SET manually_active = ?1 WHERE id = ?2", params![v, id])?;
        }
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
    pub created_at: i64,
}

impl Workspace {
    pub fn list(conn: &Connection, project_id: &str) -> Result<Vec<Workspace>> {
        let mut stmt = conn.prepare(
            "SELECT id, project_id, section_id, type, branch, name, tab_order, worktree_path, created_at \
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
                created_at: row.get(8)?,
            })
        })?;
        rows.collect()
    }

    pub fn get(conn: &Connection, id: &str) -> Result<Workspace> {
        conn.query_row(
            "SELECT id, project_id, section_id, type, branch, name, tab_order, worktree_path, created_at \
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
                    created_at: row.get(8)?,
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

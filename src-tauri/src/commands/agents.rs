use tauri::State;
use crate::db::schema::AgentPreset;
use crate::state::AppState;

// Built-in agent preset definitions for reset
const BUILT_IN_PRESETS: &[(&str, &str, &str, &str, i64)] = &[
    ("b0a1c2d3-e4f5-6789-abcd-ef0123456001", "Claude", "claude --dangerously-skip-permissions", "\u{1F916}", 0),
    ("b0a1c2d3-e4f5-6789-abcd-ef0123456002", "Codex", "codex -c model_reasoning_effort=\"high\" --dangerously-bypass-approvals-and-sandbox", "\u{1F98E}", 1),
    ("b0a1c2d3-e4f5-6789-abcd-ef0123456003", "Gemini", "gemini --yolo", "\u{1F48E}", 2),
    ("b0a1c2d3-e4f5-6789-abcd-ef0123456004", "Copilot", "copilot --allow-all", "\u{1F6F8}", 3),
    ("b0a1c2d3-e4f5-6789-abcd-ef0123456005", "Aider", "aider", "\u{1F6E0}", 4),
    ("b0a1c2d3-e4f5-6789-abcd-ef0123456006", "Cursor Agent", "cursor-agent", "\u{26A1}", 5),
    ("b0a1c2d3-e4f5-6789-abcd-ef0123456007", "OpenCode", "opencode", "\u{1F4DF}", 6),
    ("b0a1c2d3-e4f5-6789-abcd-ef0123456008", "Code Puppy", "codepuppy", "\u{1F436}", 7),
];

#[tauri::command]
pub fn presets_list(state: State<'_, AppState>) -> Result<Vec<AgentPreset>, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    AgentPreset::list(&conn).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn presets_create(
    state: State<'_, AppState>,
    label: String,
    command: String,
    icon: Option<String>,
) -> Result<AgentPreset, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let id = uuid::Uuid::new_v4().to_string();

    let existing = AgentPreset::list(&conn).unwrap_or_default();
    let max_order = existing.iter().map(|p| p.sort_order).max().unwrap_or(-1) + 1;

    AgentPreset::create(
        &conn, &id, &label, &command, icon.as_deref(), 1, max_order, 0,
    )
    .map_err(|e| e.to_string())?;

    AgentPreset::get(&conn, &id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn presets_update(
    state: State<'_, AppState>,
    id: String,
    label: Option<String>,
    command: Option<String>,
    icon: Option<String>,
    enabled: Option<i64>,
    sort_order: Option<i64>,
) -> Result<AgentPreset, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    AgentPreset::update(
        &conn,
        &id,
        label.as_deref(),
        command.as_deref(),
        icon.as_ref().map(|i| Some(i.as_str())),
        enabled,
        sort_order,
    )
    .map_err(|e| e.to_string())?;
    AgentPreset::get(&conn, &id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn presets_delete(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;

    // Prevent deleting built-in presets
    let preset = AgentPreset::get(&conn, &id).map_err(|e| e.to_string())?;
    if preset.is_built_in != 0 {
        return Err("Cannot delete built-in presets. Disable them instead.".to_string());
    }

    AgentPreset::delete(&conn, &id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn presets_reorder(state: State<'_, AppState>, ids: Vec<String>) -> Result<(), String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    for (i, id) in ids.iter().enumerate() {
        AgentPreset::update(&conn, id, None, None, None, None, Some(i as i64))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub fn presets_reset_built_ins(state: State<'_, AppState>) -> Result<Vec<AgentPreset>, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;

    // Delete all existing built-in presets and re-insert from catalog
    for (id, label, command, icon, sort_order) in BUILT_IN_PRESETS {
        conn.execute(
            "DELETE FROM agent_presets WHERE id = ?1",
            rusqlite::params![id],
        )
        .map_err(|e| e.to_string())?;

        AgentPreset::create(&conn, id, label, command, Some(icon), 1, *sort_order, 1)
            .map_err(|e| e.to_string())?;
    }

    AgentPreset::list(&conn).map_err(|e| e.to_string())
}

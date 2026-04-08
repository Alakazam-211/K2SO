use tauri::{AppHandle, Emitter, State};
use crate::db::schema::AgentPreset;
use crate::state::AppState;

// Built-in agent preset definitions for reset
const BUILT_IN_PRESETS: &[(&str, &str, &str, &str, i64)] = &[
    // Cloud CLI agents (no emoji — use custom AgentIcon SVGs)
    ("b0a1c2d3-e4f5-6789-abcd-ef0123456001", "Claude", "claude --dangerously-skip-permissions", "", 0),
    ("b0a1c2d3-e4f5-6789-abcd-ef0123456002", "Codex", "codex -c model_reasoning_effort=\"high\" --dangerously-bypass-approvals-and-sandbox", "", 1),
    ("b0a1c2d3-e4f5-6789-abcd-ef0123456003", "Gemini", "gemini --yolo", "", 2),
    ("b0a1c2d3-e4f5-6789-abcd-ef0123456004", "Copilot", "copilot --allow-all", "", 3),
    ("b0a1c2d3-e4f5-6789-abcd-ef0123456005", "Aider", "aider", "", 4),
    ("b0a1c2d3-e4f5-6789-abcd-ef0123456006", "Cursor Agent", "cursor-agent", "", 5),
    ("b0a1c2d3-e4f5-6789-abcd-ef0123456007", "OpenCode", "opencode", "", 6),
    ("b0a1c2d3-e4f5-6789-abcd-ef0123456008", "Code Puppy", "codepuppy", "", 7),
    ("b0a1c2d3-e4f5-6789-abcd-ef0123456009", "Goose", "goose", "", 8),
    ("b0a1c2d3-e4f5-6789-abcd-ef0123456010", "Pi", "pi", "", 9),
    // Local/on-device LLM tools (keep emoji — no custom icon)
    ("b0a1c2d3-e4f5-6789-abcd-ef0123456011", "Ollama", "ollama run llama3.2", "\u{1F999}", 10),
    ("b0a1c2d3-e4f5-6789-abcd-ef0123456012", "Interpreter", "interpreter", "\u{1F310}", 11),
];

#[tauri::command]
pub fn presets_list(state: State<'_, AppState>) -> Result<Vec<AgentPreset>, String> {
    let conn = state.db.lock();
    AgentPreset::list(&conn).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn presets_create(
    app: AppHandle,
    state: State<'_, AppState>,
    label: String,
    command: String,
    icon: Option<String>,
) -> Result<AgentPreset, String> {
    let conn = state.db.lock();
    let id = uuid::Uuid::new_v4().to_string();

    let existing = AgentPreset::list(&conn).unwrap_or_default();
    let max_order = existing.iter().map(|p| p.sort_order).max().unwrap_or(-1) + 1;

    AgentPreset::create(
        &conn, &id, &label, &command, icon.as_deref(), 1, max_order, 0,
    )
    .map_err(|e| e.to_string())?;

    let result = AgentPreset::get(&conn, &id).map_err(|e| e.to_string())?;
    let _ = app.emit("sync:presets", ());
    Ok(result)
}

#[tauri::command]
pub fn presets_update(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    label: Option<String>,
    command: Option<String>,
    icon: Option<String>,
    enabled: Option<i64>,
    sort_order: Option<i64>,
) -> Result<AgentPreset, String> {
    let conn = state.db.lock();
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
    let result = AgentPreset::get(&conn, &id).map_err(|e| e.to_string())?;
    let _ = app.emit("sync:presets", ());
    Ok(result)
}

#[tauri::command]
pub fn presets_delete(app: AppHandle, state: State<'_, AppState>, id: String) -> Result<(), String> {
    let conn = state.db.lock();

    // Prevent deleting built-in presets
    let preset = AgentPreset::get(&conn, &id).map_err(|e| e.to_string())?;
    if preset.is_built_in != 0 {
        return Err("Cannot delete built-in presets. Disable them instead.".to_string());
    }

    AgentPreset::delete(&conn, &id).map_err(|e| e.to_string())?;
    let _ = app.emit("sync:presets", ());
    Ok(())
}

#[tauri::command]
pub fn presets_reorder(app: AppHandle, state: State<'_, AppState>, ids: Vec<String>) -> Result<(), String> {
    let conn = state.db.lock();
    for (i, id) in ids.iter().enumerate() {
        AgentPreset::update(&conn, id, None, None, None, None, Some(i as i64))
            .map_err(|e| e.to_string())?;
    }
    let _ = app.emit("sync:presets", ());
    Ok(())
}

#[tauri::command]
pub fn presets_reset_built_ins(app: AppHandle, state: State<'_, AppState>) -> Result<Vec<AgentPreset>, String> {
    let conn = state.db.lock();

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

    let result = AgentPreset::list(&conn).map_err(|e| e.to_string())?;
    let _ = app.emit("sync:presets", ());
    Ok(result)
}

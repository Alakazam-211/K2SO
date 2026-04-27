use tauri::{AppHandle, Emitter, State};
use crate::db::schema::AgentPreset;
use crate::state::AppState;

// Built-in agent preset definitions for "reset to defaults".
// IDs and order MUST stay in sync with `crates/k2so-core/src/db/mod.rs::seed_agent_presets`.
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
    drop(conn);
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
    drop(conn);
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
    drop(conn);
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
    drop(conn);
    let _ = app.emit("sync:presets", ());
    Ok(())
}

#[tauri::command]
pub fn presets_reset_built_ins(app: AppHandle, state: State<'_, AppState>) -> Result<Vec<AgentPreset>, String> {
    let conn = state.db.lock();

    // Wipe ALL built-ins first, then re-seed from the canonical catalog.
    // Targeting `is_built_in = 1` (rather than enumerating ids) is what
    // lets us drop retired presets like Code Puppy and any rows that
    // ended up under stale ids from older versions where db/mod.rs and
    // agents.rs disagreed on the Pi/Goose/Ollama/Interpreter mapping.
    // Custom presets the user added are untouched.
    conn.execute(
        "DELETE FROM agent_presets WHERE is_built_in = 1",
        [],
    )
    .map_err(|e| e.to_string())?;

    for (id, label, command, icon, sort_order) in BUILT_IN_PRESETS {
        AgentPreset::create(&conn, id, label, command, Some(icon), 1, *sort_order, 1)
            .map_err(|e| e.to_string())?;
    }

    let result = AgentPreset::list(&conn).map_err(|e| e.to_string())?;
    drop(conn);
    let _ = app.emit("sync:presets", ());
    Ok(result)
}

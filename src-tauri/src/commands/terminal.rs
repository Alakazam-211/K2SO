use crate::state::AppState;
use crate::terminal_event_sink::TauriTerminalEventSink;
use tauri::State;
use uuid::Uuid;

#[tauri::command]
pub fn terminal_create(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    cwd: String,
    command: Option<String>,
    args: Option<Vec<String>>,
    cols: Option<u16>,
    rows: Option<u16>,
    id: Option<String>,
) -> Result<serde_json::Value, String> {
    let id = id.unwrap_or_else(|| Uuid::new_v4().to_string());

    log_debug!("[terminal] Creating terminal id={} cwd={} command={:?} size={}x{}", id, cwd, command, cols.unwrap_or(80), rows.unwrap_or(24));

    let event_sink = TauriTerminalEventSink::new(app);
    let mut manager = state
        .terminal_manager
        .lock();

    match manager.create(id.clone(), cwd, command, args, cols, rows, event_sink) {
        Ok(()) => {
            log_debug!("[terminal] Terminal {} created successfully", id);
            Ok(serde_json::json!({ "id": id }))
        }
        Err(e) => {
            log_debug!("[terminal] Terminal creation failed: {}", e);
            Err(e)
        }
    }
}

#[tauri::command]
pub fn terminal_write(
    state: State<'_, AppState>,
    id: String,
    data: String,
) -> Result<(), String> {
    state
        .terminal_manager
        .lock()
        .write(&id, &data)
}

#[tauri::command]
pub fn terminal_resize(
    state: State<'_, AppState>,
    id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    state
        .terminal_manager
        .lock()
        .resize(&id, cols, rows)
}

#[tauri::command]
pub fn terminal_kill(
    state: State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    state
        .terminal_manager
        .lock()
        .kill(&id)
}

#[tauri::command]
pub fn terminal_active_count_for_path(
    state: State<'_, AppState>,
    path: String,
) -> Result<i32, String> {
    Ok(state
        .terminal_manager
        .lock()
        .get_count_for_path(&path))
}

/// Kill only the foreground process in a terminal (like Ctrl+C),
/// without killing the shell itself.
#[cfg(unix)]
#[tauri::command]
pub fn terminal_kill_foreground(
    state: State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    state
        .terminal_manager
        .lock()
        .kill_foreground(&id)
}

/// Get the name of the foreground process in a terminal.
#[cfg(unix)]
#[tauri::command]
pub fn terminal_get_foreground_command(
    state: State<'_, AppState>,
    id: String,
) -> Result<Option<String>, String> {
    state
        .terminal_manager
        .lock()
        .get_foreground_command(&id)
}

#[tauri::command]
pub fn terminal_log(message: String) -> Result<(), String> {
    log_debug!("{}", message);
    Ok(())
}

#[tauri::command]
pub fn terminal_exists(
    state: State<'_, AppState>,
    id: String,
) -> Result<bool, String> {
    Ok(state
        .terminal_manager
        .lock()
        .exists(&id))
}

#[tauri::command]
pub fn terminal_get_grid(
    state: State<'_, AppState>,
    id: String,
) -> Result<crate::terminal::grid_types::GridUpdate, String> {
    state
        .terminal_manager
        .lock()
        .get_grid(&id)
}

#[tauri::command]
pub fn terminal_scroll(
    state: State<'_, AppState>,
    id: String,
    delta: i32,
) -> Result<(), String> {
    state
        .terminal_manager
        .lock()
        .scroll(&id, delta)
}

#[tauri::command]
pub fn terminal_set_font_size(
    state: State<'_, AppState>,
    id: String,
    font_size: f32,
    dpr: f32,
) -> Result<serde_json::Value, String> {
    let (cw, ch) = state
        .terminal_manager
        .lock()
        .set_font_size(&id, font_size, dpr)?;
    Ok(serde_json::json!({ "cell_width": cw, "cell_height": ch }))
}

#[tauri::command]
pub fn terminal_get_cell_metrics(
    state: State<'_, AppState>,
    id: String,
) -> Result<serde_json::Value, String> {
    let (cw, ch, cols, rows) = state
        .terminal_manager
        .lock()
        .get_cell_metrics(&id)?;
    Ok(serde_json::json!({ "cell_width": cw, "cell_height": ch, "cols": cols, "rows": rows }))
}

#[tauri::command]
pub fn terminal_set_focus(
    state: State<'_, AppState>,
    id: String,
    focused: bool,
) -> Result<(), String> {
    state
        .terminal_manager
        .lock()
        .set_focus(&id, focused)
}

#[tauri::command]
pub fn terminal_get_selection_text(
    state: State<'_, AppState>,
    id: String,
    start_col: u16,
    start_row: u16,
    end_col: u16,
    end_row: u16,
) -> Result<String, String> {
    state
        .terminal_manager
        .lock()
        .get_selection_text(&id, start_col, start_row, end_col, end_row)
}

/// Read the last N lines of text from the terminal buffer.
#[tauri::command]
pub fn terminal_read_lines(
    state: State<'_, AppState>,
    id: String,
    count: Option<usize>,
) -> Result<Vec<String>, String> {
    state
        .terminal_manager
        .lock()
        .read_lines(&id, count.unwrap_or(50))
}

/// List all running terminals with their foreground command (if a CLI LLM).
#[tauri::command]
pub fn terminal_list_running_agents(
    state: State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    let manager = state.terminal_manager.lock();
    let terminal_ids = manager.list_terminal_ids();
    let mut agents = Vec::new();

    for (id, cwd) in &terminal_ids {
        let command = manager.get_foreground_command(id).ok().flatten();
        agents.push(serde_json::json!({
            "terminalId": id,
            "cwd": cwd,
            "command": command,
        }));
    }

    Ok(serde_json::json!(agents))
}

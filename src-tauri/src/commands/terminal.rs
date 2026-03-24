use crate::state::AppState;
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

    eprintln!("[terminal] Creating terminal id={} cwd={} command={:?} size={}x{}", id, cwd, command, cols.unwrap_or(80), rows.unwrap_or(24));

    let mut manager = state
        .terminal_manager
        .lock()
        .map_err(|e| format!("Lock error: {}", e))?;

    match manager.create(id.clone(), cwd, command, args, cols, rows, app) {
        Ok(()) => {
            eprintln!("[terminal] Terminal {} created successfully", id);
            Ok(serde_json::json!({ "id": id }))
        }
        Err(e) => {
            eprintln!("[terminal] Terminal creation failed: {}", e);
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
        .map_err(|e| format!("Lock error: {}", e))?
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
        .map_err(|e| format!("Lock error: {}", e))?
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
        .map_err(|e| format!("Lock error: {}", e))?
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
        .map_err(|e| format!("Lock error: {}", e))?
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
        .map_err(|e| format!("Lock error: {}", e))?
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
        .map_err(|e| format!("Lock error: {}", e))?
        .get_foreground_command(&id)
}

#[tauri::command]
pub fn terminal_log(message: String) -> Result<(), String> {
    eprintln!("{}", message);
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
        .map_err(|e| format!("Lock error: {}", e))?
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
        .map_err(|e| format!("Lock error: {}", e))?
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
        .map_err(|e| format!("Lock error: {}", e))?
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
        .map_err(|e| format!("Lock error: {}", e))?
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
        .map_err(|e| format!("Lock error: {}", e))?
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
        .map_err(|e| format!("Lock error: {}", e))?
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
        .map_err(|e| format!("Lock error: {}", e))?
        .get_selection_text(&id, start_col, start_row, end_col, end_row)
}

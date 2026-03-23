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
/// Returns None if only the shell is running.
/// Used to detect active AI agents before closing tabs/app.
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

/// Log a message from the frontend to Rust stderr (for debugging).
#[tauri::command]
pub fn terminal_log(message: String) -> Result<(), String> {
    eprintln!("{}", message);
    Ok(())
}

/// Check if a terminal with the given ID exists and is alive.
/// Used by the frontend to decide whether to create a new terminal
/// or reattach to an existing one after a tab switch.
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

/// Returns which terminal backend is active: "legacy" or "alacritty".
#[tauri::command]
pub fn terminal_get_backend() -> String {
    crate::terminal::backend_name().to_string()
}

/// Get the scrollback buffer for a terminal. Called when reattaching
/// to replay output that occurred while the terminal tab was inactive.
/// Only used by the legacy (xterm.js) backend.
#[tauri::command]
pub fn terminal_get_buffer(
    state: State<'_, AppState>,
    id: String,
) -> Result<String, String> {
    state
        .terminal_manager
        .lock()
        .map_err(|e| format!("Lock error: {}", e))?
        .get_buffer(&id)
}

/// Get a full grid snapshot for a terminal (alacritty backend only).
/// Returns a GridUpdate with all lines for the current viewport.
/// Used for reattach on tab switch — replaces get_buffer for the new backend.
#[cfg(feature = "alacritty-backend")]
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

#[cfg(not(feature = "alacritty-backend"))]
#[tauri::command]
pub fn terminal_get_grid(
    _state: State<'_, AppState>,
    _id: String,
) -> Result<serde_json::Value, String> {
    Err("terminal_get_grid is only available with the alacritty backend".to_string())
}

/// Scroll the terminal display (alacritty backend only).
/// Delta > 0 scrolls up (into history), delta < 0 scrolls down.
#[cfg(feature = "alacritty-backend")]
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

#[cfg(not(feature = "alacritty-backend"))]
#[tauri::command]
pub fn terminal_scroll(
    _state: State<'_, AppState>,
    _id: String,
    _delta: i32,
) -> Result<(), String> {
    Ok(())
}

// ── Bitmap rendering commands (alacritty backend only) ───────────────────

/// Get a full bitmap frame for tab-switch reattach.
#[cfg(feature = "alacritty-backend")]
#[tauri::command]
pub fn terminal_get_frame(
    state: State<'_, AppState>,
    id: String,
) -> Result<crate::terminal::grid_types::BitmapUpdate, String> {
    state
        .terminal_manager
        .lock()
        .map_err(|e| format!("Lock error: {}", e))?
        .get_frame(&id)
}

#[cfg(not(feature = "alacritty-backend"))]
#[tauri::command]
pub fn terminal_get_frame(
    _state: State<'_, AppState>,
    _id: String,
) -> Result<serde_json::Value, String> {
    Err("terminal_get_frame requires alacritty backend".to_string())
}

/// Set font size and DPR. Returns { cell_width, cell_height }.
#[cfg(feature = "alacritty-backend")]
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

#[cfg(not(feature = "alacritty-backend"))]
#[tauri::command]
pub fn terminal_set_font_size(
    _state: State<'_, AppState>,
    _id: String,
    _font_size: f32,
    _dpr: f32,
) -> Result<serde_json::Value, String> {
    Ok(serde_json::json!({ "cell_width": 8, "cell_height": 16 }))
}

/// Get cell metrics for mouse coordinate mapping.
#[cfg(feature = "alacritty-backend")]
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

#[cfg(not(feature = "alacritty-backend"))]
#[tauri::command]
pub fn terminal_get_cell_metrics(
    _state: State<'_, AppState>,
    _id: String,
) -> Result<serde_json::Value, String> {
    Ok(serde_json::json!({ "cell_width": 8, "cell_height": 16, "cols": 80, "rows": 24 }))
}

/// Set terminal focus state (controls cursor blink).
#[cfg(feature = "alacritty-backend")]
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

#[cfg(not(feature = "alacritty-backend"))]
#[tauri::command]
pub fn terminal_set_focus(
    _state: State<'_, AppState>,
    _id: String,
    _focused: bool,
) -> Result<(), String> {
    Ok(())
}

/// Get text content from a selection range.
#[cfg(feature = "alacritty-backend")]
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

#[cfg(not(feature = "alacritty-backend"))]
#[tauri::command]
pub fn terminal_get_selection_text(
    _state: State<'_, AppState>,
    _id: String,
    _start_col: u16,
    _start_row: u16,
    _end_col: u16,
    _end_row: u16,
) -> Result<String, String> {
    Ok(String::new())
}

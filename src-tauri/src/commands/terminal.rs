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

/// Get the scrollback buffer for a terminal. Called when reattaching
/// to replay output that occurred while the terminal tab was inactive.
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

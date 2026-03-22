use crate::state::AppState;
use serde::{Deserialize, Serialize};
use tauri::Manager;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowState {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub is_maximized: bool,
}

pub fn save_window_state(app: &tauri::AppHandle) {
    let Some(win) = app.get_webview_window("main") else {
        return;
    };

    let is_maximized = win.is_maximized().unwrap_or(false);

    // Don't save position/size when maximized — keep the last windowed geometry
    if is_maximized {
        // Just update is_maximized flag if row already exists
        if let Some(state) = app.try_state::<AppState>() {
            if let Ok(db) = state.db.lock() {
                let _ = db.execute(
                    "UPDATE window_state SET is_maximized = 1, updated_at = unixepoch() WHERE id = 1",
                    [],
                );
            }
        }
        return;
    }

    let position = match win.outer_position() {
        Ok(p) => p,
        Err(_) => return,
    };
    let size = match win.outer_size() {
        Ok(s) => s,
        Err(_) => return,
    };

    if let Some(state) = app.try_state::<AppState>() {
        if let Ok(db) = state.db.lock() {
            let _ = db.execute(
                "INSERT INTO window_state (id, x, y, width, height, is_maximized, updated_at)
                 VALUES (1, ?1, ?2, ?3, ?4, ?5, unixepoch())
                 ON CONFLICT(id) DO UPDATE SET
                   x = excluded.x, y = excluded.y,
                   width = excluded.width, height = excluded.height,
                   is_maximized = excluded.is_maximized,
                   updated_at = unixepoch()",
                rusqlite::params![position.x, position.y, size.width, size.height, is_maximized as i32],
            );
        }
    }
}

pub fn load_window_state(app: &tauri::AppHandle) -> Option<WindowState> {
    let state = app.try_state::<AppState>()?;
    let db = state.db.lock().ok()?;

    db.query_row(
        "SELECT x, y, width, height, is_maximized FROM window_state WHERE id = 1",
        [],
        |row| {
            Ok(WindowState {
                x: row.get(0)?,
                y: row.get(1)?,
                width: row.get(2)?,
                height: row.get(3)?,
                is_maximized: row.get::<_, i32>(4)? != 0,
            })
        },
    )
    .ok()
}

/// Migrate from the old JSON-based window state file to SQLite.
/// Called once during app setup. Reads ~/.k2so/window-state.json,
/// inserts into the DB, then deletes the JSON file.
pub fn migrate_json_window_state(app: &tauri::AppHandle) {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return,
    };
    let json_path = home.join(".k2so").join("window-state.json");

    if !json_path.exists() {
        return;
    }

    // Read old JSON state
    let data = match std::fs::read_to_string(&json_path) {
        Ok(d) => d,
        Err(_) => return,
    };
    let old_state: WindowState = match serde_json::from_str(&data) {
        Ok(s) => s,
        Err(_) => {
            // Invalid JSON — just delete it
            let _ = std::fs::remove_file(&json_path);
            return;
        }
    };

    // Write to SQLite
    if let Some(state) = app.try_state::<AppState>() {
        if let Ok(db) = state.db.lock() {
            let _ = db.execute(
                "INSERT INTO window_state (id, x, y, width, height, is_maximized, updated_at)
                 VALUES (1, ?1, ?2, ?3, ?4, ?5, unixepoch())
                 ON CONFLICT(id) DO UPDATE SET
                   x = excluded.x, y = excluded.y,
                   width = excluded.width, height = excluded.height,
                   is_maximized = excluded.is_maximized,
                   updated_at = unixepoch()",
                rusqlite::params![
                    old_state.x,
                    old_state.y,
                    old_state.width,
                    old_state.height,
                    old_state.is_maximized as i32
                ],
            );
        }
    }

    // Remove old JSON file
    let _ = std::fs::remove_file(&json_path);
    eprintln!("[window] Migrated window state from JSON to SQLite");
}

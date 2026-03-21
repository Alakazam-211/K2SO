use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use tauri::Manager;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowState {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub is_maximized: bool,
}

fn state_file_path() -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    home.join(".k2so").join("window-state.json")
}

pub fn save_window_state(app: &tauri::AppHandle) {
    let Some(win) = app.get_webview_window("main") else {
        return;
    };

    let is_maximized = win.is_maximized().unwrap_or(false);

    // Don't save position/size when maximized — keep the last windowed geometry
    if is_maximized {
        // Just update is_maximized flag if file already exists
        if let Some(mut existing) = load_window_state() {
            existing.is_maximized = true;
            write_state(&existing);
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

    let state = WindowState {
        x: position.x,
        y: position.y,
        width: size.width,
        height: size.height,
        is_maximized,
    };

    write_state(&state);
}

fn write_state(state: &WindowState) {
    let path = state_file_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(state) {
        let _ = fs::write(&path, json);
    }
}

pub fn load_window_state() -> Option<WindowState> {
    let path = state_file_path();
    let data = fs::read_to_string(path).ok()?;
    serde_json::from_str(&data).ok()
}

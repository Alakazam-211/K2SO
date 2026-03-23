use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use tauri::{AppHandle, Emitter};

// ── Settings types ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalSettings {
    pub font_family: String,
    pub font_size: u32,
    pub cursor_style: String,
    pub scrollback: u32,
    #[serde(default = "default_natural_text_editing")]
    pub natural_text_editing: bool,
}

fn default_natural_text_editing() -> bool {
    true
}

fn default_agent() -> String {
    "claude".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimerSettings {
    #[serde(default = "default_true")]
    pub visible: bool,
    #[serde(default = "default_true")]
    pub countdown_enabled: bool,
    #[serde(default = "default_countdown_theme")]
    pub countdown_theme: String,
    #[serde(default)]
    pub skip_memo: bool,
    #[serde(default)]
    pub timezone: String,
    #[serde(default)]
    pub custom_themes: Vec<serde_json::Value>,
}

fn default_true() -> bool {
    true
}

fn default_countdown_theme() -> String {
    "rocket".to_string()
}

impl Default for TimerSettings {
    fn default() -> Self {
        Self {
            visible: true,
            countdown_enabled: true,
            countdown_theme: "rocket".to_string(),
            skip_memo: false,
            timezone: String::new(),
            custom_themes: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub terminal: TerminalSettings,
    pub keybindings: HashMap<String, String>,
    pub project_settings: HashMap<String, serde_json::Value>,
    pub focus_groups_enabled: bool,
    pub sidebar_collapsed: bool,
    pub left_panel_open: bool,
    pub right_panel_open: bool,
    #[serde(default)]
    pub workspace_layouts: HashMap<String, serde_json::Value>,
    #[serde(default = "default_agent")]
    pub default_agent: String,
    #[serde(default)]
    pub timer: TimerSettings,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            terminal: TerminalSettings {
                font_family: "MesloLGM Nerd Font".to_string(),
                font_size: 13,
                cursor_style: "bar".to_string(),
                scrollback: 5000,
                natural_text_editing: true,
            },
            keybindings: HashMap::new(),
            project_settings: HashMap::new(),
            focus_groups_enabled: false,
            sidebar_collapsed: false,
            left_panel_open: false,
            right_panel_open: false,
            workspace_layouts: HashMap::new(),
            default_agent: "claude".to_string(),
            timer: TimerSettings::default(),
        }
    }
}

// ── File I/O helpers ────────────────────────────────────────────────────

fn settings_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| {
            eprintln!("[settings] WARNING: Could not determine home directory, using current dir");
            PathBuf::from(".")
        })
        .join(".k2so")
}

fn settings_file() -> PathBuf {
    settings_dir().join("settings.json")
}

fn ensure_dir() {
    let dir = settings_dir();
    if !dir.exists() {
        fs::create_dir_all(&dir).ok();
    }
}

fn deep_merge(base: &mut serde_json::Value, overlay: &serde_json::Value) {
    if let (Some(base_map), Some(overlay_map)) = (base.as_object_mut(), overlay.as_object()) {
        for (key, val) in overlay_map {
            if val.is_object() && base_map.get(key).map_or(false, |v| v.is_object()) {
                let mut base_val = base_map.get(key).unwrap().clone();
                deep_merge(&mut base_val, val);
                base_map.insert(key.clone(), base_val);
            } else {
                base_map.insert(key.clone(), val.clone());
            }
        }
    }
}

fn read_settings() -> AppSettings {
    ensure_dir();
    let file = settings_file();
    if !file.exists() {
        return AppSettings::default();
    }
    match fs::read_to_string(&file) {
        Ok(raw) => {
            match serde_json::from_str::<serde_json::Value>(&raw) {
                Ok(parsed) => {
                    // Deep merge with defaults so new keys are always present
                    let mut defaults = serde_json::to_value(AppSettings::default()).unwrap();
                    deep_merge(&mut defaults, &parsed);
                    serde_json::from_value(defaults).unwrap_or_default()
                }
                Err(_) => AppSettings::default(),
            }
        }
        Err(_) => AppSettings::default(),
    }
}

fn write_settings(settings: &AppSettings) {
    ensure_dir();
    let file = settings_file();
    let tmp = file.with_extension("json.tmp");
    if let Ok(json) = serde_json::to_string_pretty(settings) {
        if fs::write(&tmp, &json).is_ok() {
            fs::rename(&tmp, &file).ok();
        }
    }
}

// ── Tauri Commands ──────────────────────────────────────────────────────

#[tauri::command]
pub fn settings_get() -> Result<AppSettings, String> {
    Ok(read_settings())
}

#[tauri::command]
pub fn settings_update(app: AppHandle, updates: serde_json::Value) -> Result<AppSettings, String> {
    let current = read_settings();
    let mut current_val = serde_json::to_value(&current).map_err(|e| e.to_string())?;
    deep_merge(&mut current_val, &updates);
    let merged: AppSettings = serde_json::from_value(current_val).map_err(|e| e.to_string())?;
    write_settings(&merged);
    let _ = app.emit("sync:settings", ());
    Ok(merged)
}

#[tauri::command]
pub fn settings_reset(app: AppHandle) -> Result<AppSettings, String> {
    let defaults = AppSettings::default();
    write_settings(&defaults);
    let _ = app.emit("sync:settings", ());
    Ok(defaults)
}

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter};

// ── Settings types ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalSettings {
    #[serde(default = "default_font_family")]
    pub font_family: String,
    #[serde(default = "default_font_size")]
    pub font_size: u32,
    #[serde(default = "default_cursor_style")]
    pub cursor_style: String,
    #[serde(default = "default_scrollback")]
    pub scrollback: u32,
    #[serde(default = "default_true")]
    pub natural_text_editing: bool,
}

impl Default for TerminalSettings {
    fn default() -> Self {
        default_terminal()
    }
}

fn default_font_family() -> String { "MesloLGM Nerd Font".to_string() }
fn default_font_size() -> u32 { 13 }
fn default_cursor_style() -> String { "bar".to_string() }
fn default_scrollback() -> u32 { 5000 }

fn default_agent() -> String {
    "claude".to_string()
}

fn default_left_panel_tab() -> String { "files".to_string() }
fn default_right_panel_tab() -> String { "history".to_string() }
fn default_left_panel_tabs() -> Vec<String> { vec!["files".to_string(), "agents".to_string()] }
fn default_right_panel_tabs() -> Vec<String> { vec!["history".to_string(), "changes".to_string(), "reviews".to_string()] }

fn default_terminal() -> TerminalSettings {
    TerminalSettings {
        font_family: "MesloLGM Nerd Font".to_string(),
        font_size: 13,
        cursor_style: "bar".to_string(),
        scrollback: 5000,
        natural_text_editing: true,
    }
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
    #[serde(default = "default_terminal")]
    pub terminal: TerminalSettings,
    #[serde(default)]
    pub keybindings: HashMap<String, String>,
    #[serde(default, alias = "projects")]
    pub project_settings: HashMap<String, serde_json::Value>,
    #[serde(default)]
    pub focus_groups_enabled: bool,
    #[serde(default)]
    pub active_focus_group_id: Option<String>,
    #[serde(default)]
    pub sidebar_collapsed: bool,
    #[serde(default)]
    pub left_panel_open: bool,
    #[serde(default)]
    pub right_panel_open: bool,
    #[serde(default = "default_left_panel_tab")]
    pub left_panel_active_tab: String,
    #[serde(default = "default_right_panel_tab")]
    pub right_panel_active_tab: String,
    #[serde(default = "default_left_panel_tabs")]
    pub left_panel_tabs: Vec<String>,
    #[serde(default = "default_right_panel_tabs")]
    pub right_panel_tabs: Vec<String>,
    /// Deprecated: workspace layouts now stored in SQLite workspace_sessions table.
    /// Kept for deserialization compat with old settings.json files; skipped on write.
    #[serde(default, skip_serializing)]
    pub workspace_layouts: HashMap<String, serde_json::Value>,
    #[serde(default = "default_agent")]
    pub default_agent: String,
    #[serde(default)]
    pub ai_assistant_enabled: bool,
    #[serde(default)]
    pub timer: TimerSettings,
    #[serde(default)]
    pub agentic_systems_enabled: bool,
    #[serde(default)]
    pub claude_auth_auto_refresh: bool,
    #[serde(default)]
    pub last_active_project_id: Option<String>,
    #[serde(default)]
    pub last_active_workspace_id: Option<String>,
    #[serde(default)]
    pub editor: EditorSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditorSettings {
    #[serde(default = "default_tab_size")]
    pub tab_size: u32,
    #[serde(default)]
    pub word_wrap: bool,
    #[serde(default)]
    pub show_whitespace: bool,
    #[serde(default = "default_editor_font_size")]
    pub font_size: u32,
    #[serde(default = "default_true")]
    pub indent_guides: bool,
    #[serde(default = "default_true")]
    pub fold_gutter: bool,
    #[serde(default = "default_true")]
    pub autocomplete: bool,
    #[serde(default = "default_true")]
    pub bracket_matching: bool,
    #[serde(default = "default_true")]
    pub line_numbers: bool,
    #[serde(default = "default_true")]
    pub highlight_active_line: bool,
    // Phase 6
    #[serde(default)]
    pub sticky_scroll: bool,
    #[serde(default)]
    pub minimap: bool,
    // Phase 7
    #[serde(default = "default_editor_theme")]
    pub theme: String,
    #[serde(default = "default_editor_font_family")]
    pub font_family: String,
    #[serde(default)]
    pub font_ligatures: bool,
    #[serde(default = "default_cursor_style")]
    pub cursor_style: String,
    #[serde(default = "default_true")]
    pub cursor_blink: bool,
    // Phase 8
    #[serde(default)]
    pub scroll_past_end: bool,
    #[serde(default = "default_true")]
    pub scrollbar_annotations: bool,
    #[serde(default = "default_diff_style")]
    pub diff_style: String,
    #[serde(default)]
    pub format_on_save: bool,
    #[serde(default)]
    pub vim_mode: bool,
}

fn default_tab_size() -> u32 { 2 }
fn default_editor_font_size() -> u32 { 12 }
fn default_diff_style() -> String { "gutter".to_string() }
fn default_editor_theme() -> String { "k2so-dark".to_string() }
fn default_editor_font_family() -> String { "MesloLGM Nerd Font".to_string() }

impl Default for EditorSettings {
    fn default() -> Self {
        Self {
            tab_size: 2,
            word_wrap: false,
            show_whitespace: false,
            font_size: 12,
            indent_guides: true,
            fold_gutter: true,
            autocomplete: true,
            bracket_matching: true,
            line_numbers: true,
            highlight_active_line: true,
            sticky_scroll: false,
            minimap: false,
            theme: "k2so-dark".to_string(),
            font_family: "MesloLGM Nerd Font".to_string(),
            font_ligatures: false,
            cursor_style: "bar".to_string(),
            cursor_blink: true,
            scroll_past_end: false,
            scrollbar_annotations: true,
            diff_style: "gutter".to_string(),
            format_on_save: false,
            vim_mode: false,
        }
    }
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            terminal: default_terminal(),
            keybindings: HashMap::new(),
            project_settings: HashMap::new(),
            focus_groups_enabled: false,
            active_focus_group_id: None,
            sidebar_collapsed: false,
            left_panel_open: false,
            right_panel_open: false,
            left_panel_active_tab: default_left_panel_tab(),
            right_panel_active_tab: default_right_panel_tab(),
            left_panel_tabs: default_left_panel_tabs(),
            right_panel_tabs: default_right_panel_tabs(),
            workspace_layouts: HashMap::new(),
            default_agent: "claude".to_string(),
            ai_assistant_enabled: false,
            timer: TimerSettings::default(),
            agentic_systems_enabled: false,
            claude_auth_auto_refresh: false,
            last_active_project_id: None,
            last_active_workspace_id: None,
            editor: EditorSettings::default(),
        }
    }
}

// ── File I/O helpers ────────────────────────────────────────────────────

fn settings_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| {
            log_debug!("[settings] WARNING: Could not determine home directory, using current dir");
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
                Ok(mut parsed) => {
                    // Migrate legacy "projects" key to "projectSettings"
                    if let Some(obj) = parsed.as_object_mut() {
                        if obj.contains_key("projects") && !obj.contains_key("projectSettings") {
                            if let Some(v) = obj.remove("projects") {
                                obj.insert("projectSettings".to_string(), v);
                            }
                        }
                    }
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

// ── CLI Install ────────────────────────────────────────────────────────

/// Find the bundled cli/k2so script (production or development).
fn find_cli_script() -> Option<PathBuf> {
    let exe_path = std::env::current_exe().ok()?;
    let macos_dir = exe_path.parent()?;

    // Production: K2SO.app/Contents/MacOS/k2so → Contents/Resources/_up_/cli/k2so
    // Tauri puts "../cli/*" resources under Resources/_up_/cli/
    let resources_cli = macos_dir.parent()
        .map(|contents| contents.join("Resources").join("_up_").join("cli").join("k2so"));
    if let Some(ref p) = resources_cli {
        if p.exists() { return resources_cli; }
    }

    // Development: target/debug/k2so → ../../cli/k2so
    let dev_cli = macos_dir.parent()
        .and_then(|p| p.parent())
        .map(|repo| repo.join("cli").join("k2so"));
    if let Some(ref p) = dev_cli {
        if p.exists() { return dev_cli; }
    }

    None
}

const CLI_SYMLINK_PATH: &str = "/usr/local/bin/k2so";

#[tauri::command]
pub fn cli_install_status() -> Result<serde_json::Value, String> {
    let symlink_path = Path::new(CLI_SYMLINK_PATH);
    let installed = symlink_path.exists() || symlink_path.is_symlink();
    let target = if installed {
        fs::read_link(symlink_path).ok().map(|p| p.to_string_lossy().to_string())
    } else {
        None
    };
    let bundled = find_cli_script().map(|p| p.to_string_lossy().to_string());

    Ok(serde_json::json!({
        "installed": installed,
        "symlinkPath": CLI_SYMLINK_PATH,
        "target": target,
        "bundledPath": bundled,
    }))
}

#[tauri::command]
pub fn cli_install() -> Result<String, String> {
    let cli_script = find_cli_script()
        .ok_or_else(|| "CLI script not found in app bundle".to_string())?;

    // Ensure the script is executable
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&cli_script, fs::Permissions::from_mode(0o755));
    }

    let symlink_path = Path::new(CLI_SYMLINK_PATH);

    // Check if /usr/local/bin exists and is writable
    let bin_dir = symlink_path.parent().unwrap();
    if !bin_dir.exists() {
        // Try to create /usr/local/bin via osascript (prompts for password)
        let output = std::process::Command::new("osascript")
            .args(["-e", &format!(
                "do shell script \"mkdir -p {}\" with administrator privileges",
                bin_dir.display()
            )])
            .output()
            .map_err(|e| format!("Failed to create {}: {}", bin_dir.display(), e))?;
        if !output.status.success() {
            return Err(format!("Failed to create {}: {}", bin_dir.display(),
                String::from_utf8_lossy(&output.stderr)));
        }
    }

    // Try direct symlink first (works if user owns /usr/local/bin)
    let _ = fs::remove_file(symlink_path);
    #[cfg(unix)]
    {
        if std::os::unix::fs::symlink(&cli_script, symlink_path).is_ok() {
            return Ok(CLI_SYMLINK_PATH.to_string());
        }
    }

    // Fall back to osascript with admin privileges
    let script = format!(
        "do shell script \"ln -sf '{}' '{}'\" with administrator privileges",
        cli_script.display(),
        CLI_SYMLINK_PATH
    );
    let output = std::process::Command::new("osascript")
        .args(["-e", &script])
        .output()
        .map_err(|e| format!("Failed to create symlink: {}", e))?;

    if !output.status.success() {
        return Err(format!("Failed to install CLI: {}",
            String::from_utf8_lossy(&output.stderr)));
    }

    Ok(CLI_SYMLINK_PATH.to_string())
}

#[tauri::command]
pub fn cli_uninstall() -> Result<(), String> {
    let symlink_path = Path::new(CLI_SYMLINK_PATH);
    if !symlink_path.exists() && !symlink_path.is_symlink() {
        return Ok(());
    }

    // Try direct remove first
    if fs::remove_file(symlink_path).is_ok() {
        return Ok(());
    }

    // Fall back to osascript with admin privileges
    let script = format!(
        "do shell script \"rm -f '{}'\" with administrator privileges",
        CLI_SYMLINK_PATH
    );
    let output = std::process::Command::new("osascript")
        .args(["-e", &script])
        .output()
        .map_err(|e| format!("Failed to remove symlink: {}", e))?;

    if !output.status.success() {
        return Err(format!("Failed to uninstall CLI: {}",
            String::from_utf8_lossy(&output.stderr)));
    }

    Ok(())
}

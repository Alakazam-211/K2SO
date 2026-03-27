use serde::Serialize;
use std::fs;
use std::path::PathBuf;

fn themes_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_default()
        .join(".k2so")
        .join("themes")
}

#[tauri::command]
pub fn get_themes_dir() -> Result<String, String> {
    let dir = themes_dir();
    Ok(dir.to_string_lossy().to_string())
}

#[tauri::command]
pub fn themes_ensure_dir() -> Result<String, String> {
    let dir = themes_dir();
    fs::create_dir_all(&dir).map_err(|e| format!("Failed to create themes dir: {e}"))?;
    Ok(dir.to_string_lossy().to_string())
}

/// Creates a template theme JSON file pre-populated with the given base theme JSON.
/// Returns the absolute path to the new file.
#[tauri::command]
pub fn themes_create_template(base_theme_json: String) -> Result<String, String> {
    let dir = themes_dir();
    fs::create_dir_all(&dir).map_err(|e| format!("Failed to create themes dir: {e}"))?;

    // Generate a unique filename
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let filename = format!("custom-theme-{timestamp}.json");
    let path = dir.join(&filename);

    // If the base JSON is empty or invalid, use a sensible default
    let content = if base_theme_json.trim().is_empty() {
        default_theme_json()
    } else {
        // Validate it's parseable JSON, fall back to default if not
        match serde_json::from_str::<serde_json::Value>(&base_theme_json) {
            Ok(_) => base_theme_json,
            Err(_) => default_theme_json(),
        }
    };

    fs::write(&path, content).map_err(|e| format!("Failed to write theme file: {e}"))?;

    Ok(path.to_string_lossy().to_string())
}

#[derive(Debug, Serialize)]
pub struct CustomThemeEntry {
    pub path: String,
    pub name: String,
    pub valid: bool,
}

/// Scans ~/.k2so/themes/*.json and returns metadata for each theme file.
#[tauri::command]
pub fn themes_list_custom() -> Result<Vec<CustomThemeEntry>, String> {
    let dir = themes_dir();
    if !dir.exists() {
        return Ok(vec![]);
    }

    let mut entries = Vec::new();
    let read_dir = fs::read_dir(&dir).map_err(|e| format!("Failed to read themes dir: {e}"))?;

    for entry in read_dir.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }

        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => {
                entries.push(CustomThemeEntry {
                    path: path.to_string_lossy().to_string(),
                    name: path.file_stem().unwrap_or_default().to_string_lossy().to_string(),
                    valid: false,
                });
                continue;
            }
        };

        let (name, valid) = match serde_json::from_str::<serde_json::Value>(&content) {
            Ok(val) => {
                let name = val.get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or_else(|| path.file_stem().unwrap_or_default().to_str().unwrap_or("Untitled"))
                    .to_string();
                // Basic validation: must have colors object
                let has_colors = val.get("colors").is_some();
                (name, has_colors)
            }
            Err(_) => {
                (path.file_stem().unwrap_or_default().to_string_lossy().to_string(), false)
            }
        };

        entries.push(CustomThemeEntry {
            path: path.to_string_lossy().to_string(),
            name,
            valid,
        });
    }

    Ok(entries)
}

#[tauri::command]
pub fn themes_delete(path: String) -> Result<(), String> {
    let path = PathBuf::from(&path);
    // Safety: only delete files inside ~/.k2so/themes/
    let dir = themes_dir();
    if !path.starts_with(&dir) {
        return Err("Can only delete files inside ~/.k2so/themes/".into());
    }
    fs::remove_file(&path).map_err(|e| format!("Failed to delete theme: {e}"))?;
    Ok(())
}

fn default_theme_json() -> String {
    serde_json::json!({
        "name": "My Custom Theme",
        "type": "dark",
        "colors": {
            "bg": "#0a0a0a",
            "fg": "#e4e4e7",
            "gutterBg": "#0a0a0a",
            "gutterFg": "#555555",
            "gutterBorder": "#1a1a1a",
            "activeLine": "#ffffff08",
            "selection": "#3b82f633",
            "cursor": "#3b82f6",
            "accent": "#3b82f6"
        },
        "syntax": {
            "keyword": "#c678dd",
            "string": "#98c379",
            "number": "#d19a66",
            "comment": "#5c6370",
            "function": "#61afef",
            "type": "#e5c07b",
            "variable": "#e4e4e7",
            "property": "#e06c75",
            "operator": "#56b6c2",
            "tag": "#e06c75",
            "attribute": "#d19a66",
            "regexp": "#98c379",
            "punctuation": "#abb2bf"
        }
    }).to_string()
}

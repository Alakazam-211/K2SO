use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillLayer {
    pub filename: String,
    pub title: String,
    pub preview: String,
    pub path: String,
}

fn layers_dir(tier: &str) -> Result<PathBuf, String> {
    let valid = ["manager", "agent-template", "custom-agent"];
    if !valid.contains(&tier) {
        return Err(format!("Invalid tier: {}. Must be one of: {:?}", tier, valid));
    }
    let dir = dirs::home_dir()
        .ok_or("No home directory")?
        .join(".k2so/templates")
        .join(tier);
    let _ = fs::create_dir_all(&dir);
    Ok(dir)
}

fn filename_to_title(filename: &str) -> String {
    let name = filename.trim_end_matches(".md").replace('-', " ");
    name.split_whitespace()
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().to_string() + c.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// List all custom layers for a tier.
#[tauri::command]
pub fn skill_layers_list(tier: String) -> Result<Vec<SkillLayer>, String> {
    let dir = layers_dir(&tier)?;
    let mut layers = Vec::new();

    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map_or(false, |ext| ext == "md") {
                let filename = path.file_name().unwrap_or_default().to_string_lossy().to_string();
                let content = fs::read_to_string(&path).unwrap_or_default();
                let preview = content.lines()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or("")
                    .chars().take(80).collect::<String>();

                layers.push(SkillLayer {
                    title: filename_to_title(&filename),
                    filename,
                    preview,
                    path: path.to_string_lossy().to_string(),
                });
            }
        }
    }

    layers.sort_by(|a, b| a.filename.cmp(&b.filename));
    Ok(layers)
}

/// Create a new custom layer file.
#[tauri::command]
pub fn skill_layers_create(tier: String, name: String) -> Result<SkillLayer, String> {
    let dir = layers_dir(&tier)?;

    // Sanitize name to kebab-case filename
    let filename = name.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect::<String>();
    let filename = format!("{}.md", filename.trim_matches('-'));

    let path = dir.join(&filename);
    if path.exists() {
        return Err(format!("Layer '{}' already exists", filename));
    }

    // Create with empty content (user will fill via AIFileEditor)
    fs::write(&path, "").map_err(|e| format!("Failed to create layer: {}", e))?;

    Ok(SkillLayer {
        title: filename_to_title(&filename),
        filename,
        preview: String::new(),
        path: path.to_string_lossy().to_string(),
    })
}

/// Delete a custom layer file.
#[tauri::command]
pub fn skill_layers_delete(tier: String, filename: String) -> Result<(), String> {
    let dir = layers_dir(&tier)?;
    let path = dir.join(&filename);

    if !path.exists() {
        return Err(format!("Layer '{}' not found", filename));
    }

    fs::remove_file(&path).map_err(|e| format!("Failed to delete layer: {}", e))
}

/// Get the full content of a layer file.
#[tauri::command]
pub fn skill_layers_get_content(tier: String, filename: String) -> Result<String, String> {
    let dir = layers_dir(&tier)?;
    let path = dir.join(&filename);

    fs::read_to_string(&path).map_err(|e| format!("Failed to read layer: {}", e))
}

use serde::Serialize;
use std::fs;
use std::path::Path;
use std::process::Command;

const MAX_FILE_SIZE: u64 = 10 * 1024 * 1024; // 10MB

#[derive(Debug, Serialize)]
pub struct DirEntry {
    pub name: String,
    pub path: String,
    pub is_directory: bool,
    pub size: u64,
    pub modified_at: f64,
}

#[tauri::command]
pub fn fs_read_dir(path: String, show_hidden: Option<bool>) -> Result<Vec<DirEntry>, String> {
    let show_hidden = show_hidden.unwrap_or(false);

    let entries = fs::read_dir(&path).map_err(|e| e.to_string())?;

    let mut items: Vec<DirEntry> = entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let name = entry.file_name().to_string_lossy().to_string();

            // Filter hidden files
            if !show_hidden && name.starts_with('.') {
                return None;
            }

            let full_path = Path::new(&path).join(&name);
            let full_path_str = full_path.to_string_lossy().to_string();
            let is_directory = entry.file_type().ok().map(|ft| ft.is_dir()).unwrap_or(false);

            let (size, modified_at) = match fs::metadata(&full_path) {
                Ok(meta) => {
                    let size = meta.len();
                    let modified_at = meta
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_secs_f64() * 1000.0)
                        .unwrap_or(0.0);
                    (size, modified_at)
                }
                Err(_) => (0, 0.0),
            };

            Some(DirEntry {
                name,
                path: full_path_str,
                is_directory,
                size,
                modified_at,
            })
        })
        .collect();

    // Sort: directories first, then alphabetically (case-insensitive)
    items.sort_by(|a, b| {
        if a.is_directory != b.is_directory {
            return if a.is_directory {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Greater
            };
        }
        a.name.to_lowercase().cmp(&b.name.to_lowercase())
    });

    Ok(items)
}

#[tauri::command]
pub fn fs_open_in_finder(path: String) -> Result<(), String> {
    Command::new("open")
        .arg("-R")
        .arg(&path)
        .spawn()
        .map_err(|e| format!("Failed to open Finder: {}", e))?;
    Ok(())
}

#[tauri::command]
pub fn fs_copy_path(app: tauri::AppHandle, path: String) -> Result<(), String> {
    use tauri_plugin_clipboard_manager::ClipboardExt;
    app.clipboard()
        .write_text(&path)
        .map_err(|e| format!("Failed to copy to clipboard: {}", e))?;
    Ok(())
}

#[derive(Debug, Serialize)]
pub struct FileContent {
    pub content: String,
    pub path: String,
    pub name: String,
}

#[tauri::command]
pub fn fs_read_file(path: String) -> Result<FileContent, String> {
    let p = Path::new(&path);

    // Check file exists
    if !p.exists() {
        return Err("File not found".to_string());
    }

    let meta = fs::metadata(&path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            "Permission denied".to_string()
        } else {
            e.to_string()
        }
    })?;

    // Reject files larger than 10MB
    if meta.len() > MAX_FILE_SIZE {
        return Err("File too large (>10MB)".to_string());
    }

    // Read as bytes first to detect binary content
    let buffer = fs::read(&path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            "Permission denied".to_string()
        } else {
            e.to_string()
        }
    })?;

    // Check for null bytes (binary file indicator) in first 8KB
    let check_len = std::cmp::min(buffer.len(), 8192);
    for byte in &buffer[..check_len] {
        if *byte == 0 {
            return Err("Cannot read binary file".to_string());
        }
    }

    let content = String::from_utf8(buffer)
        .map_err(|_| "Cannot read binary file".to_string())?;

    let name = p
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    Ok(FileContent {
        content,
        path,
        name,
    })
}

#[tauri::command]
pub fn fs_read_binary_file(path: String) -> Result<Vec<u8>, String> {
    let p = Path::new(&path);

    if !p.exists() {
        return Err("File not found".to_string());
    }

    let meta = fs::metadata(&path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            "Permission denied".to_string()
        } else {
            e.to_string()
        }
    })?;

    // Reject files larger than 50MB for binary reads
    if meta.len() > 50 * 1024 * 1024 {
        return Err("File too large (>50MB)".to_string());
    }

    fs::read(&path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            "Permission denied".to_string()
        } else {
            e.to_string()
        }
    })
}

#[tauri::command]
pub fn fs_write_file(path: String, content: String) -> Result<(), String> {
    fs::write(&path, &content).map_err(|e| {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            "Permission denied".to_string()
        } else {
            e.to_string()
        }
    })
}

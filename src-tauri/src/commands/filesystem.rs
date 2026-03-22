use serde::Serialize;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const MAX_FILE_SIZE: u64 = 10 * 1024 * 1024; // 10MB

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
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

// ── Disambiguation helper ───────────────────────────────────────────────

/// Given a target path that may already exist, return a unique path by appending
/// " copy", " copy 2", etc. to the file stem (before the extension).
fn disambiguate_path(target: &Path) -> PathBuf {
    if !target.exists() {
        return target.to_path_buf();
    }
    let stem = target
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let ext = target
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    let parent = target.parent().unwrap_or(Path::new("/"));

    // Try "name copy.ext", then "name copy 2.ext", etc.
    let copy_name = parent.join(format!("{stem} copy{ext}"));
    if !copy_name.exists() {
        return copy_name;
    }

    for i in 2..100 {
        let numbered = parent.join(format!("{stem} copy {i}{ext}"));
        if !numbered.exists() {
            return numbered;
        }
    }
    target.to_path_buf() // give up after 100
}

// ── File move/copy for drag-and-drop ────────────────────────────────────

/// Recursively copy a directory tree with symlink cycle detection.
fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) -> Result<(), String> {
    let mut visited = HashSet::new();
    copy_dir_inner(src, dst, &mut visited)
}

fn copy_dir_inner(
    src: &std::path::Path,
    dst: &std::path::Path,
    visited: &mut HashSet<u64>,
) -> Result<(), String> {
    // Track visited directories by inode to prevent symlink cycles
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let Ok(meta) = fs::metadata(src) {
            if !visited.insert(meta.ino()) {
                // Already visited this inode — symlink cycle, skip
                return Ok(());
            }
        }
    }

    fs::create_dir_all(dst).map_err(|e| format!("Failed to create directory: {e}"))?;
    for entry in fs::read_dir(src).map_err(|e| format!("Failed to read directory: {e}"))? {
        let entry = entry.map_err(|e| format!("Failed to read entry: {e}"))?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if src_path.is_dir() {
            copy_dir_inner(&src_path, &dst_path, visited)?;
        } else {
            fs::copy(&src_path, &dst_path)
                .map_err(|e| format!("Failed to copy {}: {e}", src_path.display()))?;
        }
    }
    Ok(())
}

/// Resolve the destination path: if destination is a directory, append the source's filename.
fn resolve_destination(source: &std::path::Path, destination: &std::path::Path) -> std::path::PathBuf {
    if destination.is_dir() {
        if let Some(name) = source.file_name() {
            return destination.join(name);
        }
    }
    destination.to_path_buf()
}

/// Move files/folders to a destination directory.
/// Default behavior for drag-and-drop without Option key.
#[tauri::command]
pub fn fs_move_files(sources: Vec<String>, destination: String) -> Result<(), String> {
    let dest = std::path::Path::new(&destination);
    if !dest.exists() {
        return Err(format!("Destination does not exist: {destination}"));
    }

    for source in &sources {
        let src = std::path::Path::new(source);
        if !src.exists() {
            return Err(format!("Source does not exist: {source}"));
        }

        let target = resolve_destination(src, dest);

        // Reject self-referential moves
        if let (Ok(s), Some(parent)) = (src.canonicalize(), target.parent()) {
            if let Ok(dp) = parent.canonicalize() {
                if dp.starts_with(&s) {
                    return Err("Cannot move a folder into itself".to_string());
                }
            }
        }

        let target = disambiguate_path(&target);

        // Try rename (fast, same-volume). Fall back to copy+delete for cross-volume.
        if fs::rename(src, &target).is_err() {
            if src.is_dir() {
                copy_dir_recursive(src, &target)?;
                fs::remove_dir_all(src)
                    .map_err(|e| format!("Moved but failed to remove source: {e}"))?;
            } else {
                fs::copy(src, &target)
                    .map_err(|e| format!("Failed to copy: {e}"))?;
                fs::remove_file(src)
                    .map_err(|e| format!("Copied but failed to remove source: {e}"))?;
            }
        }
    }

    Ok(())
}

/// Delete files/folders. Moves to macOS Trash by default (reversible).
/// Set `permanent` to true to bypass Trash and delete immediately.
#[tauri::command]
pub fn fs_delete(paths: Vec<String>, permanent: Option<bool>) -> Result<(), String> {
    let permanent = permanent.unwrap_or(false);

    for path_str in &paths {
        let p = Path::new(path_str);
        if !p.exists() {
            return Err(format!("Does not exist: {path_str}"));
        }

        if permanent {
            if p.is_dir() {
                fs::remove_dir_all(p)
                    .map_err(|e| format!("Failed to delete directory: {e}"))?;
            } else {
                fs::remove_file(p)
                    .map_err(|e| format!("Failed to delete file: {e}"))?;
            }
        } else {
            // Move to native OS Trash (uses NSFileManager on macOS)
            trash::delete(p)
                .map_err(|e| format!("Failed to trash {path_str}: {e}"))?;
        }
    }

    Ok(())
}

/// Rename a file or directory.
#[tauri::command]
pub fn fs_rename(old_path: String, new_name: String) -> Result<String, String> {
    let old = Path::new(&old_path);
    if !old.exists() {
        return Err(format!("Does not exist: {old_path}"));
    }

    // Validate the new name
    if new_name.is_empty() || new_name.contains('/') || new_name.contains('\0') {
        return Err("Invalid file name".to_string());
    }

    let parent = old.parent().ok_or("Cannot determine parent directory")?;
    let new_path = parent.join(&new_name);

    // Check for case-insensitive same-name rename (allowed on macOS)
    let is_case_rename = old
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase() == new_name.to_lowercase())
        .unwrap_or(false);

    if !is_case_rename && new_path.exists() {
        return Err(format!("Already exists: {}", new_path.display()));
    }

    fs::rename(old, &new_path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            "Permission denied".to_string()
        } else {
            format!("Rename failed: {e}")
        }
    })?;

    Ok(new_path.to_string_lossy().to_string())
}

/// Create a new file or directory.
#[tauri::command]
pub fn fs_create_entry(path: String, is_directory: bool) -> Result<(), String> {
    let p = Path::new(&path);

    if p.exists() {
        return Err(format!("Already exists: {path}"));
    }

    if is_directory {
        fs::create_dir_all(&p).map_err(|e| {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                "Permission denied".to_string()
            } else {
                format!("Failed to create directory: {e}")
            }
        })?;
    } else {
        // Ensure parent directory exists
        if let Some(parent) = p.parent() {
            if !parent.exists() {
                fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed to create parent directory: {e}"))?;
            }
        }
        fs::write(&p, "").map_err(|e| {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                "Permission denied".to_string()
            } else {
                format!("Failed to create file: {e}")
            }
        })?;
    }

    Ok(())
}

/// Copy files/folders to a destination directory.
/// Used when Option key is held during drag-and-drop.
#[tauri::command]
pub fn fs_copy_files(sources: Vec<String>, destination: String) -> Result<(), String> {
    let dest = std::path::Path::new(&destination);
    if !dest.exists() {
        return Err(format!("Destination does not exist: {destination}"));
    }

    for source in &sources {
        let src = std::path::Path::new(source);
        if !src.exists() {
            return Err(format!("Source does not exist: {source}"));
        }

        let target = resolve_destination(src, dest);

        let target = disambiguate_path(&target);

        if src.is_dir() {
            copy_dir_recursive(src, &target)?;
        } else {
            fs::copy(src, &target)
                .map_err(|e| format!("Failed to copy: {e}"))?;
        }
    }

    Ok(())
}

/// Duplicate a file or folder in-place (same parent directory).
/// Returns the path of the new copy.
#[tauri::command]
pub fn fs_duplicate(path: String) -> Result<String, String> {
    let src = Path::new(&path);
    if !src.exists() {
        return Err(format!("Does not exist: {path}"));
    }

    let parent = src.parent().unwrap_or(Path::new("/"));
    let target = disambiguate_path(&resolve_destination(src, parent));

    if src.is_dir() {
        copy_dir_recursive(src, &target)?;
    } else {
        fs::copy(src, &target).map_err(|e| format!("Failed to duplicate: {e}"))?;
    }

    Ok(target.to_string_lossy().to_string())
}

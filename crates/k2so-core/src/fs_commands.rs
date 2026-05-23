//! Filesystem primitives migrated from `src-tauri/src/commands/filesystem.rs`
//! during Phase 2 Unit 6 (daemon-headless migration).
//!
//! Each function takes plain strings / Vec<String> so it can be invoked
//! either by:
//!
//! - The legacy Tauri `#[tauri::command]` thin wrappers (during the
//!   transition window — already deleted in this Unit), or
//! - The daemon's `/cli/fs/*` route handlers, which is the post-Phase-2
//!   home of the file tree, chat history sidebar, theme manager, etc.
//!
//! Operations are deliberately daemon-side because the **daemon owns the
//! filesystem** in the K2SO Connect architecture. When the Tauri client
//! lives on Machine A and the daemon on Machine B, the file tree the user
//! browses is Machine B's tree. (`open-in-finder` and `open-external` are
//! the lone exceptions — they open the local file on the daemon's machine,
//! which is the right behavior in Bundled mode and harmless-but-unusable
//! in K2SO Connect mode. Phase 3.1 will route those through the renderer.)
//!
//! Path safety: `validate_path` rejects empty paths and resolves symlinks
//! / `..` segments via `canonicalize` for reads. Writes accept paths whose
//! parent already exists, so renderer code can build paths without
//! checking existence first.

use serde::Serialize;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// 10MB max for text reads — matches the legacy Tauri command's cap.
/// Above this the renderer should switch to binary read or stream the
/// file in chunks.
pub const MAX_FILE_SIZE: u64 = 10 * 1024 * 1024;
/// 50MB max for binary reads (images, PDFs, small media). Anything
/// larger should be streamed via a separate WS endpoint — Phase 3 work.
pub const MAX_BINARY_SIZE: u64 = 50 * 1024 * 1024;

// ── Path validation ─────────────────────────────────────────────────────

/// Validate a path before any FS op. Rejects empty paths and ensures the
/// path resolves to a real location for reads. For writes (where the
/// destination doesn't exist yet) it accepts the path if its parent
/// exists.
pub fn validate_path(path: &str) -> Result<PathBuf, String> {
    if path.is_empty() {
        return Err("Path is empty".to_string());
    }
    let p = Path::new(path);
    if p.exists() {
        p.canonicalize().map_err(|e| format!("Invalid path: {e}"))
    } else {
        let parent = p
            .parent()
            .ok_or_else(|| "Invalid path: no parent".to_string())?;
        if parent.exists() {
            Ok(p.to_path_buf())
        } else {
            Err(format!(
                "Parent directory does not exist: {}",
                parent.display()
            ))
        }
    }
}

// ── Exclusive rename (macOS atomic) ────────────────────────────────────

/// Rename a file/directory without overwriting the destination. On
/// macOS uses `renamex_np` with `RENAME_EXCL` for atomic exclusivity.
/// On other platforms falls back to an exists-check + rename (race
/// window, but catches the common case).
fn rename_exclusive(src: &Path, dst: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        use std::ffi::CString;
        let src_c = CString::new(src.to_string_lossy().as_bytes())
            .map_err(|_| "Invalid source path".to_string())?;
        let dst_c = CString::new(dst.to_string_lossy().as_bytes())
            .map_err(|_| "Invalid destination path".to_string())?;
        // RENAME_EXCL = 0x00000004 — fail if destination exists
        let ret =
            unsafe { libc::renamex_np(src_c.as_ptr(), dst_c.as_ptr(), 0x00000004) };
        if ret == 0 {
            return Ok(());
        }
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::EEXIST) {
            return Err(format!("Already exists: {}", dst.display()));
        }
        if err.raw_os_error() == Some(libc::ENOTSUP) {
            // Filesystem doesn't support renamex_np (e.g. FUSE). Fall
            // back to the same logic the non-macOS path uses.
            if dst.exists() {
                return Err(format!("Already exists: {}", dst.display()));
            }
            fs::rename(src, dst).map_err(|e| format!("Rename failed: {e}"))?;
            return Ok(());
        }
        Err(format!("Rename failed: {err}"))
    }
    #[cfg(not(target_os = "macos"))]
    {
        if dst.exists() {
            return Err(format!("Already exists: {}", dst.display()));
        }
        fs::rename(src, dst).map_err(|e| format!("Rename failed: {e}"))
    }
}

// ── read-dir / search-tree ─────────────────────────────────────────────

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirEntryInfo {
    pub name: String,
    pub path: String,
    pub is_directory: bool,
    pub size: u64,
    pub modified_at: f64,
}

pub fn read_dir(path: &str, show_hidden: bool) -> Result<Vec<DirEntryInfo>, String> {
    let path = validate_path(path)?.to_string_lossy().to_string();
    let entries = fs::read_dir(&path).map_err(|e| e.to_string())?;
    let mut items: Vec<DirEntryInfo> = entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let name = entry.file_name().to_string_lossy().to_string();
            if !show_hidden && name.starts_with('.') {
                return None;
            }
            let full_path = Path::new(&path).join(&name);
            let full_path_str = full_path.to_string_lossy().to_string();
            let is_directory = entry
                .file_type()
                .ok()
                .map(|ft| ft.is_dir())
                .unwrap_or(false);
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
            Some(DirEntryInfo {
                name,
                path: full_path_str,
                is_directory,
                size,
                modified_at,
            })
        })
        .collect();
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

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchMatch {
    pub path: String,
    pub name: String,
    pub is_directory: bool,
}

pub fn search_tree(
    root: &str,
    query: &str,
    show_hidden: bool,
    max_results: usize,
) -> Result<Vec<SearchMatch>, String> {
    let query_lower = query.trim().to_lowercase();
    if query_lower.is_empty() {
        return Ok(Vec::new());
    }
    let root_path = validate_path(root)?;
    let mut results: Vec<SearchMatch> = Vec::new();
    let mut stack: Vec<PathBuf> = vec![root_path];
    const SKIP_DIRS: &[&str] = &[
        "node_modules",
        ".git",
        "target",
        "dist",
        "build",
        ".next",
        ".turbo",
        ".cache",
        "Pods",
        ".venv",
        "venv",
        "__pycache__",
        ".mypy_cache",
        ".pytest_cache",
        "DerivedData",
    ];
    while let Some(dir) = stack.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.filter_map(|e| e.ok()) {
            let name = entry.file_name().to_string_lossy().to_string();
            if !show_hidden && name.starts_with('.') {
                continue;
            }
            let full = dir.join(&name);
            let is_dir = entry
                .file_type()
                .ok()
                .map(|ft| ft.is_dir())
                .unwrap_or(false);
            if name.to_lowercase().contains(&query_lower) {
                results.push(SearchMatch {
                    path: full.to_string_lossy().to_string(),
                    name: name.clone(),
                    is_directory: is_dir,
                });
                if results.len() >= max_results {
                    return Ok(results);
                }
            }
            if is_dir && !SKIP_DIRS.contains(&name.as_str()) {
                stack.push(full);
            }
        }
    }
    Ok(results)
}

// ── read-file / read-binary / write-file ──────────────────────────────

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileContent {
    pub content: String,
    pub path: String,
    pub name: String,
}

pub fn read_file(path: &str) -> Result<FileContent, String> {
    let path = validate_path(path)?.to_string_lossy().to_string();
    let p = Path::new(&path);
    if !p.exists() {
        return Err("File not found".to_string());
    }
    let meta = fs::metadata(&path).map_err(map_io_err)?;
    if meta.len() > MAX_FILE_SIZE {
        return Err("File too large (>10MB)".to_string());
    }
    let buffer = fs::read(&path).map_err(map_io_err)?;
    // Reject binary files (null byte in first 8KB).
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

/// Read a file as raw bytes (caller's responsibility to interpret as
/// PDF, image, etc.). HTTP transport will base64-encode this on the way
/// out — see `daemon::fs_routes` for the wrapper.
pub fn read_binary_file(path: &str) -> Result<Vec<u8>, String> {
    let path = validate_path(path)?.to_string_lossy().to_string();
    let p = Path::new(&path);
    if !p.exists() {
        return Err("File not found".to_string());
    }
    let meta = fs::metadata(&path).map_err(map_io_err)?;
    if meta.len() > MAX_BINARY_SIZE {
        return Err("File too large (>50MB)".to_string());
    }
    fs::read(&path).map_err(map_io_err)
}

pub fn write_file(path: &str, content: &str) -> Result<(), String> {
    let path = validate_path(path)?.to_string_lossy().to_string();
    fs::write(&path, content).map_err(map_io_err)
}

fn map_io_err(e: std::io::Error) -> String {
    if e.kind() == std::io::ErrorKind::PermissionDenied {
        "Permission denied".to_string()
    } else {
        e.to_string()
    }
}

// ── move / copy / delete / rename / create / duplicate ────────────────

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
    target.to_path_buf()
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    let mut visited = HashSet::new();
    copy_dir_inner(src, dst, &mut visited)
}

fn copy_dir_inner(
    src: &Path,
    dst: &Path,
    visited: &mut HashSet<u64>,
) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let Ok(meta) = fs::metadata(src) {
            if !visited.insert(meta.ino()) {
                return Ok(()); // symlink cycle: skip
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

fn resolve_destination(source: &Path, destination: &Path) -> PathBuf {
    if destination.is_dir() {
        if let Some(name) = source.file_name() {
            return destination.join(name);
        }
    }
    destination.to_path_buf()
}

pub fn move_files(sources: &[String], destination: &str) -> Result<(), String> {
    let dest = Path::new(destination);
    if !dest.exists() {
        return Err(format!("Destination does not exist: {destination}"));
    }
    for source in sources {
        let src = Path::new(source);
        if !src.exists() {
            return Err(format!("Source does not exist: {source}"));
        }
        let target = resolve_destination(src, dest);
        if let (Ok(s), Some(parent)) = (src.canonicalize(), target.parent()) {
            if let Ok(dp) = parent.canonicalize() {
                if dp.starts_with(&s) {
                    return Err("Cannot move a folder into itself".to_string());
                }
            }
        }
        let target = disambiguate_path(&target);
        if fs::rename(src, &target).is_err() {
            if src.is_dir() {
                copy_dir_recursive(src, &target)?;
                fs::remove_dir_all(src)
                    .map_err(|e| format!("Moved but failed to remove source: {e}"))?;
            } else {
                fs::copy(src, &target).map_err(|e| format!("Failed to copy: {e}"))?;
                fs::remove_file(src)
                    .map_err(|e| format!("Copied but failed to remove source: {e}"))?;
            }
        }
    }
    Ok(())
}

pub fn copy_files(sources: &[String], destination: &str) -> Result<(), String> {
    let dest = Path::new(destination);
    if !dest.exists() {
        return Err(format!("Destination does not exist: {destination}"));
    }
    for source in sources {
        let src = Path::new(source);
        if !src.exists() {
            return Err(format!("Source does not exist: {source}"));
        }
        let target = resolve_destination(src, dest);
        let target = disambiguate_path(&target);
        if src.is_dir() {
            copy_dir_recursive(src, &target)?;
        } else {
            fs::copy(src, &target).map_err(|e| format!("Failed to copy: {e}"))?;
        }
    }
    Ok(())
}

pub fn delete(paths: &[String], permanent: bool) -> Result<(), String> {
    for path_str in paths {
        validate_path(path_str)?;
        let p = Path::new(path_str);
        if !p.exists() {
            return Err(format!("Does not exist: {path_str}"));
        }
        if permanent {
            if p.is_dir() {
                fs::remove_dir_all(p)
                    .map_err(|e| format!("Failed to delete directory: {e}"))?;
            } else {
                fs::remove_file(p).map_err(|e| format!("Failed to delete file: {e}"))?;
            }
        } else {
            trash::delete(p).map_err(|e| format!("Failed to trash {path_str}: {e}"))?;
        }
    }
    Ok(())
}

pub fn rename(old_path: &str, new_name: &str) -> Result<String, String> {
    let old = Path::new(old_path);
    if !old.exists() {
        return Err(format!("Does not exist: {old_path}"));
    }
    if new_name.is_empty() || new_name.contains('/') || new_name.contains('\0') {
        return Err("Invalid file name".to_string());
    }
    let parent = old.parent().ok_or("Cannot determine parent directory")?;
    let new_path = parent.join(new_name);
    let is_case_rename = old
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase() == new_name.to_lowercase())
        .unwrap_or(false);
    if is_case_rename {
        fs::rename(old, &new_path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                "Permission denied".to_string()
            } else {
                format!("Rename failed: {e}")
            }
        })?;
    } else {
        rename_exclusive(old, &new_path)?;
    }
    Ok(new_path.to_string_lossy().to_string())
}

pub fn create_entry(path: &str, is_directory: bool) -> Result<(), String> {
    let path = validate_path(path)?.to_string_lossy().to_string();
    let p = Path::new(&path);
    if p.exists() {
        return Err(format!("Already exists: {path}"));
    }
    if is_directory {
        fs::create_dir_all(p).map_err(|e| {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                "Permission denied".to_string()
            } else {
                format!("Failed to create directory: {e}")
            }
        })?;
    } else {
        if let Some(parent) = p.parent() {
            if !parent.exists() {
                fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed to create parent directory: {e}"))?;
            }
        }
        fs::write(p, "").map_err(|e| {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                "Permission denied".to_string()
            } else {
                format!("Failed to create file: {e}")
            }
        })?;
    }
    Ok(())
}

pub fn duplicate(path: &str) -> Result<String, String> {
    let src = Path::new(path);
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

// ── open-finder / open-external ───────────────────────────────────────

/// Reveal the path in the user's Finder window. Daemon-side caveat:
/// when the daemon runs on a different machine (K2SO Connect), this
/// opens Finder on the **daemon's** machine, not the user's. Phase 3.1
/// will route this through the renderer; for now Bundled mode is the
/// only use case and the behavior matches the pre-Phase-2 Tauri
/// implementation.
pub fn open_in_finder(path: &str) -> Result<(), String> {
    Command::new("open")
        .arg("-R")
        .arg(path)
        .spawn()
        .map_err(|e| format!("Failed to open Finder: {e}"))?;
    Ok(())
}

/// Open a URL in the system default browser. Same K2SO Connect caveat
/// as `open_in_finder` — opens on the daemon's machine.
pub fn open_external(url: &str) -> Result<String, String> {
    let clean: String = url
        .chars()
        .filter(|c| !c.is_control() && *c != '\0')
        .collect();
    let clean = clean.trim().to_string();
    if clean.is_empty() {
        return Err("URL is empty after cleaning".to_string());
    }
    #[cfg(target_os = "macos")]
    {
        let output = Command::new("/usr/bin/open")
            .arg(&clean)
            .output()
            .map_err(|e| format!("Failed to open URL: {e}"))?;
        if output.status.success() {
            return Ok(format!("Opened: {clean}"));
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("open failed: {stderr}"));
        }
    }
    #[cfg(target_os = "linux")]
    {
        let _output = Command::new("xdg-open")
            .arg(&clean)
            .output()
            .map_err(|e| format!("Failed to open URL: {e}"))?;
        return Ok(format!("Opened: {clean}"));
    }
    #[allow(unreachable_code)]
    Err("Unsupported platform".to_string())
}

// ── clipboard-paths (macOS Finder-copy bridge) ────────────────────────

/// Read file paths from the macOS general pasteboard. Finder's CMD+C
/// writes `NSFilenamesPboardType`, which WKWebView does not expose via
/// the web Clipboard API. The terminal paste handler calls this to
/// match the drag-drop behavior for Finder-copied files.
///
/// K2SO Connect caveat: returns paths from the **daemon's** pasteboard.
/// In Bundled mode (daemon + Tauri on the same machine) this is the
/// user's pasteboard — same behavior as the pre-Phase-2 Tauri command.
/// In Connect mode the renderer should fall back to a renderer-side
/// path (Phase 3.1) since the user's clipboard lives on their own
/// machine.
#[cfg(target_os = "macos")]
pub fn clipboard_read_file_paths() -> Result<Vec<String>, String> {
    use cocoa::appkit::{NSFilenamesPboardType, NSPasteboard};
    use cocoa::base::{id, nil};
    use cocoa::foundation::{NSArray, NSString};
    use objc::{msg_send, sel, sel_impl};
    use std::ffi::CStr;

    unsafe {
        let pb: id = NSPasteboard::generalPasteboard(nil);
        let list: id = msg_send![pb, propertyListForType: NSFilenamesPboardType];
        if list == nil {
            return Ok(Vec::new());
        }
        let count = NSArray::count(list);
        let mut paths = Vec::with_capacity(count as usize);
        for i in 0..count {
            let ns_str: id = NSArray::objectAtIndex(list, i);
            let c_str = NSString::UTF8String(ns_str);
            if !c_str.is_null() {
                paths.push(CStr::from_ptr(c_str).to_string_lossy().into_owned());
            }
        }
        Ok(paths)
    }
}

#[cfg(not(target_os = "macos"))]
pub fn clipboard_read_file_paths() -> Result<Vec<String>, String> {
    Ok(Vec::new())
}

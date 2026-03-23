use serde::Serialize;
use std::collections::HashMap;
use tauri::Emitter;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatSession {
    pub session_id: String,
    pub project: String,
    pub title: String,
    pub timestamp: i64,
    pub provider: String,
    pub message_count: usize,
}

/// Internal struct to accumulate session data while parsing.
struct SessionAccumulator {
    session_id: String,
    project: String,
    first_display: String,
    first_timestamp: i64,
    last_timestamp: i64,
    count: usize,
}

// ── Worktree path resolution ────────────────────────────────────────────

/// Given a project path (which may be a worktree), resolve the root project path.
/// e.g. "/repo/.worktrees/feature" → "/repo"
/// e.g. "/repo" → "/repo"
fn resolve_root_project_path(path: &str) -> &str {
    if let Some(idx) = path.find("/.worktrees/") {
        &path[..idx]
    } else {
        path
    }
}

/// Check if a session's project path belongs to the given root project
/// (either the root itself or any worktree under it).
fn matches_project_family(session_project: &str, root: &str) -> bool {
    session_project == root
        || session_project.starts_with(&format!("{}/.worktrees/", root))
}

// ── Claude history parsing ───────────────────────────────────────────────

fn claude_history_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude").join("history.jsonl"))
}

fn parse_claude_sessions(project_filter: Option<&str>) -> Result<Vec<ChatSession>, String> {
    let path = match claude_history_path() {
        Some(p) => p,
        None => return Ok(vec![]),
    };

    let file = match File::open(&path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(format!("Failed to open history file: {}", e)),
    };

    let reader = BufReader::new(file);
    let mut sessions: HashMap<String, SessionAccumulator> = HashMap::new();

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };

        if line.trim().is_empty() {
            continue;
        }

        let parsed: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let session_id = match parsed.get("sessionId").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };

        let project = parsed
            .get("project")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        // Apply project filter early to avoid unnecessary work.
        // Match the root project AND all its worktrees so history
        // "collapses back" when a worktree is merged/deleted.
        if let Some(filter) = project_filter {
            let root = resolve_root_project_path(filter);
            if !matches_project_family(&project, root) {
                continue;
            }
        }

        let display = parsed
            .get("display")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let timestamp = parsed
            .get("timestamp")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);

        sessions
            .entry(session_id.clone())
            .and_modify(|acc| {
                acc.count += 1;
                if timestamp > acc.last_timestamp {
                    acc.last_timestamp = timestamp;
                }
                if timestamp < acc.first_timestamp {
                    acc.first_timestamp = timestamp;
                    acc.first_display = display.clone();
                }
            })
            .or_insert(SessionAccumulator {
                session_id,
                project,
                first_display: display,
                first_timestamp: timestamp,
                last_timestamp: timestamp,
                count: 1,
            });
    }

    Ok(sessions
        .into_values()
        .map(|acc| {
            let title = if acc.first_display.len() > 80 {
                let truncated: String = acc.first_display.chars().take(77).collect();
                format!("{}...", truncated)
            } else {
                acc.first_display
            };

            ChatSession {
                session_id: acc.session_id,
                project: acc.project,
                title,
                timestamp: acc.last_timestamp,
                provider: "claude".to_string(),
                message_count: acc.count,
            }
        })
        .collect())
}

// ── Cursor chat parsing ─────────────────────────────────────────────────

/// Convert a project path to Cursor's hash format.
/// e.g. "/Users/z3thon/DevProjects/K2SO" → "Users-z3thon-DevProjects-K2SO"
fn cursor_project_hash(project_path: &str) -> String {
    project_path
        .trim_start_matches('/')
        .replace('/', "-")
}

/// Read Cursor chat metadata from store.db to extract the chat name and timestamp.
/// Cursor stores metadata as hex-encoded JSON in the `meta` table, key "0".
fn read_cursor_chat_meta(store_db: &std::path::Path) -> Option<(String, i64)> {
    // Open the SQLite database and try to read the meta table
    let conn = rusqlite::Connection::open_with_flags(
        store_db,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ).ok()?;

    let hex_value: String = conn.query_row(
        "SELECT value FROM meta WHERE key = '0'",
        [],
        |row| row.get(0),
    ).ok()?;

    // Decode hex string to bytes
    let chars: Vec<char> = hex_value.chars().collect();
    if chars.len() % 2 != 0 { return None; }
    let mut bytes = Vec::with_capacity(chars.len() / 2);
    for chunk in chars.chunks(2) {
        let s: String = chunk.iter().collect();
        bytes.push(u8::from_str_radix(&s, 16).ok()?);
    }

    let json_str = String::from_utf8(bytes).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&json_str).ok()?;

    let name = parsed.get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("Untitled")
        .to_string();

    let created_at = parsed.get("createdAt")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    Some((name, created_at))
}

fn parse_cursor_sessions(_project_filter: Option<&str>) -> Result<Vec<ChatSession>, String> {
    let cursor_chats_dir = match dirs::home_dir() {
        Some(h) => h.join(".cursor").join("chats"),
        None => return Ok(vec![]),
    };

    if !cursor_chats_dir.exists() {
        return Ok(vec![]);
    }

    let mut results = Vec::new();

    // Cursor uses opaque hex hashes for project directories that can't be
    // reversed to project paths. Scan all hash directories and show all
    // Cursor chat sessions regardless of project filter.
    let hash_dirs: Vec<PathBuf> = match fs::read_dir(&cursor_chats_dir) {
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .map(|e| e.path())
            .collect(),
        Err(_) => vec![],
    };

    for hash_dir in hash_dirs {
        let project_name = hash_dir
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();

        // Each subdirectory under the hash dir is a chat session (UUID)
        let chat_dirs = match fs::read_dir(&hash_dir) {
            Ok(entries) => entries
                .filter_map(|e| e.ok())
                .filter(|e| e.path().is_dir())
                .collect::<Vec<_>>(),
            Err(_) => continue,
        };

        for chat_entry in chat_dirs {
            let chat_path = chat_entry.path();
            let chat_id = match chat_path.file_name() {
                Some(n) => n.to_string_lossy().to_string(),
                None => continue,
            };

            let store_db = chat_path.join("store.db");
            if !store_db.exists() {
                continue;
            }

            // Try to read chat name and timestamp from store.db metadata
            let (title, timestamp) = match read_cursor_chat_meta(&store_db) {
                Some((name, created_at)) => {
                    // Use file modification time for "last active", but prefer
                    // createdAt from metadata for ordering
                    let file_ts = fs::metadata(&store_db)
                        .ok()
                        .and_then(|m| m.modified().ok())
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_millis() as i64)
                        .unwrap_or(created_at);
                    (name, file_ts)
                }
                None => {
                    // Fallback: use file modification time and short ID as title
                    let file_ts = fs::metadata(&store_db)
                        .ok()
                        .and_then(|m| m.modified().ok())
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_millis() as i64)
                        .unwrap_or(0);
                    let short_id = if chat_id.len() > 8 { &chat_id[..8] } else { &chat_id };
                    (format!("Cursor session {}", short_id), file_ts)
                }
            };

            results.push(ChatSession {
                session_id: chat_id,
                project: project_name.clone(),
                title,
                timestamp,
                provider: "cursor".to_string(),
                message_count: 0,
            });
        }
    }

    Ok(results)
}

// ── Cursor IDE workspace storage parsing ────────────────────────────────

/// Parse Cursor IDE conversations from the workspace storage layer.
/// Cursor IDE stores conversations in:
///   ~/Library/Application Support/Cursor/User/workspaceStorage/{hash}/state.vscdb
/// Each workspace has a workspace.json mapping the hash to a project folder URI.
fn parse_cursor_ide_sessions(project_filter: Option<&str>) -> Result<Vec<ChatSession>, String> {
    let workspace_dir = match dirs::home_dir() {
        Some(h) => h.join("Library/Application Support/Cursor/User/workspaceStorage"),
        None => return Ok(vec![]),
    };

    if !workspace_dir.exists() {
        return Ok(vec![]);
    }

    let mut results = Vec::new();

    let entries = match fs::read_dir(&workspace_dir) {
        Ok(e) => e,
        Err(_) => return Ok(vec![]),
    };

    for entry in entries.flatten() {
        let ws_path = entry.path();
        if !ws_path.is_dir() {
            continue;
        }

        // Read workspace.json to get the project folder URI
        let ws_json_path = ws_path.join("workspace.json");
        let ws_json = match fs::read_to_string(&ws_json_path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let ws_data: serde_json::Value = match serde_json::from_str(&ws_json) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let folder_uri = match ws_data.get("folder").and_then(|v| v.as_str()) {
            Some(f) => f.to_string(),
            None => continue,
        };

        // Decode URI to path: "file:///Users/z3thon/DevProjects/Alakazam%20Labs/C3PO" → "/Users/.../C3PO"
        let folder_path = folder_uri
            .strip_prefix("file://")
            .unwrap_or(&folder_uri)
            .to_string();
        // Basic percent-decoding for common chars
        let folder_path = folder_path
            .replace("%20", " ")
            .replace("%28", "(")
            .replace("%29", ")")
            .replace("%5B", "[")
            .replace("%5D", "]");

        // Apply project filter
        if let Some(filter) = project_filter {
            let root = resolve_root_project_path(filter);
            if !matches_project_family(&folder_path, root) {
                continue;
            }
        }

        // Try to read state.vscdb for composer data
        let state_db_path = ws_path.join("state.vscdb");
        if !state_db_path.exists() {
            continue;
        }

        let conn = match rusqlite::Connection::open_with_flags(
            &state_db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        ) {
            Ok(c) => c,
            Err(_) => continue,
        };

        // Read the conversation index from composer.composerData
        let composer_json: String = match conn.query_row(
            "SELECT value FROM ItemTable WHERE key = 'composer.composerData'",
            [],
            |row| row.get(0),
        ) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let composer_data: serde_json::Value = match serde_json::from_str(&composer_json) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let composers = match composer_data.get("allComposers").and_then(|v| v.as_array()) {
            Some(arr) => arr,
            None => continue,
        };

        // Extract the project name from the folder path for display
        let project_display = folder_path
            .rsplit('/')
            .next()
            .unwrap_or(&folder_path)
            .to_string();

        for composer in composers {
            let composer_id = match composer.get("composerId").and_then(|v| v.as_str()) {
                Some(id) => id.to_string(),
                None => continue,
            };

            let name = composer
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("Untitled")
                .to_string();

            let title = if name.len() > 80 {
                let truncated: String = name.chars().take(77).collect();
                format!("{}...", truncated)
            } else {
                name
            };

            // Use lastUpdatedAt or createdAt for timestamp
            let timestamp = composer
                .get("lastUpdatedAt")
                .and_then(|v| v.as_i64())
                .or_else(|| composer.get("createdAt").and_then(|v| v.as_i64()))
                .unwrap_or(0);

            results.push(ChatSession {
                session_id: composer_id,
                project: project_display.clone(),
                title,
                timestamp,
                provider: "cursor".to_string(),
                message_count: 0,
            });
        }
    }

    Ok(results)
}

// ── Session detection for resume ────────────────────────────────────────

/// Detect the most recent active session ID for a CLI tool in a given project.
/// Used before app close to capture session IDs for resume on reopen.
fn detect_claude_session(project_path: &str) -> Option<String> {
    let path = claude_history_path()?;
    let file = File::open(&path).ok()?;

    // Read the last 64KB of the file to find recent sessions efficiently
    let metadata = file.metadata().ok()?;
    let file_size = metadata.len();
    let read_from = if file_size > 65536 { file_size - 65536 } else { 0 };

    let mut file = file;
    file.seek(SeekFrom::Start(read_from)).ok()?;

    let mut buf = String::new();
    file.read_to_string(&mut buf).ok()?;

    // Find the most recent sessionId matching this project (or any of its worktrees)
    let root = resolve_root_project_path(project_path);
    let mut best_session: Option<(i64, String)> = None;

    for line in buf.lines() {
        let parsed: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let project = parsed.get("project").and_then(|v| v.as_str()).unwrap_or("");
        if !matches_project_family(project, root) {
            continue;
        }

        let session_id = match parsed.get("sessionId").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };

        let timestamp = parsed.get("timestamp").and_then(|v| v.as_i64()).unwrap_or(0);

        match &best_session {
            Some((best_ts, _)) if timestamp > *best_ts => {
                best_session = Some((timestamp, session_id));
            }
            None => {
                best_session = Some((timestamp, session_id));
            }
            _ => {}
        }
    }

    best_session.map(|(_, id)| id)
}

fn detect_cursor_session(project_path: &str) -> Option<String> {
    let cursor_chats_dir = dirs::home_dir()?.join(".cursor").join("chats");
    let root = resolve_root_project_path(project_path);
    let root_hash = cursor_project_hash(root);

    // Collect matching hash dirs (root + worktrees)
    let hash_dirs: Vec<PathBuf> = match fs::read_dir(&cursor_chats_dir) {
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .filter(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                e.path().is_dir() && (name == root_hash || name.starts_with(&format!("{}-.worktrees-", root_hash)))
            })
            .map(|e| e.path())
            .collect(),
        Err(_) => return None,
    };

    // Find the most recently modified chat directory across all matching hash dirs
    let mut best: Option<(std::time::SystemTime, String)> = None;

    for hash_dir in hash_dirs {
        let entries = match fs::read_dir(&hash_dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            if !entry.path().is_dir() {
                continue;
            }
            let store_db = entry.path().join("store.db");
            if let Ok(meta) = fs::metadata(&store_db) {
                if let Ok(modified) = meta.modified() {
                    let chat_id = entry.file_name().to_string_lossy().to_string();
                    match &best {
                        Some((best_time, _)) if modified > *best_time => {
                            best = Some((modified, chat_id));
                        }
                        None => {
                            best = Some((modified, chat_id));
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    best.map(|(_, id)| id)
}

// ── Tauri commands ──────────────────────────────────────────────────────

#[tauri::command]
pub fn chat_history_list(project_path: Option<String>) -> Result<Vec<ChatSession>, String> {
    let mut all = parse_claude_sessions(project_path.as_deref())?;
    let cursor_cli = parse_cursor_sessions(project_path.as_deref())?;
    let cursor_ide = parse_cursor_ide_sessions(project_path.as_deref())?;
    all.extend(cursor_cli);
    all.extend(cursor_ide);
    all.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    all.truncate(100);
    Ok(all)
}

#[tauri::command]
pub fn chat_history_list_for_project(project_path: String) -> Result<Vec<ChatSession>, String> {
    let mut all = parse_claude_sessions(Some(&project_path))?;
    let cursor_cli = parse_cursor_sessions(Some(&project_path))?;
    let cursor_ide = parse_cursor_ide_sessions(Some(&project_path))?;
    all.extend(cursor_cli);
    all.extend(cursor_ide);
    all.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    all.truncate(100);
    Ok(all)
}

// ── Storage path discovery ──────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatStoragePaths {
    pub claude_history_file: Option<String>,
    pub claude_sessions_dirs: Vec<String>,
    pub cursor_chats_dirs: Vec<String>,
}

/// Convert a project path to Claude's project hash format.
/// Same as Cursor: strip leading `/`, replace `/` with `-`.
fn claude_project_hash(project_path: &str) -> String {
    project_path
        .trim_start_matches('/')
        .replace('/', "-")
}

#[tauri::command]
pub fn chat_history_get_storage_paths(project_path: String) -> Result<ChatStoragePaths, String> {
    let home = dirs::home_dir().ok_or("Cannot determine home directory")?;
    let root = resolve_root_project_path(&project_path);
    let root_hash = claude_project_hash(root);

    let claude_history_file = {
        let p = home.join(".claude").join("history.jsonl");
        if p.exists() { Some(p.to_string_lossy().to_string()) } else { None }
    };

    // Collect Claude sessions dirs: root + any worktree dirs matching the prefix
    let claude_sessions_dirs = {
        let projects_dir = home.join(".claude").join("projects");
        match fs::read_dir(&projects_dir) {
            Ok(entries) => entries
                .filter_map(|e| e.ok())
                .filter(|e| {
                    let name = e.file_name().to_string_lossy().to_string();
                    e.path().is_dir() && (name == root_hash || name.starts_with(&format!("{}-.worktrees-", root_hash)))
                })
                .map(|e| e.path().to_string_lossy().to_string())
                .collect(),
            Err(_) => vec![],
        }
    };

    // Collect Cursor chats dirs: root + any worktree dirs matching the prefix
    let cursor_chats_dirs = {
        let chats_dir = home.join(".cursor").join("chats");
        match fs::read_dir(&chats_dir) {
            Ok(entries) => entries
                .filter_map(|e| e.ok())
                .filter(|e| {
                    let name = e.file_name().to_string_lossy().to_string();
                    e.path().is_dir() && (name == root_hash || name.starts_with(&format!("{}-.worktrees-", root_hash)))
                })
                .map(|e| e.path().to_string_lossy().to_string())
                .collect(),
            Err(_) => vec![],
        }
    };

    Ok(ChatStoragePaths {
        claude_history_file,
        claude_sessions_dirs,
        cursor_chats_dirs,
    })
}

/// Get all custom session names from the database.
/// Returns a map of "provider:sessionId" → custom_name.
#[tauri::command]
pub fn chat_history_get_custom_names(
    state: tauri::State<'_, crate::state::AppState>,
) -> Result<HashMap<String, String>, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare("SELECT provider, session_id, custom_name FROM chat_session_names")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            let provider: String = row.get(0)?;
            let session_id: String = row.get(1)?;
            let custom_name: String = row.get(2)?;
            Ok((format!("{}:{}", provider, session_id), custom_name))
        })
        .map_err(|e| e.to_string())?;

    let mut map = HashMap::new();
    for row in rows {
        if let Ok((key, name)) = row {
            map.insert(key, name);
        }
    }
    Ok(map)
}

/// Rename a chat session. Stores the custom name in our database.
#[tauri::command]
pub fn chat_history_rename_session(
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::state::AppState>,
    provider: String,
    session_id: String,
    custom_name: String,
) -> Result<(), String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT OR REPLACE INTO chat_session_names (provider, session_id, custom_name, updated_at) \
         VALUES (?1, ?2, ?3, unixepoch())",
        rusqlite::params![provider, session_id, custom_name],
    )
    .map_err(|e| e.to_string())?;
    let _ = app.emit("sync:chat-history", ());
    Ok(())
}

#[tauri::command]
pub fn chat_history_detect_active_session(
    provider: String,
    project_path: String,
) -> Result<Option<String>, String> {
    let session_id = match provider.as_str() {
        "claude" => detect_claude_session(&project_path),
        "cursor" => detect_cursor_session(&project_path),
        _ => None,
    };
    Ok(session_id)
}

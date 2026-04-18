use serde::{Deserialize, Serialize};
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
    /// The worktree branch name if this session was created in a worktree, else None.
    pub origin_branch: Option<String>,
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

/// Extract the worktree branch name from a project path, if present.
/// e.g. "/repo/.worktrees/feature-x" → Some("feature-x")
/// e.g. "/repo" → None
fn extract_worktree_branch(project: &str) -> Option<String> {
    project.find("/.worktrees/").map(|idx| project[idx + 12..].to_string())
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
                origin_branch: extract_worktree_branch(&acc.project),
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

    // Prefer lastUpdatedAt over createdAt for display timestamp
    let timestamp = parsed.get("lastUpdatedAt")
        .and_then(|v| v.as_i64())
        .or_else(|| parsed.get("createdAt").and_then(|v| v.as_i64()))
        .unwrap_or(0);

    Some((name, timestamp))
}

fn parse_cursor_sessions(project_filter: Option<&str>) -> Result<Vec<ChatSession>, String> {
    let cursor_chats_dir = match dirs::home_dir() {
        Some(h) => h.join(".cursor").join("chats"),
        None => return Ok(vec![]),
    };

    if !cursor_chats_dir.exists() {
        return Ok(vec![]);
    }

    // Deduplicate by session ID — the same UUID can appear under multiple
    // project hash directories. Keep the most recently modified one.
    let mut best_by_id: HashMap<String, ChatSession> = HashMap::new();

    // If a project filter is provided, only scan the matching hash directory
    // (hash = MD5 of absolute project path). Otherwise scan all.
    let hash_dirs: Vec<PathBuf> = if let Some(filter) = project_filter {
        let root = resolve_root_project_path(filter);
        let root_hash = md5_hex(root.as_bytes());
        let target_dir = cursor_chats_dir.join(&root_hash);
        if target_dir.is_dir() {
            vec![target_dir]
        } else {
            vec![]
        }
    } else {
        match fs::read_dir(&cursor_chats_dir) {
            Ok(entries) => entries
                .filter_map(|e| e.ok())
                .filter(|e| e.path().is_dir())
                .map(|e| e.path())
                .collect(),
            Err(_) => vec![],
        }
    };

    for hash_dir in hash_dirs {
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
                Some((name, meta_ts)) => {
                    // Use the metadata timestamp (lastUpdatedAt or createdAt).
                    // Only fall back to file mtime if meta has no timestamp.
                    let ts = if meta_ts > 0 {
                        meta_ts
                    } else {
                        fs::metadata(&store_db)
                            .ok()
                            .and_then(|m| m.modified().ok())
                            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                            .map(|d| d.as_millis() as i64)
                            .unwrap_or(0)
                    };
                    (name, ts)
                }
                None => {
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

            let session = ChatSession {
                session_id: chat_id.clone(),
                project: String::new(),
                title,
                timestamp,
                provider: "cursor".to_string(),
                message_count: 0,
                origin_branch: None, // Cursor CLI sessions don't embed project path
            };

            // Keep the entry with the best title and latest timestamp
            match best_by_id.get(&chat_id) {
                Some(existing) => {
                    // Prefer a real name over "New Agent" or generic fallback
                    let existing_is_generic = existing.title == "New Agent"
                        || existing.title.starts_with("Cursor session ")
                        || existing.title == "Untitled";
                    let new_is_named = session.title != "New Agent"
                        && !session.title.starts_with("Cursor session ")
                        && session.title != "Untitled";

                    if (new_is_named && existing_is_generic)
                        || (new_is_named == !existing_is_generic && session.timestamp > existing.timestamp)
                    {
                        best_by_id.insert(chat_id, session);
                    }
                }
                None => {
                    best_by_id.insert(chat_id, session);
                }
            }
        }
    }

    Ok(best_by_id.into_values().collect())
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
                origin_branch: extract_worktree_branch(&project_display),
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

    // Find the most recent sessionId matching this exact path first.
    // Only fall back to project-family matching if the input is the root project
    // (not a worktree or agent subdirectory).
    let is_subpath = project_path.contains("/.worktrees/") || project_path.contains("/.k2so/");
    let root = resolve_root_project_path(project_path);
    let mut best_session: Option<(i64, String)> = None;

    for line in buf.lines() {
        let parsed: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let project = parsed.get("project").and_then(|v| v.as_str()).unwrap_or("");
        // For subpaths (worktrees, agent dirs), require exact match
        // For root projects, allow any worktree under it
        if is_subpath {
            if project != project_path { continue; }
        } else {
            if !matches_project_family(project, root) { continue; }
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

    // Verify the session file actually exists on disk before returning.
    // A session ID appears in history.jsonl when Claude launches, but the
    // session .jsonl file is only written once a prompt is sent. If the user
    // opened a session but never typed anything, the file won't exist and
    // --resume would fail with "No conversation found".
    best_session.and_then(|(_, id)| {
        let home = dirs::home_dir()?;
        let project_hash = claude_project_hash(&resolve_root_project_path(project_path));
        let projects_dir = home.join(".claude").join("projects");

        // Check all matching project dirs (root + worktree variants)
        if let Ok(entries) = fs::read_dir(&projects_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name == project_hash || name.starts_with(&format!("{}-", project_hash)) {
                    let session_file = entry.path().join(format!("{}.jsonl", id));
                    if session_file.exists() {
                        return Some(id);
                    }
                }
            }
        }
        None
    })
}

/// Check whether a claude session file exists on disk for the given
/// session_id + project path (including any worktree siblings). Used before
/// `--resume` to avoid claude bailing with "No conversation found" when the
/// DB holds a stale session_id (workspace remove+readd, claude-side session
/// pruning, migrations, etc.). Returns false if the file is missing or the
/// projects directory isn't readable.
pub fn claude_session_file_exists(session_id: &str, project_path: &str) -> bool {
    let Some(home) = dirs::home_dir() else { return false };
    let project_hash = claude_project_hash(resolve_root_project_path(project_path));
    let projects_dir = home.join(".claude").join("projects");
    let Ok(entries) = fs::read_dir(&projects_dir) else { return false };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name == project_hash || name.starts_with(&format!("{}-", project_hash)) {
            if entry.path().join(format!("{}.jsonl", session_id)).exists() {
                return true;
            }
        }
    }
    false
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
    all.extend(cursor_cli);
    all.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    all.truncate(100);
    Ok(all)
}

#[tauri::command]
pub async fn chat_history_list_for_project(project_path: String) -> Result<Vec<ChatSession>, String> {
    tokio::task::spawn_blocking(move || {
        let mut all = parse_claude_sessions(Some(&project_path))?;
        let cursor_cli = parse_cursor_sessions(Some(&project_path))?;
        all.extend(cursor_cli);
        all.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
        all.truncate(100);
        Ok(all)
    })
    .await
    .map_err(|e| format!("chat_history task failed: {}", e))?
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
    // Claude Code converts project paths to directory names by replacing / with -
    // and stripping leading dots from path components (e.g. /.k2so → /-k2so → --k2so).
    // The leading / becomes a leading -, so the directory starts with -.
    // Example: /Users/z3thon/DevProjects/TestingK2SO/.k2so/agents/coordinator
    //       → -Users-z3thon-DevProjects-TestingK2SO--k2so-agents-coordinator
    project_path
        .replace("/.", "/-")  // /.hidden → /-hidden (strip dot, keep separator)
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
    let conn = state.db.lock();
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
/// Preserves the pinned state if the row already exists.
#[tauri::command]
pub fn chat_history_rename_session(
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::state::AppState>,
    provider: String,
    session_id: String,
    custom_name: String,
) -> Result<(), String> {
    let conn = state.db.lock();
    // Upsert: insert if new, update only custom_name if exists
    conn.execute(
        "INSERT INTO chat_session_names (provider, session_id, custom_name, pinned, updated_at) \
         VALUES (?1, ?2, ?3, 0, unixepoch()) \
         ON CONFLICT(provider, session_id) DO UPDATE SET custom_name = ?3, updated_at = unixepoch()",
        rusqlite::params![provider, session_id, custom_name],
    )
    .map_err(|e| e.to_string())?;
    drop(conn);
    let _ = app.emit("sync:chat-history", ());
    Ok(())
}

/// Get pinned session IDs as a set of "provider:sessionId" keys.
#[tauri::command]
pub fn chat_history_get_pinned(
    state: tauri::State<'_, crate::state::AppState>,
) -> Result<Vec<String>, String> {
    let conn = state.db.lock();
    let mut stmt = conn
        .prepare("SELECT provider, session_id FROM chat_session_names WHERE pinned = 1")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            let provider: String = row.get(0)?;
            let session_id: String = row.get(1)?;
            Ok(format!("{}:{}", provider, session_id))
        })
        .map_err(|e| e.to_string())?;

    let mut result = Vec::new();
    for row in rows {
        if let Ok(key) = row {
            result.push(key);
        }
    }
    Ok(result)
}

/// Pin or unpin a chat session.
#[tauri::command]
pub fn chat_history_toggle_pin(
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::state::AppState>,
    provider: String,
    session_id: String,
    pinned: bool,
) -> Result<(), String> {
    let conn = state.db.lock();
    let pinned_val: i64 = if pinned { 1 } else { 0 };
    // Upsert: create row if it doesn't exist, otherwise just update pinned
    conn.execute(
        "INSERT INTO chat_session_names (provider, session_id, custom_name, pinned, updated_at) \
         VALUES (?1, ?2, '', ?3, unixepoch()) \
         ON CONFLICT(provider, session_id) DO UPDATE SET pinned = ?3, updated_at = unixepoch()",
        rusqlite::params![provider, session_id, pinned_val],
    )
    .map_err(|e| e.to_string())?;
    drop(conn);
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

// ── Cursor IDE → CLI migration ─────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CursorIdeSession {
    pub composer_id: String,
    pub name: String,
    pub created_at: i64,
    pub last_updated_at: i64,
    pub mode: String,
    pub already_migrated: bool,
    pub migratable: bool,
}

/// Discover Cursor IDE sessions for a given project path that could be migrated
/// to the CLI format. Returns sessions found in workspaceStorage.
#[tauri::command]
pub fn chat_history_discover_ide_sessions(
    project_path: String,
) -> Result<Vec<CursorIdeSession>, String> {
    let home = dirs::home_dir().ok_or("Cannot determine home directory")?;
    let ws_storage = home.join("Library/Application Support/Cursor/User/workspaceStorage");
    if !ws_storage.exists() {
        return Ok(vec![]);
    }

    let cursor_chats_dir = home.join(".cursor").join("chats");
    let project_md5 = md5_hex(project_path.as_bytes());

    // Open globalStorage to check conversationState availability
    let global_db_path = home.join("Library/Application Support/Cursor/User/globalStorage/state.vscdb");
    let global_conn = if global_db_path.exists() {
        rusqlite::Connection::open_with_flags(
            &global_db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        ).ok()
    } else {
        None
    };

    // Find the workspace storage hash for this project
    let entries = fs::read_dir(&ws_storage).map_err(|e| e.to_string())?;
    let mut results = Vec::new();

    for entry in entries.flatten() {
        let ws_path = entry.path();
        if !ws_path.is_dir() {
            continue;
        }

        // Read workspace.json to match project
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

        // Decode URI to path
        let folder_path = percent_decode_uri(&folder_uri);

        // Check if this workspace matches our project
        let root = resolve_root_project_path(&project_path);
        if !matches_project_family(&folder_path, root) {
            continue;
        }

        // Read state.vscdb for composer data
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

        for composer in composers {
            let composer_id = match composer.get("composerId").and_then(|v| v.as_str()) {
                Some(id) => id.to_string(),
                None => continue,
            };

            // Skip archived, draft, ephemeral, subagents
            if composer.get("isArchived").and_then(|v| v.as_bool()).unwrap_or(false)
                || composer.get("isDraft").and_then(|v| v.as_bool()).unwrap_or(false)
                || composer.get("isEphemeral").and_then(|v| v.as_bool()).unwrap_or(false)
                || composer.get("createdFromBackgroundAgent").is_some()
                || composer.get("subagentInfo").is_some()
            {
                continue;
            }

            let name = composer
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("Untitled")
                .to_string();

            // Skip sessions without a name (usually empty/unused)
            if name.is_empty() || name == "Untitled" {
                // Check if it has any conversation at all
                let headers = composer.get("fullConversationHeadersOnly")
                    .and_then(|v| v.as_array())
                    .map(|a| a.len())
                    .unwrap_or(0);
                if headers == 0 {
                    continue;
                }
            }

            let created_at = composer.get("createdAt").and_then(|v| v.as_i64()).unwrap_or(0);
            let last_updated_at = composer.get("lastUpdatedAt").and_then(|v| v.as_i64()).unwrap_or(created_at);
            let mode = composer.get("unifiedMode").and_then(|v| v.as_str()).unwrap_or("agent").to_string();

            // Check if already migrated (scan all hash dirs, not just this project's)
            let already_migrated = cursor_chats_dir.exists() && fs::read_dir(&cursor_chats_dir)
                .ok()
                .map(|entries| entries
                    .filter_map(|e| e.ok())
                    .any(|e| e.path().join(&composer_id).join("store.db").exists()))
                .unwrap_or(false);

            // Check if conversationState exists in globalStorage (needed for migration)
            let migratable = if already_migrated {
                true // Already done
            } else if let Some(ref gc) = global_conn {
                let key = format!("composerData:{}", composer_id);
                gc.query_row(
                    "SELECT value FROM cursorDiskKV WHERE key = ?1",
                    rusqlite::params![key],
                    |row| row.get::<_, String>(0),
                ).ok()
                .and_then(|val| serde_json::from_str::<serde_json::Value>(&val).ok())
                .map(|data| {
                    let cs = data.get("conversationState")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    cs.len() > 10
                })
                .unwrap_or(false)
            } else {
                false
            };

            results.push(CursorIdeSession {
                composer_id,
                name,
                created_at,
                last_updated_at,
                mode,
                already_migrated,
                migratable,
            });
        }
    }

    // Sort by lastUpdatedAt descending
    results.sort_by(|a, b| b.last_updated_at.cmp(&a.last_updated_at));
    Ok(results)
}

/// Migrate Cursor IDE sessions to CLI format so they can be resumed with cursor-agent --resume.
/// Creates store.db files in ~/.cursor/chats/{md5(projectPath)}/{composerId}/
#[tauri::command]
pub fn chat_history_migrate_ide_sessions(
    project_path: String,
    composer_ids: Vec<String>,
) -> Result<usize, String> {
    let home = dirs::home_dir().ok_or("Cannot determine home directory")?;
    let ws_storage = home.join("Library/Application Support/Cursor/User/workspaceStorage");
    let global_db_path = home.join("Library/Application Support/Cursor/User/globalStorage/state.vscdb");
    let cursor_chats_dir = home.join(".cursor").join("chats");
    let project_md5 = md5_hex(project_path.as_bytes());

    if !ws_storage.exists() || !global_db_path.exists() {
        return Err("Cursor data not found".to_string());
    }

    // Open globalStorage to read blobs
    let global_conn = rusqlite::Connection::open_with_flags(
        &global_db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ).map_err(|e| format!("Failed to open globalStorage: {}", e))?;

    // Find the workspace storage for this project to read composerData
    let mut composer_data_map: HashMap<String, serde_json::Value> = HashMap::new();

    let entries = fs::read_dir(&ws_storage).map_err(|e| e.to_string())?;
    for entry in entries.flatten() {
        let ws_path = entry.path();
        if !ws_path.is_dir() { continue; }

        let ws_json_path = ws_path.join("workspace.json");
        let ws_json = match fs::read_to_string(&ws_json_path) { Ok(s) => s, Err(_) => continue };
        let ws_data: serde_json::Value = match serde_json::from_str(&ws_json) { Ok(v) => v, Err(_) => continue };
        let folder_uri = match ws_data.get("folder").and_then(|v| v.as_str()) { Some(f) => f, None => continue };
        let folder_path = percent_decode_uri(folder_uri);
        let root = resolve_root_project_path(&project_path);
        if !matches_project_family(&folder_path, root) { continue; }

        let state_db_path = ws_path.join("state.vscdb");
        if !state_db_path.exists() { continue; }

        let conn = match rusqlite::Connection::open_with_flags(
            &state_db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        ) { Ok(c) => c, Err(_) => continue };

        // Read individual composerData entries from globalStorage
        for cid in &composer_ids {
            let key = format!("composerData:{}", cid);
            if let Ok(value) = global_conn.query_row(
                "SELECT value FROM cursorDiskKV WHERE key = ?1",
                rusqlite::params![key],
                |row| row.get::<_, String>(0),
            ) {
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&value) {
                    composer_data_map.insert(cid.clone(), parsed);
                }
            }
        }

        // Also try reading from workspace state.vscdb composerData
        // (some sessions may have their conversationState here)
        if let Ok(composer_json) = conn.query_row(
            "SELECT value FROM ItemTable WHERE key = 'composer.composerData'",
            [],
            |row| row.get::<_, String>(0),
        ) {
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&composer_json) {
                if let Some(all) = parsed.get("allComposers").and_then(|v| v.as_array()) {
                    for c in all {
                        if let Some(cid) = c.get("composerId").and_then(|v| v.as_str()) {
                            if composer_ids.contains(&cid.to_string()) && !composer_data_map.contains_key(cid) {
                                // Read from globalStorage composerData
                                let key = format!("composerData:{}", cid);
                                if let Ok(value) = global_conn.query_row(
                                    "SELECT value FROM cursorDiskKV WHERE key = ?1",
                                    rusqlite::params![key],
                                    |row| row.get::<_, String>(0),
                                ) {
                                    if let Ok(p) = serde_json::from_str::<serde_json::Value>(&value) {
                                        composer_data_map.insert(cid.to_string(), p);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    let mut migrated_count = 0;

    for composer_id in &composer_ids {
        let data = match composer_data_map.get(composer_id) {
            Some(d) => d,
            None => {
                log_debug!("[migrate] No composerData found for {}", composer_id);
                continue;
            }
        };

        // Get the conversationState (base64-encoded root blob)
        let conversation_state = match data.get("conversationState").and_then(|v| v.as_str()) {
            Some(cs) if !cs.is_empty() => cs.to_string(),
            _ => {
                log_debug!("[migrate] No conversationState for {}", composer_id);
                continue;
            }
        };

        // Decode root blob — conversationState can be either:
        // - "~..." → base64-encoded (newer sessions)
        // - "0a20..." → hex-encoded (older sessions)
        let root_blob_data = if conversation_state.starts_with('~') {
            let cs_clean = conversation_state.trim_start_matches('~');
            let mut padded = cs_clean.to_string();
            let pad_len = (4 - padded.len() % 4) % 4;
            for _ in 0..pad_len {
                padded.push('=');
            }
            match base64_decode(&padded) {
                Some(d) if !d.is_empty() => d,
                _ => {
                    log_debug!("[migrate] Failed to base64-decode conversationState for {}", composer_id);
                    continue;
                }
            }
        } else {
            // Try hex decode
            let chars: Vec<char> = conversation_state.chars().collect();
            if chars.len() % 2 != 0 || chars.len() < 4 {
                log_debug!("[migrate] Invalid conversationState format for {}", composer_id);
                continue;
            }
            let mut bytes = Vec::with_capacity(chars.len() / 2);
            let mut valid = true;
            for chunk in chars.chunks(2) {
                let s: String = chunk.iter().collect();
                match u8::from_str_radix(&s, 16) {
                    Ok(b) => bytes.push(b),
                    Err(_) => { valid = false; break; }
                }
            }
            if !valid || bytes.is_empty() {
                log_debug!("[migrate] Failed to hex-decode conversationState for {}", composer_id);
                continue;
            }
            bytes
        };

        // Compute root blob hash
        let root_blob_id = sha256_hex(&root_blob_data);

        // Collect all child blob hashes recursively from the root blob
        let mut all_blob_hashes: Vec<String> = Vec::new();
        collect_all_blob_hashes(&root_blob_data, &mut all_blob_hashes);

        // Create the store.db
        let session_dir = cursor_chats_dir.join(&project_md5).join(composer_id);
        if let Err(e) = fs::create_dir_all(&session_dir) {
            log_debug!("[migrate] Failed to create dir for {}: {}", composer_id, e);
            continue;
        }

        let store_db_path = session_dir.join("store.db");
        let store_conn = match rusqlite::Connection::open(&store_db_path) {
            Ok(c) => c,
            Err(e) => {
                log_debug!("[migrate] Failed to create store.db for {}: {}", composer_id, e);
                continue;
            }
        };

        store_conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS blobs (id TEXT PRIMARY KEY, data BLOB);
             CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT);"
        ).map_err(|e| format!("Failed to create tables: {}", e))?;

        // Insert root blob
        let _ = store_conn.execute(
            "INSERT OR REPLACE INTO blobs (id, data) VALUES (?1, ?2)",
            rusqlite::params![root_blob_id, root_blob_data],
        );

        // Recursively copy ALL referenced blobs from globalStorage.
        // Use a queue to handle arbitrary nesting depth — each blob may
        // reference more blobs via protobuf hash fields.
        let mut copied: std::collections::HashSet<String> = std::collections::HashSet::new();
        copied.insert(root_blob_id.clone());
        let mut queue: std::collections::VecDeque<String> = all_blob_hashes.iter().cloned().collect();
        let mut blobs_copied = 0;

        while let Some(hash) = queue.pop_front() {
            if copied.contains(&hash) {
                continue;
            }
            copied.insert(hash.clone());

            let key = format!("agentKv:blob:{}", hash);
            if let Ok(blob_data) = global_conn.query_row(
                "SELECT value FROM cursorDiskKV WHERE key = ?1",
                rusqlite::params![key],
                |row| row.get::<_, Vec<u8>>(0),
            ) {
                let _ = store_conn.execute(
                    "INSERT OR REPLACE INTO blobs (id, data) VALUES (?1, ?2)",
                    rusqlite::params![hash, blob_data],
                );
                blobs_copied += 1;

                // Find sub-references in this blob and add to queue
                let mut sub_hashes: Vec<String> = Vec::new();
                collect_all_blob_hashes(&blob_data, &mut sub_hashes);
                for sub_hash in sub_hashes {
                    if !copied.contains(&sub_hash) {
                        queue.push_back(sub_hash);
                    }
                }
            }
        }

        // Write meta
        let name = data.get("name").and_then(|v| v.as_str()).unwrap_or("Migrated Session");
        let created_at = data.get("createdAt").and_then(|v| v.as_i64()).unwrap_or(0);
        let last_updated_at = data.get("lastUpdatedAt").and_then(|v| v.as_i64()).unwrap_or(created_at);
        let mode = data.get("unifiedMode").and_then(|v| v.as_str()).unwrap_or("default");

        // Build compact JSON matching cursor-agent's native format (no spaces)
        let meta_str = format!(
            "{{\"agentId\":\"{}\",\"latestRootBlobId\":\"{}\",\"name\":{},\"mode\":\"{}\",\"createdAt\":{},\"lastUsedModel\":\"composer-2-fast\"}}",
            composer_id,
            root_blob_id,
            serde_json::to_string(name).unwrap_or_else(|_| "\"Migrated Session\"".to_string()),
            mode,
            created_at,
        );
        let meta_hex = string_to_hex(&meta_str);
        let _ = store_conn.execute(
            "INSERT OR REPLACE INTO meta (key, value) VALUES ('0', ?1)",
            rusqlite::params![meta_hex],
        );

        log_debug!("[migrate] Migrated {} ({}) with {} blobs", composer_id, name, blobs_copied);
        migrated_count += 1;
    }

    Ok(migrated_count)
}

// ── Migration helpers ──────────────────────────────────────────────────

fn percent_decode_uri(uri: &str) -> String {
    uri.strip_prefix("file://").unwrap_or(uri)
        .replace("%20", " ")
        .replace("%28", "(")
        .replace("%29", ")")
        .replace("%5B", "[")
        .replace("%5D", "]")
        .replace("%23", "#")
        .replace("%25", "%")
}

/// Simple MD5 implementation for hashing project paths to match Cursor's directory naming.
/// Compute MD5 hash and return as a 32-char lowercase hex string.
/// Uses direct byte formatting (not u128) to preserve correct byte order.
fn md5_hex(data: &[u8]) -> String {
    let digest = md5_digest(data);
    digest.iter().map(|b| format!("{:02x}", b)).collect()
}

/// Compute raw MD5 digest as 16 bytes.
fn md5_digest(data: &[u8]) -> [u8; 16] {
    // Use the md5 algorithm
    let mut state: [u32; 4] = [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476];

    let s: [u32; 64] = [
        7,12,17,22,7,12,17,22,7,12,17,22,7,12,17,22,
        5,9,14,20,5,9,14,20,5,9,14,20,5,9,14,20,
        4,11,16,23,4,11,16,23,4,11,16,23,4,11,16,23,
        6,10,15,21,6,10,15,21,6,10,15,21,6,10,15,21,
    ];

    let k: [u32; 64] = [
        0xd76aa478,0xe8c7b756,0x242070db,0xc1bdceee,0xf57c0faf,0x4787c62a,0xa8304613,0xfd469501,
        0x698098d8,0x8b44f7af,0xffff5bb1,0x895cd7be,0x6b901122,0xfd987193,0xa679438e,0x49b40821,
        0xf61e2562,0xc040b340,0x265e5a51,0xe9b6c7aa,0xd62f105d,0x02441453,0xd8a1e681,0xe7d3fbc8,
        0x21e1cde6,0xc33707d6,0xf4d50d87,0x455a14ed,0xa9e3e905,0xfcefa3f8,0x676f02d9,0x8d2a4c8a,
        0xfffa3942,0x8771f681,0x6d9d6122,0xfde5380c,0xa4beea44,0x4bdecfa9,0xf6bb4b60,0xbebfbc70,
        0x289b7ec6,0xeaa127fa,0xd4ef3085,0x04881d05,0xd9d4d039,0xe6db99e5,0x1fa27cf8,0xc4ac5665,
        0xf4292244,0x432aff97,0xab9423a7,0xfc93a039,0x655b59c3,0x8f0ccc92,0xffeff47d,0x85845dd1,
        0x6fa87e4f,0xfe2ce6e0,0xa3014314,0x4e0811a1,0xf7537e82,0xbd3af235,0x2ad7d2bb,0xeb86d391,
    ];

    // Pre-processing: adding padding bits
    let orig_len = data.len();
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    let bit_len = (orig_len as u64).wrapping_mul(8);
    msg.extend_from_slice(&bit_len.to_le_bytes());

    // Process each 512-bit block
    for chunk in msg.chunks(64) {
        let mut m = [0u32; 16];
        for (i, word) in m.iter_mut().enumerate() {
            *word = u32::from_le_bytes([chunk[i*4], chunk[i*4+1], chunk[i*4+2], chunk[i*4+3]]);
        }

        let [mut a, mut b, mut c, mut d] = state;

        for i in 0..64 {
            let (f, g) = match i {
                0..=15 => ((b & c) | (!b & d), i),
                16..=31 => ((d & b) | (!d & c), (5*i + 1) % 16),
                32..=47 => (b ^ c ^ d, (3*i + 5) % 16),
                _ => (c ^ (b | !d), (7*i) % 16),
            };
            let temp = d;
            d = c;
            c = b;
            b = b.wrapping_add(
                (a.wrapping_add(f).wrapping_add(k[i]).wrapping_add(m[g]))
                    .rotate_left(s[i]),
            );
            a = temp;
        }

        state[0] = state[0].wrapping_add(a);
        state[1] = state[1].wrapping_add(b);
        state[2] = state[2].wrapping_add(c);
        state[3] = state[3].wrapping_add(d);
    }

    let mut result = [0u8; 16];
    for (i, word) in state.iter().enumerate() {
        result[i*4..i*4+4].copy_from_slice(&word.to_le_bytes());
    }
    result
}

fn sha256_hex(data: &[u8]) -> String {
    use std::io::Write;
    // Use rusqlite's bundled SQLite for SHA-256 would be complex,
    // so we use a simple implementation
    sha256_digest(data)
}

/// Minimal SHA-256 implementation
fn sha256_digest(data: &[u8]) -> String {
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
        0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
    ];

    let k: [u32; 64] = [
        0x428a2f98,0x71374491,0xb5c0fbcf,0xe9b5dba5,0x3956c25b,0x59f111f1,0x923f82a4,0xab1c5ed5,
        0xd807aa98,0x12835b01,0x243185be,0x550c7dc3,0x72be5d74,0x80deb1fe,0x9bdc06a7,0xc19bf174,
        0xe49b69c1,0xefbe4786,0x0fc19dc6,0x240ca1cc,0x2de92c6f,0x4a7484aa,0x5cb0a9dc,0x76f988da,
        0x983e5152,0xa831c66d,0xb00327c8,0xbf597fc7,0xc6e00bf3,0xd5a79147,0x06ca6351,0x14292967,
        0x27b70a85,0x2e1b2138,0x4d2c6dfc,0x53380d13,0x650a7354,0x766a0abb,0x81c2c92e,0x92722c85,
        0xa2bfe8a1,0xa81a664b,0xc24b8b70,0xc76c51a3,0xd192e819,0xd6990624,0xf40e3585,0x106aa070,
        0x19a4c116,0x1e376c08,0x2748774c,0x34b0bcb5,0x391c0cb3,0x4ed8aa4a,0x5b9cca4f,0x682e6ff3,
        0x748f82ee,0x78a5636f,0x84c87814,0x8cc70208,0x90befffa,0xa4506ceb,0xbef9a3f7,0xc67178f2,
    ];

    let orig_len = data.len();
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 { msg.push(0); }
    let bit_len = (orig_len as u64).wrapping_mul(8);
    msg.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in msg.chunks(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([chunk[i*4], chunk[i*4+1], chunk[i*4+2], chunk[i*4+3]]);
        }
        for i in 16..64 {
            let s0 = w[i-15].rotate_right(7) ^ w[i-15].rotate_right(18) ^ (w[i-15] >> 3);
            let s1 = w[i-2].rotate_right(17) ^ w[i-2].rotate_right(19) ^ (w[i-2] >> 10);
            w[i] = w[i-16].wrapping_add(s0).wrapping_add(w[i-7]).wrapping_add(s1);
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;

        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let temp1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(k[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            hh = g; g = f; f = e;
            e = d.wrapping_add(temp1);
            d = c; c = b; b = a;
            a = temp1.wrapping_add(temp2);
        }

        h[0] = h[0].wrapping_add(a); h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c); h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e); h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g); h[7] = h[7].wrapping_add(hh);
    }

    h.iter().map(|v| format!("{:08x}", v)).collect()
}

/// Extract ALL blob hash references from a protobuf blob.
/// Hashes are 32-byte length-delimited fields (any field number with length 0x20).
/// This covers field 1 (children), field 3 (summaries), field 8 (context), field 13, etc.
fn collect_all_blob_hashes(data: &[u8], out: &mut Vec<String>) {
    let mut i = 0;
    while i + 33 < data.len() {
        let wire_type = data[i] & 0x07;
        if wire_type == 2 && data[i + 1] == 0x20 {
            // Length-delimited field with exactly 32 bytes = blob hash reference
            let hash = data[i + 2..i + 34]
                .iter()
                .map(|b| format!("{:02x}", b))
                .collect::<String>();
            if !out.contains(&hash) {
                out.push(hash);
            }
            i += 34;
        } else if wire_type == 2 && i + 1 < data.len() {
            // Other length-delimited field — skip it
            let length = data[i + 1] as usize;
            if length < 128 {
                i += 2 + length;
            } else {
                i += 1;
            }
        } else if wire_type == 0 {
            // Varint — skip
            i += 1;
            while i < data.len() && data[i] & 0x80 != 0 {
                i += 1;
            }
            i += 1;
        } else {
            i += 1;
        }
    }
}

fn base64_decode(input: &str) -> Option<Vec<u8>> {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = Vec::new();
    let bytes: Vec<u8> = input.bytes().filter(|&b| b != b'\n' && b != b'\r' && b != b' ').collect();
    for chunk in bytes.chunks(4) {
        if chunk.len() < 2 { break; }
        let mut buf = [0u8; 4];
        let mut count = 0;
        for (i, &byte) in chunk.iter().enumerate() {
            if byte == b'=' { break; }
            match TABLE.iter().position(|&t| t == byte) {
                Some(pos) => { buf[i] = pos as u8; count = i + 1; }
                None => return None,
            }
        }
        if count >= 2 { output.push((buf[0] << 2) | (buf[1] >> 4)); }
        if count >= 3 { output.push((buf[1] << 4) | (buf[2] >> 2)); }
        if count >= 4 { output.push((buf[2] << 6) | buf[3]); }
    }
    Some(output)
}

fn string_to_hex(s: &str) -> String {
    s.bytes().map(|b| format!("{:02x}", b)).collect()
}

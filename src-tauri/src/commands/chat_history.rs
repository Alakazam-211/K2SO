use serde::Serialize;
use std::collections::HashMap;
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

        // Apply project filter early to avoid unnecessary work
        if let Some(filter) = project_filter {
            if project != filter {
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

fn parse_cursor_sessions(project_filter: Option<&str>) -> Result<Vec<ChatSession>, String> {
    let cursor_chats_dir = match dirs::home_dir() {
        Some(h) => h.join(".cursor").join("chats"),
        None => return Ok(vec![]),
    };

    if !cursor_chats_dir.exists() {
        return Ok(vec![]);
    }

    let mut results = Vec::new();

    // If project filter is provided, only check the matching hash directory
    let hash_dirs: Vec<PathBuf> = if let Some(filter) = project_filter {
        let hash = cursor_project_hash(filter);
        let dir = cursor_chats_dir.join(&hash);
        if dir.exists() { vec![dir] } else { vec![] }
    } else {
        // List all project hash directories
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

            // Get modification time of store.db as timestamp
            let store_db = chat_path.join("store.db");
            let timestamp = match fs::metadata(&store_db) {
                Ok(meta) => meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0),
                Err(_) => continue,
            };

            // Use a short title derived from the chat ID
            let short_id = if chat_id.len() > 8 { &chat_id[..8] } else { &chat_id };
            let title = format!("Cursor session {}", short_id);

            results.push(ChatSession {
                session_id: chat_id,
                project: project_name.clone(),
                title,
                timestamp,
                provider: "cursor".to_string(),
                message_count: 0, // We don't count messages for Cursor
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

    // Find the most recent sessionId matching this project
    let mut best_session: Option<(i64, String)> = None;

    for line in buf.lines() {
        let parsed: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let project = parsed.get("project").and_then(|v| v.as_str()).unwrap_or("");
        if project != project_path {
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
    let hash = cursor_project_hash(project_path);
    let hash_dir = cursor_chats_dir.join(&hash);

    if !hash_dir.exists() {
        return None;
    }

    // Find the most recently modified chat directory
    let mut best: Option<(std::time::SystemTime, String)> = None;

    let entries = fs::read_dir(&hash_dir).ok()?;
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

    best.map(|(_, id)| id)
}

// ── Tauri commands ──────────────────────────────────────────────────────

#[tauri::command]
pub fn chat_history_list(project_path: Option<String>) -> Result<Vec<ChatSession>, String> {
    let mut all = parse_claude_sessions(project_path.as_deref())?;
    let cursor = parse_cursor_sessions(project_path.as_deref())?;
    all.extend(cursor);
    all.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    all.truncate(100);
    Ok(all)
}

#[tauri::command]
pub fn chat_history_list_for_project(project_path: String) -> Result<Vec<ChatSession>, String> {
    let mut all = parse_claude_sessions(Some(&project_path))?;
    let cursor = parse_cursor_sessions(Some(&project_path))?;
    all.extend(cursor);
    all.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    all.truncate(100);
    Ok(all)
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

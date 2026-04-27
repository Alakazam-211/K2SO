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
// Pure session-detection helpers moved to k2so_core::chat_history so the
// daemon's post-spawn session-save path can call the same functions the
// Tauri UI does. Re-exported here under their historical unqualified
// names so the 20+ internal call sites below resolve unchanged.
use k2so_core::chat_history::{
    claude_history_path, claude_project_hash, claude_session_file_exists as core_claude_session_file_exists,
    cursor_project_hash, detect_active_session as core_detect_active_session,
    detect_claude_session, detect_cursor_session, matches_project_family, resolve_root_project_path,
};

// Re-export under the src-tauri path so `crate::commands::chat_history::
// claude_session_file_exists` keeps working for external callers (the
// wake path uses this to verify a session file before --resume).
pub use k2so_core::chat_history::claude_session_file_exists;

/// Extract the worktree branch name from a project path, if present.
/// e.g. "/repo/.worktrees/feature-x" → Some("feature-x")
/// e.g. "/repo" → None
fn extract_worktree_branch(project: &str) -> Option<String> {
    project.find("/.worktrees/").map(|idx| project[idx + 12..].to_string())
}

// ── Claude history parsing ───────────────────────────────────────────────

// `claude_history_path` moved to k2so_core::chat_history (re-exported).

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

// `cursor_project_hash` moved to k2so_core::chat_history (re-exported).

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

// ── Gemini chat parsing ─────────────────────────────────────────────────
//
// Layout (verified against a live install on macOS):
//
//   ~/.gemini/projects.json
//     { "projects": { "/abs/cwd": "<slug>", … } }
//   ~/.gemini/tmp/<slug>/chats/session-<iso>-<short-uuid>.jsonl
//     line 1: {"sessionId","projectHash","startTime","lastUpdated","kind":"main"}
//     subsequent: {"id","timestamp","type":"user"|"gemini","content":[{"text":…}],…}
//                 OR {"$set":{"lastUpdated":"…"}} mutation lines
//
// Key advantage over Codex: filename only contains an 8-char prefix of
// the uuid, so we MUST read line 1 to get the full `sessionId` for
// `gemini --resume <uuid>`. The `projects.json` slug map gives us the
// cwd → slug mapping for free, so per-project filtering is trivial.

/// Parse the Gemini RFC3339 timestamp shape ("2026-04-27T09:19:05.013Z")
/// into unix-epoch milliseconds. Returns None on any parse failure or
/// empty input.
fn parse_rfc3339_to_ms(s: &str) -> Option<i64> {
    if s.is_empty() {
        return None;
    }
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.timestamp_millis())
}

/// Read `~/.gemini/projects.json` and return a `slug → absolute_cwd` map.
/// Returns an empty map if the file is missing or unparseable.
fn gemini_slug_to_cwd_map() -> HashMap<String, String> {
    let mut map = HashMap::new();
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return map,
    };
    let projects_json = home.join(".gemini").join("projects.json");
    let content = match fs::read_to_string(&projects_json) {
        Ok(c) => c,
        Err(_) => return map,
    };
    let parsed: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return map,
    };
    if let Some(obj) = parsed.get("projects").and_then(|v| v.as_object()) {
        for (cwd, slug_v) in obj {
            if let Some(slug) = slug_v.as_str() {
                map.insert(slug.to_string(), cwd.clone());
            }
        }
    }
    map
}

fn parse_gemini_sessions(project_filter: Option<&str>) -> Result<Vec<ChatSession>, String> {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return Ok(vec![]),
    };
    let tmp_dir = home.join(".gemini").join("tmp");
    if !tmp_dir.exists() {
        return Ok(vec![]);
    }

    let slug_to_cwd = gemini_slug_to_cwd_map();

    // Pick the slugs to scan. With a project filter, match against the
    // filter's resolved root + any worktree path inside it (same family
    // semantics as the Claude path).
    let target_slugs: Vec<(String, String)> = if let Some(filter) = project_filter {
        let root = resolve_root_project_path(filter);
        slug_to_cwd
            .iter()
            .filter(|(_slug, cwd)| matches_project_family(cwd, root))
            .map(|(slug, cwd)| (slug.clone(), cwd.clone()))
            .collect()
    } else {
        slug_to_cwd
            .iter()
            .map(|(slug, cwd)| (slug.clone(), cwd.clone()))
            .collect()
    };

    let mut results = Vec::new();
    for (slug, cwd) in target_slugs {
        let chats_dir = tmp_dir.join(&slug).join("chats");
        if !chats_dir.is_dir() {
            continue;
        }
        let entries = match fs::read_dir(&chats_dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
                continue;
            }

            let file = match File::open(&path) {
                Ok(f) => f,
                Err(_) => continue,
            };
            let reader = BufReader::new(file);
            let mut lines = reader.lines();

            // Header line: {"sessionId","projectHash","startTime","lastUpdated","kind":"main"}.
            // Skip the file if the header is malformed or missing sessionId —
            // there's nothing useful to surface without a uuid for `--resume`.
            let header_line = match lines.next() {
                Some(Ok(l)) => l,
                _ => continue,
            };
            let header: serde_json::Value = match serde_json::from_str(&header_line) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let session_id = match header.get("sessionId").and_then(|v| v.as_str()) {
                Some(s) => s.to_string(),
                None => continue,
            };

            let mut latest_ts_ms = parse_rfc3339_to_ms(
                header.get("lastUpdated").and_then(|v| v.as_str()).unwrap_or(""),
            )
            .unwrap_or(0);
            let start_ts_ms = parse_rfc3339_to_ms(
                header.get("startTime").and_then(|v| v.as_str()).unwrap_or(""),
            )
            .unwrap_or(0);

            // Walk the body. First user message wins for the title; track
            // every $set mutation to keep `latest_ts_ms` honest.
            let mut title = String::new();
            let mut message_count: usize = 0;
            for line in lines.flatten() {
                let parsed: serde_json::Value = match serde_json::from_str(&line) {
                    Ok(v) => v,
                    Err(_) => continue,
                };

                // Mutation line: {"$set":{"lastUpdated":"…"}} — bumps the timestamp.
                if let Some(set) = parsed.get("$set") {
                    if let Some(s) = set.get("lastUpdated").and_then(|v| v.as_str()) {
                        if let Some(ms) = parse_rfc3339_to_ms(s) {
                            if ms > latest_ts_ms {
                                latest_ts_ms = ms;
                            }
                        }
                    }
                    continue;
                }

                let msg_type = parsed.get("type").and_then(|v| v.as_str()).unwrap_or("");
                if msg_type != "user" && msg_type != "gemini" {
                    continue;
                }
                message_count += 1;

                if title.is_empty() && msg_type == "user" {
                    let content = parsed.get("content");
                    let extracted = if let Some(arr) = content.and_then(|v| v.as_array()) {
                        arr.iter()
                            .find_map(|item| item.get("text").and_then(|v| v.as_str()))
                            .map(String::from)
                    } else {
                        content.and_then(|v| v.as_str()).map(String::from)
                    };
                    if let Some(s) = extracted {
                        title = s.trim().to_string();
                    }
                }
            }

            // Timestamp fallback chain: latest $set / lastUpdated → startTime → file mtime → 0.
            let timestamp = if latest_ts_ms > 0 {
                latest_ts_ms
            } else if start_ts_ms > 0 {
                start_ts_ms
            } else {
                fs::metadata(&path)
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0)
            };

            let truncated_title = if title.is_empty() {
                "Untitled".to_string()
            } else if title.chars().count() > 80 {
                let truncated: String = title.chars().take(77).collect();
                format!("{}...", truncated)
            } else {
                title
            };

            results.push(ChatSession {
                session_id,
                project: cwd.clone(),
                origin_branch: extract_worktree_branch(&cwd),
                title: truncated_title,
                timestamp,
                provider: "gemini".to_string(),
                message_count,
            });
        }
    }

    Ok(results)
}

// ── Pi chat parsing ─────────────────────────────────────────────────────
//
// Layout (verified against a live install on macOS — pi-mono source at
// https://github.com/badlogic/pi-mono):
//
//   ~/.pi/agent/sessions/<cwd-slug>/<ISO-ts>_<uuidv7>.jsonl
//     line 1: {"type":"session","id":"<uuid>","cwd":"/abs/path","timestamp":"<ISO>","version":…}
//     subsequent: {"type":"message","message":{"role":"user"|"assistant","content":[{"type":"text","text":…}]}}
//                 plus thinking_level_change / model_change / tool_use lines.
//
// The slug encoding is `--<cwd-with-/-replaced-by->--` (literal spaces
// preserved) — but we don't depend on it. Instead we walk every slug
// dir and read the literal `cwd` from line 1, which is more robust
// across worktree layouts and any encoding edge case.

fn parse_pi_sessions(project_filter: Option<&str>) -> Result<Vec<ChatSession>, String> {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return Ok(vec![]),
    };
    let sessions_root = home.join(".pi").join("agent").join("sessions");
    if !sessions_root.exists() {
        return Ok(vec![]);
    }

    let filter_root = project_filter.map(resolve_root_project_path);

    let slug_dirs = match fs::read_dir(&sessions_root) {
        Ok(e) => e,
        Err(_) => return Ok(vec![]),
    };

    let mut results = Vec::new();
    for slug_entry in slug_dirs.filter_map(|e| e.ok()) {
        let slug_path = slug_entry.path();
        if !slug_path.is_dir() {
            continue;
        }

        let session_files = match fs::read_dir(&slug_path) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for f_entry in session_files.filter_map(|e| e.ok()) {
            let path = f_entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
                continue;
            }

            let file = match File::open(&path) {
                Ok(f) => f,
                Err(_) => continue,
            };
            let reader = BufReader::new(file);
            let mut lines = reader.lines();

            // Line 1: session header. Skip the file if missing required fields.
            let header_line = match lines.next() {
                Some(Ok(l)) => l,
                _ => continue,
            };
            let header: serde_json::Value = match serde_json::from_str(&header_line) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if header.get("type").and_then(|v| v.as_str()) != Some("session") {
                continue;
            }
            let session_id = match header.get("id").and_then(|v| v.as_str()) {
                Some(s) => s.to_string(),
                None => continue,
            };
            let cwd = header
                .get("cwd")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            // Filter against the project family (root + worktrees).
            if let Some(ref root) = filter_root {
                if !matches_project_family(&cwd, root) {
                    continue;
                }
            }

            let mut latest_ts_ms = parse_rfc3339_to_ms(
                header.get("timestamp").and_then(|v| v.as_str()).unwrap_or(""),
            )
            .unwrap_or(0);

            // Walk body for first user message + latest timestamp.
            let mut title = String::new();
            let mut message_count: usize = 0;
            for line in lines.flatten() {
                let parsed: serde_json::Value = match serde_json::from_str(&line) {
                    Ok(v) => v,
                    Err(_) => continue,
                };

                if let Some(ts) = parsed.get("timestamp").and_then(|v| v.as_str()) {
                    if let Some(ms) = parse_rfc3339_to_ms(ts) {
                        if ms > latest_ts_ms {
                            latest_ts_ms = ms;
                        }
                    }
                }

                if parsed.get("type").and_then(|v| v.as_str()) != Some("message") {
                    continue;
                }
                let msg = match parsed.get("message") {
                    Some(m) => m,
                    None => continue,
                };
                let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("");
                if role != "user" && role != "assistant" {
                    continue;
                }
                message_count += 1;

                if title.is_empty() && role == "user" {
                    let extracted = msg
                        .get("content")
                        .and_then(|c| c.as_array())
                        .and_then(|arr| {
                            arr.iter().find_map(|item| {
                                if item.get("type").and_then(|v| v.as_str()) == Some("text") {
                                    item.get("text").and_then(|v| v.as_str()).map(String::from)
                                } else {
                                    None
                                }
                            })
                        });
                    if let Some(s) = extracted {
                        title = s.trim().to_string();
                    }
                }
            }

            // Timestamp fallback: file mtime if everything else failed.
            let timestamp = if latest_ts_ms > 0 {
                latest_ts_ms
            } else {
                fs::metadata(&path)
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0)
            };

            let truncated_title = if title.is_empty() {
                "Untitled".to_string()
            } else if title.chars().count() > 80 {
                let truncated: String = title.chars().take(77).collect();
                format!("{}...", truncated)
            } else {
                title
            };

            results.push(ChatSession {
                session_id,
                project: cwd.clone(),
                origin_branch: extract_worktree_branch(&cwd),
                title: truncated_title,
                timestamp,
                provider: "pi".to_string(),
                message_count,
            });
        }
    }

    Ok(results)
}

// ── Codex chat parsing ──────────────────────────────────────────────────
//
// Layout (Codex 0.125+, dated partitioning):
//   ~/.codex/sessions/YYYY/MM/DD/rollout-<ISO>-<uuidv7>.jsonl
//     line 1: {"type":"session_meta","payload":{"id":"<uuid>","timestamp":"<ISO>",
//                                               "cwd":"/abs/path","originator":"codex-tui",…}}
//     subsequent: turn_context, response_item, event_msg
//   ~/.codex/history.jsonl  — flat global index, one line per user prompt:
//     {"session_id":"<uuid>","ts":<unix-secs>,"text":"<prompt>"}
//
// Title source: read `~/.codex/history.jsonl`, group by session_id, take
// the EARLIEST entry per session. The rollout file's first user message
// is polluted by an injected AGENTS.md blob, so the flat history is the
// clean source.
//
// Resume: `codex resume <uuid>` (since v0.125 — earlier versions had
// only `experimental_resume="<path>"`). Subcommand position matters,
// so the click handler builds args as [resume, uuid] without preset
// args.

/// Read all entries from `~/.codex/history.jsonl`. Returns a map of
/// `session_id → (earliest_ts_secs, prompt_text)`. Returns empty map
/// if the file is missing — codex may not have written it yet.
fn codex_history_index() -> HashMap<String, (i64, String)> {
    let mut map: HashMap<String, (i64, String)> = HashMap::new();
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return map,
    };
    let path = home.join(".codex").join("history.jsonl");
    let file = match File::open(&path) {
        Ok(f) => f,
        Err(_) => return map,
    };
    for line in BufReader::new(file).lines().flatten() {
        let parsed: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let sid = match parsed.get("session_id").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };
        let ts = parsed.get("ts").and_then(|v| v.as_i64()).unwrap_or(0);
        let text = parsed
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        map.entry(sid)
            .and_modify(|(existing_ts, existing_text)| {
                if ts < *existing_ts || existing_text.is_empty() {
                    *existing_ts = ts;
                    *existing_text = text.clone();
                }
            })
            .or_insert((ts, text));
    }
    map
}

fn parse_codex_sessions(project_filter: Option<&str>) -> Result<Vec<ChatSession>, String> {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return Ok(vec![]),
    };
    let sessions_root = home.join(".codex").join("sessions");
    if !sessions_root.exists() {
        return Ok(vec![]);
    }

    let filter_root = project_filter.map(resolve_root_project_path);
    let title_index = codex_history_index();
    let mut results = Vec::new();

    // Walk YYYY/MM/DD/*.jsonl. Codex's older flat layout is no longer
    // produced by 0.125+, so we don't need to handle it here.
    let years = match fs::read_dir(&sessions_root) {
        Ok(e) => e,
        Err(_) => return Ok(vec![]),
    };
    for year_entry in years.flatten() {
        if !year_entry.path().is_dir() {
            continue;
        }
        let months = match fs::read_dir(year_entry.path()) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for month_entry in months.flatten() {
            if !month_entry.path().is_dir() {
                continue;
            }
            let days = match fs::read_dir(month_entry.path()) {
                Ok(e) => e,
                Err(_) => continue,
            };
            for day_entry in days.flatten() {
                if !day_entry.path().is_dir() {
                    continue;
                }
                let files = match fs::read_dir(day_entry.path()) {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                for f_entry in files.flatten() {
                    let path = f_entry.path();
                    if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
                        continue;
                    }

                    let file = match File::open(&path) {
                        Ok(f) => f,
                        Err(_) => continue,
                    };
                    let mut reader = BufReader::new(file);
                    let mut first_line = String::new();
                    if reader.read_line(&mut first_line).is_err() {
                        continue;
                    }
                    let header: serde_json::Value = match serde_json::from_str(first_line.trim()) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };
                    if header.get("type").and_then(|v| v.as_str()) != Some("session_meta") {
                        continue;
                    }
                    let payload = match header.get("payload") {
                        Some(p) => p,
                        None => continue,
                    };
                    let session_id = match payload.get("id").and_then(|v| v.as_str()) {
                        Some(s) => s.to_string(),
                        None => continue,
                    };
                    let cwd = payload
                        .get("cwd")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    if let Some(ref root) = filter_root {
                        if !matches_project_family(&cwd, root) {
                            continue;
                        }
                    }

                    // Timestamp: prefer file mtime since codex updates it
                    // on every turn; fall back to header's timestamp.
                    let mtime_ms = fs::metadata(&path)
                        .ok()
                        .and_then(|m| m.modified().ok())
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_millis() as i64)
                        .unwrap_or(0);
                    let header_ms = parse_rfc3339_to_ms(
                        payload.get("timestamp").and_then(|v| v.as_str()).unwrap_or(""),
                    )
                    .unwrap_or(0);
                    let timestamp = if mtime_ms > 0 { mtime_ms } else { header_ms };

                    // Title: prefer history.jsonl (clean); fall back to
                    // a generic placeholder if the user hasn't typed
                    // anything yet (still creates an entry so resume
                    // works).
                    let raw_title = title_index
                        .get(&session_id)
                        .map(|(_, t)| t.as_str())
                        .unwrap_or("");
                    let truncated_title = if raw_title.is_empty() {
                        format!("Codex session {}", &session_id[..8.min(session_id.len())])
                    } else if raw_title.chars().count() > 80 {
                        let truncated: String = raw_title.chars().take(77).collect();
                        format!("{}...", truncated)
                    } else {
                        raw_title.to_string()
                    };

                    results.push(ChatSession {
                        session_id,
                        project: cwd.clone(),
                        origin_branch: extract_worktree_branch(&cwd),
                        title: truncated_title,
                        timestamp,
                        provider: "codex".to_string(),
                        message_count: 0,
                    });
                }
            }
        }
    }

    Ok(results)
}

// ── Session detection for resume ──────────────────────────────────────── moved to k2so_core::chat_history (re-exported).

/// Check whether a claude session file exists on disk for the given
/// session_id + project path (including any worktree siblings). Used before
/// `--resume` to avoid claude bailing with "No conversation found" when the
/// DB holds a stale session_id (workspace remove+readd, claude-side session
/// pruning, migrations, etc.). Returns false if the file is missing or the
/// projects directory isn't readable.
// pub fn claude_session_file_exists moved to k2so_core::chat_history (re-exported).

// fn detect_cursor_session moved to k2so_core::chat_history (re-exported).

// ── Tauri commands ──────────────────────────────────────────────────────

#[tauri::command]
pub fn chat_history_list(project_path: Option<String>) -> Result<Vec<ChatSession>, String> {
    let mut all = parse_claude_sessions(project_path.as_deref())?;
    all.extend(parse_cursor_sessions(project_path.as_deref())?);
    all.extend(parse_gemini_sessions(project_path.as_deref())?);
    all.extend(parse_pi_sessions(project_path.as_deref())?);
    all.extend(parse_codex_sessions(project_path.as_deref())?);
    all.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    all.truncate(100);
    Ok(all)
}

#[tauri::command]
pub async fn chat_history_list_for_project(project_path: String) -> Result<Vec<ChatSession>, String> {
    tokio::task::spawn_blocking(move || {
        let mut all = parse_claude_sessions(Some(&project_path))?;
        all.extend(parse_cursor_sessions(Some(&project_path))?);
        all.extend(parse_gemini_sessions(Some(&project_path))?);
        all.extend(parse_pi_sessions(Some(&project_path))?);
        all.extend(parse_codex_sessions(Some(&project_path))?);
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
    pub gemini_chats_dirs: Vec<String>,
    pub pi_chats_dirs: Vec<String>,
    pub codex_sessions_dirs: Vec<String>,
    pub codex_history_file: Option<String>,
}

// Convert a project path to Claude's project hash format. moved to k2so_core::chat_history (re-exported).

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

    // Gemini chats: re-use the slug map. Each `cwd → slug` whose cwd is
    // in the same project family as our root contributes its
    // `~/.gemini/tmp/<slug>/chats` dir.
    let gemini_chats_dirs = {
        let tmp_dir = home.join(".gemini").join("tmp");
        let slug_to_cwd = gemini_slug_to_cwd_map();
        slug_to_cwd
            .iter()
            .filter(|(_slug, cwd)| matches_project_family(cwd, root))
            .map(|(slug, _cwd)| tmp_dir.join(slug).join("chats").to_string_lossy().to_string())
            .filter(|p| std::path::Path::new(p).is_dir())
            .collect()
    };

    // Pi chats: walk every slug folder under ~/.pi/agent/sessions and
    // keep the ones whose first-line cwd matches our project family.
    // (Pi's slug encoding is reverseable but reading line 1 is more
    // robust across worktree layouts and any encoding edge case.)
    let pi_chats_dirs = {
        let sessions_root = home.join(".pi").join("agent").join("sessions");
        let mut out: Vec<String> = Vec::new();
        if let Ok(entries) = fs::read_dir(&sessions_root) {
            for slug_entry in entries.filter_map(|e| e.ok()) {
                let slug_path = slug_entry.path();
                if !slug_path.is_dir() {
                    continue;
                }
                // Peek at any session file to read its declared cwd.
                let session_files = match fs::read_dir(&slug_path) {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                let mut matched = false;
                for f_entry in session_files.filter_map(|e| e.ok()) {
                    let p = f_entry.path();
                    if p.extension().and_then(|s| s.to_str()) != Some("jsonl") {
                        continue;
                    }
                    if let Ok(file) = File::open(&p) {
                        let mut reader = BufReader::new(file);
                        let mut first = String::new();
                        if reader.read_line(&mut first).is_ok() {
                            if let Ok(header) = serde_json::from_str::<serde_json::Value>(first.trim()) {
                                if let Some(cwd) = header.get("cwd").and_then(|v| v.as_str()) {
                                    if matches_project_family(cwd, root) {
                                        matched = true;
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
                if matched {
                    out.push(slug_path.to_string_lossy().to_string());
                }
            }
        }
        out
    };

    // Codex storage paths. Sessions live under YYYY/MM/DD/ partitions;
    // we surface every dated dir whose direct *.jsonl files include at
    // least one session whose cwd matches our project family.
    let codex_history_file = {
        let p = home.join(".codex").join("history.jsonl");
        if p.exists() { Some(p.to_string_lossy().to_string()) } else { None }
    };
    let codex_sessions_dirs = {
        let mut out = Vec::new();
        let sessions_root = home.join(".codex").join("sessions");
        if let Ok(years) = fs::read_dir(&sessions_root) {
            for year_entry in years.filter_map(|e| e.ok()) {
                if !year_entry.path().is_dir() { continue; }
                if let Ok(months) = fs::read_dir(year_entry.path()) {
                    for month_entry in months.filter_map(|e| e.ok()) {
                        if !month_entry.path().is_dir() { continue; }
                        if let Ok(days) = fs::read_dir(month_entry.path()) {
                            for day_entry in days.filter_map(|e| e.ok()) {
                                let day_path = day_entry.path();
                                if !day_path.is_dir() { continue; }
                                let mut matched = false;
                                if let Ok(files) = fs::read_dir(&day_path) {
                                    for f in files.filter_map(|e| e.ok()) {
                                        let p = f.path();
                                        if p.extension().and_then(|s| s.to_str()) != Some("jsonl") { continue; }
                                        if let Ok(file) = File::open(&p) {
                                            let mut reader = BufReader::new(file);
                                            let mut first = String::new();
                                            if reader.read_line(&mut first).is_ok() {
                                                if let Ok(v) = serde_json::from_str::<serde_json::Value>(first.trim()) {
                                                    if let Some(cwd) = v.get("payload").and_then(|p| p.get("cwd")).and_then(|v| v.as_str()) {
                                                        if matches_project_family(cwd, root) {
                                                            matched = true;
                                                            break;
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                                if matched {
                                    out.push(day_path.to_string_lossy().to_string());
                                }
                            }
                        }
                    }
                }
            }
        }
        out
    };

    Ok(ChatStoragePaths {
        claude_history_file,
        claude_sessions_dirs,
        cursor_chats_dirs,
        gemini_chats_dirs,
        pi_chats_dirs,
        codex_sessions_dirs,
        codex_history_file,
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
    core_detect_active_session(&provider, &project_path)
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

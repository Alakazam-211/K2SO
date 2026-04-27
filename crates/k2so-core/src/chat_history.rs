//! Chat-history helpers shared between Tauri + daemon.
//!
//! When a heartbeat wake spawns `claude` via [`crate::agents::wake::
//! spawn_wake_headless`], we need to save the provider's new session
//! ID on the `agent_sessions.session_id` row ~5 seconds later so the
//! *next* wake can `--resume <id>` into the same chat instead of
//! starting fresh. That requires scanning the provider's own history
//! file (Claude: `~/.claude/history.jsonl`; Cursor: `~/.cursor/chats/
//! <hash>/*/store.db`) to find the most recent session for this
//! project path.
//!
//! The scan is pure filesystem — zero Tauri dependencies — so it lives
//! here and gets called from both the daemon's wake path and the
//! Tauri-app UI's session-rediscovery code. The corresponding
//! `#[tauri::command]` wrapper in `src-tauri/src/commands/chat_history.rs`
//! is now a three-line forward.

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;

/// Strip a `/.worktrees/<branch>` suffix to get the root project path.
/// `/repo/.worktrees/feature-x` -> `/repo`. Used so every worktree of
/// the same project participates in session discovery.
pub fn resolve_root_project_path(path: &str) -> &str {
    if let Some(idx) = path.find("/.worktrees/") {
        &path[..idx]
    } else {
        path
    }
}

/// Does `session_project` belong to the `root` project family — the
/// root itself OR any worktree under it?
pub fn matches_project_family(session_project: &str, root: &str) -> bool {
    session_project == root
        || session_project.starts_with(&format!("{}/.worktrees/", root))
}

/// `~/.claude/history.jsonl` — where Claude Code appends one JSON
/// object per launch-prompt pair. `None` only if we can't find the
/// user's home directory.
pub fn claude_history_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude").join("history.jsonl"))
}

/// Convert a project path to Claude's project-hash directory name.
/// Claude Code turns `/Users/.../TestingK2SO/.k2so/agents/foo` into
/// `-Users-...-TestingK2SO--k2so-agents-foo` — leading `/` → `-`,
/// `/.` → `/-` (hidden dir prefix preserved), remaining `/` → `-`.
pub fn claude_project_hash(project_path: &str) -> String {
    project_path.replace("/.", "/-").replace('/', "-")
}

/// Convert a project path to Cursor's chat-directory hash.
/// `Users-z3thon-DevProjects-K2SO` — strip leading `/`, then slashes
/// to dashes.
pub fn cursor_project_hash(project_path: &str) -> String {
    project_path.trim_start_matches('/').replace('/', "-")
}

/// Return the most recent Claude session ID for a project path.
///
/// Reads the last 64 KiB of `~/.claude/history.jsonl` (cap against a
/// pathologically large file), filters by project family, picks the
/// highest-timestamp entry, then verifies the corresponding session
/// JSONL file actually exists on disk before returning.
///
/// The existence check matters: a session ID appears in history the
/// moment Claude launches, but the session `.jsonl` file is only
/// written once the user submits a prompt. If the user opened the
/// session but never typed, `--resume <id>` would fail with "No
/// conversation found".
pub fn detect_claude_session(project_path: &str) -> Option<String> {
    let path = claude_history_path()?;
    let file = File::open(&path).ok()?;

    let metadata = file.metadata().ok()?;
    let file_size = metadata.len();
    let read_from = if file_size > 65536 {
        file_size - 65536
    } else {
        0
    };

    let mut file = file;
    file.seek(SeekFrom::Start(read_from)).ok()?;

    let mut buf = String::new();
    file.read_to_string(&mut buf).ok()?;

    let is_subpath =
        project_path.contains("/.worktrees/") || project_path.contains("/.k2so/");
    let root = resolve_root_project_path(project_path);
    let mut best_session: Option<(i64, String)> = None;

    for line in buf.lines() {
        let parsed: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let project = parsed.get("project").and_then(|v| v.as_str()).unwrap_or("");
        if is_subpath {
            if project != project_path {
                continue;
            }
        } else if !matches_project_family(project, root) {
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

    best_session.and_then(|(_, id)| {
        let home = dirs::home_dir()?;
        let project_hash = claude_project_hash(resolve_root_project_path(project_path));
        let projects_dir = home.join(".claude").join("projects");

        if let Ok(entries) = fs::read_dir(&projects_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name == project_hash
                    || name.starts_with(&format!("{}-", project_hash))
                {
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

/// Return the Claude session ID for a project path whose history
/// entry timestamp is **closest to `target_ms`**.
///
/// Disambiguates concurrent spawns: when two heartbeats fire on the
/// same agent within a short window, each spawn's deferred-save
/// thread calls this with its own spawn timestamp, picking the
/// session id whose creation is nearest in time to its own spawn.
/// Without this, both threads would race-pick the highest-timestamp
/// session via [`detect_claude_session`] and stamp the same id on
/// both heartbeat rows.
///
/// `target_ms` is unix-epoch milliseconds (e.g.
/// `chrono::Utc::now().timestamp_millis()` captured at spawn).
/// Considers only sessions whose history timestamp is in
/// `[target_ms - 60_000, target_ms + 60_000]` so we don't pick up
/// an unrelated old session if Claude failed to write history at
/// all for the new spawn.
pub fn detect_claude_session_near(
    project_path: &str,
    target_ms: i64,
) -> Option<String> {
    let path = claude_history_path()?;
    let file = File::open(&path).ok()?;

    let metadata = file.metadata().ok()?;
    let file_size = metadata.len();
    let read_from = if file_size > 65536 {
        file_size - 65536
    } else {
        0
    };

    let mut file = file;
    file.seek(SeekFrom::Start(read_from)).ok()?;

    let mut buf = String::new();
    file.read_to_string(&mut buf).ok()?;

    let is_subpath =
        project_path.contains("/.worktrees/") || project_path.contains("/.k2so/");
    let root = resolve_root_project_path(project_path);
    const WINDOW_MS: i64 = 60_000;
    let mut best: Option<(i64, String)> = None; // (|distance|, id)

    for line in buf.lines() {
        let parsed: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let project = parsed.get("project").and_then(|v| v.as_str()).unwrap_or("");
        if is_subpath {
            if project != project_path {
                continue;
            }
        } else if !matches_project_family(project, root) {
            continue;
        }

        let session_id = match parsed.get("sessionId").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };

        let ts = parsed.get("timestamp").and_then(|v| v.as_i64()).unwrap_or(0);
        // Claude history timestamps in this codebase are observed in
        // milliseconds. Defensive: if the value looks like seconds,
        // upscale.
        let ts_ms = if ts < 10_000_000_000 { ts * 1000 } else { ts };

        let distance = (ts_ms - target_ms).abs();
        if distance > WINDOW_MS {
            continue;
        }

        match &best {
            Some((bd, _)) if distance < *bd => {
                best = Some((distance, session_id));
            }
            None => {
                best = Some((distance, session_id));
            }
            _ => {}
        }
    }

    best.and_then(|(_, id)| {
        let home = dirs::home_dir()?;
        let project_hash = claude_project_hash(resolve_root_project_path(project_path));
        let projects_dir = home.join(".claude").join("projects");
        if let Ok(entries) = fs::read_dir(&projects_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name == project_hash
                    || name.starts_with(&format!("{}-", project_hash))
                {
                    if entry.path().join(format!("{}.jsonl", id)).exists() {
                        return Some(id);
                    }
                }
            }
        }
        None
    })
}

/// Does a Claude session `.jsonl` file exist on disk for this
/// `session_id` + `project_path` (including any worktree siblings)?
/// Used before a `--resume` to avoid "No conversation found" when
/// the DB holds a stale session_id (workspace remove+readd,
/// Claude-side pruning, migrations, etc.).
pub fn claude_session_file_exists(session_id: &str, project_path: &str) -> bool {
    let Some(home) = dirs::home_dir() else {
        return false;
    };
    let project_hash = claude_project_hash(resolve_root_project_path(project_path));
    let projects_dir = home.join(".claude").join("projects");
    let Ok(entries) = fs::read_dir(&projects_dir) else {
        return false;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name == project_hash || name.starts_with(&format!("{}-", project_hash)) {
            if entry
                .path()
                .join(format!("{}.jsonl", session_id))
                .exists()
            {
                return true;
            }
        }
    }
    false
}

/// Return the most recent Cursor chat ID for a project path (by
/// store.db modification time across all matching hash dirs,
/// including worktree variants). None if Cursor has no data for this
/// project.
pub fn detect_cursor_session(project_path: &str) -> Option<String> {
    let cursor_chats_dir = dirs::home_dir()?.join(".cursor").join("chats");
    let root = resolve_root_project_path(project_path);
    let root_hash = cursor_project_hash(root);

    let hash_dirs: Vec<PathBuf> = match fs::read_dir(&cursor_chats_dir) {
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .filter(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                e.path().is_dir()
                    && (name == root_hash
                        || name.starts_with(&format!("{}-.worktrees-", root_hash)))
            })
            .map(|e| e.path())
            .collect(),
        Err(_) => return None,
    };

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

/// Return the most recent Gemini session uuid for a project path.
///
/// Gemini's storage layout (verified against a live install):
///   ~/.gemini/projects.json     — { "projects": { "/abs/cwd": "<slug>" } }
///   ~/.gemini/tmp/<slug>/chats/session-<iso>-<short-uuid>.jsonl
///
/// The on-disk filename only carries an 8-char prefix of the uuid, so we
/// MUST read line 1 of the JSONL header to extract the full `sessionId`
/// — that's what `gemini --resume <uuid>` expects. The "most recent"
/// session is picked by file mtime across every project-family slug
/// (matching root + worktree paths the same way detect_cursor_session
/// does).
pub fn detect_gemini_session(project_path: &str) -> Option<String> {
    let home = dirs::home_dir()?;
    let projects_json = home.join(".gemini").join("projects.json");
    let tmp_dir = home.join(".gemini").join("tmp");
    if !tmp_dir.exists() {
        return None;
    }

    let content = fs::read_to_string(&projects_json).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&content).ok()?;
    let projects_obj = parsed.get("projects")?.as_object()?;

    let root = resolve_root_project_path(project_path);
    let mut best: Option<(std::time::SystemTime, std::path::PathBuf)> = None;

    // Scan every slug whose cwd is in this project's family (root +
    // worktrees), then pick the newest session file.
    for (cwd, slug_v) in projects_obj {
        if !matches_project_family(cwd, root) {
            continue;
        }
        let slug = match slug_v.as_str() {
            Some(s) => s,
            None => continue,
        };
        let chats_dir = tmp_dir.join(slug).join("chats");
        let entries = match fs::read_dir(&chats_dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
                continue;
            }
            if let Ok(meta) = fs::metadata(&path) {
                if let Ok(modified) = meta.modified() {
                    match &best {
                        Some((best_time, _)) if modified > *best_time => {
                            best = Some((modified, path));
                        }
                        None => {
                            best = Some((modified, path));
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    // Read line 1 → extract the full uuid from `sessionId`.
    let path = best.map(|(_, p)| p)?;
    let file = std::fs::File::open(&path).ok()?;
    let mut reader = std::io::BufReader::new(file);
    let mut first_line = String::new();
    use std::io::BufRead;
    reader.read_line(&mut first_line).ok()?;
    let header: serde_json::Value = serde_json::from_str(first_line.trim()).ok()?;
    header
        .get("sessionId")
        .and_then(|v| v.as_str())
        .map(String::from)
}

/// Return the most recent Pi session uuid for a project path.
///
/// Pi's storage layout (verified against pi-mono — github.com/badlogic/pi-mono):
///   ~/.pi/agent/sessions/<cwd-slug>/<ISO-ts>_<uuidv7>.jsonl
///   line 1: {"type":"session","id":"<uuid>","cwd":"/abs/path","timestamp":…}
///
/// Pi's slug encoding is reversible (`/`→`-` with `--…--` wrapping) but
/// we don't depend on it: walk every slug dir, read line 1's literal
/// `cwd`, and keep the matching ones. More robust across worktrees and
/// any encoding edge case. Picks the newest by file mtime, then reads
/// line 1 again to extract the full uuid for `pi --session <uuid>`.
pub fn detect_pi_session(project_path: &str) -> Option<String> {
    let home = dirs::home_dir()?;
    let sessions_root = home.join(".pi").join("agent").join("sessions");
    if !sessions_root.exists() {
        return None;
    }

    let root = resolve_root_project_path(project_path);
    let mut best: Option<(std::time::SystemTime, std::path::PathBuf)> = None;

    let slug_dirs = fs::read_dir(&sessions_root).ok()?;
    for slug_entry in slug_dirs.flatten() {
        let slug_path = slug_entry.path();
        if !slug_path.is_dir() {
            continue;
        }
        let session_files = match fs::read_dir(&slug_path) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for f_entry in session_files.flatten() {
            let path = f_entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
                continue;
            }

            // Peek at line 1 to confirm cwd is in our project family
            // before considering this file as a candidate.
            let file = match std::fs::File::open(&path) {
                Ok(f) => f,
                Err(_) => continue,
            };
            let mut reader = std::io::BufReader::new(file);
            let mut first_line = String::new();
            use std::io::BufRead;
            if reader.read_line(&mut first_line).is_err() {
                continue;
            }
            let header: serde_json::Value = match serde_json::from_str(first_line.trim()) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let cwd = match header.get("cwd").and_then(|v| v.as_str()) {
                Some(s) => s,
                None => continue,
            };
            if !matches_project_family(cwd, root) {
                continue;
            }

            let modified = match fs::metadata(&path).and_then(|m| m.modified()) {
                Ok(t) => t,
                Err(_) => continue,
            };
            match &best {
                Some((best_time, _)) if modified > *best_time => {
                    best = Some((modified, path));
                }
                None => {
                    best = Some((modified, path));
                }
                _ => {}
            }
        }
    }

    // Re-read the winner's line 1 to extract the uuid.
    let path = best.map(|(_, p)| p)?;
    let file = std::fs::File::open(&path).ok()?;
    let mut reader = std::io::BufReader::new(file);
    let mut first_line = String::new();
    use std::io::BufRead;
    reader.read_line(&mut first_line).ok()?;
    let header: serde_json::Value = serde_json::from_str(first_line.trim()).ok()?;
    header.get("id").and_then(|v| v.as_str()).map(String::from)
}

/// Return the most recent Codex session uuid for a project path.
///
/// Codex layout (0.125+):
///   ~/.codex/sessions/YYYY/MM/DD/rollout-<ISO>-<uuidv7>.jsonl
///   line 1: {"type":"session_meta","payload":{"id","timestamp","cwd",…}}
///
/// Walks the dated partitions, reads each rollout's line 1 to filter by
/// cwd (project family), picks the file with newest mtime, returns its
/// `payload.id` for `codex resume <uuid>`.
pub fn detect_codex_session(project_path: &str) -> Option<String> {
    let home = dirs::home_dir()?;
    let sessions_root = home.join(".codex").join("sessions");
    if !sessions_root.exists() {
        return None;
    }

    let root = resolve_root_project_path(project_path);
    let mut best: Option<(std::time::SystemTime, std::path::PathBuf)> = None;

    let years = fs::read_dir(&sessions_root).ok()?;
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
                    let file = match std::fs::File::open(&path) {
                        Ok(f) => f,
                        Err(_) => continue,
                    };
                    let mut reader = std::io::BufReader::new(file);
                    let mut first_line = String::new();
                    use std::io::BufRead;
                    if reader.read_line(&mut first_line).is_err() {
                        continue;
                    }
                    let header: serde_json::Value = match serde_json::from_str(first_line.trim()) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };
                    let cwd = match header
                        .get("payload")
                        .and_then(|p| p.get("cwd"))
                        .and_then(|v| v.as_str())
                    {
                        Some(s) => s,
                        None => continue,
                    };
                    if !matches_project_family(cwd, root) {
                        continue;
                    }
                    let modified = match fs::metadata(&path).and_then(|m| m.modified()) {
                        Ok(t) => t,
                        Err(_) => continue,
                    };
                    match &best {
                        Some((best_time, _)) if modified > *best_time => {
                            best = Some((modified, path));
                        }
                        None => {
                            best = Some((modified, path));
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    let path = best.map(|(_, p)| p)?;
    let file = std::fs::File::open(&path).ok()?;
    let mut reader = std::io::BufReader::new(file);
    let mut first_line = String::new();
    use std::io::BufRead;
    reader.read_line(&mut first_line).ok()?;
    let header: serde_json::Value = serde_json::from_str(first_line.trim()).ok()?;
    header
        .get("payload")
        .and_then(|p| p.get("id"))
        .and_then(|v| v.as_str())
        .map(String::from)
}

/// Provider dispatcher used by the daemon's post-spawn session-save
/// task (and the Tauri UI's session-rediscovery). Returns `Ok(None)`
/// when no session is detected — distinct from `Err` so callers can
/// distinguish "nothing to save" from "detection broke."
pub fn detect_active_session(
    provider: &str,
    project_path: &str,
) -> Result<Option<String>, String> {
    let session = match provider {
        "claude" => detect_claude_session(project_path),
        "cursor" => detect_cursor_session(project_path),
        "gemini" => detect_gemini_session(project_path),
        "pi" => detect_pi_session(project_path),
        "codex" => detect_codex_session(project_path),
        _ => None,
    };
    Ok(session)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_root_project_path_strips_worktree() {
        assert_eq!(resolve_root_project_path("/r/.worktrees/f-x"), "/r");
        assert_eq!(resolve_root_project_path("/r"), "/r");
        assert_eq!(resolve_root_project_path("/r/.k2so/x"), "/r/.k2so/x");
    }

    #[test]
    fn matches_project_family_covers_root_and_worktrees() {
        assert!(matches_project_family("/r", "/r"));
        assert!(matches_project_family("/r/.worktrees/a", "/r"));
        assert!(!matches_project_family("/other", "/r"));
        // Must start with the separator — `/r2` shouldn't match `/r`.
        assert!(!matches_project_family("/r2", "/r"));
    }

    #[test]
    fn claude_project_hash_handles_hidden_dirs() {
        assert_eq!(
            claude_project_hash("/Users/z/proj/.k2so/agents/a"),
            "-Users-z-proj--k2so-agents-a"
        );
        assert_eq!(claude_project_hash("/r"), "-r");
    }

    #[test]
    fn cursor_project_hash_strips_leading_slash() {
        assert_eq!(
            cursor_project_hash("/Users/z/DevProjects/K2SO"),
            "Users-z-DevProjects-K2SO"
        );
    }

    #[test]
    fn detect_active_session_unknown_provider_returns_none() {
        assert_eq!(detect_active_session("bogus", "/r").unwrap(), None);
    }
}

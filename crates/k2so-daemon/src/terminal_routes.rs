//! H1 of Phase 4 — daemon-side `/cli/terminal/read` +
//! `/cli/terminal/write`.
//!
//! Both routes operate on sessions the daemon owns
//! (`session::registry` + `session_map`). Before Phase 4, these
//! endpoints lived in Tauri's `agent_hooks.rs` and reached into
//! `AppState.terminal_manager` — which only knew about sessions
//! Tauri had spawned, not the daemon's own pool. This module closes
//! that split: for daemon-owned sessions, the daemon answers; Tauri
//! becomes a pure client calling these endpoints over HTTP.
//!
//! **`id` parameter** — the session identifier is a UUID string
//! matching `SessionId::to_string()`. Legacy Tauri `id`s (arbitrary
//! strings assigned by `TerminalManager::create`) are NOT accepted
//! here — once Phase 5 removes alacritty, every live session is a
//! `SessionId` anyway.
//!
//! **Response shape** — matches the pre-Phase-4 Tauri responses so
//! the CLI + companion clients don't have to branch:
//!   - write: `{"success":true}` or error
//!   - read:  `{"lines":["line1","line2",...]}`

use std::collections::HashMap;

use k2so_core::session::{registry, Frame, SessionId};

use crate::cli_response::CliResponse;
use crate::session_map;

/// Handler for `GET /cli/terminal/read?id=<session>&lines=<n>[&scrollback=true]`.
///
/// Walks the session's replay ring, decodes every `Frame::Text`
/// chunk back to UTF-8, joins them into a single byte stream,
/// and splits on `\n` to produce logical lines. Returns the last
/// N lines (or every line if `lines=` is missing / zero).
///
/// `scrollback=true` is accepted for back-compat with the legacy
/// Tauri endpoint but currently has no distinct behavior — the
/// replay ring IS the scrollback. A future commit can wire a
/// separate "archive replay" path that reads from the on-disk
/// NDJSON segments when the caller wants history beyond
/// `REPLAY_CAP` frames.
pub fn handle_read(params: &HashMap<String, String>) -> CliResponse {
    let id_str = match params.get("id") {
        Some(s) if !s.is_empty() => s.as_str(),
        _ => return CliResponse::bad_request("missing id param"),
    };
    let session_id = match SessionId::parse(id_str) {
        Some(id) => id,
        None => return CliResponse::bad_request("invalid session id (expected UUID)"),
    };
    let requested_lines: usize = params
        .get("lines")
        .and_then(|s| s.parse().ok())
        .unwrap_or(50);

    let entry = match registry::lookup(&session_id) {
        Some(e) => e,
        None => return CliResponse::bad_request("session not found"),
    };

    // Decode every Frame::Text's bytes. Non-text frames (CursorOp,
    // SemanticEvent, AgentSignal, RawPtyFrame) don't contribute to
    // the "read" text — the caller asked for displayable lines.
    let mut buf = Vec::<u8>::new();
    for frame in entry.replay_snapshot() {
        if let Frame::Text { bytes, .. } = frame {
            buf.extend(bytes);
        }
    }
    let text = String::from_utf8_lossy(&buf);

    // Split on line terminators, preferring CRLF then LF, and
    // strip trailing '\r' that comes from a CRLF pair.
    let mut lines: Vec<String> = text
        .split('\n')
        .map(|s| s.strip_suffix('\r').unwrap_or(s).to_string())
        .collect();
    // A trailing newline produces a final empty element; preserve
    // if present so callers can distinguish "ends on newline" from
    // "line still being typed," but don't synthesize one.
    if lines.last().map(String::is_empty).unwrap_or(false) {
        lines.pop();
    }

    let start = lines.len().saturating_sub(requested_lines);
    let tail: Vec<String> = lines[start..].to_vec();

    CliResponse::ok_json(serde_json::json!({ "lines": tail }).to_string())
}

/// Handler for `GET /cli/terminal/write?id=<session>&message=<text>[&no_submit=true]`.
///
/// Looks up the session in the daemon's `session_map` by session
/// id, writes `message` bytes to the PTY, and (unless `no_submit`)
/// fires a follow-up `\r` 150 ms later — matches the pre-Phase-4
/// Tauri endpoint's "paste then enter" cadence. CLI LLMs treat
/// paste+Enter as a single event and swallow a trailing \r, so
/// the split-write is load-bearing.
pub fn handle_write(params: &HashMap<String, String>) -> CliResponse {
    let id_str = match params.get("id") {
        Some(s) if !s.is_empty() => s.as_str(),
        _ => return CliResponse::bad_request("missing id param"),
    };
    let session_id = match SessionId::parse(id_str) {
        Some(id) => id,
        None => return CliResponse::bad_request("invalid session id (expected UUID)"),
    };
    let message = match params.get("message") {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => return CliResponse::bad_request("missing message param"),
    };
    let no_submit = params
        .get("no_submit")
        .map(|v| matches!(v.as_str(), "true" | "1"))
        .unwrap_or(false);

    let session = match session_map::lookup_by_session_id(&session_id) {
        Some(s) => s,
        None => {
            return CliResponse::bad_request(
                "session not found in daemon session_map",
            );
        }
    };

    if let Err(e) = session.write(message.as_bytes()) {
        return CliResponse::bad_request(format!("pty write failed: {e}"));
    }

    if !no_submit {
        // Follow-up Enter in a detached thread so the HTTP response
        // doesn't wait 150 ms unnecessarily.
        let session = session.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(150));
            let _ = session.write(b"\r");
        });
    }

    CliResponse::ok_json(r#"{"success":true}"#.to_string())
}

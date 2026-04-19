//! Agent lifecycle hook notification server.
//!
//! Listens on a random localhost port for HTTP GET requests from agent CLI hooks
//! (Claude Code, Cursor, Gemini). Maps agent events to canonical lifecycle types
//! and emits Tauri events to the frontend.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicU16, Ordering};
use parking_lot::Mutex;
use std::sync::OnceLock;
use tauri::{AppHandle, Emitter, Manager};

static HOOK_PORT: AtomicU16 = AtomicU16::new(0);
static HOOK_TOKEN: OnceLock<String> = OnceLock::new();
/// Guard against concurrent triage runs for the same project path.
static TRIAGE_IN_FLIGHT: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

/// Ring buffer of recent hook events for diagnostic purposes
/// (surfaced via `k2so hooks status`). Capped at RECENT_EVENTS_CAP.
static RECENT_EVENTS: OnceLock<Mutex<VecDeque<RecentEvent>>> = OnceLock::new();
const RECENT_EVENTS_CAP: usize = 50;

#[derive(Clone, serde::Serialize)]
struct RecentEvent {
    timestamp: String,
    raw_event: String,
    canonical: Option<String>,
    pane_id: String,
    tab_id: String,
    matched: bool, // did the event produce a canonical type?
}

fn recent_events() -> &'static Mutex<VecDeque<RecentEvent>> {
    RECENT_EVENTS.get_or_init(|| Mutex::new(VecDeque::with_capacity(RECENT_EVENTS_CAP)))
}

fn record_recent_event(raw: &str, canonical: Option<&str>, pane_id: &str, tab_id: &str) {
    let event = RecentEvent {
        timestamp: chrono::Utc::now().to_rfc3339(),
        raw_event: raw.to_string(),
        canonical: canonical.map(String::from),
        pane_id: pane_id.to_string(),
        tab_id: tab_id.to_string(),
        matched: canonical.is_some(),
    };
    let mut buf = recent_events().lock();
    if buf.len() >= RECENT_EVENTS_CAP {
        buf.pop_front();
    }
    buf.push_back(event);
}

/// Event queue for channel-based agents. Key: "project_path:agent_name"
static EVENT_QUEUES: OnceLock<Mutex<HashMap<String, VecDeque<ChannelEvent>>>> = OnceLock::new();

const MAX_EVENTS_PER_QUEUE: usize = 100;

#[derive(Clone, serde::Serialize)]
struct ChannelEvent {
    #[serde(rename = "type")]
    event_type: String,
    message: String,
    priority: String,
    timestamp: String,
}

fn event_queues() -> &'static Mutex<HashMap<String, VecDeque<ChannelEvent>>> {
    EVENT_QUEUES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Spawn an autonomous agent wake PTY directly from Rust.
///
/// Heartbeat wakes used to emit `cli:agent-launch` and let a frontend
/// listener call `terminal_create`. That broke whenever the K2SO window
/// was closed — the emit fires into zero windows and the wake silently
/// never happens. This helper mirrors the companion-background-spawn
/// pattern: PTY is created in the Rust-managed terminal manager so it
/// works whether or not the UI is visible, and a background-spawn event
/// is emitted so any open window creates a tab for discovery.
///
/// Returns the generated terminal ID (also the pane/tab ID used for
/// hook routing). Returns Err when the terminal manager can't be
/// reached (AppState not yet initialized).
/// Post-spawn SIGWINCH nudge. The frontend normally calls terminal_resize on
/// mount, which sends SIGWINCH to the child process — some CLI TUIs (claude
/// among them) hold off on full initialization and queued-input processing
/// until they see that first resize after exec. Without this, heartbeats
/// "fire" (PTY spawned, wake message queued in input) but the agent sits
/// idle until a user opens the tab and the frontend's mount handler runs —
/// which defeats the whole point of headless wakes. Replays the same
/// resize-to-known-dims nudge from the backend so the invariant holds.
fn nudge_wake_pty_async(app_handle: AppHandle, terminal_id: String) {
    std::thread::spawn(move || {
        // Brief settle so the shell's exec of claude has completed before
        // the signal lands. Under 1 s in practice; 800 ms buys margin.
        std::thread::sleep(std::time::Duration::from_millis(800));
        let Some(state) = app_handle.try_state::<crate::state::AppState>() else {
            return;
        };
        let manager = state.terminal_manager.lock();
        let _ = manager.resize(&terminal_id, 120, 38);
    });
}

/// Post-spawn watcher: detects Claude Code's stale-session confirmation dialog
/// and dismisses it by selecting option 3 ("never ask again"). Used by the
/// heartbeat paths which spawn with `--resume` but WITHOUT `--fork-session`
/// (to keep wakes writing into the same session — one chat per agent instead
/// of one per fire). Without this, a stale session would block the wake at
/// the dialog until a human intervened.
///
/// Detection is heuristic (looks for "3" + session/ask phrasing in the
/// terminal buffer). If the dialog isn't shown (fresh session, already
/// dismissed permanently), this is a safe no-op — we never send the "3".
fn dismiss_stale_session_dialog_async(app_handle: AppHandle, terminal_id: String) {
    std::thread::spawn(move || {
        // Wait for claude to start and render any dialog
        std::thread::sleep(std::time::Duration::from_secs(3));

        let Some(state) = app_handle.try_state::<crate::state::AppState>() else {
            return;
        };

        let lines = {
            let mgr = state.terminal_manager.lock();
            mgr.read_lines_with_scrollback(&terminal_id, 60, false).unwrap_or_default()
        };
        let buf = lines.join("\n").to_lowercase();

        // Conservative: require BOTH an option-3 marker AND a session/ask phrase.
        // Keeps us from typing "3" into a normal input prompt.
        let has_option_three = buf.contains("3.") || buf.contains("3)") || buf.contains("[3]");
        let has_dialog_phrase = buf.contains("never ask")
            || buf.contains("don't ask")
            || buf.contains("previous session")
            || buf.contains("full context")
            || buf.contains("resume with");

        if has_option_three && has_dialog_phrase {
            log_debug!(
                "[heartbeat] Stale-session dialog detected in {}; selecting option 3",
                terminal_id
            );
            {
                let mgr = state.terminal_manager.lock();
                let _ = mgr.write(&terminal_id, "3");
            }
            std::thread::sleep(std::time::Duration::from_millis(150));
            {
                let mgr = state.terminal_manager.lock();
                let _ = mgr.write(&terminal_id, "\r");
            }
        }
    });
}

fn spawn_wake_pty(
    app_handle: &AppHandle,
    agent_name: &str,
    project_path: &str,
    command: &str,
    args: Vec<String>,
    cwd: &str,
) -> Result<String, String> {
    let terminal_id = format!("wake-{}-{}", agent_name, uuid::Uuid::new_v4());
    let state = app_handle
        .try_state::<crate::state::AppState>()
        .ok_or_else(|| "AppState not available".to_string())?;
    {
        let mut manager = state.terminal_manager.lock();
        manager
            .create(
                terminal_id.clone(),
                cwd.to_string(),
                Some(command.to_string()),
                Some(args.clone()),
                Some(120),
                Some(38),
                app_handle.clone(),
            )
            .map_err(|e| format!("Failed to spawn wake PTY: {}", e))?;
    }
    log_debug!(
        "[agent-hooks] Wake PTY spawned for {} ({}): id={}",
        agent_name, command, terminal_id
    );

    // Kick claude's TUI into full initialization with a deferred SIGWINCH.
    // Headless heartbeats would otherwise sit idle until a user opens the
    // tab — see nudge_wake_pty_async for the rationale.
    nudge_wake_pty_async(app_handle.clone(), terminal_id.clone());

    // Record the session as `running` with owner=system so the scheduler
    // won't double-launch this agent on the next tick. The frontend's
    // cli:agent-launch listener used to call this via invoke() — with
    // backend-direct spawn we have to do it here, otherwise two quick
    // heartbeat ticks can race and spawn duplicate PTYs.
    let _ = crate::commands::k2so_agents::k2so_agents_lock(
        project_path.to_string(),
        agent_name.to_string(),
        Some(terminal_id.clone()),
        Some("system".to_string()),
    );

    // Tab-creation event for any currently-open window. Uses the same
    // event name the companion-spawn path already emits so the frontend
    // discovery code doesn't need a second listener.
    let _ = app_handle.emit(
        "cli:terminal-spawn-background",
        serde_json::json!({
            "terminalId": &terminal_id,
            "command": command,
            "cwd": cwd,
            "projectPath": project_path,
            "agentName": agent_name,
        }),
    );

    // Save Claude's new session ID so the next wake can --resume it.
    // This used to run in the frontend `cli:agent-launch` listener,
    // but since backend-direct spawn bypasses the frontend entirely we
    // have to do it here — otherwise every wake reads a stale DB row
    // and hits "No conversation found" on resume.
    //
    // Claude takes a moment to write the session JSONL file, so we
    // wait a few seconds, scan the provider's history dir for the
    // newest session, and persist it in agent_sessions.session_id.
    let agent_name_owned = agent_name.to_string();
    let project_path_owned = project_path.to_string();
    let cwd_owned = cwd.to_string();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(5));
        let detected = crate::commands::chat_history::chat_history_detect_active_session(
            "claude".to_string(),
            cwd_owned.clone(),
        );
        if let Ok(Some(session_id)) = detected {
            if !session_id.is_empty() {
                if let Err(e) = crate::commands::k2so_agents::k2so_agents_save_session_id(
                    project_path_owned.clone(),
                    agent_name_owned.clone(),
                    session_id.clone(),
                ) {
                    log_debug!("[agent-hooks] Failed to save session ID for {}: {}", agent_name_owned, e);
                } else {
                    log_debug!("[agent-hooks] Saved session ID for {}: {}", agent_name_owned, session_id);
                }
            }
        }
    });

    Ok(terminal_id)
}

/// Force-fire a specific heartbeat by name, bypassing its schedule. The
/// frontend uses this for per-row Launch buttons in the workspace drawer
/// so the user can manually kick off a scheduled workflow without waiting
/// for the cron window. Path is identical to a scheduled fire: resolve
/// the row → read its wakeup.md → spawn_wake_pty → stamp last_fired →
/// write a heartbeat_fires audit row (decision='fired', reason='forced').
///
/// Returns the spawned terminal_id on success so the caller can focus it.
#[tauri::command]
pub fn k2so_heartbeat_force_fire(
    app_handle: AppHandle,
    project_path: String,
    name: String,
) -> Result<String, String> {
    use crate::db::schema::{AgentHeartbeat, HeartbeatFire};

    let db = crate::db::shared();
    let conn = db.lock();
    let project_id: String = conn.query_row(
        "SELECT id FROM projects WHERE path = ?1",
        rusqlite::params![project_path],
        |row| row.get(0),
    ).map_err(|e| format!("Project not found: {}", e))?;

    let hb = AgentHeartbeat::get_by_name(&conn, &project_id, &name)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Heartbeat '{}' not found", name))?;

    let agent_name = crate::commands::k2so_agents::find_primary_agent(&project_path)
        .ok_or("No scheduleable agent in this workspace")?;

    let wakeup_abs = std::path::Path::new(&project_path).join(&hb.wakeup_path);
    if !wakeup_abs.exists() {
        let _ = HeartbeatFire::insert_with_schedule(
            &conn, &project_id, Some(&agent_name), Some(&hb.name),
            &hb.frequency, "wakeup_file_missing",
            Some(&format!("forced fire failed: {} not found", hb.wakeup_path)),
            None, None, None,
        );
        return Err(format!("wakeup.md missing at {}", hb.wakeup_path));
    }

    if crate::commands::k2so_agents::is_agent_locked(&project_path, &agent_name) {
        let _ = HeartbeatFire::insert_with_schedule(
            &conn, &project_id, Some(&agent_name), Some(&hb.name),
            &hb.frequency, "skipped_locked",
            Some("forced fire refused: agent already running"),
            None, None, None,
        );
        return Err(format!("agent '{}' is already running — close its session first", agent_name));
    }

    // Use the full launch-args builder (same as /cli/agents/launch) so heartbeats
    // --resume the saved session, --fork-session past the stale-session dialog,
    // attach the agent's CLAUDE.md as --append-system-prompt, and honor the
    // wakes-since-compact counter. Prior code here shipped only
    // `--dangerously-skip-permissions <prompt>`, which (a) spawned a fresh
    // claude each fire (breaking session continuity) and (b) let claude
    // v2.1.114 silently drop the argv prompt in minimal-argv mode — agents
    // looked "fired" in the audit log but never read their wakeup.md.
    let launch = crate::commands::k2so_agents::k2so_agents_build_launch(
        project_path.clone(),
        agent_name.clone(),
        None,
        Some(wakeup_abs.to_string_lossy().to_string()),
        Some(true), // skip --fork-session so wakes resume the same session (one chat per agent)
    )?;
    let command = launch.get("command").and_then(|v| v.as_str()).unwrap_or("claude").to_string();
    let cwd = launch.get("cwd").and_then(|v| v.as_str()).unwrap_or(&project_path).to_string();
    let args: Vec<String> = launch.get("args")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|s| s.as_str().map(String::from)).collect())
        .unwrap_or_default();

    let terminal_id = spawn_wake_pty(
        &app_handle, &agent_name, &project_path, &command, args, &cwd,
    )?;
    dismiss_stale_session_dialog_async(app_handle.clone(), terminal_id.clone());
    crate::commands::k2so_agents::stamp_heartbeat_fired(&project_path, &hb.name);
    let _ = HeartbeatFire::insert_with_schedule(
        &conn, &project_id, Some(&agent_name), Some(&hb.name),
        &hb.frequency, "fired",
        Some("forced from UI"), None, None, None,
    );
    Ok(terminal_id)
}

/// Push an event into an agent's channel event queue.
pub fn push_agent_event(project_path: &str, agent_name: &str, event_type: &str, message: &str, priority: &str) {
    let key = format!("{}:{}", project_path, agent_name);
    let event = ChannelEvent {
        event_type: event_type.to_string(),
        message: message.to_string(),
        priority: priority.to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
    };
    let mut queues = event_queues().lock();
    let queue = queues.entry(key).or_insert_with(VecDeque::new);
    queue.push_back(event);
    // Cap queue size
    while queue.len() > MAX_EVENTS_PER_QUEUE {
        queue.pop_front();
    }
}

/// Drain all pending events for an agent (returns them and clears the queue).
fn drain_agent_events(project_path: &str, agent_name: &str) -> Vec<ChannelEvent> {
    let key = format!("{}:{}", project_path, agent_name);
    let mut queues = event_queues().lock();
    queues.remove(&key).map(|q| q.into_iter().collect()).unwrap_or_default()
}

fn triage_lock() -> &'static Mutex<HashSet<String>> {
    TRIAGE_IN_FLIGHT.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Get the port the notification server is listening on.
pub fn get_port() -> u16 {
    HOOK_PORT.load(Ordering::Relaxed)
}

/// Get the auth token for hook requests.
pub fn get_token() -> &'static str {
    HOOK_TOKEN.get().map(|s| s.as_str()).unwrap_or("")
}

/// Generate a cryptographically secure random hex token.
fn generate_token() -> String {
    let mut buf = [0u8; 16];
    getrandom::getrandom(&mut buf).expect("failed to generate random token");
    buf.iter().map(|b| format!("{:02x}", b)).collect()
}

/// Canonical agent lifecycle event types.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentLifecycleEvent {
    pub pane_id: String,
    pub tab_id: String,
    pub event_type: String, // "start", "stop", "permission"
}

/// Map raw agent event names to canonical types.
fn map_event_type(raw: &str) -> Option<&'static str> {
    match raw {
        // Start events
        "Start" | "UserPromptSubmit" | "PostToolUse" | "PostToolUseFailure"
        | "BeforeAgent" | "AfterTool" | "sessionStart" | "userPromptSubmitted"
        | "postToolUse" | "beforeSubmitPrompt" => Some("start"),

        // Stop events
        "Stop" | "agent-turn-complete" | "AfterAgent" | "sessionEnd" | "stop" => Some("stop"),

        // Permission request events
        "PermissionRequest" | "Notification" | "preToolUse"
        | "beforeShellExecution" | "beforeMCPExecution" => Some("permission"),

        _ => None,
    }
}

/// Parse query string from a URL path like `/hook/complete?paneId=...&tabId=...&eventType=...`
fn parse_query_params(url: &str) -> std::collections::HashMap<String, String> {
    let mut params = std::collections::HashMap::new();
    if let Some(query) = url.split('?').nth(1) {
        for pair in query.split('&') {
            if let Some((key, value)) = pair.split_once('=') {
                let decoded = urldecode(value);
                params.insert(key.to_string(), decoded);
            }
        }
    }
    params
}

fn urldecode(s: &str) -> String {
    // Decode percent-encoded bytes into a byte buffer first, then convert
    // to UTF-8. This correctly handles multi-byte UTF-8 sequences like
    // em dash (— = %E2%80%94) which span multiple percent-encoded bytes.
    let mut bytes = Vec::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '%' {
            let hex: String = chars.by_ref().take(2).collect();
            if hex.len() == 2 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
                if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                    bytes.push(byte);
                } else {
                    bytes.push(b'%');
                    bytes.extend_from_slice(hex.as_bytes());
                }
            } else {
                bytes.push(b'%');
                bytes.extend_from_slice(hex.as_bytes());
            }
        } else if c == '+' {
            bytes.push(b' ');
        } else {
            // Regular ASCII/UTF-8 chars — encode as UTF-8 bytes
            let mut buf = [0u8; 4];
            let encoded = c.encode_utf8(&mut buf);
            bytes.extend_from_slice(encoded.as_bytes());
        }
    }
    String::from_utf8(bytes).unwrap_or_else(|e| {
        // Fallback: lossy conversion if somehow the result isn't valid UTF-8
        String::from_utf8_lossy(e.into_bytes().as_slice()).into_owned()
    })
}

/// Shell-escape a string for safe interpolation into shell commands.
/// Uses single-quote wrapping with escaped internal single quotes.
fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Gather git context (branch, status, diff stat, recent log) for AI commit prompts.
/// Each git command has a 5-second timeout to prevent blocking the HTTP thread.
fn gather_git_context(project_path: &str) -> serde_json::Value {
    let run = |args: &[&str]| -> String {
        let mut child = match std::process::Command::new("git")
            .args(args)
            .current_dir(project_path)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(c) => c,
            Err(_) => return String::new(),
        };

        // Wait with 5-second timeout
        let start = std::time::Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    if !status.success() { return String::new(); }
                    return child.stdout.take()
                        .and_then(|mut out| {
                            let mut buf = String::new();
                            std::io::Read::read_to_string(&mut out, &mut buf).ok()?;
                            Some(buf.trim().to_string())
                        })
                        .unwrap_or_default();
                }
                Ok(None) => {
                    if start.elapsed() > std::time::Duration::from_secs(5) {
                        let _ = child.kill();
                        return String::new();
                    }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(_) => return String::new(),
            }
        }
    };

    let branch = run(&["rev-parse", "--abbrev-ref", "HEAD"]);
    let status = run(&["status", "--short"]);
    let diff_stat = run(&["diff", "--stat"]);
    let staged_stat = run(&["diff", "--cached", "--stat"]);
    let log = run(&["log", "--oneline", "-5"]);

    serde_json::json!({
        "branch": branch,
        "status": status,
        "diffStat": diff_stat,
        "stagedStat": staged_stat,
        "recentLog": log,
    })
}

/// Start the notification server on a random port. Returns the port, or
/// an error describing why the bind failed (e.g., port exhaustion, sandbox
/// restrictions). Previously panicked on bind failure, which would kill
/// the whole Tauri process; now the caller can surface a diagnostic and
/// continue launching the UI without the HTTP endpoint.
pub fn start_server(app_handle: AppHandle) -> Result<u16, String> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|e| format!("bind notification server on 127.0.0.1: {}", e))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("read bound port: {}", e))?
        .port();
    HOOK_PORT.store(port, Ordering::Relaxed);

    let token = generate_token();
    let _ = HOOK_TOKEN.set(token.clone());
    log_debug!("[agent-hooks] Notification server listening on port {}", port);

    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };

            // Read the HTTP request (64KB buffer for large query strings with long paths)
            let mut buf = [0u8; 65536];
            let n = match stream.read(&mut buf) {
                Ok(0) => continue, // Connection closed
                Ok(n) => n,
                Err(_) => continue,
            };
            // Detect truncation: if buffer is completely full, the request may have been cut off
            if n == buf.len() {
                let body = r#"{"error":"Request too large (>64KB)"}"#;
                let resp = format!(
                    "HTTP/1.1 413 Payload Too Large\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(), body
                );
                let _ = stream.write_all(resp.as_bytes());
                continue;
            }
            let request = String::from_utf8_lossy(&buf[..n]);

            // Parse the request line: "GET /hook/complete?... HTTP/1.1"
            let first_line = request.lines().next().unwrap_or("");
            let parts: Vec<&str> = first_line.split_whitespace().collect();

            let (method, path) = match parts.as_slice() {
                [m, p, ..] => (*m, *p),
                _ => {
                    let _ = stream.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n");
                    continue;
                }
            };

            if method != "GET" {
                let body = r#"{"error":"Only GET requests are supported"}"#;
                let resp = format!(
                    "HTTP/1.1 405 Method Not Allowed\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(), body
                );
                let _ = stream.write_all(resp.as_bytes());
                continue;
            }

            if path.starts_with("/hook/complete") {
                let params = parse_query_params(path);

                // Validate auth token
                let req_token = params.get("token").cloned().unwrap_or_default();
                if req_token != token {
                    let body = r#"{"error":"Invalid or missing auth token"}"#;
                    let resp = format!(
                        "HTTP/1.1 403 Forbidden\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(), body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                    continue;
                }
                let pane_id = params.get("paneId").cloned().unwrap_or_default();
                let tab_id = params.get("tabId").cloned().unwrap_or_default();
                let raw_event = params.get("eventType").cloned().unwrap_or_default();

                let canonical_opt = map_event_type(&raw_event);
                record_recent_event(&raw_event, canonical_opt, &pane_id, &tab_id);

                if let Some(canonical) = canonical_opt {
                    let event = AgentLifecycleEvent {
                        pane_id: pane_id.clone(),
                        tab_id: tab_id.clone(),
                        event_type: canonical.to_string(),
                    };

                    log_debug!("[agent-hooks] {} → {} (pane={}, tab={})", raw_event, canonical, pane_id, tab_id);
                    let _ = app_handle.emit("agent:lifecycle", &event);

                    // Sync AgentSession.status so the scheduler's
                    // is_agent_locked check reflects reality. Without
                    // this, a single wake leaves status='running'
                    // forever and every subsequent heartbeat silently
                    // skips the agent. Resolved via terminal_id lookup
                    // — pane_id is the K2SO_PANE_ID env var we set at
                    // PTY creation.
                    let new_status: Option<&str> = match canonical {
                        "start" => Some("running"),
                        "stop" => Some("sleeping"),
                        "permission" => Some("permission"),
                        _ => None,
                    };
                    if let Some(new_status) = new_status {
                        let db = crate::db::shared();
                        let conn = db.lock();
                        if let Ok(Some(s)) = crate::db::schema::AgentSession::get_by_terminal_id(&conn, &pane_id) {
                            if s.status != new_status {
                                let _ = crate::db::schema::AgentSession::update_status(
                                    &conn, &s.project_id, &s.agent_name, new_status,
                                );
                            }
                        }
                    }
                }

                let body = r#"{"success":true}"#;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
            } else if path.starts_with("/cli/") {
                // K2SO CLI bridge endpoints
                let params = parse_query_params(path);

                // Validate auth token
                let req_token = params.get("token").cloned().unwrap_or_default();
                if req_token != token {
                    let body = r#"{"error":"Invalid or missing auth token"}"#;
                    let resp = format!(
                        "HTTP/1.1 403 Forbidden\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(), body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                    continue;
                }

                let route = path.split('?').next().unwrap_or("");
                let project_path = params.get("project").cloned().unwrap_or_default();

                let result: Result<String, String> = match route {
                    "/cli/agents/list" => {
                        crate::commands::k2so_agents::k2so_agents_list(project_path)
                            .map(|agents| serde_json::to_string(&agents).unwrap_or_default())
                    }
                    "/cli/agents/work" => {
                        let agent = params.get("agent").cloned().unwrap_or_default();
                        let folder = params.get("folder").cloned();
                        crate::commands::k2so_agents::k2so_agents_work_list(project_path, agent, folder)
                            .map(|items| serde_json::to_string(&items).unwrap_or_default())
                    }
                    "/cli/agents/create" => {
                        let name = params.get("name").cloned().unwrap_or_default();
                        let role = params.get("role").cloned().unwrap_or_default();
                        let prompt = params.get("prompt").cloned();
                        let agent_type = params.get("agent_type").cloned();
                        crate::commands::k2so_agents::k2so_agents_create(project_path, name, role, prompt, agent_type)
                            .map(|info| serde_json::to_string(&info).unwrap_or_default())
                    }
                    "/cli/agents/work/create" => {
                        let agent = params.get("agent").cloned();
                        let title = params.get("title").cloned().unwrap_or_default();
                        let body = params.get("body").cloned().unwrap_or_default();
                        let priority = params.get("priority").cloned();
                        let item_type = params.get("type").cloned();
                        let source = params.get("source").cloned();
                        crate::commands::k2so_agents::k2so_agents_work_create(
                            project_path, agent, title, body, priority, item_type, source,
                        )
                        .map(|item| serde_json::to_string(&item).unwrap_or_default())
                    }
                    "/cli/agents/delegate" => {
                        let target = params.get("target").cloned().unwrap_or_default();
                        let file = params.get("file").cloned().unwrap_or_default();
                        match crate::commands::k2so_agents::k2so_agents_delegate(project_path.clone(), target, file) {
                            Ok(launch_info) => {
                                // Backend-direct spawn — delegate works even with no K2SO window open.
                                let command = launch_info.get("command").and_then(|v| v.as_str()).unwrap_or("claude").to_string();
                                let cwd = launch_info.get("cwd").and_then(|v| v.as_str()).unwrap_or(&project_path).to_string();
                                let agent_name = launch_info.get("agentName").and_then(|v| v.as_str()).unwrap_or("delegated").to_string();
                                let args: Vec<String> = launch_info.get("args")
                                    .and_then(|v| v.as_array())
                                    .map(|arr| arr.iter().filter_map(|s| s.as_str().map(String::from)).collect())
                                    .unwrap_or_default();
                                let _ = spawn_wake_pty(&app_handle, &agent_name, &project_path, &command, args, &cwd);
                                // Refresh sidebar — new worktree was registered in DB
                                let _ = app_handle.emit("sync:projects", ());
                                Ok(serde_json::to_string(&launch_info).unwrap_or_default())
                            }
                            Err(e) => Err(e),
                        }
                    }
                    "/cli/agents/work/move" => {
                        let agent = params.get("agent").cloned().unwrap_or_default();
                        let filename = params.get("filename").cloned().unwrap_or_default();
                        let from = params.get("from").cloned().unwrap_or_default();
                        let to = params.get("to").cloned().unwrap_or_default();
                        crate::commands::k2so_agents::k2so_agents_work_move(
                            project_path, agent, filename, from, to,
                        )
                        .map(|_| r#"{"success":true}"#.to_string())
                    }
                    "/cli/agents/profile" => {
                        let agent = params.get("agent").cloned().unwrap_or_default();
                        crate::commands::k2so_agents::k2so_agents_get_profile(project_path, agent)
                            .map(|content| serde_json::json!({"content": content}).to_string())
                    }
                    "/cli/agent/update" => {
                        let agent = params.get("agent").cloned().unwrap_or_default();
                        let field = params.get("field").cloned().unwrap_or_default();
                        let value = params.get("value").cloned().unwrap_or_default();
                        crate::commands::k2so_agents::k2so_agents_update_field(
                            project_path, agent, field, value,
                        )
                        .map(|content| serde_json::json!({"success": true, "content": content}).to_string())
                    }
                    "/cli/work/inbox" => {
                        crate::commands::k2so_agents::k2so_agents_workspace_inbox_list(project_path)
                            .map(|items| serde_json::to_string(&items).unwrap_or_default())
                    }
                    "/cli/work/inbox/create" => {
                        let workspace = params.get("workspace").cloned().unwrap_or(project_path.clone());
                        let title = params.get("title").cloned().unwrap_or_default();
                        let body = params.get("body").cloned().unwrap_or_default();
                        let priority = params.get("priority").cloned();
                        let item_type = params.get("type").cloned();
                        let assigned_by = params.get("assigned_by").cloned();
                        let source = params.get("source").cloned();
                        crate::commands::k2so_agents::k2so_agents_workspace_inbox_create(
                            workspace, title, body, priority, item_type, assigned_by, source,
                        )
                        .map(|item| serde_json::to_string(&item).unwrap_or_default())
                    }
                    "/cli/agents/delete" => {
                        let agent = params.get("agent").cloned().unwrap_or_default();
                        let force = params.get("force").map_or(false, |v| v == "1" || v == "true");
                        crate::commands::k2so_agents::k2so_agents_delete_inner(&project_path, &agent, force)
                            .map(|_| format!(r#"{{"success":true,"deleted":"{}"}}"#, agent))
                    }
                    "/cli/agents/lock" => {
                        let agent = params.get("agent").cloned().unwrap_or_default();
                        let terminal_id = params.get("terminal_id").cloned();
                        let owner = params.get("owner").cloned();
                        crate::commands::k2so_agents::k2so_agents_lock(project_path, agent, terminal_id, owner)
                            .map(|_| r#"{"success":true}"#.to_string())
                    }
                    "/cli/agents/unlock" => {
                        let agent = params.get("agent").cloned().unwrap_or_default();
                        crate::commands::k2so_agents::k2so_agents_unlock(project_path, agent)
                            .map(|_| r#"{"success":true}"#.to_string())
                    }
                    "/cli/agents/triage" => {
                        crate::commands::k2so_agents::k2so_agents_triage_summary(project_path)
                    }
                    "/cli/agents/generate-claude-md" => {
                        let agent = params.get("agent").cloned().unwrap_or_default();
                        crate::commands::k2so_agents::k2so_agents_generate_claude_md(
                            project_path, agent,
                        )
                        .map(|content| serde_json::json!({"success": true, "length": content.len()}).to_string())
                    }
                    "/cli/agents/launch" => {
                        let agent = params.get("agent").cloned().unwrap_or_default();
                        let cli_command = params.get("command").cloned();
                        let agent_clone = agent.clone();
                        match crate::commands::k2so_agents::k2so_agents_build_launch(
                            project_path.clone(), agent, cli_command, None, None,
                        ) {
                            Ok(launch_info) => {
                                // Backend-direct spawn — works when K2SO window is closed.
                                let command = launch_info.get("command").and_then(|v| v.as_str()).unwrap_or("claude").to_string();
                                let cwd = launch_info.get("cwd").and_then(|v| v.as_str()).unwrap_or(&project_path).to_string();
                                let args: Vec<String> = launch_info.get("args")
                                    .and_then(|v| v.as_array())
                                    .map(|arr| arr.iter().filter_map(|s| s.as_str().map(String::from)).collect())
                                    .unwrap_or_default();
                                let _ = spawn_wake_pty(&app_handle, &agent_clone, &project_path, &command, args, &cwd);
                                Ok(serde_json::json!({
                                    "success": true,
                                    "note": "Agent session will be launched by K2SO"
                                }).to_string())
                            }
                            Err(e) => Err(e),
                        }
                    }
                    "/cli/reviews" => {
                        crate::commands::k2so_agents::k2so_agents_review_queue_inner(&project_path)
                            .map(|items| serde_json::to_string(&items).unwrap_or_default())
                    }
                    "/cli/review/approve" => {
                        let agent = params.get("agent").cloned().unwrap_or_default();
                        let branch = params.get("branch").cloned().unwrap_or_default();
                        crate::commands::k2so_agents::k2so_agents_review_approve(project_path, branch, agent)
                            .map(|msg| serde_json::json!({"success": true, "message": msg}).to_string())
                    }
                    "/cli/review/reject" => {
                        let agent = params.get("agent").cloned().unwrap_or_default();
                        let reason = params.get("reason").cloned();
                        crate::commands::k2so_agents::k2so_agents_review_reject(project_path, agent, reason)
                            .map(|_| r#"{"success":true}"#.to_string())
                    }
                    "/cli/review/feedback" => {
                        let agent = params.get("agent").cloned().unwrap_or_default();
                        let feedback = params.get("feedback").cloned().unwrap_or_default();
                        crate::commands::k2so_agents::k2so_agents_review_request_changes(project_path, agent, feedback)
                            .map(|_| r#"{"success":true}"#.to_string())
                    }
                    "/cli/mode" => {
                        // Get or set workspace agent mode — persists directly to DB
                        let new_mode = params.get("set").cloned();
                        if let Some(mode) = new_mode {
                            match cli_update_project_setting(&project_path, "agent_mode", &mode) {
                                Ok(_) => {
                                    // Also scaffold/disable CLAUDE.md based on mode
                                    if mode == "off" {
                                        let _ = crate::commands::k2so_agents::k2so_agents_disable_workspace_claude_md(project_path.clone());
                                    } else {
                                        let _ = crate::commands::k2so_agents::k2so_agents_generate_workspace_claude_md(project_path.clone());
                                    }
                                    // Notify frontend to refresh
                                    let _ = app_handle.emit("sync:projects", ());
                                    Ok(serde_json::json!({"success": true, "mode": mode}).to_string())
                                }
                                Err(e) => Err(e),
                            }
                        } else {
                            // Read current mode from DB
                            match cli_get_project_settings(&project_path) {
                                Ok(settings) => Ok(serde_json::to_string(&settings).unwrap_or_default()),
                                Err(_) => {
                                    // Fallback: detect from filesystem
                                    let k2so_dir = std::path::PathBuf::from(&project_path).join(".k2so");
                                    let agents_dir = k2so_dir.join("agents");
                                    let has_agents = agents_dir.exists() && std::fs::read_dir(&agents_dir)
                                        .map(|e| e.count() > 0).unwrap_or(false);
                                    let claude_md = std::path::PathBuf::from(&project_path).join("CLAUDE.md");
                                    let mode = if !claude_md.exists() { "off" } else if has_agents { "manager" } else { "agent" };
                                    Ok(serde_json::json!({"mode": mode}).to_string())
                                }
                            }
                        }
                    }
                    "/cli/worktree" => {
                        // Enable/disable worktree mode for this project
                        let enable = params.get("enable").cloned().unwrap_or_default();
                        let value = if enable == "1" || enable == "true" || enable == "on" { "1" } else { "0" };
                        match cli_update_project_setting(&project_path, "worktree_mode", value) {
                            Ok(_) => {
                                let _ = app_handle.emit("sync:projects", ());
                                Ok(serde_json::json!({"success": true, "worktreeMode": value == "1"}).to_string())
                            }
                            Err(e) => Err(e),
                        }
                    }
                    "/cli/heartbeat" => {
                        // If "enable" param present → toggle heartbeat; otherwise → triage
                        if let Some(enable) = params.get("enable").cloned() {
                            let value = if enable == "1" || enable == "true" || enable == "on" { "1" } else { "0" };
                            match cli_update_project_setting(&project_path, "heartbeat_enabled", value) {
                                Ok(_) => {
                                    let _ = app_handle.emit("sync:projects", ());
                                    Ok(serde_json::json!({"success": true, "heartbeatEnabled": value == "1"}).to_string())
                                }
                                Err(e) => Err(e),
                            }
                        } else {
                            // Concurrency guard: skip if triage already running for this project
                            let already_running = {
                                let mut in_flight = triage_lock().lock();
                                if in_flight.contains(&project_path) {
                                    true
                                } else {
                                    in_flight.insert(project_path.clone());
                                    false
                                }
                            };

                            if already_running {
                                Ok(serde_json::json!({"count": 0, "launched": [], "skipped": "triage already in flight"}).to_string())
                            } else {
                                let triage_result = crate::commands::k2so_agents::k2so_agents_triage_decide(project_path.clone())
                                    .map(|agents| {
                                        // Emit launch events for each agent
                                        for agent_name in &agents {
                                            if agent_name == "__lead__" {
                                                // triage_decide returns the literal "__lead__" sentinel, but the
                                                // on-disk primary may be a real agent dir (coordinator/pod-leader/
                                                // manager). Resolve it so build_launch reads the right SKILL.md /
                                                // CLAUDE.md. wakeup_override comes from the triage heartbeat row.
                                                let _ = crate::commands::k2so_agents::k2so_agents_generate_workspace_claude_md(project_path.clone());
                                                let primary = crate::commands::k2so_agents::find_primary_agent(&project_path)
                                                    .unwrap_or_else(|| "__lead__".to_string());
                                                let wakeup_override = crate::commands::k2so_agents::default_heartbeat_wakeup_abs(&project_path, &primary);
                                                if let Ok(launch) = crate::commands::k2so_agents::k2so_agents_build_launch(
                                                    project_path.clone(), primary.clone(), None, wakeup_override, Some(true),
                                                ) {
                                                    let command = launch.get("command").and_then(|v| v.as_str()).unwrap_or("claude").to_string();
                                                    let cwd = launch.get("cwd").and_then(|v| v.as_str()).unwrap_or(&project_path).to_string();
                                                    let args: Vec<String> = launch.get("args")
                                                        .and_then(|v| v.as_array())
                                                        .map(|arr| arr.iter().filter_map(|s| s.as_str().map(String::from)).collect())
                                                        .unwrap_or_default();
                                                    let _ = spawn_wake_pty(&app_handle, &primary, &project_path, &command, args, &cwd);
                                                }
                                            } else if let Ok(launch) = crate::commands::k2so_agents::k2so_agents_build_launch(
                                                project_path.clone(), agent_name.clone(), None, None, None,
                                            ) {
                                                // Backend-direct spawn so wakes fire regardless of window state.
                                                let command = launch.get("command").and_then(|v| v.as_str()).unwrap_or("claude").to_string();
                                                let cwd = launch.get("cwd").and_then(|v| v.as_str()).unwrap_or(&project_path).to_string();
                                                let args: Vec<String> = launch.get("args")
                                                    .and_then(|v| v.as_array())
                                                    .map(|arr| arr.iter().filter_map(|s| s.as_str().map(String::from)).collect())
                                                    .unwrap_or_default();
                                                let _ = spawn_wake_pty(&app_handle, agent_name, &project_path, &command, args, &cwd);
                                            }
                                        }
                                        serde_json::json!({"count": agents.len(), "launched": agents}).to_string()
                                    });

                                // Release the triage lock
                                {
                                    let mut in_flight = triage_lock().lock();
                                    in_flight.remove(&project_path);
                                }

                                triage_result
                            }
                        }
                    }
                    "/cli/heartbeat/schedule" => {
                        // Get or set project heartbeat schedule
                        (|| -> Result<String, String> {
                            let db_path = dirs::home_dir()
                                .unwrap_or_else(|| std::path::PathBuf::from("."))
                                .join(".k2so/k2so.db");
                            let db = crate::db::shared();
                            let conn = db.lock();

                            if let Some(mode) = params.get("mode").cloned() {
                                let schedule = params.get("schedule").cloned();
                                let hb_enabled = if mode == "off" { "0" } else { "1" };

                                conn.execute(
                                    "UPDATE projects SET heartbeat_mode = ?1, heartbeat_schedule = ?2, heartbeat_enabled = ?3 WHERE path = ?4",
                                    rusqlite::params![mode, schedule, hb_enabled, project_path],
                                ).map_err(|e| format!("DB update failed: {}", e))?;

                                let _ = app_handle.emit("sync:projects", ());
                                let state = app_handle.state::<crate::state::AppState>();
                                let _ = crate::commands::k2so_agents::k2so_agents_update_heartbeat_projects(state);

                                Ok(serde_json::json!({
                                    "success": true,
                                    "mode": mode,
                                    "schedule": params.get("schedule").cloned(),
                                }).to_string())
                            } else {
                                let (mode, schedule, last_fire) = conn.query_row(
                                    "SELECT heartbeat_mode, heartbeat_schedule, heartbeat_last_fire FROM projects WHERE path = ?1",
                                    rusqlite::params![project_path],
                                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?, row.get::<_, Option<String>>(2)?)),
                                ).map_err(|e| format!("Project not found: {}", e))?;

                                Ok(serde_json::json!({
                                    "mode": mode,
                                    "schedule": schedule,
                                    "lastFire": last_fire,
                                }).to_string())
                            }
                        })()
                    }
                    "/cli/terminal/spawn" => {
                        // Spawn a sub-terminal for an agent (pane split within agent's tab)
                        let agent = params.get("agent").cloned().unwrap_or_default();
                        let command = params.get("command").cloned().unwrap_or_default();
                        let title = params.get("title").cloned().unwrap_or("Sub-task".to_string());
                        let wait = params.get("wait").map(|v| v == "1" || v == "true").unwrap_or(false);
                        let cwd = params.get("cwd").cloned().unwrap_or(project_path.clone());

                        let _ = app_handle.emit("cli:terminal-spawn", serde_json::json!({
                            "agentName": agent,
                            "command": command,
                            "cwd": cwd,
                            "title": title,
                            "wait": wait,
                            "projectPath": &project_path,
                        }));
                        Ok(serde_json::json!({"success": true}).to_string())
                    }
                    "/cli/terminal/spawn-background" => {
                        // Spawn a background terminal via terminal_create() directly.
                        // Does NOT emit cli:terminal-spawn — no desktop UI disruption.
                        // The PTY runs in the background, accessible via terminal read/write/subscribe.
                        let command = params.get("command").cloned().unwrap_or_default();
                        let cwd = params.get("cwd").cloned().unwrap_or(project_path.clone());
                        let id = params.get("id").cloned()
                            .unwrap_or_else(|| format!("companion-{}", uuid::Uuid::new_v4()));

                        if command.is_empty() {
                            Err("Missing 'command' parameter".to_string())
                        } else if let Some(state) = app_handle.try_state::<crate::state::AppState>() {
                            let mut manager = state.terminal_manager.lock();
                            // Split command into program + args
                            let parts: Vec<&str> = command.split_whitespace().collect();
                            let (prog, args) = if parts.len() > 1 {
                                (Some(parts[0].to_string()), Some(parts[1..].iter().map(|s| s.to_string()).collect::<Vec<_>>()))
                            } else {
                                (Some(command.clone()), None)
                            };
                            match manager.create(id.clone(), cwd.clone(), prog, args, Some(80), Some(24), app_handle.clone()) {
                                Ok(()) => {
                                    log_debug!("[companion] Background terminal spawned: {} ({})", id, command);
                                    // Emit background spawn event for the frontend to create a tab
                                    let _ = app_handle.emit("cli:terminal-spawn-background", serde_json::json!({
                                        "terminalId": &id,
                                        "command": &command,
                                        "cwd": &cwd,
                                        "projectPath": &project_path,
                                    }));
                                    Ok(serde_json::json!({
                                        "success": true,
                                        "terminalId": id,
                                        "command": command,
                                    }).to_string())
                                }
                                Err(e) => Err(format!("Failed to spawn terminal: {}", e)),
                            }
                        } else {
                            Err("AppState not available".to_string())
                        }
                    }
                    "/cli/events" => {
                        // Drain pending events for a channel-based agent
                        let agent = params.get("agent").cloned().unwrap_or("__lead__".to_string());
                        let events = drain_agent_events(&project_path, &agent);
                        Ok(serde_json::to_string(&events).unwrap_or("[]".to_string()))
                    }
                    "/cli/agent/reply" => {
                        // Agent sends a message back to K2SO via channel
                        let agent = params.get("agent").cloned().unwrap_or_default();
                        let message = params.get("message").cloned().unwrap_or_default();
                        // Emit to frontend so the UI can show it
                        let _ = app_handle.emit("agent:reply", serde_json::json!({
                            "agentName": agent,
                            "message": message,
                            "projectPath": &project_path,
                            "timestamp": chrono::Utc::now().to_rfc3339(),
                        }));
                        Ok(r#"{"success":true}"#.to_string())
                    }
                    "/cli/agents/heartbeat/noop" => {
                        let agent = params.get("agent").cloned().unwrap_or_default();
                        crate::commands::k2so_agents::k2so_agents_heartbeat_noop(project_path, agent)
                            .map(|config| serde_json::to_string(&config).unwrap_or_default())
                    }
                    "/cli/agents/heartbeat/action" => {
                        let agent = params.get("agent").cloned().unwrap_or_default();
                        crate::commands::k2so_agents::k2so_agents_heartbeat_action(project_path, agent)
                            .map(|config| serde_json::to_string(&config).unwrap_or_default())
                    }
                    "/cli/agents/heartbeat" => {
                        let agent = params.get("agent").cloned().unwrap_or_default();
                        let interval = params.get("interval").and_then(|v| v.parse::<u64>().ok());
                        let phase = params.get("phase").cloned();
                        let mode = params.get("mode").cloned();
                        let cost_budget = params.get("cost_budget").cloned();

                        if interval.is_some() || phase.is_some() || mode.is_some() || cost_budget.is_some() {
                            // Update
                            let force_wake = params.get("force_wake").map(|v| v == "1" || v == "true");
                            crate::commands::k2so_agents::k2so_agents_set_heartbeat(
                                project_path, agent, interval, phase, mode, cost_budget, force_wake,
                            )
                            .map(|config| serde_json::to_string(&config).unwrap_or_default())
                        } else {
                            // Read
                            crate::commands::k2so_agents::k2so_agents_get_heartbeat(project_path, agent)
                                .map(|config| serde_json::to_string(&config).unwrap_or_default())
                        }
                    }
                    "/cli/heartbeat/add" => {
                        // Multi-heartbeat CRUD. See .k2so/prds/multi-schedule-heartbeat.md
                        let name = params.get("name").cloned().unwrap_or_default();
                        let frequency = params.get("frequency").cloned().unwrap_or_default();
                        let spec_json = params.get("spec").cloned().unwrap_or_else(|| "{}".to_string());
                        if name.is_empty() || frequency.is_empty() {
                            Err("Missing 'name' or 'frequency' parameter".to_string())
                        } else {
                            crate::commands::k2so_agents::k2so_heartbeat_add(
                                project_path.clone(), name, frequency, spec_json,
                            ).map(|v| v.to_string())
                        }
                    }
                    "/cli/heartbeat/list" => {
                        crate::commands::k2so_agents::k2so_heartbeat_list(project_path.clone())
                            .map(|rows| serde_json::to_string(&rows).unwrap_or_default())
                    }
                    "/cli/heartbeat/remove" => {
                        let name = params.get("name").cloned().unwrap_or_default();
                        if name.is_empty() {
                            Err("Missing 'name' parameter".to_string())
                        } else {
                            crate::commands::k2so_agents::k2so_heartbeat_remove(
                                project_path.clone(), name,
                            ).map(|_| r#"{"success":true}"#.to_string())
                        }
                    }
                    "/cli/heartbeat/enable" => {
                        let name = params.get("name").cloned().unwrap_or_default();
                        let enabled = params.get("enabled").map(|v| v == "true" || v == "1").unwrap_or(true);
                        if name.is_empty() {
                            Err("Missing 'name' parameter".to_string())
                        } else {
                            crate::commands::k2so_agents::k2so_heartbeat_set_enabled(
                                project_path.clone(), name, enabled,
                            ).map(|_| r#"{"success":true}"#.to_string())
                        }
                    }
                    "/cli/heartbeat/edit" => {
                        let name = params.get("name").cloned().unwrap_or_default();
                        let frequency = params.get("frequency").cloned().unwrap_or_default();
                        let spec_json = params.get("spec").cloned().unwrap_or_default();
                        if name.is_empty() || frequency.is_empty() {
                            Err("Missing 'name' or 'frequency' parameter".to_string())
                        } else {
                            crate::commands::k2so_agents::k2so_heartbeat_edit(
                                project_path.clone(), name, frequency, spec_json,
                            ).map(|_| r#"{"success":true}"#.to_string())
                        }
                    }
                    "/cli/heartbeat/rename" => {
                        let old_name = params.get("from").cloned().unwrap_or_default();
                        let new_name = params.get("to").cloned().unwrap_or_default();
                        if old_name.is_empty() || new_name.is_empty() {
                            Err("Missing 'from' or 'to' parameter".to_string())
                        } else {
                            crate::commands::k2so_agents::k2so_heartbeat_rename(
                                project_path.clone(), old_name, new_name,
                            ).map(|_| r#"{"success":true}"#.to_string())
                        }
                    }
                    "/cli/heartbeat/status" => {
                        // Last N fires for a specific heartbeat by name.
                        let name = params.get("name").cloned().unwrap_or_default();
                        let limit = params.get("limit").and_then(|s| s.parse::<i64>().ok()).unwrap_or(10).clamp(1, 200);
                        if name.is_empty() {
                            Err("Missing 'name' parameter".to_string())
                        } else {
                            (|| -> Result<String, String> {
                                let db_path = dirs::home_dir().map(|h| h.join(".k2so/k2so.db")).ok_or("No home dir")?;
                                let db = crate::db::shared();
                                let conn = db.lock();
                                let project_id: String = conn.query_row(
                                    "SELECT id FROM projects WHERE path = ?1",
                                    rusqlite::params![project_path], |row| row.get(0),
                                ).map_err(|e| format!("Project not found: {}", e))?;
                                // Filter heartbeat_fires by schedule_name (may be null for legacy fires)
                                let rows = crate::db::schema::HeartbeatFire::list_by_schedule_name(
                                    &conn, &project_id, &name, limit
                                ).map_err(|e| e.to_string())?;
                                Ok(serde_json::to_string(&rows).unwrap_or_default())
                            })()
                        }
                    }
                    "/cli/hooks/status" => {
                        // Diagnostic endpoint: returns hook injection state across
                        // supported LLM CLIs plus the last N hook events received.
                        // Consumed by `k2so hooks status` to verify the pipeline
                        // end-to-end without human interaction.
                        let limit = params
                            .get("limit")
                            .and_then(|s| s.parse::<usize>().ok())
                            .unwrap_or(20)
                            .min(RECENT_EVENTS_CAP);

                        (|| -> Result<String, String> {
                            let injections = check_hook_injections();
                            let events: Vec<RecentEvent> = {
                                let buf = recent_events().lock();
                                buf.iter().rev().take(limit).cloned().collect()
                            };
                            let payload = serde_json::json!({
                                "port": get_port(),
                                "notify_script": dirs::home_dir()
                                    .map(|h| h.join(".k2so/hooks/notify.sh").to_string_lossy().to_string())
                                    .unwrap_or_default(),
                                "injections": injections,
                                "recent_events": events,
                                "recent_events_cap": RECENT_EVENTS_CAP,
                            });
                            Ok(payload.to_string())
                        })()
                    }
                    "/cli/heartbeat-log" => {
                        // Return the most recent heartbeat fire rows for a project.
                        // Query params: limit (default 50, max 500).
                        let limit = params
                            .get("limit")
                            .and_then(|s| s.parse::<i64>().ok())
                            .unwrap_or(50)
                            .clamp(1, 500);

                        (|| -> Result<String, String> {
                            let db_path = dirs::home_dir()
                                .map(|h| h.join(".k2so/k2so.db"))
                                .ok_or("No home dir")?;
                            let db = crate::db::shared();
                            let conn = db.lock();
                            let project_id: String = conn
                                .query_row(
                                    "SELECT id FROM projects WHERE path = ?1",
                                    rusqlite::params![project_path],
                                    |row| row.get(0),
                                )
                                .map_err(|e| format!("Project not found: {}", e))?;

                            crate::db::schema::HeartbeatFire::list_by_project(&conn, &project_id, limit)
                                .map(|rows| serde_json::to_string(&rows).unwrap_or_default())
                                .map_err(|e| e.to_string())
                        })()
                    }
                    "/cli/scheduler-tick" => {
                        // Scripted triage: deterministic priority/gate logic, no LLM.
                        // Runs synchronously — the function itself is <10ms in typical
                        // workspaces, so we can return the real launch count.
                        //
                        // The triage-in-flight lock remains to prevent overlap when two
                        // ticks race (rare, but possible if launchd fires while the
                        // previous tick is still auditing). Skipped ticks are visible
                        // in `heartbeat_fires` so users aren't left guessing.
                        let already_running = {
                            let mut in_flight = triage_lock().lock();
                            if in_flight.contains(&project_path) {
                                true
                            } else {
                                in_flight.insert(project_path.clone());
                                false
                            }
                        };

                        if already_running {
                            Ok(serde_json::json!({
                                "count": 0,
                                "launched": [],
                                "skipped": "triage already in flight",
                            }).to_string())
                        } else {
                            let result = crate::commands::k2so_agents::k2so_agents_scheduler_tick(project_path.clone());
                            let launched = result.as_ref().cloned().unwrap_or_default();

                            // Fire launch events to the UI for each chosen agent.
                            for agent_name in &launched {
                                if agent_name == "__lead__" {
                                    // Unified builder — resolve the real primary agent like the triage-wake site above.
                                    let _ = crate::commands::k2so_agents::k2so_agents_generate_workspace_claude_md(project_path.clone());
                                    let primary = crate::commands::k2so_agents::find_primary_agent(&project_path)
                                        .unwrap_or_else(|| "__lead__".to_string());
                                    let wakeup_override = crate::commands::k2so_agents::default_heartbeat_wakeup_abs(&project_path, &primary);
                                    if let Ok(launch) = crate::commands::k2so_agents::k2so_agents_build_launch(
                                        project_path.clone(), primary.clone(), None, wakeup_override, Some(true),
                                    ) {
                                        let command = launch.get("command").and_then(|v| v.as_str()).unwrap_or("claude").to_string();
                                        let cwd = launch.get("cwd").and_then(|v| v.as_str()).unwrap_or(&project_path).to_string();
                                        let args: Vec<String> = launch.get("args")
                                            .and_then(|v| v.as_array())
                                            .map(|arr| arr.iter().filter_map(|s| s.as_str().map(String::from)).collect())
                                            .unwrap_or_default();
                                        let _ = spawn_wake_pty(&app_handle, &primary, &project_path, &command, args, &cwd);
                                    }
                                } else if let Ok(launch) = crate::commands::k2so_agents::k2so_agents_build_launch(
                                    project_path.clone(), agent_name.clone(), None, None, None,
                                ) {
                                    let command = launch.get("command").and_then(|v| v.as_str()).unwrap_or("claude").to_string();
                                    let cwd = launch.get("cwd").and_then(|v| v.as_str()).unwrap_or(&project_path).to_string();
                                    let args: Vec<String> = launch.get("args")
                                        .and_then(|v| v.as_array())
                                        .map(|arr| arr.iter().filter_map(|s| s.as_str().map(String::from)).collect())
                                        .unwrap_or_default();
                                    let _ = spawn_wake_pty(&app_handle, agent_name, &project_path, &command, args, &cwd);
                                }
                            }

                            // ── Multi-heartbeat tick ────────────────────────────
                            // Iterate agent_heartbeats and spawn any whose schedules
                            // are eligible. Each candidate carries its own wakeup_path
                            // so different heartbeats can fire different workflows.
                            let hb_candidates = crate::commands::k2so_agents::k2so_agents_heartbeat_tick(&project_path);
                            let mut hb_fired: Vec<String> = Vec::new();
                            for cand in &hb_candidates {
                                if crate::commands::k2so_agents::is_agent_locked(&project_path, &cand.agent_name) {
                                    // Lock-skip: DON'T stamp last_fired — stays eligible for next tick.
                                    log_debug!(
                                        "[heartbeat-tick] {} skipped_locked ({})",
                                        cand.name, cand.agent_name
                                    );
                                    continue;
                                }
                                // Full launch-args builder — see forced-fire site above for rationale.
                                let launched = crate::commands::k2so_agents::k2so_agents_build_launch(
                                    project_path.clone(),
                                    cand.agent_name.clone(),
                                    None,
                                    Some(cand.wakeup_path_abs.clone()),
                                    Some(true), // skip --fork-session (see forced-fire site)
                                )
                                .ok()
                                .and_then(|launch| {
                                    let command = launch.get("command").and_then(|v| v.as_str()).unwrap_or("claude").to_string();
                                    let cwd = launch.get("cwd").and_then(|v| v.as_str()).unwrap_or(&project_path).to_string();
                                    let args: Vec<String> = launch.get("args")
                                        .and_then(|v| v.as_array())
                                        .map(|arr| arr.iter().filter_map(|s| s.as_str().map(String::from)).collect())
                                        .unwrap_or_default();
                                    spawn_wake_pty(
                                        &app_handle,
                                        &cand.agent_name,
                                        &project_path,
                                        &command,
                                        args,
                                        &cwd,
                                    ).ok().map(|tid| {
                                        dismiss_stale_session_dialog_async(app_handle.clone(), tid.clone());
                                        tid
                                    })
                                });
                                if launched.is_some() {
                                    crate::commands::k2so_agents::stamp_heartbeat_fired(&project_path, &cand.name);
                                    hb_fired.push(cand.name.clone());
                                }
                            }

                            // Release triage lock
                            {
                                let mut in_flight = triage_lock().lock();
                                in_flight.remove(&project_path);
                            }

                            match result {
                                Ok(agents) => {
                                    let mut all = agents.clone();
                                    all.extend(hb_fired.clone());
                                    Ok(serde_json::json!({
                                        "count": all.len(),
                                        "launched": all,
                                        "heartbeats": hb_fired,
                                    }).to_string())
                                }
                                Err(e) => Err(e),
                            }
                        }
                    }
                    "/cli/agentic" => {
                        // Master agentic systems toggle (global, not per-project)
                        if let Some(enable) = params.get("enable").cloned() {
                            let on = enable == "1" || enable == "true" || enable == "on";
                            // Update settings via the app settings system
                            use tauri::Manager;
                            if let Some(state) = app_handle.try_state::<crate::state::AppState>() {
                                let conn = state.db.lock();
                                let _ = conn.execute(
                                    "INSERT OR REPLACE INTO app_settings (key, value) VALUES ('agentic_systems_enabled', ?1)",
                                    rusqlite::params![if on { "1" } else { "0" }],
                                );
                            }
                            let _ = app_handle.emit("sync:settings", ());
                            Ok(serde_json::json!({"success": true, "agenticEnabled": on}).to_string())
                        } else {
                            // Read current state
                            use tauri::Manager;
                            let enabled = app_handle.try_state::<crate::state::AppState>()
                                .and_then(|state| {
                                    let conn = state.db.lock();
                                    conn.query_row(
                                        "SELECT value FROM app_settings WHERE key = 'agentic_systems_enabled'",
                                        [],
                                        |row| row.get::<_, String>(0),
                                    ).ok()
                                })
                                .map(|v| v == "1")
                                .unwrap_or(false);
                            Ok(serde_json::json!({"agenticEnabled": enabled}).to_string())
                        }
                    }
                    "/cli/states/list" => {
                        use tauri::Manager;
                        let result = app_handle.try_state::<crate::state::AppState>()
                            .map(|state| {
                                let conn = state.db.lock();
                                crate::db::schema::WorkspaceState::list(&conn)
                                    .map(|states| serde_json::to_string(&states).unwrap_or("[]".to_string()))
                                    .unwrap_or_else(|e| format!("{{\"error\":\"{}\"}}", e))
                            })
                            .unwrap_or_else(|| "[]".to_string());
                        Ok(result)
                    }
                    "/cli/states/get" => {
                        let id = params.get("id").cloned().unwrap_or_default();
                        use tauri::Manager;
                        let result = app_handle.try_state::<crate::state::AppState>()
                            .and_then(|state| {
                                let conn = state.db.lock();
                                crate::db::schema::WorkspaceState::get(&conn, &id).ok()
                            });
                        match result {
                            Some(s) => Ok(serde_json::to_string(&s).unwrap_or_default()),
                            None => Err(format!("State '{}' not found", id)),
                        }
                    }
                    "/cli/states/set" => {
                        // Assign a state to the current workspace
                        let state_id = params.get("state_id").cloned().unwrap_or_default();
                        match cli_update_project_setting(&project_path, "tier_id", &state_id) {
                            Ok(_) => {
                                let _ = app_handle.emit("sync:projects", ());
                                Ok(serde_json::json!({"success": true, "stateId": state_id}).to_string())
                            }
                            Err(e) => Err(e),
                        }
                    }
                    "/cli/settings" => {
                        // Get all settings for this project
                        match cli_get_project_settings(&project_path) {
                            Ok(settings) => Ok(serde_json::to_string(&settings).unwrap_or_default()),
                            Err(e) => Err(e),
                        }
                    }
                    "/cli/commit" | "/cli/commit-merge" => {
                        let include_merge = route == "/cli/commit-merge";
                        let message = params.get("message").cloned().unwrap_or_default();

                        // Gather git context so the AI agent has immediate visibility
                        let git_context = gather_git_context(&project_path);

                        let event_payload = serde_json::json!({
                            "projectPath": project_path,
                            "includeMerge": include_merge,
                            "message": message,
                            "gitContext": git_context,
                        });
                        let _ = app_handle.emit("cli:ai-commit", &event_payload);
                        Ok(serde_json::json!({
                            "success": true,
                            "action": if include_merge { "commit-merge" } else { "commit" },
                            "note": "AI commit terminal session will be launched by K2SO"
                        }).to_string())
                    }
                    "/cli/workspace/create" => {
                        // Create a new folder + register as workspace
                        let target = params.get("path").cloned().unwrap_or_default();
                        if target.is_empty() {
                            Err("Missing 'path' parameter".to_string())
                        } else if std::path::Path::new(&target).exists() {
                            Err(format!("Directory already exists: {}", target))
                        } else {
                            match std::fs::create_dir_all(&target) {
                                Ok(_) => cli_register_workspace(&target, &app_handle),
                                Err(e) => Err(format!("Failed to create directory: {}", e)),
                            }
                        }
                    }
                    "/cli/workspace/remove" => {
                        // Deregister a workspace. If `mode` is passed,
                        // runs the teardown (keep_current | restore_original)
                        // BEFORE the DB row is deleted, so symlinks are
                        // resolved first. Without `mode`, behavior matches
                        // the pre-0.32.7 contract: DB-only delete, files
                        // left as-is (symlinks stay, pointing at the still-
                        // intact canonical SKILL.md).
                        let target = params.get("path").cloned().unwrap_or_default();
                        let mode = params.get("mode").cloned();
                        if target.is_empty() {
                            Err("Missing 'path' parameter".to_string())
                        } else {
                            cli_remove_workspace(&target, mode.as_deref(), &app_handle)
                        }
                    }
                    "/cli/workspace/cleanup" => {
                        // Remove workspace DB records for worktrees that no longer exist on disk
                        cli_cleanup_stale_workspaces(&app_handle)
                    }
                    "/cli/workspace/open" => {
                        // Register an existing folder as workspace
                        let target = params.get("path").cloned().unwrap_or_default();
                        if target.is_empty() {
                            Err("Missing 'path' parameter".to_string())
                        } else if !std::path::Path::new(&target).is_dir() {
                            Err(format!("Directory not found: {}", target))
                        } else {
                            cli_register_workspace(&target, &app_handle)
                        }
                    }
                    "/cli/agent/complete" => {
                        // Sub-agent completion: reads workspace state, auto-merges or moves to done
                        let agent = params.get("agent").cloned().unwrap_or_default();
                        let file = params.get("file").cloned().unwrap_or_default();
                        if agent.is_empty() || file.is_empty() {
                            Err("Missing 'agent' or 'file' parameter".to_string())
                        } else {
                            crate::commands::k2so_agents::k2so_agent_complete(project_path, agent, file)
                        }
                    }
                    "/cli/agents/running" => {
                        // List all terminals with running CLI LLM agents
                        if let Some(state) = app_handle.try_state::<crate::state::AppState>() {
                            let manager = state.terminal_manager.lock();
                            let terminal_ids = manager.list_terminal_ids();
                            let mut agents = Vec::new();
                            for (id, cwd) in &terminal_ids {
                                let command = manager.get_foreground_command(id).ok().flatten();
                                agents.push(serde_json::json!({
                                    "terminalId": id,
                                    "cwd": cwd,
                                    "command": command,
                                }));
                            }
                            Ok(serde_json::to_string(&agents).unwrap_or("[]".to_string()))
                        } else {
                            Ok("[]".to_string())
                        }
                    }
                    "/cli/terminal/write" => {
                        // Write text to a running terminal (virtual input)
                        // Two-phase write: paste text first, then send Enter separately
                        // after a delay. CLI LLMs treat paste+Enter as one event and
                        // swallow the trailing \r.
                        let terminal_id = params.get("id").cloned().unwrap_or_default();
                        let message = params.get("message").cloned().unwrap_or_default();
                        let no_submit = params.get("no_submit").map(|v| v == "true" || v == "1").unwrap_or(false);
                        if terminal_id.is_empty() || message.is_empty() {
                            Err("Missing 'id' or 'message' parameter".to_string())
                        } else if let Some(state) = app_handle.try_state::<crate::state::AppState>() {
                            // Phase 1: write the message text (no \r)
                            let write_result = state.terminal_manager.lock().write(&terminal_id, &message);
                            if let Err(e) = write_result {
                                Err(e)
                            } else if no_submit {
                                Ok(r#"{"success":true}"#.to_string())
                            } else {
                                // Phase 2: wait for paste to settle, then send Enter
                                let tid = terminal_id.clone();
                                let app = app_handle.clone();
                                std::thread::spawn(move || {
                                    std::thread::sleep(std::time::Duration::from_millis(150));
                                    if let Some(s) = app.try_state::<crate::state::AppState>() {
                                        let _ = s.terminal_manager.lock().write(&tid, "\r");
                                    }
                                });
                                Ok(r#"{"success":true}"#.to_string())
                            }
                        } else {
                            Err("AppState not available".to_string())
                        }
                    }
                    "/cli/terminal/read" => {
                        // Read last N lines from a terminal buffer.
                        // With scrollback=true, reads from scrollback history (up to N lines).
                        let terminal_id = params.get("id").cloned().unwrap_or_default();
                        let count: usize = params.get("lines").and_then(|s| s.parse().ok()).unwrap_or(50);
                        let scrollback = params.get("scrollback").map(|v| v == "true" || v == "1").unwrap_or(false);
                        if terminal_id.is_empty() {
                            Err("Missing 'id' parameter".to_string())
                        } else if let Some(state) = app_handle.try_state::<crate::state::AppState>() {
                            match state.terminal_manager.lock().read_lines_with_scrollback(&terminal_id, count, scrollback) {
                                Ok(lines) => Ok(serde_json::json!({ "lines": lines }).to_string()),
                                Err(e) => Err(e),
                            }
                        } else {
                            Err("AppState not available".to_string())
                        }
                    }
                    "/cli/checkin" => {
                        // Aggregated check-in: task + inbox + peers + reservations + feed
                        let agent = params.get("agent").cloned().unwrap_or_default();
                        if agent.is_empty() {
                            Err("Missing 'agent' parameter".to_string())
                        } else {
                            (|| -> Result<String, String> {
                                let db_path = dirs::home_dir()
                                    .ok_or("No home dir")?
                                    .join(".k2so/k2so.db");
                                let db = crate::db::shared();
                                let conn = db.lock();
                                let _ = conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;");

                                let project_id: String = conn.query_row(
                                    "SELECT id FROM projects WHERE path = ?1",
                                    rusqlite::params![project_path],
                                    |row| row.get(0),
                                ).map_err(|e| format!("Project not found: {}", e))?;

                                // Helper: parse frontmatter from markdown content
                                fn parse_work_item(filename: &str, content: &str) -> serde_json::Value {
                                    let mut title = filename.trim_end_matches(".md").to_string();
                                    let mut priority = "normal".to_string();
                                    let mut item_type = "task".to_string();
                                    let mut from = serde_json::Value::Null;
                                    let mut body = content.to_string();

                                    if content.starts_with("---\n") {
                                        if let Some(end) = content[4..].find("\n---") {
                                            let fm = &content[4..4+end];
                                            body = content[4+end+4..].trim().to_string();
                                            for line in fm.lines() {
                                                let parts: Vec<&str> = line.splitn(2, ':').collect();
                                                if parts.len() == 2 {
                                                    let key = parts[0].trim();
                                                    let val = parts[1].trim().trim_matches('"');
                                                    match key {
                                                        "title" => title = val.to_string(),
                                                        "priority" => priority = val.to_string(),
                                                        "type" => item_type = val.to_string(),
                                                        "from" => from = serde_json::Value::String(val.to_string()),
                                                        "assigned_by" if from.is_null() => from = serde_json::Value::String(val.to_string()),
                                                        _ => {}
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    serde_json::json!({
                                        "file": filename,
                                        "title": title,
                                        "priority": priority,
                                        "type": item_type,
                                        "from": from,
                                        "body": body,
                                    })
                                }

                                // Current task: first file in active/ (structured)
                                let active_dir = std::path::PathBuf::from(&project_path)
                                    .join(".k2so/agents").join(&agent).join("work/active");
                                let task: serde_json::Value = if active_dir.is_dir() {
                                    std::fs::read_dir(&active_dir).ok()
                                        .and_then(|mut entries| entries.next())
                                        .and_then(|e| e.ok())
                                        .map(|e| {
                                            let fname = e.file_name().to_string_lossy().to_string();
                                            let content = std::fs::read_to_string(e.path()).unwrap_or_default();
                                            parse_work_item(&fname, &content)
                                        })
                                        .unwrap_or(serde_json::Value::Null)
                                } else {
                                    serde_json::Value::Null
                                };

                                // Work items: from filesystem (.md files with detailed specs)
                                let inbox_dir = std::path::PathBuf::from(&project_path)
                                    .join(".k2so/agents").join(&agent).join("work/inbox");
                                let mut work_items: Vec<serde_json::Value> = if inbox_dir.is_dir() {
                                    std::fs::read_dir(&inbox_dir).ok()
                                        .map(|entries| entries.filter_map(|e| e.ok())
                                            .map(|e| {
                                                let fname = e.file_name().to_string_lossy().to_string();
                                                let content = std::fs::read_to_string(e.path()).unwrap_or_default();
                                                parse_work_item(&fname, &content)
                                            })
                                            .collect())
                                        .unwrap_or_default()
                                } else {
                                    vec![]
                                };

                                // Also check workspace-level inbox for manager agents
                                let ws_inbox_dir = std::path::PathBuf::from(&project_path).join(".k2so/work/inbox");
                                if ws_inbox_dir.is_dir() {
                                    if let Ok(entries) = std::fs::read_dir(&ws_inbox_dir) {
                                        for e in entries.flatten() {
                                            let fname = e.file_name().to_string_lossy().to_string();
                                            let content = std::fs::read_to_string(e.path()).unwrap_or_default();
                                            work_items.push(parse_work_item(&fname, &content));
                                        }
                                    }
                                }

                                // Messages: from DB (fast, indexed, scoped to this agent)
                                let messages: Vec<serde_json::Value> = crate::db::schema::get_unread_messages(&conn, &project_id, &agent)
                                    .unwrap_or_default()
                                    .into_iter()
                                    .map(|m| {
                                        let text = m.metadata.as_deref()
                                            .and_then(|md| serde_json::from_str::<serde_json::Value>(md).ok())
                                            .and_then(|v| v.get("text").and_then(|t| t.as_str()).map(|s| s.to_string()))
                                            .unwrap_or_else(|| m.summary.clone().unwrap_or_default());
                                        serde_json::json!({
                                            "type": "message",
                                            "from": m.from_agent,
                                            "text": text,
                                            "at": m.created_at,
                                            "id": m.id,
                                        })
                                    })
                                    .collect();

                                // Mark messages as read after retrieval
                                let _ = crate::db::schema::mark_messages_read(&conn, &project_id, &agent);

                                // Combine into unified inbox
                                let inbox = serde_json::json!({
                                    "work": work_items,
                                    "messages": messages,
                                });

                                // Peers: all agent_sessions in this project + related projects
                                let mut peer_project_ids = vec![project_id.clone()];
                                if let Ok(rels) = crate::db::schema::WorkspaceRelation::list_for_source(&conn, &project_id) {
                                    for r in &rels {
                                        peer_project_ids.push(r.target_project_id.clone());
                                    }
                                }
                                if let Ok(rels) = crate::db::schema::WorkspaceRelation::list_for_target(&conn, &project_id) {
                                    for r in &rels {
                                        peer_project_ids.push(r.source_project_id.clone());
                                    }
                                }
                                // Build project name lookup for readable peer info
                                let mut project_names: std::collections::HashMap<String, String> = std::collections::HashMap::new();
                                for pid in &peer_project_ids {
                                    if let Ok(name) = conn.query_row(
                                        "SELECT name FROM projects WHERE id = ?1",
                                        rusqlite::params![pid],
                                        |row| row.get::<_, String>(0),
                                    ) {
                                        project_names.insert(pid.clone(), name);
                                    }
                                }

                                let mut peers = Vec::new();
                                for pid in &peer_project_ids {
                                    if let Ok(sessions) = crate::db::schema::AgentSession::list_by_project(&conn, pid) {
                                        let pname = project_names.get(pid).cloned().unwrap_or_default();
                                        for s in sessions {
                                            if s.agent_name == agent && s.project_id == project_id {
                                                continue; // skip self
                                            }
                                            peers.push(serde_json::json!({
                                                "agent": s.agent_name,
                                                "status": s.status,
                                                "statusMessage": s.status_message,
                                                "terminalId": s.terminal_id,
                                                "project": pname,
                                                "projectId": s.project_id,
                                                "harness": s.harness,
                                            }));
                                        }
                                    }
                                }

                                // Reservations
                                let reservations_path = std::path::PathBuf::from(&project_path)
                                    .join(".k2so/reservations.json");
                                let reservations: serde_json::Value = if reservations_path.exists() {
                                    std::fs::read_to_string(&reservations_path).ok()
                                        .and_then(|s| serde_json::from_str(&s).ok())
                                        .unwrap_or(serde_json::json!({}))
                                } else {
                                    serde_json::json!({})
                                };

                                // Recent feed: last 10
                                let feed: Vec<serde_json::Value> = crate::db::schema::ActivityFeedEntry::list_by_project(&conn, &project_id, 10, 0)
                                    .unwrap_or_default()
                                    .into_iter()
                                    .map(|e| serde_json::json!({
                                        "eventType": e.event_type,
                                        "agent": e.agent_name,
                                        "from": e.from_agent,
                                        "to": e.to_agent,
                                        "summary": e.summary,
                                        "createdAt": e.created_at,
                                    }))
                                    .collect();

                                // Log checkin
                                crate::db::schema::log_activity(&conn, &project_id, Some(&agent), "checkin", Some(&agent), None, None, None);

                                // Wake-up instructions — the agent's wakeup.md content,
                                // or the shipped template if the user hasn't customized.
                                // `null` for agents whose type doesn't use wake-up
                                // (agent-template). Workspace-level (__lead__) uses a
                                // different file at .k2so/wakeup.md.
                                let wakeup_instructions: serde_json::Value = if agent == "__lead__" {
                                    serde_json::Value::String(crate::commands::k2so_agents::compose_wake_prompt_for_lead(&project_path))
                                } else {
                                    match crate::commands::k2so_agents::compose_wake_prompt_for_agent(&project_path, &agent) {
                                        Some(s) => serde_json::Value::String(s),
                                        None => serde_json::Value::Null,
                                    }
                                };

                                Ok(serde_json::json!({
                                    "agent": agent,
                                    "project": project_path,
                                    "task": task,
                                    "inbox": inbox,
                                    "peers": peers,
                                    "reservations": reservations,
                                    "feed": feed,
                                    "wakeupInstructions": wakeup_instructions,
                                }).to_string())
                            })()
                        }
                    }
                    "/cli/status" => {
                        // Update agent status message
                        let agent = params.get("agent").cloned().unwrap_or_default();
                        let message = params.get("message").cloned().unwrap_or_default();
                        if agent.is_empty() {
                            Err("Missing 'agent' parameter".to_string())
                        } else {
                            (|| -> Result<String, String> {
                                let db_path = dirs::home_dir()
                                    .ok_or("No home dir")?
                                    .join(".k2so/k2so.db");
                                let db = crate::db::shared();
                                let conn = db.lock();
                                let _ = conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;");

                                let project_id: String = conn.query_row(
                                    "SELECT id FROM projects WHERE path = ?1",
                                    rusqlite::params![project_path],
                                    |row| row.get(0),
                                ).map_err(|e| format!("Project not found: {}", e))?;

                                crate::db::schema::AgentSession::update_status_message(&conn, &project_id, &agent, &message)
                                    .map_err(|e| format!("Failed to update status: {}", e))?;

                                crate::db::schema::log_activity(&conn, &project_id, Some(&agent), "status", Some(&agent), None, None, Some(&message));

                                Ok(serde_json::json!({"success": true}).to_string())
                            })()
                        }
                    }
                    "/cli/done" => {
                        // Complete agent's current task
                        let agent = params.get("agent").cloned().unwrap_or_default();
                        let blocked = params.get("blocked").cloned();
                        if agent.is_empty() {
                            Err("Missing 'agent' parameter".to_string())
                        } else {
                            (|| -> Result<String, String> {
                                let db_path = dirs::home_dir()
                                    .ok_or("No home dir")?
                                    .join(".k2so/k2so.db");
                                let db = crate::db::shared();
                                let conn = db.lock();
                                let _ = conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;");

                                let project_id: String = conn.query_row(
                                    "SELECT id FROM projects WHERE path = ?1",
                                    rusqlite::params![project_path],
                                    |row| row.get(0),
                                ).map_err(|e| format!("Project not found: {}", e))?;

                                // Move first file from active/ to done/
                                let active_dir = std::path::PathBuf::from(&project_path)
                                    .join(".k2so/agents").join(&agent).join("work/active");
                                let done_dir = std::path::PathBuf::from(&project_path)
                                    .join(".k2so/agents").join(&agent).join("work/done");
                                let mut moved_file = None;

                                if active_dir.is_dir() {
                                    if let Some(Ok(entry)) = std::fs::read_dir(&active_dir).ok().and_then(|mut d| d.next()) {
                                        let _ = std::fs::create_dir_all(&done_dir);
                                        let dest = done_dir.join(entry.file_name());
                                        if std::fs::rename(entry.path(), &dest).is_ok() {
                                            moved_file = Some(entry.file_name().to_string_lossy().to_string());
                                        }
                                    }
                                }

                                // Update agent status to sleeping
                                let _ = crate::db::schema::AgentSession::update_status(&conn, &project_id, &agent, "sleeping");

                                let event_type = if blocked.is_some() { "task.blocked" } else { "task.done" };
                                let summary = moved_file.as_deref().unwrap_or("no active task");
                                crate::db::schema::log_activity(&conn, &project_id, Some(&agent), event_type, Some(&agent), None, None, Some(summary));

                                Ok(serde_json::json!({
                                    "success": true,
                                    "event": event_type,
                                    "file": moved_file,
                                }).to_string())
                            })()
                        }
                    }
                    "/cli/msg" => {
                        // Send a message to another agent or workspace inbox
                        // --wake flag: also wake the target agent (PTY inject → resume → fresh)
                        let agent = params.get("agent").cloned().unwrap_or_default();
                        let target = params.get("target").cloned().unwrap_or_default();
                        let text = params.get("text").cloned().unwrap_or_default();
                        let wake = params.get("wake").map(|v| v == "true" || v == "1").unwrap_or(false);
                        if agent.is_empty() || target.is_empty() || text.is_empty() {
                            Err("Missing 'agent', 'target', or 'text' parameter".to_string())
                        } else {
                            (|| -> Result<String, String> {
                                let db_path = dirs::home_dir()
                                    .ok_or("No home dir")?
                                    .join(".k2so/k2so.db");
                                let db = crate::db::shared();
                                let conn = db.lock();
                                let _ = conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;");

                                let project_id: String = conn.query_row(
                                    "SELECT id FROM projects WHERE path = ?1",
                                    rusqlite::params![project_path],
                                    |row| row.get(0),
                                ).map_err(|e| format!("Project not found: {}", e))?;

                                let now = chrono::Local::now().to_rfc3339();
                                let filename = format!("msg-{}-{}.md", agent, chrono::Local::now().format("%Y%m%d-%H%M%S"));

                                // Resolve sender's workspace name for qualified from field
                                let sender_workspace: String = conn.query_row(
                                    "SELECT name FROM projects WHERE path = ?1",
                                    rusqlite::params![project_path],
                                    |row| row.get(0),
                                ).unwrap_or_else(|_| {
                                    std::path::Path::new(&project_path)
                                        .file_name()
                                        .map(|n| n.to_string_lossy().to_string())
                                        .unwrap_or_default()
                                });
                                let qualified_from = format!("{}:{}", sender_workspace, agent);

                                let content = format!(
                                    "---\ntitle: Message from {}\ntype: message\npriority: normal\nassigned_by: {}\nfrom: {}\ncreated: {}\n---\n\n{}",
                                    qualified_from, agent, qualified_from, now, text
                                );

                                // Resolve target — supports:
                                //   "agent-name"           → same workspace, agent inbox
                                //   "inbox"                → current workspace inbox
                                //   "workspace:inbox"      → cross-workspace, workspace inbox
                                //   "workspace:agent-name" → cross-workspace, agent inbox
                                let (inbox_dir, to_agent_name, to_project_id) = if target.contains(':') {
                                    // Cross-workspace target
                                    let parts: Vec<&str> = target.splitn(2, ':').collect();
                                    let target_workspace = parts[0];
                                    let target_agent = parts.get(1).unwrap_or(&"inbox");

                                    // Resolve workspace name → project path via DB
                                    // First check workspace_relations, then try direct name lookup
                                    let mut target_path: Option<String> = None;
                                    let mut target_pid: Option<String> = None;

                                    // Look up by project name (case-insensitive match on name)
                                    if let Ok(row) = conn.query_row(
                                        "SELECT id, path FROM projects WHERE name = ?1",
                                        rusqlite::params![target_workspace],
                                        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                                    ) {
                                        target_pid = Some(row.0);
                                        target_path = Some(row.1);
                                    }

                                    // Verify there's a relation (connected workspaces only)
                                    if let Some(ref tpid) = target_pid {
                                        let has_relation = crate::db::schema::WorkspaceRelation::list_for_source(&conn, &project_id)
                                            .unwrap_or_default()
                                            .iter()
                                            .any(|r| r.target_project_id == *tpid);
                                        let has_incoming = crate::db::schema::WorkspaceRelation::list_for_target(&conn, &project_id)
                                            .unwrap_or_default()
                                            .iter()
                                            .any(|r| r.source_project_id == *tpid);
                                        if !has_relation && !has_incoming {
                                            return Err(format!("Workspace '{}' is not connected. Use 'k2so connections add {}' first.", target_workspace, target_workspace));
                                        }
                                    }

                                    let resolved_path = target_path.ok_or(format!("Workspace '{}' not found", target_workspace))?;

                                    if *target_agent == "inbox" {
                                        // Workspace inbox (for manager triage)
                                        let dir = std::path::PathBuf::from(&resolved_path).join(".k2so/work/inbox");
                                        (dir, None, target_pid)
                                    } else {
                                        // Specific agent inbox in target workspace
                                        let dir = std::path::PathBuf::from(&resolved_path)
                                            .join(".k2so/agents").join(target_agent).join("work/inbox");
                                        (dir, Some(target_agent.to_string()), target_pid)
                                    }
                                } else if target == "inbox" {
                                    // Current workspace inbox
                                    let dir = std::path::PathBuf::from(&project_path).join(".k2so/work/inbox");
                                    (dir, None, None)
                                } else {
                                    // Same project: write to agent's inbox
                                    let dir = std::path::PathBuf::from(&project_path)
                                        .join(".k2so/agents").join(&target).join("work/inbox");
                                    (dir, Some(target.clone()), None)
                                };

                                // Messages go to DB only (fast, indexed, queryable).
                                // Work items (.md files) are for tasks with detailed specs.
                                // Store full message text in metadata for retrieval.
                                let msg_metadata = serde_json::json!({
                                    "text": text,
                                    "from_qualified": qualified_from,
                                }).to_string();

                                // Log to sender's project feed
                                crate::db::schema::ActivityFeedEntry::insert(
                                    &conn, &project_id, Some(&agent), "message.sent",
                                    Some(&qualified_from), to_agent_name.as_deref(), to_project_id.as_deref(),
                                    Some(&format!("To {}: {}", target, &text[..text.len().min(100)])),
                                    Some(&msg_metadata),
                                ).ok();

                                // Log to recipient's project feed (so they can query it)
                                let recipient_project_id = to_project_id.as_deref().unwrap_or(&project_id);
                                crate::db::schema::ActivityFeedEntry::insert(
                                    &conn, recipient_project_id, to_agent_name.as_deref(), "message.received",
                                    Some(&qualified_from), to_agent_name.as_deref(), Some(&project_id),
                                    Some(&format!("{}: {}", qualified_from, &text[..text.len().min(200)])),
                                    Some(&msg_metadata),
                                ).ok();

                                // Wake chain (only if --wake flag set)
                                let mut wake_status = "inbox_only".to_string();
                                if wake {
                                    // Determine which agent to wake and in which project
                                    let wake_project_path = if let Some(ref tpid) = to_project_id {
                                        conn.query_row(
                                            "SELECT path FROM projects WHERE id = ?1",
                                            rusqlite::params![tpid],
                                            |row| row.get::<_, String>(0),
                                        ).ok()
                                    } else {
                                        Some(project_path.to_string())
                                    };

                                    // Resolve the agent name to wake — for :inbox targets, wake the manager (__lead__)
                                    let wake_agent = to_agent_name.clone().unwrap_or_else(|| "__lead__".to_string());

                                    if let Some(ref wp) = wake_project_path {
                                        let terminal_id = format!("agent-chat-{}", wake_agent);

                                        // Step 1: Try direct PTY injection — send the message inline
                                        use tauri::Manager;
                                        let wake_msg = format!(
                                            "[Message from {}]: {}\n\nRun `k2so checkin` to see your full inbox.",
                                            qualified_from,
                                            text.chars().take(500).collect::<String>(),
                                        );
                                        let injected = app_handle.try_state::<crate::state::AppState>()
                                            .map(|state| {
                                                let mgr = state.terminal_manager.lock();
                                                if mgr.exists(&terminal_id) {
                                                    if mgr.write(&terminal_id, &wake_msg).is_ok() {
                                                        // Two-phase write: send Enter separately after paste settles
                                                        let tid = terminal_id.clone();
                                                        let app = app_handle.clone();
                                                        std::thread::spawn(move || {
                                                            std::thread::sleep(std::time::Duration::from_millis(150));
                                                            if let Some(s) = app.try_state::<crate::state::AppState>() {
                                                                let _ = s.terminal_manager.lock().write(&tid, "\r");
                                                            }
                                                        });
                                                        true
                                                    } else { false }
                                                } else {
                                                    false
                                                }
                                            })
                                            .unwrap_or(false);

                                        if injected {
                                            wake_status = "injected".to_string();
                                        } else {
                                            // Step 2: Check DB for session to resume
                                            let wake_project_id = to_project_id.clone()
                                                .or_else(|| Some(project_id.clone()));

                                            if let Some(ref wpid) = wake_project_id {
                                                let session = crate::db::schema::AgentSession::get_by_agent(&conn, wpid, &wake_agent).ok().flatten();
                                                let has_prior = session.as_ref().map(|s| s.session_id.is_some()).unwrap_or(false);
                                                if let Ok(info) = crate::commands::k2so_agents::k2so_agents_build_launch(
                                                    wp.clone(), wake_agent.clone(), None, None, None,
                                                ) {
                                                    // Backend-direct spawn — work-send wake fires regardless of window state.
                                                    let command = info.get("command").and_then(|v| v.as_str()).unwrap_or("claude").to_string();
                                                    let cwd = info.get("cwd").and_then(|v| v.as_str()).unwrap_or(&wp).to_string();
                                                    let args: Vec<String> = info.get("args")
                                                        .and_then(|v| v.as_array())
                                                        .map(|arr| arr.iter().filter_map(|s| s.as_str().map(String::from)).collect())
                                                        .unwrap_or_default();
                                                    let _ = spawn_wake_pty(&app_handle, &wake_agent, &wp, &command, args, &cwd);
                                                    wake_status = if has_prior { "resumed".to_string() } else { "launched_fresh".to_string() };
                                                }
                                            }
                                        }
                                    }
                                }

                                Ok(serde_json::json!({
                                    "success": true,
                                    "file": filename,
                                    "target": target,
                                    "wake": wake_status,
                                }).to_string())
                            })()
                        }
                    }
                    "/cli/reserve" => {
                        // Claim files for exclusive editing
                        let agent = params.get("agent").cloned().unwrap_or_default();
                        let paths_str = params.get("paths").cloned().unwrap_or_default();
                        if agent.is_empty() || paths_str.is_empty() {
                            Err("Missing 'agent' or 'paths' parameter".to_string())
                        } else {
                            (|| -> Result<String, String> {
                                let db_path = dirs::home_dir()
                                    .ok_or("No home dir")?
                                    .join(".k2so/k2so.db");
                                let db = crate::db::shared();
                                let conn = db.lock();
                                let _ = conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;");

                                let project_id: String = conn.query_row(
                                    "SELECT id FROM projects WHERE path = ?1",
                                    rusqlite::params![project_path],
                                    |row| row.get(0),
                                ).map_err(|e| format!("Project not found: {}", e))?;

                                let reservations_path = std::path::PathBuf::from(&project_path)
                                    .join(".k2so/reservations.json");
                                let _ = std::fs::create_dir_all(std::path::PathBuf::from(&project_path).join(".k2so"));

                                let mut reservations: serde_json::Map<String, serde_json::Value> = if reservations_path.exists() {
                                    std::fs::read_to_string(&reservations_path).ok()
                                        .and_then(|s| serde_json::from_str(&s).ok())
                                        .unwrap_or_default()
                                } else {
                                    serde_json::Map::new()
                                };

                                let now = chrono::Local::now().to_rfc3339();
                                let paths: Vec<&str> = paths_str.split(',').map(|s| s.trim()).collect();
                                let mut reserved = Vec::new();
                                let mut conflicts = Vec::new();

                                for p in &paths {
                                    if let Some(existing) = reservations.get(*p) {
                                        let existing_agent = existing.get("agent").and_then(|v| v.as_str()).unwrap_or("");
                                        if existing_agent != agent {
                                            conflicts.push(serde_json::json!({"path": p, "heldBy": existing_agent}));
                                            continue;
                                        }
                                    }
                                    reservations.insert(p.to_string(), serde_json::json!({
                                        "agent": agent,
                                        "reason": "",
                                        "timestamp": now,
                                    }));
                                    reserved.push(p.to_string());
                                }

                                std::fs::write(&reservations_path, serde_json::to_string_pretty(&reservations).unwrap_or_default())
                                    .map_err(|e| format!("Failed to write reservations: {}", e))?;

                                crate::db::schema::log_activity(
                                    &conn, &project_id, Some(&agent), "reserve",
                                    Some(&agent), None, None,
                                    Some(&format!("Reserved {} file(s)", reserved.len())),
                                );

                                Ok(serde_json::json!({
                                    "success": true,
                                    "reserved": reserved,
                                    "conflicts": conflicts,
                                }).to_string())
                            })()
                        }
                    }
                    "/cli/release" => {
                        // Release file reservations
                        let agent = params.get("agent").cloned().unwrap_or_default();
                        let paths_str = params.get("paths").cloned().unwrap_or_default();
                        if agent.is_empty() {
                            Err("Missing 'agent' parameter".to_string())
                        } else {
                            (|| -> Result<String, String> {
                                let db_path = dirs::home_dir()
                                    .ok_or("No home dir")?
                                    .join(".k2so/k2so.db");
                                let db = crate::db::shared();
                                let conn = db.lock();
                                let _ = conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;");

                                let project_id: String = conn.query_row(
                                    "SELECT id FROM projects WHERE path = ?1",
                                    rusqlite::params![project_path],
                                    |row| row.get(0),
                                ).map_err(|e| format!("Project not found: {}", e))?;

                                let reservations_path = std::path::PathBuf::from(&project_path)
                                    .join(".k2so/reservations.json");

                                if !reservations_path.exists() {
                                    return Ok(serde_json::json!({"success": true, "released": 0}).to_string());
                                }

                                let mut reservations: serde_json::Map<String, serde_json::Value> =
                                    std::fs::read_to_string(&reservations_path).ok()
                                        .and_then(|s| serde_json::from_str(&s).ok())
                                        .unwrap_or_default();

                                let specific_paths: Vec<&str> = if paths_str.is_empty() {
                                    vec![]
                                } else {
                                    paths_str.split(',').map(|s| s.trim()).collect()
                                };

                                let mut released = 0;
                                let keys_to_remove: Vec<String> = reservations.iter()
                                    .filter(|(key, val)| {
                                        let held_by = val.get("agent").and_then(|v| v.as_str()).unwrap_or("");
                                        if held_by != agent { return false; }
                                        if specific_paths.is_empty() { return true; }
                                        specific_paths.contains(&key.as_str())
                                    })
                                    .map(|(key, _)| key.clone())
                                    .collect();

                                for key in &keys_to_remove {
                                    reservations.remove(key);
                                    released += 1;
                                }

                                std::fs::write(&reservations_path, serde_json::to_string_pretty(&reservations).unwrap_or_default())
                                    .map_err(|e| format!("Failed to write reservations: {}", e))?;

                                crate::db::schema::log_activity(
                                    &conn, &project_id, Some(&agent), "release",
                                    Some(&agent), None, None,
                                    Some(&format!("Released {} file(s)", released)),
                                );

                                Ok(serde_json::json!({
                                    "success": true,
                                    "released": released,
                                }).to_string())
                            })()
                        }
                    }
                    "/cli/connections" => {
                        // List, create, or delete workspace relations
                        let action = params.get("action").cloned().unwrap_or_else(|| "list".to_string());
                        (|| -> Result<String, String> {
                            let db_path = dirs::home_dir()
                                .ok_or("No home dir")?
                                .join(".k2so/k2so.db");
                            let db = crate::db::shared();
                            let conn = db.lock();
                            let _ = conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;");

                            let project_id: String = conn.query_row(
                                "SELECT id FROM projects WHERE path = ?1",
                                rusqlite::params![project_path],
                                |row| row.get(0),
                            ).map_err(|e| format!("Project not found: {}", e))?;

                            match action.as_str() {
                                "list" => {
                                    // List outgoing connections (this project oversees)
                                    let outgoing = crate::db::schema::WorkspaceRelation::list_for_source(&conn, &project_id)
                                        .map_err(|e| e.to_string())?;
                                    // List incoming connections (overseen by)
                                    let incoming = crate::db::schema::WorkspaceRelation::list_for_target(&conn, &project_id)
                                        .map_err(|e| e.to_string())?;

                                    // Resolve project names
                                    let mut connections = Vec::new();
                                    for rel in &outgoing {
                                        let name: String = conn.query_row(
                                            "SELECT name FROM projects WHERE id = ?1",
                                            rusqlite::params![rel.target_project_id],
                                            |row| row.get(0),
                                        ).unwrap_or_else(|_| "Unknown".to_string());
                                        connections.push(serde_json::json!({
                                            "id": rel.id,
                                            "direction": "outgoing",
                                            "type": rel.relation_type,
                                            "projectId": rel.target_project_id,
                                            "projectName": name,
                                        }));
                                    }
                                    for rel in &incoming {
                                        let name: String = conn.query_row(
                                            "SELECT name FROM projects WHERE id = ?1",
                                            rusqlite::params![rel.source_project_id],
                                            |row| row.get(0),
                                        ).unwrap_or_else(|_| "Unknown".to_string());
                                        connections.push(serde_json::json!({
                                            "id": rel.id,
                                            "direction": "incoming",
                                            "type": rel.relation_type,
                                            "projectId": rel.source_project_id,
                                            "projectName": name,
                                        }));
                                    }
                                    Ok(serde_json::json!({ "connections": connections }).to_string())
                                }
                                "add" => {
                                    let target_name = params.get("target").cloned().unwrap_or_default();
                                    if target_name.is_empty() {
                                        return Err("Missing 'target' parameter (workspace name or path)".to_string());
                                    }
                                    // Resolve target by name or path
                                    let target_id: String = conn.query_row(
                                        "SELECT id FROM projects WHERE name = ?1 OR path = ?1",
                                        rusqlite::params![target_name],
                                        |row| row.get(0),
                                    ).map_err(|_| format!("Workspace '{}' not found", target_name))?;

                                    let id = uuid::Uuid::new_v4().to_string();
                                    let rel_type = params.get("type").cloned().unwrap_or_else(|| "oversees".to_string());
                                    crate::db::schema::WorkspaceRelation::create(&conn, &id, &project_id, &target_id, &rel_type)
                                        .map_err(|e| e.to_string())?;

                                    let target_display: String = conn.query_row(
                                        "SELECT name FROM projects WHERE id = ?1",
                                        rusqlite::params![target_id],
                                        |row| row.get(0),
                                    ).unwrap_or_else(|_| target_name.clone());

                                    crate::db::schema::log_activity(
                                        &conn, &project_id, None, "connection.created",
                                        None, None, Some(&target_id),
                                        Some(&format!("Connected to {}", target_display)),
                                    );

                                    Ok(serde_json::json!({
                                        "success": true,
                                        "id": id,
                                        "target": target_display,
                                    }).to_string())
                                }
                                "remove" => {
                                    let target_name = params.get("target").cloned().unwrap_or_default();
                                    if target_name.is_empty() {
                                        return Err("Missing 'target' parameter".to_string());
                                    }
                                    // Resolve target
                                    let target_id: String = conn.query_row(
                                        "SELECT id FROM projects WHERE name = ?1 OR path = ?1",
                                        rusqlite::params![target_name],
                                        |row| row.get(0),
                                    ).map_err(|_| format!("Workspace '{}' not found", target_name))?;

                                    // Find and delete the relation
                                    let rel_id: Result<String, _> = conn.query_row(
                                        "SELECT id FROM workspace_relations WHERE source_project_id = ?1 AND target_project_id = ?2",
                                        rusqlite::params![project_id, target_id],
                                        |row| row.get(0),
                                    );
                                    match rel_id {
                                        Ok(id) => {
                                            crate::db::schema::WorkspaceRelation::delete(&conn, &id)
                                                .map_err(|e| e.to_string())?;
                                            crate::db::schema::log_activity(
                                                &conn, &project_id, None, "connection.removed",
                                                None, None, Some(&target_id),
                                                Some(&format!("Disconnected from {}", target_name)),
                                            );
                                            Ok(serde_json::json!({"success": true}).to_string())
                                        }
                                        Err(_) => Err(format!("No connection to '{}' found", target_name)),
                                    }
                                }
                                _ => Err(format!("Unknown action '{}'. Use: list, add, remove", action)),
                            }
                        })()
                    }
                    "/cli/companion/start" => {
                        match crate::companion::start_companion(app_handle.clone()) {
                            Ok(url) => Ok(serde_json::json!({"ok": true, "url": url}).to_string()),
                            Err(e) => Err(e),
                        }
                    }
                    "/cli/companion/stop" => {
                        match crate::companion::stop_companion() {
                            Ok(()) => Ok(serde_json::json!({"ok": true}).to_string()),
                            Err(e) => Err(e),
                        }
                    }
                    "/cli/companion/status" => {
                        Ok(crate::companion::companion_status().to_string())
                    }
                    "/cli/skills/regenerate" => {
                        // Regenerate SKILL.md files for all agents in this workspace
                        match crate::commands::k2so_agents::k2so_agents_regenerate_skills(project_path.to_string()) {
                            Ok(result) => Ok(result.to_string()),
                            Err(e) => Err(e),
                        }
                    }
                    "/cli/feed" => {
                        // Query the activity feed
                        let limit = params.get("limit").and_then(|s| s.parse::<i64>().ok()).unwrap_or(20);
                        let agent = params.get("agent").cloned();
                        (|| -> Result<String, String> {
                            let db_path = dirs::home_dir()
                                .ok_or("No home dir")?
                                .join(".k2so/k2so.db");
                            let db = crate::db::shared();
                            let conn = db.lock();
                            let _ = conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;");

                            let project_id: String = conn.query_row(
                                "SELECT id FROM projects WHERE path = ?1",
                                rusqlite::params![project_path],
                                |row| row.get(0),
                            ).map_err(|e| format!("Project not found: {}", e))?;

                            let entries = if let Some(agent_name) = agent {
                                crate::db::schema::ActivityFeedEntry::list_by_agent(&conn, &project_id, &agent_name, limit)
                            } else {
                                crate::db::schema::ActivityFeedEntry::list_by_project(&conn, &project_id, limit, 0)
                            }.map_err(|e| e.to_string())?;

                            let items: Vec<serde_json::Value> = entries.iter().map(|e| {
                                serde_json::json!({
                                    "id": e.id,
                                    "agent": e.agent_name,
                                    "type": e.event_type,
                                    "from": e.from_agent,
                                    "to": e.to_agent,
                                    "summary": e.summary,
                                    "at": e.created_at,
                                })
                            }).collect();

                            Ok(serde_json::json!({ "feed": items }).to_string())
                        })()
                    }
                    "/cli/companion/projects" => {
                        // List all registered workspaces (global — ignores project_path)
                        (|| -> Result<String, String> {
                            let db_path = dirs::home_dir()
                                .ok_or("No home dir")?
                                .join(".k2so/k2so.db");
                            let db = crate::db::shared();
                            let conn = db.lock();
                            let _ = conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;");

                            let mut stmt = conn.prepare(
                                "SELECT p.id, p.name, p.path, p.color, p.icon_url, p.agent_mode, p.pinned, \
                                 p.tab_order, p.focus_group_id, fg.name, fg.color \
                                 FROM projects p \
                                 LEFT JOIN focus_groups fg ON p.focus_group_id = fg.id \
                                 ORDER BY p.pinned DESC, p.tab_order ASC, p.name ASC"
                            ).map_err(|e| e.to_string())?;

                            let projects: Vec<serde_json::Value> = stmt.query_map([], |row| {
                                let fg_id: Option<String> = row.get(8)?;
                                let fg_name: Option<String> = row.get(9)?;
                                let fg_color: Option<String> = row.get(10)?;
                                let focus_group = if let (Some(id), Some(name)) = (&fg_id, &fg_name) {
                                    serde_json::json!({ "id": id, "name": name, "color": fg_color })
                                } else {
                                    serde_json::Value::Null
                                };

                                Ok(serde_json::json!({
                                    "id": row.get::<_, String>(0)?,
                                    "name": row.get::<_, String>(1)?,
                                    "path": row.get::<_, String>(2)?,
                                    "color": row.get::<_, String>(3)?,
                                    "iconUrl": row.get::<_, Option<String>>(4)?,
                                    "agentMode": row.get::<_, String>(5)?,
                                    "pinned": row.get::<_, bool>(6)?,
                                    "tabOrder": row.get::<_, i32>(7)?,
                                    "focusGroup": focus_group,
                                }))
                            }).map_err(|e| e.to_string())?
                            .filter_map(|r| r.ok())
                            .collect();

                            Ok(serde_json::to_string(&projects).unwrap_or("[]".to_string()))
                        })()
                    }
                    "/cli/companion/sessions" => {
                        // All active agent sessions across ALL workspaces (global)
                        (|| -> Result<String, String> {
                            let db_path = dirs::home_dir()
                                .ok_or("No home dir")?
                                .join(".k2so/k2so.db");
                            let db = crate::db::shared();
                            let conn = db.lock();
                            let _ = conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;");

                            let mut stmt = conn.prepare(
                                "SELECT id, name, path, color FROM projects ORDER BY name ASC"
                            ).map_err(|e| e.to_string())?;

                            let workspaces: Vec<(String, String, String, String)> = stmt.query_map([], |row| {
                                Ok((
                                    row.get::<_, String>(0)?,
                                    row.get::<_, String>(1)?,
                                    row.get::<_, String>(2)?,
                                    row.get::<_, String>(3)?,
                                ))
                            }).map_err(|e| e.to_string())?
                            .filter_map(|r| r.ok())
                            .collect();

                            let mut sessions = Vec::new();

                            // Check each workspace for running terminals with CLI LLM agents
                            if let Some(state) = app_handle.try_state::<crate::state::AppState>() {
                                let manager = state.terminal_manager.lock();
                                let terminal_ids = manager.list_terminal_ids();

                                for (tid, cwd) in &terminal_ids {
                                    let command = manager.get_foreground_command(tid).ok().flatten();
                                    // Match terminal CWD to a workspace (longest path match wins)
                                    let matched_ws = workspaces.iter()
                                        .filter(|(_, _, path, _)| cwd.starts_with(path.as_str()))
                                        .max_by_key(|(_, _, path, _)| path.len());

                                    if let Some((ws_id, ws_name, ws_path, ws_color)) = matched_ws {
                                        // Determine session label:
                                        // 1. Worktree agent: .k2so/worktrees/<agent-name>/...
                                        // 2. Same as workspace root: use workspace name
                                        // 3. Subfolder: use folder name
                                        let worktree_prefix = format!("{}/.k2so/worktrees/", ws_path);
                                        let (agent_name, label) = if let Some(rest) = cwd.strip_prefix(&worktree_prefix) {
                                            let name = rest.split('/').next().unwrap_or("agent");
                                            (name.to_string(), name.to_string())
                                        } else {
                                            // Use the last path component as the label
                                            let folder = std::path::Path::new(cwd)
                                                .file_name()
                                                .map(|f| f.to_string_lossy().to_string())
                                                .unwrap_or_else(|| ws_name.clone());
                                            ("shell".to_string(), folder)
                                        };

                                        sessions.push(serde_json::json!({
                                            "workspaceName": ws_name,
                                            "workspaceId": ws_id,
                                            "workspaceColor": ws_color,
                                            "agentName": agent_name,
                                            "label": label,
                                            "terminalId": tid,
                                            "command": command,
                                            "cwd": cwd,
                                        }));
                                    }
                                }
                            }

                            Ok(serde_json::to_string(&sessions).unwrap_or("[]".to_string()))
                        })()
                    }
                    "/cli/companion/projects-summary" => {
                        // Projects with running agent + review counts (global)
                        (|| -> Result<String, String> {
                            let db_path = dirs::home_dir()
                                .ok_or("No home dir")?
                                .join(".k2so/k2so.db");
                            let db = crate::db::shared();
                            let conn = db.lock();
                            let _ = conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;");

                            let mut stmt = conn.prepare(
                                "SELECT p.id, p.name, p.path, p.color, p.agent_mode, p.pinned, p.tab_order, \
                                 p.focus_group_id, fg.name, fg.color \
                                 FROM projects p \
                                 LEFT JOIN focus_groups fg ON p.focus_group_id = fg.id \
                                 ORDER BY p.pinned DESC, p.tab_order ASC, p.name ASC"
                            ).map_err(|e| e.to_string())?;

                            struct WsRow {
                                id: String, name: String, path: String, color: String,
                                agent_mode: String, pinned: bool, tab_order: i32,
                                fg_id: Option<String>, fg_name: Option<String>, fg_color: Option<String>,
                            }

                            let workspaces: Vec<WsRow> = stmt.query_map([], |row| {
                                Ok(WsRow {
                                    id: row.get(0)?, name: row.get(1)?, path: row.get(2)?,
                                    color: row.get(3)?, agent_mode: row.get(4)?,
                                    pinned: row.get(5)?, tab_order: row.get(6)?,
                                    fg_id: row.get(7)?, fg_name: row.get(8)?, fg_color: row.get(9)?,
                                })
                            }).map_err(|e| e.to_string())?
                            .filter_map(|r| r.ok())
                            .collect();

                            // Count running terminals per workspace
                            let terminal_counts: HashMap<String, usize> = if let Some(state) = app_handle.try_state::<crate::state::AppState>() {
                                let manager = state.terminal_manager.lock();
                                let mut counts = HashMap::new();
                                for (_, cwd) in manager.list_terminal_ids() {
                                    for ws in &workspaces {
                                        if cwd.starts_with(ws.path.as_str()) {
                                            *counts.entry(ws.id.clone()).or_insert(0) += 1;
                                            break;
                                        }
                                    }
                                }
                                counts
                            } else {
                                HashMap::new()
                            };

                            let mut summaries = Vec::new();
                            for ws in &workspaces {
                                // Count pending reviews (done/ items)
                                let review_count = {
                                    let agents_dir = std::path::Path::new(&ws.path).join(".k2so/agents");
                                    let mut count = 0usize;
                                    if agents_dir.exists() {
                                        if let Ok(entries) = std::fs::read_dir(&agents_dir) {
                                            for entry in entries.flatten() {
                                                let done_dir = entry.path().join("work/done");
                                                if done_dir.exists() {
                                                    if let Ok(files) = std::fs::read_dir(&done_dir) {
                                                        count += files.filter_map(|f| f.ok()).filter(|f| {
                                                            f.path().extension().map_or(false, |ext| ext == "md")
                                                        }).count();
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    count
                                };

                                let focus_group = if let (Some(ref id), Some(ref name)) = (&ws.fg_id, &ws.fg_name) {
                                    serde_json::json!({ "id": id, "name": name, "color": ws.fg_color })
                                } else {
                                    serde_json::Value::Null
                                };

                                summaries.push(serde_json::json!({
                                    "id": ws.id,
                                    "name": ws.name,
                                    "path": ws.path,
                                    "color": ws.color,
                                    "agentMode": ws.agent_mode,
                                    "pinned": ws.pinned,
                                    "tabOrder": ws.tab_order,
                                    "focusGroup": focus_group,
                                    "agentsRunning": terminal_counts.get(&ws.id).copied().unwrap_or(0),
                                    "reviewsPending": review_count,
                                }));
                            }

                            Ok(serde_json::to_string(&summaries).unwrap_or("[]".to_string()))
                        })()
                    }
                    "/cli/companion/presets" => {
                        // List available CLI LLM tool presets (global)
                        (|| -> Result<String, String> {
                            let db_path = dirs::home_dir()
                                .ok_or("No home dir")?
                                .join(".k2so/k2so.db");
                            let db = crate::db::shared();
                            let conn = db.lock();
                            let _ = conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;");

                            let mut stmt = conn.prepare(
                                "SELECT id, label, command, icon FROM agent_presets WHERE enabled = 1 ORDER BY sort_order ASC, label ASC"
                            ).map_err(|e| e.to_string())?;

                            let presets: Vec<serde_json::Value> = stmt.query_map([], |row| {
                                Ok(serde_json::json!({
                                    "id": row.get::<_, String>(0)?,
                                    "name": row.get::<_, String>(1)?,
                                    "command": row.get::<_, String>(2)?,
                                    "icon": row.get::<_, Option<String>>(3)?,
                                }))
                            }).map_err(|e| e.to_string())?
                            .filter_map(|r| r.ok())
                            .collect();

                            Ok(serde_json::to_string(&presets).unwrap_or("[]".to_string()))
                        })()
                    }
                    _ => Err("Unknown CLI endpoint".to_string()),
                };

                let (status_code, body) = match result {
                    Ok(json) => ("200 OK", json),
                    Err(e) => ("400 Bad Request", serde_json::json!({"error": e}).to_string()),
                };
                let response = format!(
                    "HTTP/1.1 {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\n\r\n{}",
                    status_code,
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
            } else if path == "/health" {
                let body = r#"{"status":"ok"}"#;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
            } else {
                let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
            }
        }
    });

    Ok(port)
}

// ── CLI DB Helpers ──────────────────────────────────────────────────────
// These open a direct SQLite connection so CLI endpoints can read/write
// project settings without needing the Tauri AppState.

/// Update a single project setting in the DB by matching on project path.
fn cli_update_project_setting(project_path: &str, field: &str, value: &str) -> Result<(), String> {
    let db_path = dirs::home_dir()
        .ok_or("No home dir")?
        .join(".k2so")
        .join("k2so.db");
    // Shared process-wide connection — WAL + busy_timeout are already
    // enabled by db::init_database, so no per-call PRAGMA setup needed.
    let db = crate::db::shared();
    let conn = db.lock();

    // Validate field name to prevent SQL injection
    let allowed = ["agent_mode", "worktree_mode", "heartbeat_enabled", "agent_enabled", "pinned", "tier_id"];
    if !allowed.contains(&field) {
        return Err(format!("Unknown setting: {}", field));
    }

    let sql = format!("UPDATE projects SET {} = ?1 WHERE path = ?2", field);
    let rows = conn.execute(&sql, rusqlite::params![value, project_path])
        .map_err(|e| format!("DB update failed: {}", e))?;

    if rows == 0 {
        return Err(format!("Project not found in DB: {}", project_path));
    }

    // Keep agent_enabled in sync when agent_mode changes
    if field == "agent_mode" {
        let enabled = if value == "off" { "0" } else { "1" };
        let _ = conn.execute(
            "UPDATE projects SET agent_enabled = ?1 WHERE path = ?2",
            rusqlite::params![enabled, project_path],
        );
    }

    Ok(())
}

/// Read current project settings from the DB.
/// Register a directory as a new K2SO workspace (project + default workspace).
fn cli_register_workspace(path: &str, app_handle: &tauri::AppHandle) -> Result<String, String> {
    let db_path = dirs::home_dir()
        .ok_or("No home dir")?
        .join(".k2so")
        .join("k2so.db");
    let db = crate::db::shared();
    let conn = db.lock();

    // Check if already registered
    let exists: bool = conn.query_row(
        "SELECT COUNT(*) > 0 FROM projects WHERE path = ?1",
        rusqlite::params![path],
        |row| row.get(0),
    ).unwrap_or(false);
    if exists {
        return Err(format!("Workspace already registered: {}", path));
    }

    let name = std::path::Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "workspace".to_string());

    let project_id = uuid::Uuid::new_v4().to_string();
    let workspace_id = uuid::Uuid::new_v4().to_string();

    // Detect git branch
    let branch = std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(path)
        .output()
        .ok()
        .and_then(|o| if o.status.success() { Some(String::from_utf8_lossy(&o.stdout).trim().to_string()) } else { None })
        .unwrap_or_else(|| "main".to_string());

    let tab_order: i64 = conn.query_row(
        "SELECT COALESCE(MAX(tab_order), -1) + 1 FROM projects",
        [],
        |row| row.get(0),
    ).unwrap_or(0);

    conn.execute_batch("BEGIN").map_err(|e| e.to_string())?;

    let insert_result = (|| -> Result<(), String> {
        conn.execute(
            "INSERT INTO projects (id, name, path, color, tab_order, worktree_mode, icon_url, focus_group_id) \
             VALUES (?1, ?2, ?3, '#3b82f6', ?4, 0, NULL, NULL)",
            rusqlite::params![project_id, name, path, tab_order],
        ).map_err(|e| format!("Failed to create project: {}", e))?;

        conn.execute(
            "INSERT INTO workspaces (id, project_id, section_id, type, branch, name, tab_order, worktree_path) \
             VALUES (?1, ?2, NULL, 'branch', ?3, ?3, 0, NULL)",
            rusqlite::params![workspace_id, project_id, branch],
        ).map_err(|e| format!("Failed to create workspace: {}", e))?;
        Ok(())
    })();

    match insert_result {
        Ok(_) => {
            let _ = conn.execute_batch("COMMIT");
            let _ = app_handle.emit("sync:projects", ());
            Ok(serde_json::json!({
                "success": true,
                "projectId": project_id,
                "workspaceId": workspace_id,
                "name": name,
                "path": path,
            }).to_string())
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

/// Remove a workspace from K2SO's DB (deregister). Does NOT delete files on disk.
fn cli_remove_workspace(
    path: &str,
    mode: Option<&str>,
    app_handle: &tauri::AppHandle,
) -> Result<String, String> {
    let db_path = dirs::home_dir()
        .ok_or("No home dir")?
        .join(".k2so")
        .join("k2so.db");
    let db = crate::db::shared();
    let conn = db.lock();

    // Find the project ID
    let project_id: String = conn.query_row(
        "SELECT id FROM projects WHERE path = ?1",
        rusqlite::params![path],
        |row| row.get(0),
    ).map_err(|_| format!("Workspace not found: {}", path))?;

    // Phase 7e: if the caller specified a teardown mode, resolve the
    // workspace-root symlinks first so downstream CLIs keep working or
    // the workspace fully reverts. `.k2so/` is always preserved.
    let teardown = match mode {
        Some("keep_current") | Some("keep-current") => Some(
            crate::commands::k2so_agents::teardown_workspace_harness_files(
                path,
                crate::commands::k2so_agents::TeardownMode::KeepCurrent,
            ),
        ),
        Some("restore_original") | Some("restore-original") => Some(
            crate::commands::k2so_agents::teardown_workspace_harness_files(
                path,
                crate::commands::k2so_agents::TeardownMode::RestoreOriginal,
            ),
        ),
        Some(other) => return Err(format!(
            "Unknown teardown mode '{}'. Expected 'keep_current' or 'restore_original'.",
            other
        )),
        None => None,
    };

    // Delete workspaces first (foreign key)
    conn.execute("DELETE FROM workspaces WHERE project_id = ?1", rusqlite::params![project_id])
        .map_err(|e| format!("Failed to delete workspaces: {}", e))?;
    conn.execute("DELETE FROM projects WHERE id = ?1", rusqlite::params![project_id])
        .map_err(|e| format!("Failed to delete project: {}", e))?;

    let _ = app_handle.emit("sync:projects", ());
    Ok(serde_json::json!({
        "success": true,
        "removed": path,
        "teardown": teardown,
    }).to_string())
}

fn cli_cleanup_stale_workspaces(app_handle: &tauri::AppHandle) -> Result<String, String> {
    let db_path = dirs::home_dir()
        .ok_or("No home dir")?
        .join(".k2so").join("k2so.db");
    let db = crate::db::shared();
    let conn = db.lock();

    let mut stmt = conn.prepare(
        "SELECT id, worktree_path FROM workspaces WHERE worktree_path IS NOT NULL AND worktree_path != ''"
    ).map_err(|e| e.to_string())?;
    let stale: Vec<(String, String)> = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    }).map_err(|e| e.to_string())?
    .filter_map(|r| r.ok())
    .filter(|(_, path)| !std::path::Path::new(path).exists())
    .collect();

    let removed = stale.len();
    for (id, _) in &stale {
        let _ = conn.execute("DELETE FROM workspaces WHERE id = ?1", rusqlite::params![id]);
    }
    let _ = app_handle.emit("sync:projects", ());
    Ok(serde_json::json!({
        "removed": removed,
        "stale": stale.iter().map(|(_, p)| p.clone()).collect::<Vec<_>>()
    }).to_string())
}

fn cli_get_project_settings(project_path: &str) -> Result<serde_json::Value, String> {
    let db_path = dirs::home_dir()
        .ok_or("No home dir")?
        .join(".k2so")
        .join("k2so.db");
    // Read-only path — the shared connection is writable, so we can't
    // use PRAGMA query_only=ON without affecting concurrent writers.
    // The query below is a single SELECT which is intrinsically safe.
    let db = crate::db::shared();
    let conn = db.lock();

    conn.query_row(
        "SELECT agent_mode, worktree_mode, heartbeat_enabled, agent_enabled, pinned, name, tier_id FROM projects WHERE path = ?1",
        rusqlite::params![project_path],
        |row| {
            Ok(serde_json::json!({
                "mode": row.get::<_, String>(0).unwrap_or_else(|_| "off".to_string()),
                "worktreeMode": row.get::<_, i64>(1).unwrap_or(0) == 1,
                "heartbeatEnabled": row.get::<_, i64>(2).unwrap_or(0) == 1,
                "agentEnabled": row.get::<_, i64>(3).unwrap_or(0) == 1,
                "pinned": row.get::<_, i64>(4).unwrap_or(0) == 1,
                "name": row.get::<_, String>(5).unwrap_or_default(),
                "stateId": row.get::<_, Option<String>>(6).unwrap_or(None),
            }))
        },
    ).map_err(|e| format!("Project not found: {}", e))
}

/// Generate the notify hook bash script content.
///
/// The script reads the port and (optionally) the auth token from
/// `~/.k2so/heartbeat.port` / `~/.k2so/heartbeat.token` at exec time
/// rather than baking them in. This means a K2SO restart (which picks a
/// new random port + may rotate the token) doesn't silently break
/// hooks in already-running Claude/Cursor/Gemini sessions.
///
/// The `_port` parameter is kept for API compatibility but no longer
/// used — the caller can regenerate the script whenever they want.
pub fn generate_hook_script(_port: u16) -> String {
    r#"#!/bin/bash
# K2SO Agent Lifecycle Hook — DO NOT EDIT (managed by K2SO)
# This script is called by agent CLIs to notify K2SO of lifecycle events.

# Port and token are read at exec time so a K2SO restart (new random
# port / rotated token) doesn't break hooks in long-running LLM sessions.
K2SO_PORT_FILE="$HOME/.k2so/heartbeat.port"
K2SO_TOKEN_FILE="$HOME/.k2so/heartbeat.token"

if [ ! -r "$K2SO_PORT_FILE" ]; then
    # K2SO isn't running — exit silently so we don't block the agent
    exit 0
fi
K2SO_PORT=$(cat "$K2SO_PORT_FILE" 2>/dev/null)
[ -z "$K2SO_PORT" ] && exit 0

# Token from disk takes precedence over the PTY-injected env var so that
# a K2SO restart (which rotates the token) immediately picks up the new
# value instead of sending 403-rejected requests for the rest of the
# session's life.
if [ -r "$K2SO_TOKEN_FILE" ]; then
    K2SO_HOOK_TOKEN=$(cat "$K2SO_TOKEN_FILE" 2>/dev/null)
fi

# Read JSON from argument or stdin
INPUT="${1:-}"
if [ -z "$INPUT" ]; then
    INPUT=$(cat 2>/dev/null || true)
fi

# Extract event type — try multiple JSON key patterns
EVENT_TYPE=""
for key in hook_event_name type event eventType; do
    val=$(echo "$INPUT" | grep -o "\"$key\"[[:space:]]*:[[:space:]]*\"[^\"]*\"" | head -1 | sed 's/.*"'"$key"'"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/')
    if [ -n "$val" ]; then
        EVENT_TYPE="$val"
        break
    fi
done

# If event type was passed as first arg (Cursor style: script.sh Start)
if [ -z "$EVENT_TYPE" ] && [ -n "$1" ] && ! echo "$1" | grep -q '{'; then
    EVENT_TYPE="$1"
fi

[ -z "$EVENT_TYPE" ] && exit 0
[ -z "$K2SO_TAB_ID" ] && exit 0

curl -sG "http://127.0.0.1:$K2SO_PORT/hook/complete" \
    --connect-timeout 1 --max-time 2 \
    --data-urlencode "paneId=$K2SO_PANE_ID" \
    --data-urlencode "tabId=$K2SO_TAB_ID" \
    --data-urlencode "eventType=$EVENT_TYPE" \
    --data-urlencode "token=$K2SO_HOOK_TOKEN" \
    >/dev/null 2>&1 || true

exit 0
"#.to_string()
}

/// Write the hook script to ~/.k2so/hooks/notify.sh
pub fn write_hook_script(port: u16) -> Result<String, String> {
    let home = dirs::home_dir().ok_or("No home directory")?;
    let hooks_dir = home.join(".k2so").join("hooks");
    std::fs::create_dir_all(&hooks_dir).map_err(|e| e.to_string())?;

    let script_path = hooks_dir.join("notify.sh");
    let content = generate_hook_script(port);
    std::fs::write(&script_path, &content).map_err(|e| e.to_string())?;

    // Make executable
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(&script_path, perms).map_err(|e| e.to_string())?;
    }

    Ok(script_path.to_string_lossy().to_string())
}

/// Register hooks with Claude Code (~/.claude/settings.json)
fn register_claude_hooks(hook_script: &str) -> Result<(), String> {
    let home = dirs::home_dir().ok_or("No home directory")?;
    let settings_path = home.join(".claude").join("settings.json");

    // Read existing settings or create new
    let mut settings: serde_json::Value = if settings_path.exists() {
        let raw = std::fs::read_to_string(&settings_path).map_err(|e| e.to_string())?;
        serde_json::from_str(&raw).unwrap_or(serde_json::json!({}))
    } else {
        std::fs::create_dir_all(settings_path.parent().unwrap()).map_err(|e| e.to_string())?;
        serde_json::json!({})
    };

    let escaped = shell_escape(hook_script);
    let hook_cmd = format!("[ -x {} ] && {} \"$@\" || true", escaped, escaped);

    let hook_entry = serde_json::json!([{
        "hooks": [{
            "type": "command",
            "command": hook_cmd
        }]
    }]);

    // Events we want to hook into
    let events = ["UserPromptSubmit", "Stop", "PostToolUse", "PostToolUseFailure", "PermissionRequest"];

    let hooks = settings.as_object_mut()
        .ok_or("Invalid settings format")?
        .entry("hooks")
        .or_insert(serde_json::json!({}));

    let hooks_obj = hooks.as_object_mut().ok_or("Invalid hooks format")?;

    for event in &events {
        // Check if we already have a K2SO hook registered
        let existing = hooks_obj.get(*event);
        let already_registered = existing.map_or(false, |v| {
            v.to_string().contains(".k2so/hooks/notify.sh")
        });

        if !already_registered {
            // Merge: keep existing hooks, append ours
            if let Some(existing_arr) = existing.and_then(|v| v.as_array()) {
                let mut merged = existing_arr.clone();
                merged.push(hook_entry[0].clone());
                hooks_obj.insert(event.to_string(), serde_json::Value::Array(merged));
            } else {
                hooks_obj.insert(event.to_string(), hook_entry.clone());
            }
        }
    }

    let json = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
    std::fs::write(&settings_path, json).map_err(|e| e.to_string())?;

    log_debug!("[agent-hooks] Registered Claude Code hooks");
    Ok(())
}

/// Register hooks with Cursor (~/.cursor/hooks.json)
fn register_cursor_hooks(hook_script: &str) -> Result<(), String> {
    let home = dirs::home_dir().ok_or("No home directory")?;
    let hooks_path = home.join(".cursor").join("hooks.json");

    let mut settings: serde_json::Value = if hooks_path.exists() {
        let raw = std::fs::read_to_string(&hooks_path).map_err(|e| e.to_string())?;
        serde_json::from_str(&raw).unwrap_or(serde_json::json!({}))
    } else {
        std::fs::create_dir_all(hooks_path.parent().unwrap()).map_err(|e| e.to_string())?;
        serde_json::json!({})
    };

    let events = [
        ("beforeSubmitPrompt", "Start"),
        ("stop", "Stop"),
        ("beforeShellExecution", "PermissionRequest"),
        ("beforeMCPExecution", "PermissionRequest"),
    ];

    let hooks_obj = settings.as_object_mut().ok_or("Invalid hooks format")?;

    for (event, mapped_type) in &events {
        let escaped = shell_escape(hook_script);
        let hook_cmd = format!("[ -x {} ] && {} {} || true", escaped, escaped, mapped_type);

        let already_registered = hooks_obj.get(*event).map_or(false, |v| {
            v.to_string().contains(".k2so/hooks/notify.sh")
        });

        if !already_registered {
            let hook_entry = serde_json::json!({
                "command": hook_cmd
            });

            if let Some(existing_arr) = hooks_obj.get(*event).and_then(|v| v.as_array()) {
                let mut merged = existing_arr.clone();
                merged.push(hook_entry);
                hooks_obj.insert(event.to_string(), serde_json::Value::Array(merged));
            } else {
                hooks_obj.insert(event.to_string(), serde_json::json!([hook_entry]));
            }
        }
    }

    let json = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
    std::fs::write(&hooks_path, json).map_err(|e| e.to_string())?;

    log_debug!("[agent-hooks] Registered Cursor hooks");
    Ok(())
}

/// Register hooks with Gemini CLI (~/.gemini/settings.json)
fn register_gemini_hooks(hook_script: &str) -> Result<(), String> {
    let home = dirs::home_dir().ok_or("No home directory")?;
    let settings_path = home.join(".gemini").join("settings.json");

    let mut settings: serde_json::Value = if settings_path.exists() {
        let raw = std::fs::read_to_string(&settings_path).map_err(|e| e.to_string())?;
        serde_json::from_str(&raw).unwrap_or(serde_json::json!({}))
    } else {
        std::fs::create_dir_all(settings_path.parent().unwrap()).map_err(|e| e.to_string())?;
        serde_json::json!({})
    };

    let escaped = shell_escape(hook_script);
    let hook_cmd = format!("[ -x {} ] && {} \"$@\" || true", escaped, escaped);

    let hook_entry = serde_json::json!([{
        "hooks": [{
            "type": "command",
            "command": hook_cmd
        }]
    }]);

    let events = ["BeforeAgent", "AfterAgent", "AfterTool"];

    let hooks = settings.as_object_mut()
        .ok_or("Invalid settings format")?
        .entry("hooks")
        .or_insert(serde_json::json!({}));

    let hooks_obj = hooks.as_object_mut().ok_or("Invalid hooks format")?;

    for event in &events {
        let already_registered = hooks_obj.get(*event).map_or(false, |v| {
            v.to_string().contains(".k2so/hooks/notify.sh")
        });

        if !already_registered {
            if let Some(existing_arr) = hooks_obj.get(*event).and_then(|v| v.as_array()) {
                let mut merged = existing_arr.clone();
                merged.push(hook_entry[0].clone());
                hooks_obj.insert(event.to_string(), serde_json::Value::Array(merged));
            } else {
                hooks_obj.insert(event.to_string(), hook_entry.clone());
            }
        }
    }

    let json = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
    std::fs::write(&settings_path, json).map_err(|e| e.to_string())?;

    log_debug!("[agent-hooks] Registered Gemini CLI hooks");
    Ok(())
}

/// Check each supported CLI's config file for our notify.sh entries.
/// Used by `/cli/hooks/status` to let users verify injection end-to-end.
///
/// An entry is "injected" when the config file exists and contains at least
/// one command pointing at our notify.sh script. We don't validate the full
/// hook list — partial injection is still reported as injected=true so users
/// see events flow in `recent_events` while also noticing a mismatch.
pub fn check_hook_injections() -> serde_json::Value {
    let home = dirs::home_dir();
    let notify_fragment = ".k2so/hooks/notify.sh";

    let check = |relative: &str| -> serde_json::Value {
        let path = match &home {
            Some(h) => h.join(relative),
            None => return serde_json::json!({ "path": null, "exists": false, "injected": false }),
        };
        let path_str = path.to_string_lossy().to_string();
        if !path.exists() {
            return serde_json::json!({
                "path": path_str,
                "exists": false,
                "injected": false,
            });
        }
        let content = std::fs::read_to_string(&path).unwrap_or_default();
        let injected = content.contains(notify_fragment);
        serde_json::json!({
            "path": path_str,
            "exists": true,
            "injected": injected,
        })
    };

    let script_path = home
        .as_ref()
        .map(|h| h.join(notify_fragment))
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();

    serde_json::json!({
        "notify_script": {
            "path": script_path,
            "exists": home.as_ref().map(|h| h.join(notify_fragment).exists()).unwrap_or(false),
        },
        "claude": check(".claude/settings.json"),
        "cursor": check(".cursor/hooks.json"),
        "gemini": check(".config/gemini/hooks.json"),
    })
}

/// Register hooks with all supported agents. Called on app startup.
///
/// Per-CLI failures are collected and emitted as a `hook-injection-failed`
/// Tauri event so the frontend can surface a toast — previously these
/// failures only hit debug logs and users never knew their spinner was
/// broken because of a malformed `~/.claude/settings.json` or similar.
pub fn register_all_hooks(app_handle: &AppHandle, hook_script: &str) {
    let mut failures: Vec<serde_json::Value> = Vec::new();

    if let Err(e) = register_claude_hooks(hook_script) {
        log_debug!("[agent-hooks] Failed to register Claude hooks: {}", e);
        failures.push(serde_json::json!({ "cli": "claude", "error": e }));
    }
    if let Err(e) = register_cursor_hooks(hook_script) {
        log_debug!("[agent-hooks] Failed to register Cursor hooks: {}", e);
        failures.push(serde_json::json!({ "cli": "cursor", "error": e }));
    }
    if let Err(e) = register_gemini_hooks(hook_script) {
        log_debug!("[agent-hooks] Failed to register Gemini hooks: {}", e);
        failures.push(serde_json::json!({ "cli": "gemini", "error": e }));
    }

    if !failures.is_empty() {
        let _ = app_handle.emit(
            "hook-injection-failed",
            serde_json::json!({ "failures": failures }),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ring-buffer tests share global state — serialize them. Uses the
    /// super's `parking_lot::Mutex` (import inherited via `use super::*`).
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn reset_recent_events() {
        let mut buf = recent_events().lock();
        buf.clear();
    }

    fn snapshot_recent_events() -> Vec<RecentEvent> {
        let buf = recent_events().lock();
        buf.iter().cloned().collect()
    }

    #[test]
    fn ring_buffer_records_matched_events() {
        let _g = TEST_LOCK.lock();
        reset_recent_events();
        record_recent_event("UserPromptSubmit", Some("start"), "pane-1", "tab-1");
        let events = snapshot_recent_events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].raw_event, "UserPromptSubmit");
        assert_eq!(events[0].canonical.as_deref(), Some("start"));
        assert_eq!(events[0].pane_id, "pane-1");
        assert_eq!(events[0].tab_id, "tab-1");
        assert!(events[0].matched);
    }

    #[test]
    fn ring_buffer_records_unmatched_events() {
        let _g = TEST_LOCK.lock();
        reset_recent_events();
        record_recent_event("NoSuchEvent", None, "pane-2", "tab-2");
        let events = snapshot_recent_events();
        assert_eq!(events.len(), 1);
        assert!(!events[0].matched);
        assert!(events[0].canonical.is_none());
    }

    #[test]
    fn ring_buffer_caps_at_limit() {
        let _g = TEST_LOCK.lock();
        reset_recent_events();
        for i in 0..RECENT_EVENTS_CAP + 10 {
            record_recent_event(
                &format!("Event{}", i),
                Some("start"),
                "pane-cap",
                "tab-cap",
            );
        }
        let events = snapshot_recent_events();
        assert_eq!(events.len(), RECENT_EVENTS_CAP);
        // Oldest should have been dropped — first recorded was Event0
        assert_eq!(events[0].raw_event, format!("Event{}", 10));
        assert_eq!(
            events.last().unwrap().raw_event,
            format!("Event{}", RECENT_EVENTS_CAP + 9)
        );
    }

    #[test]
    fn map_event_type_covers_primary_lifecycle() {
        // Claude Code
        assert_eq!(map_event_type("UserPromptSubmit"), Some("start"));
        assert_eq!(map_event_type("PostToolUse"), Some("start"));
        assert_eq!(map_event_type("Stop"), Some("stop"));
        assert_eq!(map_event_type("Notification"), Some("permission"));
        // Codex
        assert_eq!(map_event_type("agent-turn-complete"), Some("stop"));
        // Cursor
        assert_eq!(map_event_type("beforeSubmitPrompt"), Some("start"));
        assert_eq!(map_event_type("beforeShellExecution"), Some("permission"));
        // Unknown events must return None
        assert_eq!(map_event_type("NoSuchEvent"), None);
    }

    #[test]
    fn hook_script_reads_port_and_token_dynamically() {
        // The script must read port + token from disk at exec time so a
        // K2SO restart (new random port / rotated token) doesn't silently
        // break hooks in long-running LLM sessions.
        let script = generate_hook_script(12345);
        assert!(
            !script.contains("K2SO_PORT=\"12345\""),
            "port must not be baked in as a literal"
        );
        assert!(
            script.contains("K2SO_PORT_FILE=\"$HOME/.k2so/heartbeat.port\""),
            "script must read port from heartbeat.port file"
        );
        assert!(
            script.contains("K2SO_TOKEN_FILE=\"$HOME/.k2so/heartbeat.token\""),
            "script must prefer token from heartbeat.token file"
        );
        assert!(
            script.contains("K2SO_PORT=$(cat \"$K2SO_PORT_FILE\""),
            "script must cat the port file at exec time"
        );
    }

    #[test]
    fn check_hook_injections_returns_expected_shape() {
        let val = check_hook_injections();
        // Top-level keys present
        assert!(val.get("notify_script").is_some());
        assert!(val.get("claude").is_some());
        assert!(val.get("cursor").is_some());
        assert!(val.get("gemini").is_some());
        // Each CLI entry has path/exists/injected booleans
        for key in &["claude", "cursor", "gemini"] {
            let entry = val.get(*key).expect("cli entry");
            assert!(entry.get("path").is_some(), "{} missing path", key);
            assert!(entry.get("exists").is_some(), "{} missing exists", key);
            assert!(entry.get("injected").is_some(), "{} missing injected", key);
            assert!(
                entry.get("exists").unwrap().is_boolean(),
                "{}.exists must be bool",
                key
            );
            assert!(
                entry.get("injected").unwrap().is_boolean(),
                "{}.injected must be bool",
                key
            );
        }
    }
}

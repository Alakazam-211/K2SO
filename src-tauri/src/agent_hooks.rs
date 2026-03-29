//! Agent lifecycle hook notification server.
//!
//! Listens on a random localhost port for HTTP GET requests from agent CLI hooks
//! (Claude Code, Cursor, Gemini). Maps agent events to canonical lifecycle types
//! and emits Tauri events to the frontend.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Mutex, OnceLock};
use tauri::{AppHandle, Emitter};

static HOOK_PORT: AtomicU16 = AtomicU16::new(0);
static HOOK_TOKEN: OnceLock<String> = OnceLock::new();
/// Guard against concurrent triage runs for the same project path.
static TRIAGE_IN_FLIGHT: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

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

/// Push an event into an agent's channel event queue.
pub fn push_agent_event(project_path: &str, agent_name: &str, event_type: &str, message: &str, priority: &str) {
    let key = format!("{}:{}", project_path, agent_name);
    let event = ChannelEvent {
        event_type: event_type.to_string(),
        message: message.to_string(),
        priority: priority.to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
    };
    let mut queues = event_queues().lock().unwrap_or_else(|e| e.into_inner());
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
    let mut queues = event_queues().lock().unwrap_or_else(|e| e.into_inner());
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
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '%' {
            let hex: String = chars.by_ref().take(2).collect();
            // Validate we got exactly 2 hex chars before parsing
            if hex.len() == 2 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
                if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                    result.push(byte as char);
                } else {
                    // Invalid hex — preserve the original characters
                    result.push('%');
                    result.push_str(&hex);
                }
            } else {
                // Malformed percent encoding (e.g., %Z or lone %) — preserve as-is
                result.push('%');
                result.push_str(&hex);
            }
        } else if c == '+' {
            result.push(' ');
        } else {
            result.push(c);
        }
    }
    result
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

/// Start the notification server on a random port. Returns the port.
pub fn start_server(app_handle: AppHandle) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("Failed to bind notification server");
    let port = listener.local_addr().unwrap().port();
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

                if let Some(canonical) = map_event_type(&raw_event) {
                    let event = AgentLifecycleEvent {
                        pane_id: pane_id.clone(),
                        tab_id: tab_id.clone(),
                        event_type: canonical.to_string(),
                    };

                    log_debug!("[agent-hooks] {} → {} (pane={}, tab={})", raw_event, canonical, pane_id, tab_id);
                    let _ = app_handle.emit("agent:lifecycle", &event);
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
                        match crate::commands::k2so_agents::k2so_agents_delegate(project_path, target, file) {
                            Ok(launch_info) => {
                                // Emit launch event so the frontend opens a terminal
                                let _ = app_handle.emit("cli:agent-launch", launch_info.clone());
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
                    "/cli/agents/lock" => {
                        let agent = params.get("agent").cloned().unwrap_or_default();
                        crate::commands::k2so_agents::k2so_agents_lock(project_path, agent)
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
                        match crate::commands::k2so_agents::k2so_agents_build_launch(
                            project_path, agent, cli_command,
                        ) {
                            Ok(launch_info) => {
                                let _ = app_handle.emit("cli:agent-launch", &launch_info);
                                Ok(serde_json::json!({
                                    "success": true,
                                    "note": "Agent session will be launched by K2SO"
                                }).to_string())
                            }
                            Err(e) => Err(e),
                        }
                    }
                    "/cli/reviews" => {
                        crate::commands::k2so_agents::k2so_agents_review_queue(project_path)
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
                                    let mode = if !claude_md.exists() { "off" } else if has_agents { "pod" } else { "agent" };
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
                                let mut in_flight = triage_lock().lock().unwrap_or_else(|e| e.into_inner());
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
                                                // Wake the lead agent — generate workspace CLAUDE.md and launch in project root
                                                let _ = crate::commands::k2so_agents::k2so_agents_generate_workspace_claude_md(project_path.clone());
                                                let _ = app_handle.emit("cli:agent-launch", serde_json::json!({
                                                    "command": "claude",
                                                    "args": ["--append-system-prompt", "Check the workspace inbox: `k2so work inbox` and triage any new items to the appropriate sub-agents using `k2so delegate`."],
                                                    "cwd": &project_path,
                                                    "agentName": "__lead__",
                                                }));
                                            } else {
                                                // Build and emit launch for sub-agent
                                                if let Ok(launch) = crate::commands::k2so_agents::k2so_agents_build_launch(
                                                    project_path.clone(), agent_name.clone(), None
                                                ) {
                                                    let _ = app_handle.emit("cli:agent-launch", &launch);
                                                }
                                            }
                                        }
                                        serde_json::json!({"count": agents.len(), "launched": agents}).to_string()
                                    });

                                // Release the triage lock
                                {
                                    let mut in_flight = triage_lock().lock().unwrap_or_else(|e| e.into_inner());
                                    in_flight.remove(&project_path);
                                }

                                triage_result
                            }
                        }
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
                    "/cli/scheduler-tick" => {
                        // Enhanced triage: LLM-powered decision with filesystem fallback.
                        // Runs in a background thread to avoid blocking the HTTP server
                        // (LLM inference can take 2-30 seconds).
                        let already_running = {
                            let mut in_flight = triage_lock().lock().unwrap_or_else(|e| e.into_inner());
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
                            // Spawn triage in background thread — return immediately to unblock HTTP server
                            let bg_project_path = project_path.clone();
                            let bg_app_handle = app_handle.clone();
                            std::thread::spawn(move || {
                                let agents_result = {
                                    use tauri::Manager;
                                    let llm_result = bg_app_handle.try_state::<crate::state::AppState>()
                                        .and_then(|state| {
                                            let manager = state.llm_manager.lock();
                                            if manager.is_loaded() {
                                                Some(crate::commands::k2so_agents::llm_triage_decide(
                                                    &bg_project_path,
                                                    &manager,
                                                ))
                                            } else {
                                                None
                                            }
                                        });

                                    match llm_result {
                                        Some(result) => result,
                                        None => {
                                            crate::commands::k2so_agents::k2so_agents_scheduler_tick(bg_project_path.clone())
                                        }
                                    }
                                };

                                if let Ok(agents) = agents_result {
                                    for agent_name in &agents {
                                        if agent_name == "__lead__" {
                                            let _ = crate::commands::k2so_agents::k2so_agents_generate_workspace_claude_md(bg_project_path.clone());
                                            let _ = bg_app_handle.emit("cli:agent-launch", serde_json::json!({
                                                "command": "claude",
                                                "args": ["--append-system-prompt", "Check the workspace inbox: `k2so work inbox` and triage any new items to the appropriate sub-agents using `k2so delegate`."],
                                                "cwd": &bg_project_path,
                                                "agentName": "__lead__",
                                            }));
                                        } else {
                                            if let Ok(launch) = crate::commands::k2so_agents::k2so_agents_build_launch(
                                                bg_project_path.clone(), agent_name.clone(), None
                                            ) {
                                                let _ = bg_app_handle.emit("cli:agent-launch", &launch);
                                            }
                                        }
                                    }
                                }

                                // Release triage lock
                                let mut in_flight = triage_lock().lock().unwrap_or_else(|e| e.into_inner());
                                in_flight.remove(&bg_project_path);
                            });

                            // Return immediately — triage runs in background
                            Ok(serde_json::json!({"status": "triage_started"}).to_string())
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

    port
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
    let conn = rusqlite::Connection::open(&db_path)
        .map_err(|e| format!("Failed to open DB: {}", e))?;
    // Safety pragmas for CLI DB connections (Zed pattern: WAL + busy_timeout + query safety)
    let _ = conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;");

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
fn cli_get_project_settings(project_path: &str) -> Result<serde_json::Value, String> {
    let db_path = dirs::home_dir()
        .ok_or("No home dir")?
        .join(".k2so")
        .join("k2so.db");
    let conn = rusqlite::Connection::open(&db_path)
        .map_err(|e| format!("Failed to open DB: {}", e))?;
    // Safety pragmas: WAL for concurrent reads, busy_timeout for lock contention, query_only for safety
    let _ = conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000; PRAGMA query_only=ON;");

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
pub fn generate_hook_script(port: u16) -> String {
    format!(r#"#!/bin/bash
# K2SO Agent Lifecycle Hook — DO NOT EDIT (managed by K2SO)
# This script is called by agent CLIs to notify K2SO of lifecycle events.

K2SO_PORT="{port}"

# Read JSON from argument or stdin
INPUT="${{1:-}}"
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
if [ -z "$EVENT_TYPE" ] && [ -n "$1" ] && ! echo "$1" | grep -q '{{'; then
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
"#)
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

/// Register hooks with all supported agents. Called on app startup.
pub fn register_all_hooks(hook_script: &str) {
    if let Err(e) = register_claude_hooks(hook_script) {
        log_debug!("[agent-hooks] Failed to register Claude hooks: {}", e);
    }
    if let Err(e) = register_cursor_hooks(hook_script) {
        log_debug!("[agent-hooks] Failed to register Cursor hooks: {}", e);
    }
    if let Err(e) = register_gemini_hooks(hook_script) {
        log_debug!("[agent-hooks] Failed to register Gemini hooks: {}", e);
    }
}

//! Agent lifecycle hook notification server.
//!
//! Listens on a random localhost port for HTTP GET requests from agent CLI hooks
//! (Claude Code, Cursor, Gemini). Maps agent events to canonical lifecycle types
//! and emits Tauri events to the frontend.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::OnceLock;
use tauri::{AppHandle, Emitter};

static HOOK_PORT: AtomicU16 = AtomicU16::new(0);
static HOOK_TOKEN: OnceLock<String> = OnceLock::new();

/// Get the port the notification server is listening on.
pub fn get_port() -> u16 {
    HOOK_PORT.load(Ordering::Relaxed)
}

/// Get the auth token for hook requests.
pub fn get_token() -> &'static str {
    HOOK_TOKEN.get().map(|s| s.as_str()).unwrap_or("")
}

/// Generate a random hex token.
fn generate_token() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    // Mix with process ID for uniqueness across instances
    let mixed = seed ^ (std::process::id() as u128) << 32;
    format!("{:032x}", mixed)
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
            if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                result.push(byte as char);
            }
        } else if c == '+' {
            result.push(' ');
        } else {
            result.push(c);
        }
    }
    result
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

            // Read the HTTP request (just need the first line for GET)
            let mut buf = [0u8; 4096];
            let n = match stream.read(&mut buf) {
                Ok(n) => n,
                Err(_) => continue,
            };
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
                let _ = stream.write_all(b"HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\n\r\n");
                continue;
            }

            if path.starts_with("/hook/complete") {
                let params = parse_query_params(path);

                // Validate auth token
                let req_token = params.get("token").cloned().unwrap_or_default();
                if req_token != token {
                    let _ = stream.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n");
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
                    let _ = stream.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n");
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
                        crate::commands::k2so_agents::k2so_agents_create(project_path, name, role, prompt)
                            .map(|info| serde_json::to_string(&info).unwrap_or_default())
                    }
                    "/cli/agents/work/create" => {
                        let agent = params.get("agent").cloned();
                        let title = params.get("title").cloned().unwrap_or_default();
                        let body = params.get("body").cloned().unwrap_or_default();
                        let priority = params.get("priority").cloned();
                        let item_type = params.get("type").cloned();
                        crate::commands::k2so_agents::k2so_agents_work_create(
                            project_path, agent, title, body, priority, item_type,
                        )
                        .map(|item| serde_json::to_string(&item).unwrap_or_default())
                    }
                    "/cli/agents/delegate" => {
                        let target = params.get("target").cloned().unwrap_or_default();
                        let file = params.get("file").cloned().unwrap_or_default();
                        crate::commands::k2so_agents::k2so_agents_delegate(project_path, target, file)
                            .map(|_| r#"{"success":true}"#.to_string())
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
                    "/cli/heartbeat" => {
                        // Heartbeat triage: find agents with actionable work and launch them
                        match crate::commands::k2so_agents::k2so_agents_triage_decide(project_path.clone()) {
                            Ok(agents_to_launch) => {
                                for agent_name in &agents_to_launch {
                                    if let Ok(launch_info) = crate::commands::k2so_agents::k2so_agents_build_launch(
                                        project_path.clone(), agent_name.clone(), None,
                                    ) {
                                        let _ = app_handle.emit("cli:agent-launch", &launch_info);
                                    }
                                }
                                Ok(serde_json::json!({
                                    "launched": agents_to_launch,
                                    "count": agents_to_launch.len(),
                                }).to_string())
                            }
                            Err(e) => Err(e),
                        }
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
                    "/cli/commit" | "/cli/commit-merge" => {
                        let include_merge = route == "/cli/commit-merge";
                        let message = params.get("message").cloned().unwrap_or_default();
                        let event_payload = serde_json::json!({
                            "projectPath": project_path,
                            "includeMerge": include_merge,
                            "message": message,
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

    let hook_cmd = format!(r#"[ -x "{}" ] && "{}" "$@" || true"#, hook_script, hook_script);

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
        let hook_cmd = format!(r#"[ -x "{}" ] && "{}" {} || true"#, hook_script, hook_script, mapped_type);

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

    let hook_cmd = format!(r#"[ -x "{}" ] && "{}" "$@" || true"#, hook_script, hook_script);

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

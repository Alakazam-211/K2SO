use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use super::auth;
use super::types::CompanionState;

/// Route mapping: companion endpoint → (internal route, allowed methods)
fn map_route(method: &str, path: &str) -> Option<&'static str> {
    let clean = path.split('?').next().unwrap_or("");
    match (method, clean) {
        ("GET",  "/companion/agents")         => Some("/cli/agents/list"),
        ("GET",  "/companion/agents/running")  => Some("/cli/agents/running"),
        ("GET",  "/companion/agents/work")     => Some("/cli/agents/work"),
        ("GET",  "/companion/reviews")         => Some("/cli/reviews"),
        ("POST", "/companion/review/approve")  => Some("/cli/review/approve"),
        ("POST", "/companion/review/reject")   => Some("/cli/review/reject"),
        ("POST", "/companion/review/feedback") => Some("/cli/review/feedback"),
        ("GET",  "/companion/terminal/read")   => Some("/cli/terminal/read"),
        ("POST", "/companion/terminal/write")  => Some("/cli/terminal/write"),
        ("GET",  "/companion/status")          => Some("/cli/mode"),
        ("POST", "/companion/agents/wake")     => Some("/cli/agents/launch"),
        ("GET",  "/companion/projects")         => Some("/cli/companion/projects"),
        ("GET",  "/companion/projects/summary") => Some("/cli/companion/projects-summary"),
        ("GET",  "/companion/sessions")         => Some("/cli/companion/sessions"),
        ("GET",  "/companion/presets")          => Some("/cli/companion/presets"),
        ("POST", "/companion/terminal/spawn")   => Some("/cli/terminal/spawn"),
        ("POST", "/companion/terminal/spawn-background") => Some("/cli/terminal/spawn-background"),
        _ => None,
    }
}

/// Parse HTTP headers from raw request bytes.
pub fn parse_headers(request: &str) -> HashMap<String, String> {
    let mut headers = HashMap::new();
    for line in request.lines().skip(1) {
        if line.is_empty() { break; }
        if let Some((key, val)) = line.split_once(':') {
            headers.insert(key.trim().to_lowercase(), val.trim().to_string());
        }
    }
    headers
}

/// Extract query parameters from a URL path.
pub fn parse_query(path: &str) -> HashMap<String, String> {
    let mut params = HashMap::new();
    if let Some(query) = path.split_once('?').map(|(_, q)| q) {
        for pair in query.split('&') {
            if let Some((k, v)) = pair.split_once('=') {
                params.insert(
                    urldecode(k),
                    urldecode(v),
                );
            }
        }
    }
    params
}

/// Parse JSON body from POST request (after the blank line separator).
pub fn parse_json_body(request: &str) -> Option<serde_json::Value> {
    let body = request.split("\r\n\r\n").nth(1)?;
    serde_json::from_str(body).ok()
}

/// URL-decode a string (handles %XX and +).
fn urldecode(s: &str) -> String {
    let s = s.replace('+', " ");
    let mut result = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                result.push(byte);
                i += 3;
                continue;
            }
        }
        result.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&result).to_string()
}

/// Map a WebSocket method name to an internal route.
/// Returns (internal_route, is_global) — global routes don't need a project param.
pub fn map_ws_method(method: &str) -> Option<(&'static str, bool)> {
    match method {
        "projects.list"    => Some(("/cli/companion/projects", true)),
        "projects.summary" => Some(("/cli/companion/projects-summary", true)),
        "sessions.list"    => Some(("/cli/companion/sessions", true)),
        "agents.list"      => Some(("/cli/agents/list", false)),
        "agents.running"   => Some(("/cli/agents/running", false)),
        "agents.work"      => Some(("/cli/agents/work", false)),
        "agents.wake"      => Some(("/cli/agents/launch", false)),
        "reviews.list"     => Some(("/cli/reviews", false)),
        "review.approve"   => Some(("/cli/review/approve", false)),
        "review.reject"    => Some(("/cli/review/reject", false)),
        "review.feedback"  => Some(("/cli/review/feedback", false)),
        "terminal.read"    => Some(("/cli/terminal/read", false)),
        "terminal.write"   => Some(("/cli/terminal/write", false)),
        "terminal.spawn"   => Some(("/cli/terminal/spawn", false)),
        "terminal.spawn_background" => Some(("/cli/terminal/spawn-background", false)),
        "presets.list"     => Some(("/cli/companion/presets", true)),
        "status"           => Some(("/cli/mode", false)),
        _ => None,
    }
}

/// Execute a WebSocket method by forwarding to the internal K2SO HTTP server.
/// Converts method params to query params on the internal request.
pub fn dispatch_ws_method(
    state: &CompanionState,
    method: &str,
    params: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let (internal_route, is_global) = map_ws_method(method)
        .ok_or_else(|| format!("Unknown method: {}", method))?;

    let project = params.get("project").and_then(|v| v.as_str()).unwrap_or("");
    if !is_global && project.is_empty() {
        return Err("Missing 'project' param".to_string());
    }

    // Build query params
    let mut query = vec![
        format!("token={}", urlencode(&state.hook_token)),
    ];
    if !project.is_empty() {
        query.push(format!("project={}", urlencode(project)));
    }

    // Forward all params (except project/token which are already handled)
    if let Some(obj) = params.as_object() {
        for (k, v) in obj {
            if k == "project" || k == "token" { continue; }
            let val = match v {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            query.push(format!("{}={}", urlencode(k), urlencode(&val)));
        }
    }

    let url = format!(
        "http://127.0.0.1:{}{}?{}",
        state.hook_port, internal_route, query.join("&")
    );

    let resp = reqwest::blocking::get(&url)
        .map_err(|e| format!("Internal request failed: {}", e))?;

    let status = resp.status();
    let text = resp.text().unwrap_or_default();

    if status.is_success() {
        let data: serde_json::Value = serde_json::from_str(&text)
            .unwrap_or(serde_json::Value::String(text));
        Ok(serde_json::json!({"ok": true, "data": data}))
    } else {
        let error: serde_json::Value = serde_json::from_str(&text)
            .unwrap_or(serde_json::json!({"error": text}));
        let msg = error.get("error").and_then(|e| e.as_str()).unwrap_or("Internal error");
        Err(msg.to_string())
    }
}

/// URL-encode a string for query parameters.
fn urlencode(s: &str) -> String {
    let mut result = String::new();
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                result.push(byte as char);
            }
            _ => {
                result.push_str(&format!("%{:02X}", byte));
            }
        }
    }
    result
}

/// Send a JSON response with the companion envelope format.
pub fn send_response(stream: &mut TcpStream, status: u16, body: &serde_json::Value) {
    let body_str = serde_json::to_string(body).unwrap_or_else(|_| "{}".to_string());
    let status_text = match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        429 => "Too Many Requests",
        _ => "Error",
    };
    let response = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Headers: Authorization, Content-Type\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\n\r\n{}",
        status, status_text, body_str.len(), body_str
    );
    let _ = stream.write_all(response.as_bytes());
}

/// Forward a validated companion request to the internal K2SO HTTP server.
/// Converts POST JSON bodies to GET query params for the internal server.
pub fn proxy_to_internal(
    state: &CompanionState,
    method: &str,
    path: &str,
    headers: &HashMap<String, String>,
    request: &str,
    project: &str,
) -> Result<serde_json::Value, String> {
    let internal_route = map_route(method, path)
        .ok_or_else(|| "Unknown companion endpoint".to_string())?;

    // Build query params: start with token + project
    let mut params = vec![
        format!("token={}", urlencode(&state.hook_token)),
        format!("project={}", urlencode(project)),
    ];

    // Forward query params from the companion request
    let query_params = parse_query(path);
    for (k, v) in &query_params {
        if k != "token" && k != "project" {
            params.push(format!("{}={}", urlencode(k), urlencode(v)));
        }
    }

    // For POST requests, convert JSON body fields to query params
    if method == "POST" {
        if let Some(body) = parse_json_body(request) {
            if let Some(obj) = body.as_object() {
                for (k, v) in obj {
                    let val = match v {
                        serde_json::Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    params.push(format!("{}={}", urlencode(k), urlencode(&val)));
                }
            }
        }
    }

    let url = format!(
        "http://127.0.0.1:{}{}?{}",
        state.hook_port, internal_route, params.join("&")
    );

    // Forward to internal server
    let resp = reqwest::blocking::get(&url)
        .map_err(|e| format!("Internal proxy failed: {}", e))?;

    let status = resp.status();
    let text = resp.text().unwrap_or_default();

    // Parse internal response and wrap in companion envelope
    if status.is_success() {
        let data: serde_json::Value = serde_json::from_str(&text)
            .unwrap_or(serde_json::Value::String(text));
        Ok(serde_json::json!({"ok": true, "data": data}))
    } else {
        let error: serde_json::Value = serde_json::from_str(&text)
            .unwrap_or(serde_json::json!({"error": text}));
        let msg = error.get("error").and_then(|e| e.as_str()).unwrap_or("Internal error");
        Err(msg.to_string())
    }
}

/// Handle a companion HTTP request: auth check, route, proxy.
pub fn handle_request(
    stream: &mut TcpStream,
    state: &CompanionState,
    method: &str,
    path: &str,
    headers: &HashMap<String, String>,
    request: &str,
    remote_addr: &str,
) {
    let clean_path = path.split('?').next().unwrap_or("");

    // OPTIONS (CORS preflight)
    if method == "OPTIONS" {
        send_response(stream, 200, &serde_json::json!({"ok": true}));
        return;
    }

    // Auth endpoint — no Bearer token needed, uses Basic Auth
    if clean_path == "/companion/auth" && method == "POST" {
        handle_auth(stream, state, headers, remote_addr);
        return;
    }

    // WebSocket upgrade — handled separately
    if clean_path == "/companion/ws" {
        // WebSocket upgrade is handled at the caller level before this function
        send_response(stream, 400, &serde_json::json!({"ok": false, "error": "WebSocket upgrade must use GET with Upgrade header"}));
        return;
    }

    // All other routes require Bearer token
    let auth_header = headers.get("authorization").cloned().unwrap_or_default();
    let token = match auth::parse_bearer(&auth_header) {
        Some(t) => t,
        None => {
            send_response(stream, 401, &serde_json::json!({"ok": false, "error": "Missing Authorization: Bearer <token>"}));
            return;
        }
    };

    // Validate session + rate limit
    if let Err(msg) = auth::validate_bearer(&token, state) {
        let status = if msg.contains("Rate limit") { 429 } else { 401 };
        send_response(stream, status, &serde_json::json!({"ok": false, "error": msg}));
        return;
    }

    // Extract project from query params (companion clients must specify which workspace)
    let query = parse_query(path);
    let project = query.get("project").cloned().unwrap_or_default();

    // Proxy to internal server
    match proxy_to_internal(state, method, path, headers, request, &project) {
        Ok(data) => send_response(stream, 200, &data),
        Err(e) => send_response(stream, 400, &serde_json::json!({"ok": false, "error": e})),
    }
}

/// Handle POST /companion/auth — Basic Auth login.
fn handle_auth(
    stream: &mut TcpStream,
    state: &CompanionState,
    headers: &HashMap<String, String>,
    remote_addr: &str,
) {
    let auth_header = headers.get("authorization").cloned().unwrap_or_default();
    let (username, password) = match auth::parse_basic_auth(&auth_header) {
        Some(creds) => creds,
        None => {
            send_response(stream, 401, &serde_json::json!({"ok": false, "error": "Missing Authorization: Basic <credentials>"}));
            return;
        }
    };

    // Read settings to validate credentials
    let settings = crate::commands::settings::read_settings();
    if username != settings.companion.username {
        send_response(stream, 401, &serde_json::json!({"ok": false, "error": "Invalid credentials"}));
        return;
    }

    if !auth::verify_password(&password, &settings.companion.password_hash) {
        send_response(stream, 401, &serde_json::json!({"ok": false, "error": "Invalid credentials"}));
        return;
    }

    // Create session
    let session = auth::create_session(remote_addr);
    let token = session.token.clone();
    let expires_at = session.expires_at.to_rfc3339();

    state.sessions.lock().unwrap().insert(token.clone(), session);

    send_response(stream, 200, &serde_json::json!({
        "ok": true,
        "data": {
            "token": token,
            "expiresAt": expires_at,
        }
    }));
}

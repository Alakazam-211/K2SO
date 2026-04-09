//! K2SO Mobile Companion API
//!
//! Exposes a curated subset of K2SO's internal HTTP API through an ngrok tunnel.
//! The companion app connects to this tunnel to monitor and interact with agents remotely.
//!
//! Architecture:
//! - Starts an ngrok tunnel and accepts connections directly from it
//! - Proxies validated requests to the internal K2SO HTTP server
//! - Supports WebSocket for real-time event push
//! - Tokio runtime kept alive for the tunnel's lifetime

pub mod auth;
pub mod proxy;
pub mod types;
pub mod websocket;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Listener};
use futures::TryStreamExt;

use types::CompanionState;

/// Module-level companion state. Mutex<Option> allows stop + restart (unlike OnceLock).
pub(crate) static STATE: Mutex<Option<CompanionState>> = Mutex::new(None);
/// Flag indicating the companion is running.
static RUNNING: AtomicBool = AtomicBool::new(false);

/// Start the companion API proxy.
/// Reads settings for credentials + ngrok token, starts tunnel, spawns listener.
/// Returns the public tunnel URL.
pub fn start_companion(app_handle: AppHandle) -> Result<String, String> {
    if RUNNING.load(Ordering::Relaxed) {
        return Err("Companion is already running".to_string());
    }

    let settings = crate::commands::settings::read_settings();
    let companion = &settings.companion;

    if companion.username.is_empty() {
        return Err("Username is required".to_string());
    }
    if companion.password_hash.is_empty() {
        return Err("Password must be set before enabling companion".to_string());
    }
    if companion.ngrok_auth_token.is_empty() {
        return Err("ngrok auth token is required".to_string());
    }

    let hook_port = crate::agent_hooks::get_port();
    let hook_token = crate::agent_hooks::get_token();

    if hook_port == 0 || hook_token.is_empty() {
        return Err("K2SO internal server not ready yet".to_string());
    }

    // Start ngrok tunnel — runs on a background thread with its own tokio runtime.
    // The tunnel accepts connections directly (no separate TcpListener needed).
    let ngrok_token = companion.ngrok_auth_token.clone();

    // Channel to keep the tunnel thread alive
    let (keepalive_tx, keepalive_rx) = std::sync::mpsc::channel::<()>();

    // Start the ngrok tunnel on a background thread with a persistent tokio runtime
    let (tunnel_url, tunnel_listener) = start_ngrok_tunnel(&ngrok_token)?;

    log_debug!("[companion] Tunnel: {}", tunnel_url);

    // Initialize state
    let state = CompanionState {
        tunnel_url: Mutex::new(Some(tunnel_url.clone())),
        sessions: Mutex::new(HashMap::new()),
        ws_clients: Mutex::new(Vec::new()),
        shutdown: AtomicBool::new(false),
        hook_port,
        hook_token: hook_token.to_string(),
        _tunnel_keepalive: Mutex::new(Some(keepalive_tx)),
    };
    *STATE.lock().unwrap() = Some(state);
    RUNNING.store(true, Ordering::Relaxed);

    // Spawn the proxy thread — accepts connections from ngrok tunnel directly
    std::thread::spawn(move || {
        run_ngrok_listener(tunnel_listener, keepalive_rx);
    });

    // Spawn session cleanup thread (every 5 minutes, expire old sessions)
    std::thread::spawn(|| {
        loop {
            std::thread::sleep(std::time::Duration::from_secs(300));
            if !RUNNING.load(Ordering::Relaxed) { break; }
            if let Some(ref state) = *STATE.lock().unwrap() {
                let mut sessions = state.sessions.lock().unwrap();
                sessions.retain(|_, s| !s.is_expired());
            }
        }
    });

    // Register Tauri event listeners for WebSocket broadcast
    register_event_listeners(&app_handle);

    // Spawn terminal output polling thread
    let poll_handle = app_handle.clone();
    std::thread::spawn(move || {
        run_terminal_polling(&poll_handle);
    });

    Ok(tunnel_url)
}

/// Stop the companion API proxy.
pub fn stop_companion() -> Result<(), String> {
    if !RUNNING.load(Ordering::Relaxed) {
        return Ok(());
    }

    // Signal shutdown and drop keepalive
    {
        let guard = STATE.lock().unwrap();
        if let Some(ref state) = *guard {
            state.shutdown.store(true, Ordering::Relaxed);
            *state._tunnel_keepalive.lock().unwrap() = None;
            *state.tunnel_url.lock().unwrap() = None;
            state.ws_clients.lock().unwrap().clear();
            state.sessions.lock().unwrap().clear();
        }
    }

    // Clear the state entirely so a fresh one can be created on restart
    *STATE.lock().unwrap() = None;

    // Clear tunnel handle
    *NGROK_TUNNEL.lock().unwrap() = None;

    // Keep the ngrok session alive in NGROK_SESSION for reuse on restart.
    // Free tier allows only one session — closing and reconnecting causes auth failures.
    // The tunnel listener thread has already exited (shutdown flag set), so the
    // endpoint goes offline. On restart, we reuse the existing session.

    RUNNING.store(false, Ordering::Relaxed);
    log_debug!("[companion] Stopped");

    Ok(())
}

/// Get companion status.
pub fn companion_status() -> serde_json::Value {
    if !RUNNING.load(Ordering::Relaxed) {
        return serde_json::json!({
            "running": false,
            "tunnelUrl": null,
            "connectedClients": 0,
            "sessions": [],
        });
    }

    if let Some(ref state) = *STATE.lock().unwrap() {
        let url = state.tunnel_url.lock().unwrap().clone();
        let sessions = state.sessions.lock().unwrap();
        let ws_clients = state.ws_clients.lock().unwrap();

        let session_list: Vec<serde_json::Value> = sessions.values().map(|s| {
            serde_json::json!({
                "token": format!("{}...", &s.token[..8.min(s.token.len())]),
                "remoteAddr": s.remote_addr,
                "createdAt": s.created_at.to_rfc3339(),
                "expiresAt": s.expires_at.to_rfc3339(),
            })
        }).collect();

        serde_json::json!({
            "running": true,
            "tunnelUrl": url,
            "connectedClients": ws_clients.len(),
            "sessions": session_list,
        })
    } else {
        serde_json::json!({ "running": false })
    }
}

/// ngrok tunnel listener wrapper that can be sent between threads.
pub struct NgrokTunnelListener {
    rt: tokio::runtime::Runtime,
    // The actual listener is consumed by the accept loop
}

/// Start the ngrok tunnel. Returns the URL and a runtime handle.
/// Reuses existing ngrok session if available (avoids one-session-per-account limit on free tier).
fn start_ngrok_tunnel(ngrok_token: &str) -> Result<(String, tokio::runtime::Runtime), String> {
    let rt = tokio::runtime::Runtime::new()
        .map_err(|e| format!("Failed to create tokio runtime: {}", e))?;

    let token = ngrok_token.to_string();
    let url = rt.block_on(async {
        // Try to reuse existing session (avoids auth failure from one-session limit)
        let session = {
            let existing = NGROK_SESSION.lock().unwrap().take();
            if let Some(s) = existing {
                log_debug!("[companion] Reusing existing ngrok session");
                s
            } else {
                log_debug!("[companion] Creating new ngrok session");
                ngrok::Session::builder()
                    .authtoken(&token)
                    .connect()
                    .await
                    .map_err(|e| format!("ngrok connect failed: {}", e))?
            }
        };

        use ngrok::config::TunnelBuilder;
        let tunnel = session
            .http_endpoint()
            .listen()
            .await
            .map_err(|e| format!("ngrok tunnel failed: {}", e))?;

        use ngrok::tunnel::EndpointInfo;
        let url = tunnel.url().to_string();

        NGROK_SESSION.lock().unwrap().replace(session);
        NGROK_TUNNEL.lock().unwrap().replace(tunnel);

        Ok::<String, String>(url)
    })?;

    Ok((url, rt))
}

/// Global storage for the ngrok tunnel (moved to listener thread)
static NGROK_TUNNEL: Mutex<Option<ngrok::tunnel::HttpTunnel>> = Mutex::new(None);
static NGROK_SESSION: Mutex<Option<ngrok::Session>> = Mutex::new(None);

/// Accept connections from the ngrok tunnel and handle them as HTTP requests.
/// The tokio runtime is kept alive by holding it in this thread.
fn run_ngrok_listener(rt: tokio::runtime::Runtime, _keepalive: std::sync::mpsc::Receiver<()>) {
    // Take the tunnel from global storage
    let tunnel = NGROK_TUNNEL.lock().unwrap().take();
    let Some(mut tunnel) = tunnel else {
        log_debug!("[companion] No tunnel available for listener");
        return;
    };

    rt.block_on(async {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        loop {
            if let Some(ref state) = *STATE.lock().unwrap() {
                if state.shutdown.load(Ordering::Relaxed) { break; }
            }

            // Accept a connection from the ngrok tunnel
            log_debug!("[companion] Waiting for next connection...");
            let conn = match tunnel.try_next().await {
                Ok(Some(conn)) => {
                    log_debug!("[companion] Connection accepted");
                    conn
                }
                Ok(None) => {
                    log_debug!("[companion] Tunnel closed (try_next returned None)");
                    break;
                }
                Err(e) => {
                    log_debug!("[companion] Tunnel accept error: {}", e);
                    // Don't break — try to accept next connection
                    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                    continue;
                }
            };

            // Read the HTTP request from the ngrok connection
            let mut stream = conn;
            let mut buf = vec![0u8; 65536];
            let n = match stream.read(&mut buf).await {
                Ok(0) => continue,
                Ok(n) => n,
                Err(_) => continue,
            };

            let request = String::from_utf8_lossy(&buf[..n]).to_string();
            let first_line = request.lines().next().unwrap_or("");
            let parts: Vec<&str> = first_line.split_whitespace().collect();
            let (method, path) = match parts.as_slice() {
                [m, p, ..] => (*m, *p),
                _ => continue,
            };

            let headers = proxy::parse_headers(&request);

            if let Some(ref state) = *STATE.lock().unwrap() {
                // For now, handle HTTP requests synchronously by proxying to internal server
                // WebSocket upgrades over ngrok require a different approach — skip for now
                let remote_addr = "ngrok".to_string();

                let clean_path = path.split('?').next().unwrap_or("");

                // Build response
                let response_body = if clean_path == "/companion/auth" && method == "POST" {
                    // Auth handler
                    handle_auth_inline(state, &headers, &remote_addr)
                } else {
                    // Validate bearer + proxy
                    let auth_header = headers.get("authorization").cloned().unwrap_or_default();
                    match proxy::parse_query(path).get("token").cloned()
                        .or_else(|| crate::companion::auth::parse_bearer(&auth_header))
                    {
                        Some(token) => {
                            match crate::companion::auth::validate_bearer(&token, state) {
                                Ok(_) => {
                                    let query = proxy::parse_query(path);
                                    let project = query.get("project").cloned().unwrap_or_default();
                                    match proxy::proxy_to_internal(state, method, path, &headers, &request, &project) {
                                        Ok(data) => format_response(200, &data),
                                        Err(e) => format_response(400, &serde_json::json!({"ok": false, "error": e})),
                                    }
                                }
                                Err(msg) => {
                                    let status = if msg.contains("Rate limit") { 429 } else { 401 };
                                    format_response(status, &serde_json::json!({"ok": false, "error": msg}))
                                }
                            }
                        }
                        None => format_response(401, &serde_json::json!({"ok": false, "error": "Missing authorization"})),
                    }
                };

                // Write response back through ngrok
                log_debug!("[companion] Sending response ({} bytes)", response_body.len());
                let _ = stream.write_all(response_body.as_bytes()).await;
                let _ = stream.flush().await;
                let _ = stream.shutdown().await;
                log_debug!("[companion] Response sent, connection closed");
            }
        }
    });

    log_debug!("[companion] Listener thread ended");
    RUNNING.store(false, Ordering::Relaxed);
}

/// Format an HTTP response string.
fn format_response(status: u16, body: &serde_json::Value) -> String {
    let body_str = serde_json::to_string(body).unwrap_or_else(|_| "{}".to_string());
    let status_text = match status {
        200 => "OK", 400 => "Bad Request", 401 => "Unauthorized",
        429 => "Too Many Requests", _ => "Error",
    };
    format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Headers: Authorization, Content-Type\r\n\r\n{}",
        status, status_text, body_str.len(), body_str
    )
}

/// Inline auth handler for ngrok connections (can't use TcpStream directly).
fn handle_auth_inline(state: &CompanionState, headers: &HashMap<String, String>, remote_addr: &str) -> String {
    let auth_header = headers.get("authorization").cloned().unwrap_or_default();
    let (username, password) = match auth::parse_basic_auth(&auth_header) {
        Some(creds) => creds,
        None => return format_response(401, &serde_json::json!({"ok": false, "error": "Missing Basic Auth"})),
    };

    let settings = crate::commands::settings::read_settings();
    if username != settings.companion.username {
        return format_response(401, &serde_json::json!({"ok": false, "error": "Invalid credentials"}));
    }
    if !auth::verify_password(&password, &settings.companion.password_hash) {
        return format_response(401, &serde_json::json!({"ok": false, "error": "Invalid credentials"}));
    }

    let session = auth::create_session(remote_addr);
    let token = session.token.clone();
    let expires_at = session.expires_at.to_rfc3339();
    state.sessions.lock().unwrap().insert(token.clone(), session);

    format_response(200, &serde_json::json!({
        "ok": true,
        "data": { "token": token, "expiresAt": expires_at }
    }))
}

/// Register Tauri event listeners to broadcast to WebSocket clients.
fn register_event_listeners(app_handle: &AppHandle) {
    let events = ["agent:lifecycle", "agent:reply", "sync:projects"];
    for event_name in &events {
        let name = event_name.to_string();
        app_handle.listen(event_name.to_string(), move |event| {
            if let Some(ref state) = *STATE.lock().unwrap() {
                let payload = event.payload();
                let ws_event = serde_json::json!({
                    "type": name,
                    "data": payload,
                });
                websocket::broadcast_event(state, &ws_event.to_string());
            }
        });
    }
}

/// Poll terminal output for subscribed WebSocket clients.
fn run_terminal_polling(app_handle: &AppHandle) {
    let mut last_snapshots: HashMap<String, String> = HashMap::new();

    loop {
        std::thread::sleep(std::time::Duration::from_millis(500));

        if !RUNNING.load(Ordering::Relaxed) { break; }

        let guard = STATE.lock().unwrap(); let state = match guard.as_ref() {
            Some(s) => s,
            None => break,
        };

        // Collect all subscribed terminal IDs
        let terminal_ids: Vec<String> = {
            let clients = state.ws_clients.lock().unwrap();
            let mut ids = std::collections::HashSet::new();
            for client in clients.iter() {
                ids.extend(client.subscribed_terminals.iter().cloned());
            }
            ids.into_iter().collect()
        };

        if terminal_ids.is_empty() { continue; }

        // Poll each terminal
        for tid in &terminal_ids {
            let url = format!(
                "http://127.0.0.1:{}/cli/terminal/read?token={}&id={}&lines=50",
                state.hook_port,
                crate::agent_hooks::get_token(),
                tid
            );

            if let Ok(resp) = reqwest::blocking::get(&url) {
                if let Ok(text) = resp.text() {
                    let prev = last_snapshots.get(tid).cloned().unwrap_or_default();
                    if text != prev {
                        // Content changed — broadcast diff
                        let lines: Vec<String> = text.lines().map(|l| l.to_string()).collect();
                        websocket::broadcast_terminal_output(state, tid, &lines);
                        last_snapshots.insert(tid.clone(), text);
                    }
                }
            }
        }
    }
}

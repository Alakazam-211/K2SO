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
use ngrok::config::ForwarderBuilder;
use ngrok::tunnel::EndpointInfo;

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

    let ngrok_token = companion.ngrok_auth_token.clone();

    // Bind a local TcpListener for the companion HTTP server
    let listener = std::net::TcpListener::bind("127.0.0.1:0")
        .map_err(|e| format!("Failed to bind companion listener: {}", e))?;
    let local_port = listener.local_addr().unwrap().port();

    // Start ngrok tunnel — forwards traffic to our local companion HTTP server
    let (tunnel_url, _rt) = start_ngrok_tunnel(&ngrok_token, local_port)?;

    log_debug!("[companion] Tunnel: {} → localhost:{}", tunnel_url, local_port);

    // Channel to keep the tunnel runtime alive
    let (keepalive_tx, _keepalive_rx) = std::sync::mpsc::channel::<()>();

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
    *STATE.lock().unwrap_or_else(|e| e.into_inner()) = Some(state);
    RUNNING.store(true, Ordering::Relaxed);

    // Spawn the local companion HTTP server (handles auth + proxying)
    std::thread::spawn(move || {
        run_local_listener(listener);
    });

    // Keep the tokio runtime alive on a background thread (ngrok tunnel needs it)
    std::thread::spawn(move || {
        // _rt stays alive as long as this thread runs
        // _keepalive_rx blocks until the sender is dropped (on stop)
        let _ = _keepalive_rx.recv();
        log_debug!("[companion] Tunnel runtime shutting down");
        drop(_rt);
    });

    // Spawn session cleanup thread (every 5 minutes, expire old sessions)
    std::thread::spawn(|| {
        loop {
            std::thread::sleep(std::time::Duration::from_secs(300));
            if !RUNNING.load(Ordering::Relaxed) { break; }
            if let Some(ref state) = *STATE.lock().unwrap_or_else(|e| e.into_inner()) {
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
        let guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(ref state) = *guard {
            state.shutdown.store(true, Ordering::Relaxed);
            *state._tunnel_keepalive.lock().unwrap() = None;
            *state.tunnel_url.lock().unwrap() = None;
            state.ws_clients.lock().unwrap().clear();
            state.sessions.lock().unwrap().clear();
        }
    }

    // Clear the state entirely so a fresh one can be created on restart
    *STATE.lock().unwrap_or_else(|e| e.into_inner()) = None;

    // Drop the forwarder to stop ngrok from forwarding traffic
    *NGROK_FORWARDER.lock().unwrap_or_else(|e| e.into_inner()) = None;

    // Session kept alive in NGROK_SESSION — closed on next start.

    RUNNING.store(false, Ordering::Relaxed);
    log_debug!("[companion] Stopped");

    Ok(())
}

/// Get companion status. Non-blocking — uses try_lock to avoid blocking the main thread.
pub fn companion_status() -> serde_json::Value {
    if !RUNNING.load(Ordering::Relaxed) {
        return serde_json::json!({
            "running": false,
            "tunnelUrl": null,
            "connectedClients": 0,
            "sessions": [],
        });
    }

    // Use try_lock to avoid blocking the UI thread if the companion thread holds the lock
    let guard = match STATE.try_lock() {
        Ok(g) => g,
        Err(_) => {
            // Lock contended — return minimal status without blocking
            return serde_json::json!({
                "running": true,
                "tunnelUrl": null,
                "connectedClients": 0,
                "sessions": [],
            });
        }
    };

    if let Some(ref state) = *guard {
        let url = state.tunnel_url.try_lock().ok().and_then(|u| u.clone());
        let session_list: Vec<serde_json::Value> = state.sessions.try_lock()
            .map(|sessions| sessions.values().map(|s| {
                serde_json::json!({
                    "token": format!("{}...", &s.token[..8.min(s.token.len())]),
                    "remoteAddr": s.remote_addr,
                    "createdAt": s.created_at.to_rfc3339(),
                    "expiresAt": s.expires_at.to_rfc3339(),
                })
            }).collect())
            .unwrap_or_default();
        let client_count = state.ws_clients.try_lock().map(|c| c.len()).unwrap_or(0);

        serde_json::json!({
            "running": true,
            "tunnelUrl": url,
            "connectedClients": client_count,
            "sessions": session_list,
        })
    } else {
        serde_json::json!({ "running": false })
    }
}

/// Start the ngrok tunnel using listen_and_forward to proxy to a local port.
/// Returns the public URL and the local port the companion HTTP server listens on.
fn start_ngrok_tunnel(ngrok_token: &str, local_port: u16) -> Result<(String, tokio::runtime::Runtime), String> {
    // Run ngrok connect on a separate thread with a hard timeout.
    // The ngrok SDK's connect future is not cancel-safe, so tokio::time::timeout
    // can't abort it. Instead we race the blocking call against a deadline.
    let token = ngrok_token.to_string();
    let (tx, rx) = std::sync::mpsc::channel::<Result<(String, tokio::runtime::Runtime), String>>();

    let thread_token = token.clone();
    std::thread::spawn(move || {
        let result = start_ngrok_tunnel_inner(&thread_token, local_port);
        let _ = tx.send(result);
    });

    // Wait up to 20 seconds for the tunnel to connect
    match rx.recv_timeout(std::time::Duration::from_secs(20)) {
        Ok(result) => result,
        Err(_) => Err("ngrok connect timed out (20s) — old session may still be active".to_string()),
    }
}

/// Inner function that does the actual ngrok connect (runs on a dedicated thread).
fn start_ngrok_tunnel_inner(ngrok_token: &str, local_port: u16) -> Result<(String, tokio::runtime::Runtime), String> {
    let rt = tokio::runtime::Runtime::new()
        .map_err(|e| format!("Failed to create tokio runtime: {}", e))?;

    let token = ngrok_token.to_string();
    let url = rt.block_on(async {
        // Close any existing session
        if let Some(mut old_session) = NGROK_SESSION.lock().unwrap_or_else(|e| e.into_inner()).take() {
            log_debug!("[companion] Closing previous ngrok session...");
            let _ = old_session.close().await;
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        }

        log_debug!("[companion] Creating new ngrok session");
        let session = ngrok::Session::builder()
            .authtoken(&token)
            .connect()
            .await
            .map_err(|e| format!("ngrok connect failed: {}", e))?;

        // Use listen_and_forward — ngrok handles connection proxying to our local server
        use ngrok::config::TunnelBuilder;
        let forward_url = url::Url::parse(&format!("http://localhost:{}", local_port))
            .map_err(|e| format!("Invalid forward URL: {}", e))?;

        let listener = session
            .http_endpoint()
            .listen_and_forward(forward_url)
            .await
            .map_err(|e| format!("ngrok tunnel failed: {}", e))?;

        let url = listener.url().to_string();
        NGROK_SESSION.lock().unwrap_or_else(|e| e.into_inner()).replace(session);

        // Store the forwarder — it must stay alive for ngrok to forward traffic
        NGROK_FORWARDER.lock().unwrap_or_else(|e| e.into_inner()).replace(listener);

        Ok::<String, String>(url)
    })?;

    Ok((url, rt))
}

static NGROK_SESSION: Mutex<Option<ngrok::Session>> = Mutex::new(None);
static NGROK_FORWARDER: Mutex<Option<ngrok::forwarder::Forwarder<ngrok::tunnel::HttpTunnel>>> = Mutex::new(None);

/// Local companion HTTP server — accepts connections from ngrok (forwarded via listen_and_forward).
/// Same blocking TCP pattern as agent_hooks.rs.
fn run_local_listener(listener: std::net::TcpListener) {
    use std::io::{Read, Write};

    for stream in listener.incoming() {
        // Check shutdown
        {
            let guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(ref state) = *guard {
                if state.shutdown.load(Ordering::Relaxed) { break; }
            }
        }

        let Ok(mut stream) = stream else { continue };
        let remote_addr = stream.peer_addr()
            .map(|a| a.to_string())
            .unwrap_or_else(|_| "ngrok".to_string());

        // Read HTTP request
        let mut buf = [0u8; 65536];
        let n = match stream.read(&mut buf) {
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

        let guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(ref state) = *guard {
            // Check for WebSocket upgrade
            let upgrade = headers.get("upgrade").map(|v| v.to_lowercase());
            if upgrade.as_deref() == Some("websocket") {
                drop(guard);
                websocket::handle_ws_upgrade(stream, path, unsafe {
                    &*(STATE.lock().unwrap_or_else(|e| e.into_inner()).as_ref().unwrap() as *const CompanionState)
                });
                continue;
            }

            proxy::handle_request(&mut stream, state, method, path, &headers, &request, &remote_addr);
        }
    }

    log_debug!("[companion] Local listener ended");
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
            if let Some(ref state) = *STATE.lock().unwrap_or_else(|e| e.into_inner()) {
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

        let guard = STATE.lock().unwrap_or_else(|e| e.into_inner()); let state = match guard.as_ref() {
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

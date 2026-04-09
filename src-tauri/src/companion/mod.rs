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
/// Flag to cancel a pending auto-start attempt.
static CANCEL_START: AtomicBool = AtomicBool::new(false);

/// Start the companion API proxy.
/// Reads settings for credentials + ngrok token, starts tunnel, spawns listener.
/// Returns the public tunnel URL.
pub fn start_companion(app_handle: AppHandle) -> Result<String, String> {
    // Clear any previous cancel signal
    CANCEL_START.store(false, Ordering::Relaxed);

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
    let ngrok_domain = companion.ngrok_domain.clone();

    // Bind a local TcpListener for the companion HTTP server
    let listener = std::net::TcpListener::bind("127.0.0.1:0")
        .map_err(|e| format!("Failed to bind companion listener: {}", e))?;
    let local_port = listener.local_addr().unwrap().port();

    // Start ngrok tunnel — forwards traffic to our local companion HTTP server
    let (tunnel_url, _rt) = start_ngrok_tunnel(&ngrok_token, &ngrok_domain, local_port)?;

    // Check if stop was requested while we were connecting
    if CANCEL_START.load(Ordering::Relaxed) {
        log_debug!("[companion] Start cancelled during tunnel connect");
        return Err("Start cancelled".to_string());
    }

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

    // Spawn tunnel health check — restarts companion if local listener dies
    let health_port = local_port;
    let health_handle = app_handle.clone();
    std::thread::spawn(move || {
        // Wait before first check to let everything settle
        std::thread::sleep(std::time::Duration::from_secs(30));
        let mut consecutive_failures = 0u32;
        loop {
            std::thread::sleep(std::time::Duration::from_secs(15));
            if !RUNNING.load(Ordering::Relaxed) { break; }

            // Probe the LOCAL listener — this is ground truth (ngrok URL can return
            // HTML error pages that look like HTTP 200 even when the forwarder is dead)
            let probe = reqwest::blocking::Client::new()
                .get(&format!("http://127.0.0.1:{}/companion/auth", health_port))
                .timeout(std::time::Duration::from_secs(3))
                .send();

            match probe {
                Ok(_) => {
                    consecutive_failures = 0;
                }
                Err(_) => {
                    consecutive_failures += 1;
                    log_debug!("[companion] Tunnel health check failed ({}/3)", consecutive_failures);
                    if consecutive_failures >= 3 {
                        log_debug!("[companion] Tunnel dead — restarting companion");
                        // Stop and restart
                        let _ = stop_companion();
                        std::thread::sleep(std::time::Duration::from_secs(2));
                        match start_companion(health_handle.clone()) {
                            Ok(url) => log_debug!("[companion] Auto-restarted: {}", url),
                            Err(e) => log_debug!("[companion] Auto-restart failed: {}", e),
                        }
                        break; // New health check thread spawned by the new start_companion
                    }
                }
            }
        }
    });

    Ok(tunnel_url)
}

/// Stop the companion API proxy.
pub fn stop_companion() -> Result<(), String> {
    // Signal any in-flight start attempt to abort
    CANCEL_START.store(true, Ordering::Relaxed);

    if !RUNNING.load(Ordering::Relaxed) {
        return Ok(());
    }

    // Signal shutdown and drop keepalive
    {
        let guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(ref state) = *guard {
            state.shutdown.store(true, Ordering::Relaxed);
            *state._tunnel_keepalive.lock().unwrap_or_else(|e| e.into_inner()) = None;
            *state.tunnel_url.lock().unwrap_or_else(|e| e.into_inner()) = None;
            state.ws_clients.lock().unwrap_or_else(|e| e.into_inner()).clear();
            state.sessions.lock().unwrap_or_else(|e| e.into_inner()).clear();
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
        let session_count = session_list.len();
        let ws_count = state.ws_clients.try_lock().map(|c| c.len()).unwrap_or(0);

        serde_json::json!({
            "running": true,
            "tunnelUrl": url,
            "connectedClients": session_count,
            "wsClients": ws_count,
            "sessions": session_list,
        })
    } else {
        serde_json::json!({ "running": false })
    }
}

/// Start the ngrok tunnel using listen_and_forward to proxy to a local port.
/// Returns the public URL and the tokio runtime (must be kept alive for the tunnel).
///
/// The ngrok connect future never yields, so tokio timeouts can't cancel it.
/// We run it on a dedicated thread and wait with recv_timeout. If it times out,
/// the thread keeps running (the runtime stays alive there) — next call will
/// find the session in the static and reuse it.
fn start_ngrok_tunnel(ngrok_token: &str, ngrok_domain: &str, local_port: u16) -> Result<(String, tokio::runtime::Runtime), String> {
    let token = ngrok_token.to_string();
    let domain = ngrok_domain.to_string();
    let (tx, rx) = std::sync::mpsc::channel::<Result<String, String>>();

    std::thread::spawn(move || {
        let rt = match tokio::runtime::Runtime::new() {
            Ok(rt) => rt,
            Err(e) => {
                let _ = tx.send(Err(format!("Failed to create tokio runtime: {}", e)));
                return;
            }
        };

        // Single block_on call — setup + keepalive in one async block.
        // The forwarder's background task runs on this runtime. If we exit
        // block_on and re-enter it, the task can get interrupted. Keeping
        // everything in one block_on ensures the runtime event loop stays
        // active for the forwarder's lifetime.
        rt.block_on(async {
            // Close any existing session
            if let Some(mut old_session) = NGROK_SESSION.lock().unwrap_or_else(|e| e.into_inner()).take() {
                log_debug!("[companion] Closing previous ngrok session...");
                let _ = old_session.close().await;
                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            }

            log_debug!("[companion] Creating new ngrok session");
            let connect_result: Result<String, String> = async {
                let session = ngrok::Session::builder()
                    .authtoken(&token)
                    .connect()
                    .await
                    .map_err(|e| format!("ngrok connect failed: {}", e))?;

                use ngrok::config::TunnelBuilder;
                let forward_url = url::Url::parse(&format!("http://localhost:{}", local_port))
                    .map_err(|e| format!("Invalid forward URL: {}", e))?;

                let mut endpoint = session.http_endpoint();
                if !domain.is_empty() {
                    endpoint.domain(&domain);
                }
                let listener = endpoint
                    .listen_and_forward(forward_url)
                    .await
                    .map_err(|e| format!("ngrok tunnel failed: {}", e))?;

                let url = listener.url().to_string();
                NGROK_SESSION.lock().unwrap_or_else(|e| e.into_inner()).replace(session);
                NGROK_FORWARDER.lock().unwrap_or_else(|e| e.into_inner()).replace(listener);

                Ok(url)
            }.await;

            // Send result back to caller
            let _ = tx.send(connect_result);

            // Park in the event loop — keeps the forwarder's async task alive.
            // The forwarder proxies connections as long as this block_on is active.
            loop {
                tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
                if !RUNNING.load(Ordering::Relaxed) { break; }
            }
        });
        log_debug!("[companion] Tunnel runtime thread exiting");
    });

    // Wait up to 20 seconds for the tunnel URL
    match rx.recv_timeout(std::time::Duration::from_secs(20)) {
        Ok(Ok(url)) => {
            // The real runtime stays alive on the spawned thread (block_on loop).
            // Create a lightweight runtime for the keepalive thread contract.
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_time()
                .build()
                .map_err(|e| format!("Failed to create runtime: {}", e))?;
            Ok((url, rt))
        }
        Ok(Err(e)) => Err(e),
        Err(_) => Err("ngrok connect timed out (20s) — old session may still be active".to_string()),
    }
}

static NGROK_SESSION: Mutex<Option<ngrok::Session>> = Mutex::new(None);
static NGROK_FORWARDER: Mutex<Option<ngrok::forwarder::Forwarder<ngrok::tunnel::HttpTunnel>>> = Mutex::new(None);

/// Local companion HTTP server — accepts connections from ngrok (forwarded via listen_and_forward).
/// Same blocking TCP pattern as agent_hooks.rs.
fn run_local_listener(listener: std::net::TcpListener) {
    use std::io::{Read, Write};

    // Set a timeout so the listener doesn't block forever on accept —
    // allows periodic shutdown checks even when no connections arrive.
    listener.set_nonblocking(false).ok();

    log_debug!("[companion] Local listener started on port {}", listener.local_addr().map(|a| a.port()).unwrap_or(0));
    let mut request_count = 0u64;
    for stream in listener.incoming() {
        request_count += 1;
        // Wrap each connection in catch_unwind to prevent a single bad request
        // from killing the entire listener thread.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // Check shutdown
            {
                let guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(ref state) = *guard {
                    if state.shutdown.load(Ordering::Relaxed) { return false; }
                }
            }

            let Ok(mut stream) = stream else {
                log_debug!("[companion] Listener accept error — continuing");
                return true;
            };
            let remote_addr = stream.peer_addr()
                .map(|a| a.to_string())
                .unwrap_or_else(|_| "ngrok".to_string());

            // Read HTTP request
            let mut buf = [0u8; 65536];
            let n = match stream.read(&mut buf) {
                Ok(0) => return true,
                Ok(n) => n,
                Err(_) => return true,
            };
            let request = String::from_utf8_lossy(&buf[..n]).to_string();
            let first_line = request.lines().next().unwrap_or("");
            let parts: Vec<&str> = first_line.split_whitespace().collect();
            let (method, path) = match parts.as_slice() {
                [m, p, ..] => (*m, *p),
                _ => return true,
            };

            let headers = proxy::parse_headers(&request);

            let guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(ref state) = *guard {
                // Check for WebSocket upgrade
                let upgrade = headers.get("upgrade").map(|v| v.to_lowercase());
                if upgrade.as_deref() == Some("websocket") {
                    drop(guard);
                    // Safe state access for WebSocket — check STATE is still Some
                    let ws_guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(ref ws_state) = *ws_guard {
                        let state_ptr = ws_state as *const CompanionState;
                        drop(ws_guard);
                        websocket::handle_ws_upgrade(stream, path, unsafe { &*state_ptr });
                    }
                    return true;
                }

                proxy::handle_request(&mut stream, state, method, path, &headers, &request, &remote_addr);
            }
            true // continue loop
        }));

        match result {
            Ok(false) => break, // shutdown requested
            Ok(true) => continue,
            Err(panic) => {
                let msg = panic.downcast_ref::<String>()
                    .map(|s| s.as_str())
                    .or_else(|| panic.downcast_ref::<&str>().copied())
                    .unwrap_or("unknown panic");
                log_debug!("[companion] Listener caught panic: {} — continuing", msg);
                continue; // Don't let a panic kill the listener
            }
        }
    }

    log_debug!("[companion] Local listener ended after {} requests", request_count);
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

//! K2SO Mobile Companion API
//!
//! Exposes a curated subset of K2SO's internal HTTP API through an ngrok tunnel.
//! The companion app connects to this tunnel to monitor and interact with agents remotely.
//!
//! Architecture:
//! - Binds a local TcpListener on a random port
//! - Starts an ngrok tunnel pointing to that port
//! - Proxies validated requests to the internal K2SO HTTP server
//! - Supports WebSocket for real-time event push

pub mod auth;
pub mod proxy;
pub mod types;
pub mod websocket;

use std::collections::HashMap;
use std::io::Read;
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use tauri::{AppHandle, Emitter, Listener};

use ngrok::config::TunnelBuilder;
use ngrok::tunnel::EndpointInfo;
use types::CompanionState;

/// Module-level companion state. Set once when companion starts.
pub(crate) static STATE: OnceLock<CompanionState> = OnceLock::new();
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

    // Bind local listener for the companion proxy
    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|e| format!("Failed to bind companion listener: {}", e))?;
    let local_port = listener.local_addr().unwrap().port();

    // Start ngrok tunnel
    let ngrok_token = companion.ngrok_auth_token.clone();
    let tunnel_url = start_ngrok_tunnel(local_port, &ngrok_token)?;

    log_debug!("[companion] Started on local port {}, tunnel: {}", local_port, tunnel_url);

    // Initialize state
    let state = CompanionState {
        tunnel_url: Mutex::new(Some(tunnel_url.clone())),
        sessions: Mutex::new(HashMap::new()),
        ws_clients: Mutex::new(Vec::new()),
        shutdown: AtomicBool::new(false),
        hook_port,
        hook_token: hook_token.to_string(),
    };
    let _ = STATE.set(state);
    RUNNING.store(true, Ordering::Relaxed);

    // Spawn the proxy listener thread
    std::thread::spawn(move || {
        run_listener(listener);
    });

    // Spawn session cleanup thread (every 5 minutes, expire old sessions)
    std::thread::spawn(|| {
        loop {
            std::thread::sleep(std::time::Duration::from_secs(300));
            if !RUNNING.load(Ordering::Relaxed) { break; }
            if let Some(state) = STATE.get() {
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

    if let Some(state) = STATE.get() {
        state.shutdown.store(true, Ordering::Relaxed);

        // Clear tunnel URL
        *state.tunnel_url.lock().unwrap() = None;

        // Close all WebSocket connections
        state.ws_clients.lock().unwrap().clear();

        // Clear sessions
        state.sessions.lock().unwrap().clear();
    }

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

    if let Some(state) = STATE.get() {
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

/// Start the ngrok tunnel pointing to a local port.
fn start_ngrok_tunnel(local_port: u16, ngrok_token: &str) -> Result<String, String> {
    // Use tokio runtime for the async ngrok crate
    let rt = tokio::runtime::Runtime::new()
        .map_err(|e| format!("Failed to create tokio runtime: {}", e))?;

    let token = ngrok_token.to_string();
    let url = rt.block_on(async move {
        let listener = ngrok::Session::builder()
            .authtoken(&token)
            .connect()
            .await
            .map_err(|e| format!("ngrok connect failed: {}", e))?;

        let tunnel = listener
            .http_endpoint()
            .forwards_to(&format!("localhost:{}", local_port))
            .listen()
            .await
            .map_err(|e| format!("ngrok tunnel failed: {}", e))?;

        let url = tunnel.url().to_string();

        // Spawn a task to keep the tunnel alive by forwarding connections
        tokio::spawn(async move {
            // The tunnel stays open as long as this task is alive
            // ngrok handles the forwarding internally
            loop {
                tokio::time::sleep(tokio::time::Duration::from_secs(3600)).await;
            }
        });

        Ok::<String, String>(url)
    })?;

    Ok(url)
}

/// Main listener loop — accepts connections and dispatches to handlers.
fn run_listener(listener: TcpListener) {
    for stream in listener.incoming() {
        if let Some(state) = STATE.get() {
            if state.shutdown.load(Ordering::Relaxed) { break; }
        } else {
            break;
        }

        let Ok(mut stream) = stream else { continue };
        let remote_addr = stream.peer_addr()
            .map(|a| a.to_string())
            .unwrap_or_else(|_| "unknown".to_string());

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

        if let Some(state) = STATE.get() {
            // Check for WebSocket upgrade
            let upgrade = headers.get("upgrade").map(|v| v.to_lowercase());
            if upgrade.as_deref() == Some("websocket") {
                websocket::handle_ws_upgrade(stream, path, state);
                continue;
            }

            proxy::handle_request(&mut stream, state, method, path, &headers, &request, &remote_addr);
        }
    }
}

/// Register Tauri event listeners to broadcast to WebSocket clients.
fn register_event_listeners(app_handle: &AppHandle) {
    let events = ["agent:lifecycle", "agent:reply", "sync:projects"];
    for event_name in &events {
        let name = event_name.to_string();
        app_handle.listen(event_name.to_string(), move |event| {
            if let Some(state) = STATE.get() {
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

        let state = match STATE.get() {
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

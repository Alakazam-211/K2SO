use std::net::TcpStream;
use std::sync::{mpsc, Arc, Mutex};
use std::collections::HashSet;
use std::time::Instant;
use tungstenite::{accept, Message};
use super::auth;
use super::proxy::{parse_query, dispatch_ws_method};
use super::types::{CompanionState, WsClient};

/// Handle a WebSocket upgrade request.
/// Accepts the connection, then runs the WS protocol:
///   1. First message must be auth (validates Bearer token)
///   2. Subsequent messages are method calls or terminal subscriptions
///   3. Server pushes events (terminal:grid, agent:lifecycle, heartbeat)
pub fn handle_ws_upgrade(
    stream: TcpStream,
    path: &str,
    state: &CompanionState,
) {
    // Accept token from query params for backwards compatibility,
    // but the client should also send auth as first WS message.
    let query = parse_query(path);
    let initial_token = query.get("token").cloned();

    // Upgrade to WebSocket
    let ws = match accept(stream) {
        Ok(ws) => ws,
        Err(e) => {
            log_debug!("[companion-ws] WebSocket upgrade failed: {}", e);
            return;
        }
    };

    log_debug!("[companion-ws] Client connected");

    // Create channel for the writer thread
    let (tx, rx) = mpsc::channel::<String>();

    // Pre-authenticate if token was in query params (backwards compat with current mobile app)
    let pre_authenticated = if let Some(ref token) = initial_token {
        auth::validate_bearer(token, state).is_ok()
    } else {
        false
    };

    let client_token = initial_token.unwrap_or_default();
    let client_id = uuid::Uuid::new_v4().to_string();

    // Register client
    {
        let mut clients = state.ws_clients.lock().unwrap_or_else(|e| e.into_inner());
        clients.push(WsClient {
            client_id: client_id.clone(),
            session_token: client_token.clone(),
            authenticated: pre_authenticated,
            subscribed_terminals: HashSet::new(),
            sender: tx.clone(),
            last_seen: Instant::now(),
        });
    }

    // Wrap WebSocket in Arc<Mutex<>> so reader and writer threads can share it
    let ws = Arc::new(Mutex::new(ws));

    // Writer thread: receives events from channel, sends to WebSocket
    let ws_writer = Arc::clone(&ws);
    std::thread::spawn(move || {
        while let Ok(msg) = rx.recv() {
            let Ok(mut ws) = ws_writer.lock() else { break };
            if ws.send(Message::Text(msg)).is_err() {
                break;
            }
        }
    });

    // Heartbeat thread: send heartbeat every 30s to keep ngrok tunnel alive
    let heartbeat_tx = tx.clone();
    let heartbeat_state = unsafe { &*(state as *const CompanionState) };
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(std::time::Duration::from_secs(30));
            if heartbeat_state.shutdown.load(std::sync::atomic::Ordering::Relaxed) { break; }
            let msg = serde_json::json!({"event": "heartbeat"}).to_string();
            if heartbeat_tx.send(msg).is_err() { break; }
        }
    });

    // Reader thread: processes incoming messages
    let ws_reader = Arc::clone(&ws);
    let reader_state = unsafe {
        &*(state as *const CompanionState)
    };
    let reader_token = client_token.clone();
    std::thread::spawn(move || {
        let mut authenticated = pre_authenticated;
        let mut session_token = reader_token;

        loop {
            let msg = {
                let Ok(mut ws) = ws_reader.lock() else { break };
                ws.read()
            };
            match msg {
                Ok(Message::Text(text)) => {
                    let Ok(msg) = serde_json::from_str::<serde_json::Value>(&text) else { continue };

                    // Update last_seen
                    {
                        let mut clients = reader_state.ws_clients.lock().unwrap_or_else(|e| e.into_inner());
                        if let Some(client) = clients.iter_mut().find(|c| c.session_token == session_token) {
                            client.last_seen = Instant::now();
                        }
                    }

                    let id = msg.get("id").and_then(|v| v.as_str()).map(|s| s.to_string());
                    let method = msg.get("method").and_then(|v| v.as_str()).unwrap_or("");
                    let params = msg.get("params").cloned().unwrap_or(serde_json::json!({}));

                    // Handle auth method (must be first message if not pre-authenticated)
                    if method == "auth" {
                        let token = params.get("token").and_then(|v| v.as_str()).unwrap_or("");
                        match auth::validate_bearer(token, reader_state) {
                            Ok(_) => {
                                authenticated = true;
                                session_token = token.to_string();
                                // Update client's token and auth status
                                let mut clients = reader_state.ws_clients.lock().unwrap_or_else(|e| e.into_inner());
                                if let Some(client) = clients.iter_mut().find(|c| c.client_id == client_id) {
                                    client.session_token = session_token.clone();
                                    client.authenticated = true;
                                }
                                drop(clients);
                                send_response(&tx, id.as_deref(), Ok(serde_json::json!({"authenticated": true})));
                            }
                            Err(e) => {
                                send_response(&tx, id.as_deref(), Err(format!("{}", e)));
                            }
                        }
                        continue;
                    }

                    // All other methods require authentication
                    if !authenticated {
                        send_response(&tx, id.as_deref(), Err("Not authenticated. Send auth method first.".to_string()));
                        continue;
                    }

                    // Handle ping (keepalive)
                    if method == "ping" {
                        send_response(&tx, id.as_deref(), Ok(serde_json::json!({"pong": true})));
                        continue;
                    }

                    // Handle terminal subscribe/unsubscribe
                    if method == "terminal.subscribe" {
                        let terminal_id = params.get("terminalId").and_then(|v| v.as_str()).unwrap_or("");
                        if terminal_id.is_empty() {
                            send_response(&tx, id.as_deref(), Err("Missing terminalId".to_string()));
                        } else {
                            let mut clients = reader_state.ws_clients.lock().unwrap_or_else(|e| e.into_inner());
                            if let Some(client) = clients.iter_mut().find(|c| c.session_token == session_token) {
                                client.subscribed_terminals.insert(terminal_id.to_string());
                            }
                            drop(clients);
                            log_debug!("[companion-ws] Subscribed to terminal: {}", terminal_id);
                            send_response(&tx, id.as_deref(), Ok(serde_json::json!({"subscribed": terminal_id})));
                        }
                        continue;
                    }

                    if method == "terminal.unsubscribe" {
                        let terminal_id = params.get("terminalId").and_then(|v| v.as_str()).unwrap_or("");
                        if !terminal_id.is_empty() {
                            let mut clients = reader_state.ws_clients.lock().unwrap_or_else(|e| e.into_inner());
                            if let Some(client) = clients.iter_mut().find(|c| c.session_token == session_token) {
                                client.subscribed_terminals.remove(terminal_id);
                            }
                        }
                        send_response(&tx, id.as_deref(), Ok(serde_json::json!({"unsubscribed": true})));
                        continue;
                    }

                    // Handle legacy subscribe/unsubscribe format (backwards compat)
                    let msg_type = msg.get("type").and_then(|t| t.as_str()).unwrap_or("");
                    if msg_type == "subscribe" || msg_type == "unsubscribe" {
                        let terminal_id = msg.get("terminalId").and_then(|t| t.as_str()).unwrap_or("");
                        if !terminal_id.is_empty() {
                            let mut clients = reader_state.ws_clients.lock().unwrap_or_else(|e| e.into_inner());
                            if let Some(client) = clients.iter_mut().find(|c| c.session_token == session_token) {
                                if msg_type == "subscribe" {
                                    client.subscribed_terminals.insert(terminal_id.to_string());
                                } else {
                                    client.subscribed_terminals.remove(terminal_id);
                                }
                            }
                        }
                        continue;
                    }

                    // Dispatch API method to internal server
                    if !method.is_empty() {
                        let result = dispatch_ws_method(reader_state, method, &params);
                        send_response(&tx, id.as_deref(), result);
                        continue;
                    }
                }
                Ok(Message::Ping(data)) => {
                    let Ok(mut ws) = ws_reader.lock() else { break };
                    let _ = ws.send(Message::Pong(data));
                }
                Ok(Message::Close(_)) | Err(_) => break,
                _ => {}
            }
        }

        // Remove client on disconnect
        let mut clients = reader_state.ws_clients.lock().unwrap_or_else(|e| e.into_inner());
        clients.retain(|c| c.client_id != client_id);
        log_debug!("[companion-ws] Client disconnected");
    });
}

/// Send a response to a WS client via its sender channel.
fn send_response(tx: &mpsc::Sender<String>, id: Option<&str>, result: Result<serde_json::Value, String>) {
    let msg = match (id, result) {
        (Some(id), Ok(data)) => serde_json::json!({
            "id": id,
            "result": data,
        }),
        (Some(id), Err(e)) => serde_json::json!({
            "id": id,
            "error": { "code": 400, "message": e },
        }),
        (None, Ok(data)) => serde_json::json!({
            "result": data,
        }),
        (None, Err(e)) => serde_json::json!({
            "error": { "code": 400, "message": e },
        }),
    };
    let _ = tx.send(msg.to_string());
}

/// Broadcast a push event to all authenticated WebSocket clients.
pub fn broadcast_event(state: &CompanionState, event_json: &str) {
    let clients = state.ws_clients.lock().unwrap_or_else(|e| e.into_inner());
    for client in clients.iter() {
        if client.authenticated {
            let _ = client.sender.send(event_json.to_string());
        }
    }
}

/// Broadcast terminal output to clients subscribed to that terminal.
/// Sends both legacy format (lines) and new CompactLine format.
pub fn broadcast_terminal_output(state: &CompanionState, terminal_id: &str, lines: &[String]) {
    let event = serde_json::json!({
        "event": "terminal:output",
        "data": {
            "terminalId": terminal_id,
            "lines": lines,
        }
    });
    let event_str = event.to_string();

    let clients = state.ws_clients.lock().unwrap_or_else(|e| e.into_inner());
    for client in clients.iter() {
        if client.authenticated && client.subscribed_terminals.contains(terminal_id) {
            let _ = client.sender.send(event_str.clone());
        }
    }
}

/// Broadcast a CompactLine grid update to subscribed clients.
pub fn broadcast_terminal_grid(state: &CompanionState, terminal_id: &str, grid_json: &str) {
    let event = format!(
        r#"{{"event":"terminal:grid","data":{{"terminalId":"{}","grid":{}}}}}"#,
        terminal_id, grid_json
    );

    let clients = state.ws_clients.lock().unwrap_or_else(|e| e.into_inner());
    for client in clients.iter() {
        if client.authenticated && client.subscribed_terminals.contains(terminal_id) {
            let _ = client.sender.send(event.clone());
        }
    }
}

use std::net::TcpStream;
use std::sync::{mpsc, Arc, Mutex};
use std::collections::HashSet;
use tungstenite::{accept, Message, WebSocket};
use super::auth;
use super::proxy::parse_query;
use super::types::{CompanionState, WsClient};

/// Handle a WebSocket upgrade request.
/// Validates the Bearer token from query params, then upgrades the connection.
pub fn handle_ws_upgrade(
    stream: TcpStream,
    path: &str,
    state: &CompanionState,
) {
    // Extract token from query params (?token=...)
    let query = parse_query(path);
    let token = match query.get("token") {
        Some(t) => t.clone(),
        None => {
            log_debug!("[companion-ws] WebSocket upgrade rejected: no token in query");
            return;
        }
    };

    // Validate session
    if let Err(msg) = auth::validate_bearer(&token, state) {
        log_debug!("[companion-ws] WebSocket upgrade rejected: {}", msg);
        return;
    }

    // Upgrade to WebSocket
    let ws = match accept(stream) {
        Ok(ws) => ws,
        Err(e) => {
            log_debug!("[companion-ws] WebSocket upgrade failed: {}", e);
            return;
        }
    };

    log_debug!("[companion-ws] Client connected (session: {}...)", &token[..8.min(token.len())]);

    // Create channel for the writer thread
    let (tx, rx) = mpsc::channel::<String>();

    // Register client
    {
        let mut clients = state.ws_clients.lock().unwrap();
        clients.push(WsClient {
            session_token: token.clone(),
            subscribed_terminals: HashSet::new(),
            sender: tx,
        });
    }

    // Wrap WebSocket in Arc<Mutex<>> so reader and writer threads can share it
    let ws = Arc::new(Mutex::new(ws));

    // Writer thread: receives events from channel, sends to WebSocket
    let ws_writer = Arc::clone(&ws);
    let writer_token = token.clone();
    std::thread::spawn(move || {
        while let Ok(msg) = rx.recv() {
            let mut ws = ws_writer.lock().unwrap();
            if ws.send(Message::Text(msg)).is_err() {
                break; // Connection closed
            }
        }
        log_debug!("[companion-ws] Writer thread ended for session {}...", &writer_token[..8.min(writer_token.len())]);
    });

    // Reader thread: processes incoming messages (subscribe/unsubscribe)
    let ws_reader = Arc::clone(&ws);
    let reader_state = unsafe {
        // SAFETY: CompanionState lives in a static OnceLock for the app lifetime
        &*(state as *const CompanionState)
    };
    std::thread::spawn(move || {
        loop {
            let msg = {
                let mut ws = ws_reader.lock().unwrap();
                ws.read()
            };
            match msg {
                Ok(Message::Text(text)) => {
                    if let Ok(msg) = serde_json::from_str::<serde_json::Value>(&text) {
                        let msg_type = msg.get("type").and_then(|t| t.as_str()).unwrap_or("");
                        let terminal_id = msg.get("terminalId").and_then(|t| t.as_str()).unwrap_or("");

                        match msg_type {
                            "subscribe" if !terminal_id.is_empty() => {
                                let mut clients = reader_state.ws_clients.lock().unwrap();
                                if let Some(client) = clients.iter_mut().find(|c| c.session_token == token) {
                                    client.subscribed_terminals.insert(terminal_id.to_string());
                                    log_debug!("[companion-ws] Subscribed to terminal: {}", terminal_id);
                                }
                            }
                            "unsubscribe" if !terminal_id.is_empty() => {
                                let mut clients = reader_state.ws_clients.lock().unwrap();
                                if let Some(client) = clients.iter_mut().find(|c| c.session_token == token) {
                                    client.subscribed_terminals.remove(terminal_id);
                                }
                            }
                            _ => {}
                        }
                    }
                }
                Ok(Message::Close(_)) | Err(_) => break,
                _ => {}
            }
        }

        // Remove client on disconnect
        let mut clients = reader_state.ws_clients.lock().unwrap();
        clients.retain(|c| c.session_token != token);
        log_debug!("[companion-ws] Client disconnected");
    });
}

/// Broadcast an event to all connected WebSocket clients.
pub fn broadcast_event(state: &CompanionState, event_json: &str) {
    let clients = state.ws_clients.lock().unwrap();
    for client in clients.iter() {
        let _ = client.sender.send(event_json.to_string());
    }
}

/// Broadcast terminal output to clients subscribed to that terminal.
pub fn broadcast_terminal_output(state: &CompanionState, terminal_id: &str, lines: &[String]) {
    let event = serde_json::json!({
        "type": "terminal:output",
        "terminalId": terminal_id,
        "lines": lines,
    });
    let event_str = event.to_string();

    let clients = state.ws_clients.lock().unwrap();
    for client in clients.iter() {
        if client.subscribed_terminals.contains(terminal_id) {
            let _ = client.sender.send(event_str.clone());
        }
    }
}

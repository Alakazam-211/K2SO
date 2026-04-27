use std::net::TcpStream;
use std::sync::mpsc;
use std::collections::HashSet;
use std::time::Instant;
use tungstenite::{accept, Message};
use super::auth;
use super::proxy::{parse_query, dispatch_ws_method};
use super::types::{CompanionState, ReflowCacheEntry, WsClient};
use crate::terminal::grid_types::CompactLine;

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

    // Upgrade to WebSocket.
    // The stream must NOT have been read yet — tungstenite::accept reads the
    // HTTP upgrade request itself and sends the 101 Switching Protocols response.
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
        let mut clients = state.ws_clients.lock();
        clients.push(WsClient {
            client_id: client_id.clone(),
            session_token: client_token.clone(),
            authenticated: pre_authenticated,
            subscribed_terminals: HashSet::new(),
            mobile_dims: None,
            sender: tx.clone(),
            last_seen: Instant::now(),
        });
    }

    // Split WebSocket into read and write halves via the underlying TcpStream.
    // tungstenite wraps a Read+Write stream — we can't split it directly.
    // Instead, we run BOTH read and write on the SAME thread, using non-blocking
    // channel receives between blocking reads.
    let reader_state = unsafe { &*(state as *const CompanionState) };
    let reader_token = client_token.clone();

    std::thread::spawn(move || {
        let mut ws = ws;
        let mut authenticated = pre_authenticated;
        let mut session_token = reader_token;
        let mut last_heartbeat = Instant::now();

        // Set a read timeout so we can interleave writes between reads
        let _ = ws.get_ref().set_read_timeout(Some(std::time::Duration::from_millis(50)));

        loop {
            // Phase 1: Try to send any pending outbound messages (non-blocking)
            while let Ok(msg) = rx.try_recv() {
                if ws.send(Message::Text(msg)).is_err() {
                    return; // connection dead
                }
            }

            // Send heartbeat every 30s
            if last_heartbeat.elapsed() >= std::time::Duration::from_secs(30) {
                let hb = serde_json::json!({"event": "heartbeat"}).to_string();
                if ws.send(Message::Text(hb)).is_err() { return; }
                last_heartbeat = Instant::now();
            }

            // Phase 2: Try to read one incoming message (with 50ms timeout)
            match ws.read() {
                Ok(Message::Text(text)) => {
                    let Ok(msg) = serde_json::from_str::<serde_json::Value>(&text) else { continue };

                    // Update last_seen
                    {
                        let mut clients = reader_state.ws_clients.lock();
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
                                let mut clients = reader_state.ws_clients.lock();
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

                    // Logout: purge the current session and close the socket.
                    if method == "auth.revoke" {
                        use subtle::ConstantTimeEq;
                        {
                            let mut sessions = reader_state.sessions.lock();
                            let mut matched: Option<String> = None;
                            for key in sessions.keys() {
                                if key.as_bytes().ct_eq(session_token.as_bytes()).into() {
                                    matched = Some(key.clone());
                                    break;
                                }
                            }
                            if let Some(k) = matched {
                                sessions.remove(&k);
                            }
                        }
                        send_response(&tx, id.as_deref(), Ok(serde_json::json!({"revoked": true})));
                        // Tear this client down — next loop iteration will see
                        // the send-channel drop and exit cleanly.
                        let _ = ws.send(Message::Close(None));
                        break;
                    }

                    // Handle terminal subscribe/unsubscribe/resize
                    if method == "terminal.subscribe" {
                        let terminal_id = params.get("terminalId").and_then(|v| v.as_str()).unwrap_or("");
                        if terminal_id.is_empty() {
                            send_response(&tx, id.as_deref(), Err("Missing terminalId".to_string()));
                        } else {
                            // Extract optional mobile dimensions for shadow terminal reflow.
                            // Subtract 1 column as safety margin — mobile font metrics
                            // (sub-pixel rounding, webview rendering) can differ slightly
                            // from integer column math, causing the prompt line to wrap.
                            let cols = params.get("cols").and_then(|v| v.as_u64()).map(|v| v.saturating_sub(1).max(10) as u16);
                            let rows = params.get("rows").and_then(|v| v.as_u64()).map(|v| v as u16);
                            let dims = match (cols, rows) {
                                (Some(c), Some(r)) if c > 0 && r > 0 => Some((c, r)),
                                _ => None,
                            };

                            let mut clients = reader_state.ws_clients.lock();
                            if let Some(client) = clients.iter_mut().find(|c| c.client_id == client_id) {
                                client.subscribed_terminals.insert(terminal_id.to_string());
                                if dims.is_some() {
                                    client.mobile_dims = dims;
                                }
                            }
                            drop(clients);
                            log_debug!("[companion-ws] Subscribed to terminal: {} (dims: {:?})", terminal_id, dims);
                            send_response(&tx, id.as_deref(), Ok(serde_json::json!({
                                "subscribed": terminal_id,
                                "mobileDims": dims.map(|(c, r)| serde_json::json!({"cols": c, "rows": r})),
                            })));
                        }
                        continue;
                    }

                    if method == "terminal.resize" {
                        let terminal_id = params.get("terminalId").and_then(|v| v.as_str()).unwrap_or("");
                        // Same 1-column safety margin as subscribe
                        let cols = params.get("cols").and_then(|v| v.as_u64()).map(|v| v.saturating_sub(1).max(10) as u16).unwrap_or(0);
                        let rows = params.get("rows").and_then(|v| v.as_u64()).map(|v| v as u16).unwrap_or(0);
                        if terminal_id.is_empty() || cols == 0 || rows == 0 {
                            send_response(&tx, id.as_deref(), Err("Missing terminalId, cols, or rows".to_string()));
                        } else {
                            let mut clients = reader_state.ws_clients.lock();
                            if let Some(client) = clients.iter_mut().find(|c| c.client_id == client_id) {
                                client.mobile_dims = Some((cols, rows));
                            }
                            drop(clients);
                            log_debug!("[companion-ws] Terminal resize: {} → {}x{}", terminal_id, cols, rows);
                            send_response(&tx, id.as_deref(), Ok(serde_json::json!({"resized": true})));
                        }
                        continue;
                    }

                    if method == "terminal.unsubscribe" {
                        let terminal_id = params.get("terminalId").and_then(|v| v.as_str()).unwrap_or("");
                        if !terminal_id.is_empty() {
                            let mut clients = reader_state.ws_clients.lock();
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
                            let mut clients = reader_state.ws_clients.lock();
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
                        // Gate privileged spawn methods: refuse unless the
                        // operator has explicitly enabled remote spawn.
                        if super::proxy::is_privileged_spawn_method(method)
                            && !reader_state.allow_remote_spawn
                        {
                            send_response(
                                &tx,
                                id.as_deref(),
                                Err("Remote terminal spawn is disabled. Enable 'Allow remote spawn' in Companion settings and restart the tunnel.".to_string()),
                            );
                            continue;
                        }
                        let result = dispatch_ws_method(reader_state, method, &params);
                        send_response(&tx, id.as_deref(), result);
                        continue;
                    }
                }
                Ok(Message::Ping(data)) => {
                    let _ = ws.send(Message::Pong(data));
                }
                Ok(Message::Close(_)) => break,
                Err(tungstenite::Error::Io(ref e)) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    // Read timeout — loop back to send pending messages, then retry read
                    continue;
                }
                Err(_) => break,
                _ => {}
            }
        }

        // Remove client on disconnect
        let mut clients = reader_state.ws_clients.lock();
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
    let clients = state.ws_clients.lock();
    for client in clients.iter() {
        if client.authenticated {
            let _ = client.sender.send(event_json.to_string());
        }
    }
}

/// Broadcast full scrollback history to subscribed clients.
/// Fires at the same frequency as terminal:grid (~10fps during active output).
/// Enables smooth real-time streaming on mobile without request-response round-trips.
pub fn broadcast_terminal_scrollback(state: &CompanionState, terminal_id: &str, lines: &[String]) {
    let event = serde_json::json!({
        "event": "terminal:scrollback",
        "data": {
            "terminalId": terminal_id,
            "lines": lines,
            "totalLines": lines.len(),
        }
    });
    let event_str = event.to_string();

    let clients = state.ws_clients.lock();
    for client in clients.iter() {
        if client.authenticated && client.subscribed_terminals.contains(terminal_id) {
            let _ = client.sender.send(event_str.clone());
        }
    }
}

/// Broadcast terminal output to clients subscribed to that terminal.
///
/// **Retired 0.32.13.** No longer called from the poll loop — mobile clients
/// reconstruct plain text from the richer `terminal:grid` event's
/// `CompactLine.text` field. Kept on the call surface for one release cycle
/// so older clients that explicitly opt into it (via a feature flag) can
/// still receive it if needed; schedule removal in 0.33.x.
#[allow(dead_code)]
pub fn broadcast_terminal_output(state: &CompanionState, terminal_id: &str, lines: &[String]) {
    let event = serde_json::json!({
        "event": "terminal:output",
        "data": {
            "terminalId": terminal_id,
            "lines": lines,
        }
    });
    let event_str = event.to_string();

    let clients = state.ws_clients.lock();
    for client in clients.iter() {
        if client.authenticated && client.subscribed_terminals.contains(terminal_id) {
            let _ = client.sender.send(event_str.clone());
        }
    }
}

/// Broadcast a CompactLine grid update to subscribed clients.
/// If a client has mobile_dims set, the grid is reflowed to those dimensions.
///
/// Reflow is cached per `(terminal_id, (cols, rows))` keyed by grid seqno.
/// When multiple clients share the same mobile dimensions (or the next
/// tick arrives without a grid change), the expensive reflow + serialize
/// is reused instead of recomputed per-client-per-tick. Criterion shows
/// the cache-hit path is ~2,250× faster than a fresh reflow.
pub fn broadcast_terminal_grid(state: &CompanionState, terminal_id: &str, grid: &crate::terminal::grid_types::GridUpdate) {
    let _h = crate::perf_hist!("broadcast_grid");
    let clients = state.ws_clients.lock();

    // Lazily serialize the desktop (un-reflowed) JSON once per call.
    let mut desktop_json: Option<String> = None;

    for client in clients.iter() {
        if !client.authenticated || !client.subscribed_terminals.contains(terminal_id) {
            continue;
        }

        let grid_json = if let Some((cols, rows)) = client.mobile_dims {
            // Per-dimension reflow cache. Only valid while seqno matches
            // the current grid; otherwise recompute + replace.
            let cache_key = (terminal_id.to_string(), (cols, rows));
            let cached = {
                let cache = state.reflow_cache.lock();
                cache.get(&cache_key).and_then(|entry| {
                    if entry.seqno == grid.seqno && grid.seqno != 0 {
                        Some(entry.json.clone())
                    } else {
                        None
                    }
                })
            };
            if let Some(json) = cached {
                json
            } else {
                let mut reflowed = crate::terminal::reflow::reflow_grid(grid, cols, rows);

                // Read the prior frame's tally + hashes BEFORE we overwrite the
                // cache entry. First frame defaults to (0, empty); the next
                // miss will compare against this frame's hashes.
                let (prev_offset, prev_hashes) = {
                    let cache = state.reflow_cache.lock();
                    cache
                        .get(&cache_key)
                        .map(|e| (e.cumulative_offset, e.line_hashes.clone()))
                        .unwrap_or((0, Vec::new()))
                };

                let new_hashes = compute_line_hashes(&reflowed.lines);
                let scrolled_off = compute_scroll_delta(&prev_hashes, &new_hashes) as u64;
                let new_offset = prev_offset.saturating_add(scrolled_off);
                // Stamp the cumulative offset over the per-frame trim count
                // that reflow_grid put here. The companion uses this to
                // build a continuous scrollback thread:
                //   absolute_row = display_offset + ws_row.
                reflowed.display_offset = new_offset as usize;

                let json = serde_json::to_string(&reflowed).unwrap_or_default();
                // Store for subsequent clients this tick + future ticks at
                // the same seqno + dims.
                state.reflow_cache.lock().insert(
                    cache_key,
                    ReflowCacheEntry {
                        seqno: grid.seqno,
                        cumulative_offset: new_offset,
                        line_hashes: new_hashes,
                        json: json.clone(),
                    },
                );
                json
            }
        } else {
            // Desktop dimensions — no reflow, just serialize once and reuse.
            if desktop_json.is_none() {
                desktop_json = Some(serde_json::to_string(grid).unwrap_or_default());
            }
            desktop_json.clone().unwrap_or_default()
        };

        let event = format!(
            r#"{{"event":"terminal:grid","data":{{"terminalId":"{}","grid":{}}}}}"#,
            terminal_id, grid_json
        );
        let _ = client.sender.send(event);
    }
}

/// Hash a reflowed line by its visible text. Style spans don't affect
/// scroll detection — two lines that render the same text are "the same"
/// for the purposes of detecting how many rows scrolled off the top.
fn hash_line(line: &CompactLine) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    line.text.hash(&mut h);
    h.finish()
}

fn compute_line_hashes(lines: &[CompactLine]) -> Vec<u64> {
    lines.iter().map(hash_line).collect()
}

/// Count reflowed lines that scrolled off the top of the viewport since
/// the previous frame at this `(terminal, dims)`.
///
/// Uses the longest suffix-of-prev that matches a prefix-of-current. For
/// the streaming case (Claude responding, content appended at the bottom)
/// this nails the scroll amount exactly:
///
///   prev    = [a, b, c, d, e]
///   current = [c, d, e, f, g]   ← suffix [c,d,e] == prefix [c,d,e]
///   scroll  = prev.len() - 3 = 2
///
/// On no overlap (clear screen, full TUI redraw, etc.) the whole prior
/// frame is treated as "scrolled off". The cumulative offset advances by
/// `prev.len()`, which keeps `display_offset` monotonic. Fine-grained
/// in-place updates (vim, htop) aren't represented well by this model —
/// they aren't the streaming case the companion is solving for.
pub(crate) fn compute_scroll_delta(prev: &[u64], current: &[u64]) -> usize {
    if prev.is_empty() {
        return 0;
    }
    let max_n = prev.len().min(current.len());
    for n in (1..=max_n).rev() {
        if prev[prev.len() - n..] == current[..n] {
            return prev.len() - n;
        }
    }
    prev.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str) -> CompactLine {
        CompactLine { row: 0, text: text.to_string(), spans: vec![], wrapped: false }
    }

    #[test]
    fn scroll_delta_first_frame_is_zero() {
        // No prior state → nothing has scrolled yet.
        let prev: Vec<u64> = vec![];
        let current = compute_line_hashes(&[line("a"), line("b")]);
        assert_eq!(compute_scroll_delta(&prev, &current), 0);
    }

    #[test]
    fn scroll_delta_no_change_is_zero() {
        // Same content → no scroll.
        let lines = [line("a"), line("b"), line("c")];
        let prev = compute_line_hashes(&lines);
        let current = prev.clone();
        assert_eq!(compute_scroll_delta(&prev, &current), 0);
    }

    #[test]
    fn scroll_delta_pure_scroll_matches_overlap() {
        // [a,b,c,d,e] → [c,d,e,f,g] : two lines fell off the top.
        let prev = compute_line_hashes(&[line("a"), line("b"), line("c"), line("d"), line("e")]);
        let current =
            compute_line_hashes(&[line("c"), line("d"), line("e"), line("f"), line("g")]);
        assert_eq!(compute_scroll_delta(&prev, &current), 2);
    }

    #[test]
    fn scroll_delta_total_replacement_is_prev_len() {
        // No overlap (clear+redraw) → the whole prior frame scrolled off.
        let prev = compute_line_hashes(&[line("a"), line("b"), line("c")]);
        let current = compute_line_hashes(&[line("x"), line("y"), line("z")]);
        assert_eq!(compute_scroll_delta(&prev, &current), 3);
    }

    #[test]
    fn scroll_delta_appended_only_is_zero() {
        // prev is shorter than current AND is a prefix of it → nothing scrolled,
        // content was simply appended at the bottom.
        let prev = compute_line_hashes(&[line("a"), line("b"), line("c")]);
        let current =
            compute_line_hashes(&[line("a"), line("b"), line("c"), line("d"), line("e")]);
        assert_eq!(compute_scroll_delta(&prev, &current), 0);
    }

    #[test]
    fn scroll_delta_resize_down() {
        // [a,b,c,d,e] → [d,e,f] : viewport is smaller AND scrolled. Suffix
        // [d,e] of prev matches prefix [d,e] of current; 3 fell off.
        let prev = compute_line_hashes(&[line("a"), line("b"), line("c"), line("d"), line("e")]);
        let current = compute_line_hashes(&[line("d"), line("e"), line("f")]);
        assert_eq!(compute_scroll_delta(&prev, &current), 3);
    }

    #[test]
    fn scroll_delta_picks_longest_overlap() {
        // Repeating pattern admits multiple suffix-prefix matches; the
        // algorithm must pick the LONGEST (= smallest scroll).
        //
        //   prev    = [a, b, a, b, a]
        //   current = [a, b, a, b, c]
        //
        // Both [a] (n=1) and [a,b,a] (n=3) match. Picking n=3 means
        // scroll = 2; picking n=1 would mean scroll = 4. The smaller
        // scroll is the right answer (every shorter answer also works,
        // so the smallest is the conservative pick that doesn't double-
        // count lines as "scrolled off" prematurely).
        let prev = compute_line_hashes(&[line("a"), line("b"), line("a"), line("b"), line("a")]);
        let current =
            compute_line_hashes(&[line("a"), line("b"), line("a"), line("b"), line("c")]);
        assert_eq!(compute_scroll_delta(&prev, &current), 2);
    }

    #[test]
    fn hash_line_collides_only_on_text() {
        // Different style spans + wrap flag, same text → same hash.
        let a = line("hello");
        let b = CompactLine {
            row: 99,
            text: "hello".to_string(),
            spans: vec![crate::terminal::grid_types::StyleSpan {
                s: 0, e: 4, fg: Some(0xff0000), bg: None, fl: None,
            }],
            wrapped: true,
        };
        assert_eq!(hash_line(&a), hash_line(&b));
    }
}

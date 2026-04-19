//! K2SO daemon entry point.
//!
//! Launched by launchd (`~/Library/LaunchAgents/com.k2so.k2so-daemon.plist`,
//! `KeepAlive: true`), this process owns the persistent-agent runtime —
//! SQLite, the heartbeat scheduler, the companion WebSocket + ngrok tunnel,
//! the agent_hooks HTTP server — so that agents keep running while the
//! Tauri app is quit and the laptop lid is closed.
//!
//! # Tokio runtime
//!
//! The binary is async-first: a multi-thread `#[tokio::main]` runtime hosts
//! the HTTP accept loop and (as more modules migrate in) the scheduler
//! ticks, companion WS, and the daemon→Tauri event channel. Each inbound
//! connection is handled by its own `tokio::spawn` task so a slow or
//! long-lived connection (future WS upgrades, streaming responses) never
//! stalls the accept loop.
//!
//! # Scaffolding pass (0.33.0-dev)
//!
//! Binds a loopback TCP listener on a random port, writes the port +
//! freshly-generated auth token to `~/.k2so/daemon.port` and
//! `~/.k2so/daemon.token` (note: **not** `heartbeat.port` yet — that's
//! still owned by src-tauri's agent_hooks server; reconciliation comes
//! when agent_hooks migrates into core), then answers `GET /ping` with a
//! 200 OK. Enough to prove the lifecycle end-to-end (launchd spawns the
//! binary, plist is loadable, port file discoverable, token-auth path
//! exercised) without yet taking responsibility for agent state.

mod events;

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;

use k2so_core::log_debug;

use crate::events::{DaemonBroadcastSink, WireEvent, EVENT_CHANNEL_CAP};

const BANNER: &str = concat!(
    "k2so-daemon ",
    env!("CARGO_PKG_VERSION"),
    " — scaffolding build (tokio)",
);

/// Shared per-process state pulled into every connection task. Cheap to
/// clone: all fields are either `Copy`, `&'static`, or `Arc`-wrapped.
#[derive(Clone)]
struct DaemonState {
    token: Arc<String>,
    started_at: Instant,
    port: u16,
    /// Broadcast channel the daemon's `AgentHookEventSink` publishes into.
    /// Every `/events` WS subscriber takes a `Receiver` off this sender.
    event_tx: Arc<broadcast::Sender<WireEvent>>,
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    // Force a reference to k2so-core so the crate boundary is exercised
    // at build-time until we actually use it for real work.
    k2so_core::__scaffolding_marker();

    log_debug!("[daemon] {}", BANNER);

    let k2so_dir = match dirs::home_dir() {
        Some(h) => h.join(".k2so"),
        None => {
            log_debug!("[daemon] FATAL: cannot determine home directory");
            std::process::exit(2);
        }
    };
    if let Err(e) = fs::create_dir_all(&k2so_dir) {
        log_debug!("[daemon] FATAL: create ~/.k2so: {e}");
        std::process::exit(2);
    }

    // Open (or create) ~/.k2so/k2so.db and populate k2so_core's process-
    // wide shared connection. Every migrated hook handler (e.g.
    // handle_hook_complete) reads via db::shared(), so this has to run
    // before any route accepts traffic. Both the Tauri app and the
    // daemon can hold their own handles to the same file — SQLite's WAL
    // mode coordinates multi-writer access.
    if let Err(e) = k2so_core::db::init_database() {
        log_debug!("[daemon] FATAL: db::init_database: {e}");
        std::process::exit(2);
    }

    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(l) => l,
        Err(e) => {
            log_debug!("[daemon] FATAL: bind 127.0.0.1:0: {e}");
            std::process::exit(2);
        }
    };
    let port = match listener.local_addr() {
        Ok(a) => a.port(),
        Err(e) => {
            log_debug!("[daemon] FATAL: local_addr: {e}");
            std::process::exit(2);
        }
    };

    let token = generate_token();

    if let Err(e) = write_restricted(&k2so_dir.join("daemon.port"), port.to_string().as_bytes()) {
        log_debug!("[daemon] WARN: write daemon.port: {e}");
    }
    if let Err(e) = write_restricted(&k2so_dir.join("daemon.token"), token.as_bytes()) {
        log_debug!("[daemon] WARN: write daemon.token: {e}");
    }

    // Publish port + token into the shared static so the rest of core
    // (terminal, etc.) can inject them into spawned child-process envs.
    k2so_core::hook_config::set_port(port);
    k2so_core::hook_config::set_token(token.clone());

    log_debug!(
        "[daemon] Listening on 127.0.0.1:{} — port file {} token file {}",
        port,
        k2so_dir.join("daemon.port").display(),
        k2so_dir.join("daemon.token").display()
    );

    // Event broadcast channel: the daemon's AgentHookEventSink publishes
    // here; each /events subscriber takes its own Receiver.
    let (event_tx, _) = broadcast::channel::<WireEvent>(EVENT_CHANNEL_CAP);
    let event_tx = Arc::new(event_tx);
    k2so_core::agent_hooks::set_sink(Box::new(DaemonBroadcastSink::new((*event_tx).clone())));

    let state = DaemonState {
        token: Arc::new(token),
        started_at: Instant::now(),
        port,
        event_tx,
    };

    // Graceful-shutdown channel. launchd sends SIGTERM on system shutdown
    // or `launchctl unload`; Ctrl+C is the local-dev path. Both land on
    // the same broadcast so in-flight handlers get a chance to flush.
    let (shutdown_tx, _shutdown_rx) = broadcast::channel::<()>(1);
    let shutdown_tx_for_signal = shutdown_tx.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            log_debug!("[daemon] Ctrl+C received, shutting down");
            let _ = shutdown_tx_for_signal.send(());
        }
    });

    let mut shutdown_rx = shutdown_tx.subscribe();
    loop {
        tokio::select! {
            res = listener.accept() => {
                match res {
                    Ok((stream, _addr)) => {
                        let st = state.clone();
                        let mut shutdown = shutdown_tx.subscribe();
                        tokio::spawn(async move {
                            tokio::select! {
                                _ = handle_connection(stream, st) => {}
                                _ = shutdown.recv() => {}
                            }
                        });
                    }
                    Err(e) => {
                        log_debug!("[daemon] accept error: {e}");
                    }
                }
            }
            _ = shutdown_rx.recv() => {
                log_debug!("[daemon] accept loop exiting");
                break;
            }
        }
    }
}

/// Serve one connection. On any IO error or malformed request we drop the
/// socket — every response also sets `Connection: close` so callers don't
/// reuse the socket.
///
/// `/events` is the one exception: on a valid token we hand the raw
/// [`TcpStream`] off to [`events::serve_events_connection`] which performs
/// the WebSocket upgrade via `tokio_tungstenite::accept_async` — that
/// function consumes the handshake bytes itself, so we DO NOT read the
/// request body here for that route.
async fn handle_connection(mut stream: TcpStream, state: DaemonState) {
    // Peek just the request line + headers so we can route on path
    // without consuming the body. Enough for WS handshakes (which
    // tokio-tungstenite will re-read) and the small GET bodies (which
    // have no body).
    let mut buf = [0u8; 4096];
    let n = match stream.peek(&mut buf).await {
        Ok(n) if n > 0 => n,
        _ => return,
    };
    let req = String::from_utf8_lossy(&buf[..n]);

    let first_line = req.lines().next().unwrap_or("");
    let parts: Vec<&str> = first_line.split_whitespace().collect();
    let (method, path_and_query) = match parts.as_slice() {
        [m, p, ..] => (*m, *p),
        _ => {
            // Consume what we peeked so the error gets delivered.
            let _ = stream.read(&mut buf).await;
            send_response(&mut stream, "400 Bad Request", "text/plain", "bad request\n").await;
            return;
        }
    };

    if method != "GET" {
        let _ = stream.read(&mut buf).await;
        send_response(
            &mut stream,
            "405 Method Not Allowed",
            "application/json",
            r#"{"error":"only GET is supported"}"#,
        )
        .await;
        return;
    }

    let (path, query) = match path_and_query.split_once('?') {
        Some((p, q)) => (p, q),
        None => (path_and_query, ""),
    };
    // Copy out of the lossy Cow so we can consume the read buffer below
    // without extending the immutable borrow.
    let path = path.to_string();
    let query = query.to_string();
    drop(req);

    match path.as_str() {
        "/ping" => {
            let _ = stream.read(&mut buf).await;
            // Unauthenticated. Smallest liveness check.
            send_response(&mut stream, "200 OK", "text/plain; charset=utf-8", BANNER).await;
        }
        "/status" => {
            let _ = stream.read(&mut buf).await;
            // Token-gated. Returns a small JSON blob describing the
            // daemon's state so the Tauri app can verify it's talking to
            // the right process.
            if !token_ok(&query, state.token.as_str()) {
                send_response(
                    &mut stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return;
            }
            let uptime_secs = state.started_at.elapsed().as_secs();
            let pid = std::process::id();
            let body = format!(
                r#"{{"version":"{}","uptime_secs":{},"pid":{},"port":{}}}"#,
                env!("CARGO_PKG_VERSION"),
                uptime_secs,
                pid,
                state.port,
            );
            send_response(&mut stream, "200 OK", "application/json", &body).await;
        }
        "/hook/complete" => {
            // Agent-lifecycle hook endpoint. URL-encoded query params
            // carry paneId / tabId / eventType / token. Business logic
            // (ring buffer, emit, AgentSession.status sync) lives in
            // k2so_core so src-tauri's existing server hits the same
            // code path.
            let _ = stream.read(&mut buf).await;
            let params = parse_params(&path, &query);
            let req_token = params.get("token").cloned().unwrap_or_default();
            if req_token != *state.token {
                send_response(
                    &mut stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"Invalid or missing auth token"}"#,
                )
                .await;
                return;
            }
            let body = k2so_core::agent_hooks::handle_hook_complete(&params);
            send_response(&mut stream, "200 OK", "application/json", body).await;
        }
        // Heartbeat CRUD + fire-audit — fires while the Tauri app is
        // quit, so the daemon has to own these routes for the
        // persistent-agents feature to mean anything.
        // Scheduler tick: decides which agents are ready to wake.
        // Returns the ordered list of agent names. PTY spawning + wake
        // prompt assembly is still caller-side (future daemon commit)
        // but the core decision path already lives in
        // k2so_core::agents::scheduler.
        "/cli/scheduler-tick" => {
            let _ = stream.read(&mut buf).await;
            let params = parse_params(&path, &query);
            let req_token = params.get("token").cloned().unwrap_or_default();
            if req_token != *state.token {
                send_response(
                    &mut stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"Invalid or missing auth token"}"#,
                )
                .await;
                return;
            }
            let project_path = match params.get("project_path") {
                Some(p) if !p.is_empty() => p.clone(),
                _ => {
                    send_response(
                        &mut stream,
                        "400 Bad Request",
                        "application/json",
                        r#"{"error":"Missing project_path"}"#,
                    )
                    .await;
                    return;
                }
            };
            match k2so_core::agents::scheduler::k2so_agents_scheduler_tick(project_path) {
                Ok(agents) => {
                    let body = serde_json::to_string(&agents).unwrap_or_else(|_| "[]".to_string());
                    send_response(&mut stream, "200 OK", "application/json", &body).await
                }
                Err(e) => {
                    let body = serde_json::json!({"error": e}).to_string();
                    send_response(&mut stream, "400 Bad Request", "application/json", &body)
                        .await
                }
            }
        }
        p if p.starts_with("/cli/heartbeat/") || p == "/cli/heartbeat-log" => {
            let _ = stream.read(&mut buf).await;
            let params = parse_params(&path, &query);
            let req_token = params.get("token").cloned().unwrap_or_default();
            if req_token != *state.token {
                send_response(
                    &mut stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"Invalid or missing auth token"}"#,
                )
                .await;
                return;
            }
            let project_path = match params.get("project_path") {
                Some(p) if !p.is_empty() => p.clone(),
                _ => {
                    send_response(
                        &mut stream,
                        "400 Bad Request",
                        "application/json",
                        r#"{"error":"Missing project_path"}"#,
                    )
                    .await;
                    return;
                }
            };
            let result = if p == "/cli/heartbeat-log" {
                handle_cli_heartbeat_log(&project_path, &params)
            } else {
                handle_cli_heartbeat(p, &project_path, &params)
            };
            match result {
                Ok(body) => {
                    send_response(&mut stream, "200 OK", "application/json", &body).await
                }
                Err(msg) => {
                    let body = serde_json::json!({"error": msg}).to_string();
                    send_response(&mut stream, "400 Bad Request", "application/json", &body)
                        .await
                }
            }
        }
        "/events" => {
            // Token check BEFORE the upgrade so unauthenticated clients
            // see an HTTP 403 instead of a dangling WS close.
            if !token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                send_response(
                    &mut stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return;
            }
            // Hand off to tokio-tungstenite; the handshake is still
            // unread in the stream buffer.
            events::serve_events_connection(stream, state.event_tx.clone()).await;
        }
        _ => {
            let _ = stream.read(&mut buf).await;
            send_response(&mut stream, "404 Not Found", "text/plain", "not found\n").await;
        }
    }
}

/// Reassemble a full `path?query` URL and hand off to k2so_core's
/// URL-decoding query parser. The core helper knows how to unescape
/// `%20`/`+` and multi-byte UTF-8 — we just combine the pieces.
fn parse_params(
    path: &str,
    query: &str,
) -> std::collections::HashMap<String, String> {
    let path_and_query = if query.is_empty() {
        path.to_string()
    } else {
        format!("{}?{}", path, query)
    };
    k2so_core::agent_hooks::parse_query_params(&path_and_query)
}

/// Dispatch an authenticated `/cli/heartbeat/*` request to the matching
/// core function. Returns the JSON response body on success or an error
/// message the caller turns into a 400.
///
/// Mirrors the dispatch shape in src-tauri's agent_hooks server so the
/// CLI sees identical responses regardless of which process is
/// listening. Unrecognized sub-paths return 404 (caller picks the
/// status code from the string).
fn handle_cli_heartbeat(
    path: &str,
    project_path: &str,
    params: &std::collections::HashMap<String, String>,
) -> Result<String, String> {
    use k2so_core::agents::heartbeat as hb;

    match path {
        "/cli/heartbeat/add" => {
            let name = params.get("name").cloned().unwrap_or_default();
            let frequency = params.get("frequency").cloned().unwrap_or_default();
            let spec_json = params
                .get("spec")
                .cloned()
                .unwrap_or_else(|| "{}".to_string());
            if name.is_empty() || frequency.is_empty() {
                return Err("Missing 'name' or 'frequency' parameter".to_string());
            }
            hb::k2so_heartbeat_add(project_path.to_string(), name, frequency, spec_json)
                .map(|v| v.to_string())
        }
        "/cli/heartbeat/list" => hb::k2so_heartbeat_list(project_path.to_string())
            .map(|rows| serde_json::to_string(&rows).unwrap_or_default()),
        "/cli/heartbeat/remove" => {
            let name = params.get("name").cloned().unwrap_or_default();
            if name.is_empty() {
                return Err("Missing 'name' parameter".to_string());
            }
            hb::k2so_heartbeat_remove(project_path.to_string(), name)
                .map(|_| r#"{"success":true}"#.to_string())
        }
        "/cli/heartbeat/enable" => {
            let name = params.get("name").cloned().unwrap_or_default();
            let enabled = params
                .get("enabled")
                .map(|v| v == "true" || v == "1")
                .unwrap_or(true);
            if name.is_empty() {
                return Err("Missing 'name' parameter".to_string());
            }
            hb::k2so_heartbeat_set_enabled(project_path.to_string(), name, enabled)
                .map(|_| r#"{"success":true}"#.to_string())
        }
        "/cli/heartbeat/edit" => {
            let name = params.get("name").cloned().unwrap_or_default();
            let frequency = params.get("frequency").cloned().unwrap_or_default();
            let spec_json = params.get("spec").cloned().unwrap_or_default();
            if name.is_empty() || frequency.is_empty() {
                return Err("Missing 'name' or 'frequency' parameter".to_string());
            }
            hb::k2so_heartbeat_edit(project_path.to_string(), name, frequency, spec_json)
                .map(|_| r#"{"success":true}"#.to_string())
        }
        "/cli/heartbeat/rename" => {
            let old_name = params.get("from").cloned().unwrap_or_default();
            let new_name = params.get("to").cloned().unwrap_or_default();
            if old_name.is_empty() || new_name.is_empty() {
                return Err("Missing 'from' or 'to' parameter".to_string());
            }
            hb::k2so_heartbeat_rename(project_path.to_string(), old_name, new_name)
                .map(|_| r#"{"success":true}"#.to_string())
        }
        "/cli/heartbeat/status" => {
            // Last N fires for a specific schedule name.
            let name = params.get("name").cloned().unwrap_or_default();
            let limit = params
                .get("limit")
                .and_then(|s| s.parse::<i64>().ok())
                .unwrap_or(10)
                .clamp(1, 200);
            if name.is_empty() {
                return Err("Missing 'name' parameter".to_string());
            }
            let db = k2so_core::db::shared();
            let conn = db.lock();
            let project_id = k2so_core::agents::resolve_project_id(&conn, project_path)
                .ok_or_else(|| format!("Project not found: {project_path}"))?;
            k2so_core::db::schema::HeartbeatFire::list_by_schedule_name(
                &conn,
                &project_id,
                &name,
                limit,
            )
            .map(|rows| serde_json::to_string(&rows).unwrap_or_default())
            .map_err(|e| e.to_string())
        }
        "/cli/heartbeat/fires-list" => {
            // Recent fires for the whole project. Powers the Settings
            // History panel. Migrated alongside the rest of the
            // heartbeat CRUD so the daemon serves the same surface
            // src-tauri did.
            let limit = params
                .get("limit")
                .and_then(|s| s.parse::<i64>().ok());
            hb::k2so_heartbeat_fires_list(project_path.to_string(), limit)
                .map(|rows| serde_json::to_string(&rows).unwrap_or_default())
        }
        _ => Err(format!("Unknown heartbeat route: {path}")),
    }
}

/// Dispatch `/cli/heartbeat-log` (the "all recent fires" diagnostic
/// route). Same pattern as handle_cli_heartbeat but factored out
/// because the URL sits at /cli/heartbeat-log, not under /cli/heartbeat/.
fn handle_cli_heartbeat_log(
    project_path: &str,
    params: &std::collections::HashMap<String, String>,
) -> Result<String, String> {
    let limit = params
        .get("limit")
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(50)
        .clamp(1, 500);
    let db = k2so_core::db::shared();
    let conn = db.lock();
    let project_id = k2so_core::agents::resolve_project_id(&conn, project_path)
        .ok_or_else(|| format!("Project not found: {project_path}"))?;
    k2so_core::db::schema::HeartbeatFire::list_by_project(&conn, &project_id, limit)
        .map(|rows| serde_json::to_string(&rows).unwrap_or_default())
        .map_err(|e| e.to_string())
}

/// Parse `token=<value>` out of a URL-encoded query string and compare
/// against the expected value. No full urlencoded decoding — the token
/// is always 32 hex chars so there's nothing to decode.
fn token_ok(query: &str, expected: &str) -> bool {
    for pair in query.split('&') {
        if let Some(v) = pair.strip_prefix("token=") {
            return v == expected;
        }
    }
    false
}

async fn send_response(stream: &mut TcpStream, status: &str, ct: &str, body: &str) {
    let resp = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {ct}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body,
    );
    let _ = stream.write_all(resp.as_bytes()).await;
}

/// Write `contents` to `path` with permissions 0600 so other users on the
/// same machine can't read the auth token or port.
fn write_restricted(path: &PathBuf, contents: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(contents)?;
    Ok(())
}

/// 32-hex-char cryptographically random token. Same shape as the
/// agent_hooks server's `generate_token` so a future unification is a
/// trivial move.
fn generate_token() -> String {
    let mut buf = [0u8; 16];
    getrandom::getrandom(&mut buf).expect("getrandom failed");
    buf.iter().map(|b| format!("{:02x}", b)).collect()
}

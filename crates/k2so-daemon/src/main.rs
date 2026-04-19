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

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;

use k2so_core::log_debug;

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

    let state = DaemonState {
        token: Arc::new(token),
        started_at: Instant::now(),
        port,
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
async fn handle_connection(mut stream: TcpStream, state: DaemonState) {
    let mut buf = [0u8; 4096];
    let n = match stream.read(&mut buf).await {
        Ok(n) if n > 0 => n,
        _ => return,
    };
    let req = String::from_utf8_lossy(&buf[..n]);

    let first_line = req.lines().next().unwrap_or("");
    let parts: Vec<&str> = first_line.split_whitespace().collect();
    let (method, path_and_query) = match parts.as_slice() {
        [m, p, ..] => (*m, *p),
        _ => {
            send_response(&mut stream, "400 Bad Request", "text/plain", "bad request\n").await;
            return;
        }
    };

    if method != "GET" {
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

    match path {
        "/ping" => {
            // Unauthenticated. Smallest liveness check.
            send_response(&mut stream, "200 OK", "text/plain; charset=utf-8", BANNER).await;
        }
        "/status" => {
            // Token-gated. Returns a small JSON blob describing the
            // daemon's state so the Tauri app can verify it's talking to
            // the right process.
            if !token_ok(query, state.token.as_str()) {
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
        _ => {
            send_response(&mut stream, "404 Not Found", "text/plain", "not found\n").await;
        }
    }
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

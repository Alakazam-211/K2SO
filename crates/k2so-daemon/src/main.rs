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
//! Binds a loopback TCP listener on a random port and publishes the
//! port + freshly-generated auth token through four filesystem
//! channels:
//!
//! - `~/.k2so/daemon.port` / `~/.k2so/daemon.token` — daemon-specific
//!   addresses used by Tauri's `DaemonClient` and by
//!   `k2so daemon status` to reach the daemon's control-plane
//!   endpoints regardless of who owns the CLI-facing HTTP surface.
//! - `~/.k2so/heartbeat.port` / `~/.k2so/heartbeat.token` — **the**
//!   CLI-facing surface since Phase 4 H7. Pre-Phase-4 this file was
//!   owned by Tauri's agent_hooks HTTP server; H7 retires that
//!   listener and makes the daemon the sole writer. The CLI
//!   (`cli/k2so`) + every filesystem hook script reads these files
//!   to discover the server on every request, so a daemon restart
//!   (which rotates the random port) propagates instantly without
//!   any running consumer needing to be restarted itself.

mod agents_routes;
mod awareness_ws;
mod cli;
mod cli_response;
mod companion_routes;
mod events;
mod heartbeat_launch;
mod pending_live;
mod providers;
mod session_lookup;
mod session_map;
mod sessions_bytes_ws;
mod sessions_grid_ws;
mod sessions_ws;
mod signal_format;
mod spawn;
mod terminal_routes;
mod triage;
mod v2_session_map;
mod v2_spawn;
mod wake_headless;
mod watchdog;
mod workspace_msg;

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

    // launchd hands us a sparse PATH; enrich from the user's login shell
    // BEFORE anything else, so child posix_spawn calls (alacritty's
    // tty::new for v2 sessions, plus any Command::new in handlers) can
    // resolve user-installed tools like `claude`, `cursor-agent`,
    // homebrew binaries, etc. See docs in k2so_core::enrich_path_from_login_shell.
    #[cfg(unix)]
    k2so_core::enrich_path_from_login_shell();

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

    // P5.3: clear stale heartbeat leases left behind by a daemon that
    // crashed mid-spawn. Without this, a row's `in_flight_started_at`
    // would stay set forever under `concurrency_policy='forbid'` and
    // the heartbeat would never fire again. River + Oban use the same
    // boot-sweep pattern. Threshold matches the largest reasonable
    // active_deadline_secs (5 min) — anything older than that is
    // definitely an abandoned lease, not an in-progress spawn.
    {
        let db = k2so_core::db::shared();
        let conn = db.lock();
        match k2so_core::db::schema::AgentHeartbeat::sweep_stale_leases(&conn, 300) {
            Ok(0) => {}
            Ok(n) => log_debug!("[daemon] swept {} stale heartbeat lease(s) from prior crash", n),
            Err(e) => log_debug!("[daemon] WARN: sweep_stale_leases: {e}"),
        }
    }

    // 0036: clear `active_terminal_id` for any heartbeat whose pointed-at
    // PTY died with the daemon. After a daemon restart, `v2_session_map`
    // is empty until rehydrated, so any non-NULL `active_terminal_id` is
    // by definition pointing at a corpse. Lazy cleanup on read also
    // catches stragglers, but doing the sweep on boot keeps the column
    // honest from the start. Companion of `heartbeat-active-session`
    // PRD's PtyExited observer.
    {
        let db = k2so_core::db::shared();
        let conn = db.lock();
        match k2so_core::db::schema::AgentHeartbeat::list_with_active_terminal(&conn) {
            Ok(rows) => {
                let mut cleared = 0usize;
                for (_pid, _name, term_id) in &rows {
                    // No PTYs exist yet at this point in boot, so every
                    // row is stale — null it.
                    if let Ok(n) = k2so_core::db::schema::AgentHeartbeat::clear_active_terminal_id_by_terminal(&conn, term_id) {
                        cleared += n;
                    }
                }
                if cleared > 0 {
                    log_debug!(
                        "[daemon] swept {} stale active_terminal_id(s) from prior daemon",
                        cleared
                    );
                }
            }
            Err(e) => log_debug!("[daemon] WARN: list_with_active_terminal: {e}"),
        }
    }

    // P5.6: legacy heartbeat-projects.txt has been retired in favor
    // of `/cli/heartbeat/active-projects`. If it's still on disk from
    // a pre-P5 install (or if the user only ever runs the daemon
    // headlessly without Tauri's `k2so_agents_install_heartbeat`),
    // delete it so heartbeat.sh can't be tempted to read stale data
    // even if a stray pre-P5 script is still around.
    let legacy_projects_file = k2so_dir.join("heartbeat-projects.txt");
    if legacy_projects_file.exists() {
        match fs::remove_file(&legacy_projects_file) {
            Ok(_) => log_debug!("[daemon] removed legacy heartbeat-projects.txt"),
            Err(e) => log_debug!("[daemon] WARN: remove heartbeat-projects.txt: {e}"),
        }
    }

    // 0.37.0 workspace–agent unification migration. Per-workspace,
    // sentinel-gated, idempotent. Runs synchronously before the
    // listener accepts traffic so route handlers always see the
    // unified layout. See `.k2so/prds/workspace-agent-unification.md`
    // and `k2so_core::agents::unification`.
    run_workspace_unification_sweep();

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

    // Daemon-specific port/token files (read by Tauri's daemon_client
    // for its internal HTTP client and by `k2so daemon status`). These
    // have existed since 0.33.0 and are intentionally separate from
    // heartbeat.port to avoid clashes while the Tauri agent_hooks
    // listener coexisted.
    if let Err(e) = write_restricted(&k2so_dir.join("daemon.port"), port.to_string().as_bytes()) {
        log_debug!("[daemon] WARN: write daemon.port: {e}");
    }
    if let Err(e) = write_restricted(&k2so_dir.join("daemon.token"), token.as_bytes()) {
        log_debug!("[daemon] WARN: write daemon.token: {e}");
    }

    // H7: eager claim of heartbeat.port + heartbeat.token. Before
    // Phase 4, Tauri's agent_hooks HTTP server was the primary owner
    // of these files; the daemon only took over via the 2-second-
    // delayed `run_heartbeat_port_watchdog` when Tauri wasn't
    // running. H7 flips that around: the daemon owns heartbeat.port
    // unconditionally at startup, and Tauri stops binding its own
    // listener. The CLI (`cli/k2so`) + every launchd hook script
    // read these files to discover the sole HTTP server.
    if let Err(e) = write_restricted(&k2so_dir.join("heartbeat.port"), port.to_string().as_bytes()) {
        log_debug!("[daemon] WARN: write heartbeat.port: {e}");
    }
    if let Err(e) = write_restricted(&k2so_dir.join("heartbeat.token"), token.as_bytes()) {
        log_debug!("[daemon] WARN: write heartbeat.token: {e}");
    }

    // Publish port + token into the shared static so the rest of core
    // (terminal, etc.) can inject them into spawned child-process envs.
    k2so_core::hook_config::set_port(port);
    k2so_core::hook_config::set_token(token.clone());

    log_debug!(
        "[daemon] Listening on 127.0.0.1:{} — daemon.{{port,token}} + heartbeat.{{port,token}} published to {}",
        port,
        k2so_dir.display()
    );

    // Event broadcast channel: the daemon's AgentHookEventSink publishes
    // here; each /events subscriber takes its own Receiver.
    let (event_tx, _) = broadcast::channel::<WireEvent>(EVENT_CHANNEL_CAP);
    let event_tx = Arc::new(event_tx);
    k2so_core::agent_hooks::set_sink(Box::new(DaemonBroadcastSink::new((*event_tx).clone())));

    // 0.34.0 Phase 3.1 — register the daemon-side InjectProvider +
    // WakeProvider so awareness::egress can actually reach live
    // sessions. Before this, signals to live targets landed in the
    // bus + activity_feed but never in the target's PTY.
    providers::register_all();

    // Phase 3.1 F3 — boot-time pending-live replay. Previous
    // daemon-run may have queued signals for offline agents that
    // never got injected (daemon crashed before the session came
    // online). Log them so operators can eyeball the queue; the
    // signals stay on disk until a session spawns for that agent
    // and drains them.
    let pending_summary = pending_live::replay_all();
    for (agent, sigs) in &pending_summary {
        log_debug!(
            "[daemon/boot] {} pending-live signals queued for agent {} (will deliver on next spawn)",
            sigs.len(),
            agent
        );
        // Re-enqueue so the next spawn's drain path finds them —
        // `replay_all` deletes on read, so we need to put them
        // back for the spawn-time drain to pick up. Tests cover
        // this round-trip.
        for sig in sigs {
            let _ = pending_live::enqueue(sig, agent);
        }
    }

    // Phase 3.2 G1 — harness watchdog. Tails session_map + the
    // session registry, logs + emits watchdog SemanticEvent frames
    // when sessions go idle past configured thresholds, and
    // escalates to Ctrl-C / SIGKILL. Config is read from env vars
    // (K2SO_WATCHDOG_*); set K2SO_WATCHDOG_DISABLED=1 to turn it
    // off entirely. See `watchdog::config_from_env` for the
    // defaults.
    let _watchdog_handle = watchdog::spawn(watchdog::config_from_env());

    // heartbeat.port watchdog — see `run_heartbeat_port_watchdog` docs.
    // The daemon takes over `~/.k2so/heartbeat.port` whenever Tauri
    // isn't writing to it, so the CLI and launchd-triggered heartbeat
    // script always find a reachable server.
    {
        let k2so_dir = k2so_dir.clone();
        let token = token.clone();
        tokio::spawn(async move {
            run_heartbeat_port_watchdog(k2so_dir, port, token).await;
        });
    }

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

    // Phase 4.5: handle CORS preflight before the method allowlist.
    // The Tauri WebView origin (tauri://localhost or http://localhost:5173
    // in dev) is cross-origin relative to http://127.0.0.1:<port>, so
    // the browser sends an OPTIONS preflight before every POST. We
    // answer it with permissive CORS headers — token auth still
    // gates every real request, so `Access-Control-Allow-Origin: *`
    // adds no security risk and avoids hard-coding every possible
    // Tauri dev-server port.
    if method == "OPTIONS" {
        let _ = stream.read(&mut buf).await;
        send_cors_preflight(&mut stream).await;
        return;
    }

    // Most routes are GET. Specific POST-accepting routes are
    // allowlisted here so non-GET hits other paths get a clean 405.
    let is_post = method == "POST";
    let post_allowed = matches!(
        path_and_query.split_once('?').map(|(p, _)| p).unwrap_or(path_and_query),
        "/cli/awareness/publish"
            | "/cli/sessions/spawn"
            | "/cli/sessions/close"
            | "/cli/sessions/v2/spawn"
            | "/cli/sessions/v2/close"
    );
    if method != "GET" && !(is_post && post_allowed) {
        let _ = stream.read(&mut buf).await;
        send_response(
            &mut stream,
            "405 Method Not Allowed",
            "application/json",
            r#"{"error":"method not allowed for this route"}"#,
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
        "/health" => {
            // Unauthenticated liveness probe the behavior test suite
            // polls before it does anything. Mirrors the body shape
            // src-tauri's agent_hooks server returns so tests can talk
            // to either process without branching.
            let _ = stream.read(&mut buf).await;
            send_response(
                &mut stream,
                "200 OK",
                "application/json",
                r#"{"status":"ok"}"#,
            )
            .await;
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
            // (ring buffer, emit, WorkspaceSession.status sync) lives in
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
        // Session Stream WS subscribe endpoint (0.34.0 Phase 2).
        // Lives on a /cli/ path but routes to the WS handler rather
        // than cli::dispatch because it's an HTTP upgrade, not a
        // JSON request. Branch must precede the generic /cli/
        // catchall below or the dispatch would swallow it.
        "/cli/sessions/subscribe" => {
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
            let params = parse_params(&path, &query);
            sessions_ws::serve_session_subscribe_connection(stream, params).await;
        }
        // Canvas Plan Phase 2: raw-byte stream subscribe. Parallel
        // to /cli/sessions/subscribe but streams PTY bytes as
        // binary WS frames for clients running their own vte.
        "/cli/sessions/bytes" => {
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
            let params = parse_params(&path, &query);
            sessions_bytes_ws::serve_session_bytes_connection(stream, params).await;
        }
        // Alacritty_v2 (A3): grid snapshot + delta WS endpoint.
        // Serves one Tauri thin client per session. Single-subscriber
        // by design. See `.k2so/prds/alacritty-v2.md`.
        "/cli/sessions/grid" => {
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
            let params = parse_params(&path, &query);
            sessions_grid_ws::serve_session_grid_connection(stream, params).await;
        }
        // Awareness Bus endpoints (0.34.0 Phase 3).
        // `/cli/awareness/publish` — POST JSON body → egress::deliver
        // `/cli/awareness/subscribe` — WS, streams bus signals out
        "/cli/awareness/publish" => {
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
            let body_bytes = read_post_body(&mut stream, &mut buf).await;
            let result = awareness_ws::handle_publish(&body_bytes);
            send_response(&mut stream, result.status, "application/json", &result.body)
                .await;
        }
        "/cli/awareness/subscribe" => {
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
            awareness_ws::serve_awareness_subscribe_connection(stream).await;
        }
        // POST /cli/sessions/spawn — daemon-side session spawn
        // (Phase 3.1 F2). External callers send a JSON SpawnRequest;
        // daemon spawns the session, registers it in session_map
        // keyed by agent_name, returns {sessionId, agentName}.
        "/cli/sessions/spawn" => {
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
            let body_bytes = read_post_body(&mut stream, &mut buf).await;
            let result = awareness_ws::handle_sessions_spawn(&body_bytes).await;
            send_response(&mut stream, result.status, "application/json", &result.body)
                .await;
        }
        // POST /cli/sessions/close — frontend calls this on tab
        // unmount. Removes from session_map; Arc drop → child kill
        // + PTY master FD close. Without this, every Cmd+T leaks an
        // FD and ~14 spawns hit the per-process limit.
        "/cli/sessions/close" => {
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
            let body_bytes = read_post_body(&mut stream, &mut buf).await;
            let result = awareness_ws::handle_sessions_close(&body_bytes);
            send_response(&mut stream, result.status, "application/json", &result.body)
                .await;
        }
        // POST /cli/sessions/v2/spawn — Alacritty_v2 find-or-spawn
        // (A4). Parallel to /cli/sessions/spawn but produces a
        // DaemonPtySession (registered in v2_session_map) instead
        // of a SessionStreamSession. Idempotent on agent_name: same
        // name → same session, suitable for remount reattach.
        "/cli/sessions/v2/spawn" => {
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
            let body_bytes = read_post_body(&mut stream, &mut buf).await;
            let result = v2_spawn::handle_v2_spawn(&body_bytes);
            send_response(&mut stream, result.status, "application/json", &result.body)
                .await;
        }
        // POST /cli/sessions/v2/close — explicit teardown of a v2
        // session. Called only from `tabs.ts::removeTab` (A6).
        "/cli/sessions/v2/close" => {
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
            let body_bytes = read_post_body(&mut stream, &mut buf).await;
            let result = v2_spawn::handle_v2_close(&body_bytes);
            send_response(&mut stream, result.status, "application/json", &result.body)
                .await;
        }
        // Unified /cli/* dispatch. Auth + param validation +
        // per-route handler all live in `cli::dispatch`; main.rs
        // just translates the CliResponse into bytes.
        p if p.starts_with("/cli/") => {
            let _ = stream.read(&mut buf).await;
            let params = parse_params(&path, &query);
            let req_token = params.get("token").cloned().unwrap_or_default();
            if req_token != *state.token {
                let r = cli::CliResponse::forbidden();
                send_response(&mut stream, r.status, r.content_type, &r.body).await;
                return;
            }
            let resp = cli::dispatch(p, &params);
            send_response(&mut stream, resp.status, resp.content_type, &resp.body).await;
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

/// Re-claim `~/.k2so/heartbeat.port` if something else has stomped it.
///
/// As of Phase 4 H7 the daemon is the **sole** writer of this file:
/// it's written eagerly during `main()` startup (alongside
/// daemon.port/daemon.token). Before H7 the Tauri agent_hooks server
/// owned it, and this watchdog existed to fill the gap when Tauri
/// wasn't running.
///
/// Post-H7 the watchdog is a pure safety net — its job is to restore
/// the file if an external process deletes it (disk cleanup, a stale
/// Tauri build that didn't get the H7 patch, user `rm`). Every
/// `INTERVAL_SECS` seconds it:
///
/// 1. Reads `heartbeat.port`. Missing? → re-write own port + token.
/// 2. Parses the port, tries a TCP connect to 127.0.0.1:<that_port>.
///    - Connect succeeds → a server holds the port (should be us;
///      if something else took it, we can't take back without
///      restarting). Leave alone.
///    - Connect fails → stale file, we've lost the bind for some
///      reason. Re-claim.
///
/// The 2-second startup delay avoids redundant writes with the eager
/// startup write path — we've already staked our claim before any
/// other process could.
async fn run_heartbeat_port_watchdog(
    k2so_dir: PathBuf,
    own_port: u16,
    own_token: String,
) {
    use tokio::net::TcpStream as TokioTcpStream;
    use tokio::time::{sleep, timeout, Duration};

    // Startup delay lets Tauri's own port-write land first if both
    // came up at roughly the same moment — avoids a write race where
    // the daemon's first-pass write beats Tauri's by milliseconds.
    sleep(Duration::from_secs(2)).await;

    const INTERVAL_SECS: u64 = 30;
    const CONNECT_TIMEOUT_MS: u64 = 500;
    let port_path = k2so_dir.join("heartbeat.port");
    let token_path = k2so_dir.join("heartbeat.token");

    loop {
        let claim = match fs::read_to_string(&port_path) {
            Ok(contents) => match contents.trim().parse::<u16>() {
                Ok(current) => {
                    // Is someone actually listening there?
                    let conn = timeout(
                        Duration::from_millis(CONNECT_TIMEOUT_MS),
                        TokioTcpStream::connect(("127.0.0.1", current)),
                    )
                    .await;
                    match conn {
                        Ok(Ok(_)) => false, // live server holds the port
                        _ => true,          // stale
                    }
                }
                Err(_) => true, // malformed — claim
            },
            Err(_) => true, // missing — claim
        };

        if claim {
            if let Err(e) = write_restricted(&port_path, own_port.to_string().as_bytes()) {
                log_debug!("[daemon/watchdog] write heartbeat.port: {e}");
            } else {
                log_debug!(
                    "[daemon/watchdog] claimed heartbeat.port -> {} (previous writer was gone)",
                    own_port
                );
            }
            if let Err(e) = write_restricted(&token_path, own_token.as_bytes()) {
                log_debug!("[daemon/watchdog] write heartbeat.token: {e}");
            }
        }

        sleep(Duration::from_secs(INTERVAL_SECS)).await;
    }
}

/// Extract the project directory from query params. Accepts BOTH
/// `project=<path>` (the short form src-tauri's agent_hooks server
/// uses and the k2so CLI sends) and `project_path=<path>` (the long
/// form earlier daemon routes adopted). Empty values are treated the
/// same as missing.
fn project_param(
    params: &std::collections::HashMap<String, String>,
) -> Option<String> {
    for key in &["project_path", "project"] {
        if let Some(v) = params.get(*key) {
            if !v.is_empty() {
                return Some(v.clone());
            }
        }
    }
    None
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
        "/cli/heartbeat/list-archived" => {
            hb::k2so_heartbeat_list_archived(project_path.to_string())
                .map(|rows| serde_json::to_string(&rows).unwrap_or_default())
        }
        "/cli/heartbeat/archive" => {
            let name = params.get("name").cloned().unwrap_or_default();
            if name.is_empty() {
                return Err("Missing 'name' parameter".to_string());
            }
            hb::k2so_heartbeat_archive(project_path.to_string(), name)
                .map(|_| r#"{"success":true}"#.to_string())
        }
        "/cli/heartbeat/unarchive" => {
            let name = params.get("name").cloned().unwrap_or_default();
            if name.is_empty() {
                return Err("Missing 'name' parameter".to_string());
            }
            hb::k2so_heartbeat_unarchive(project_path.to_string(), name)
                .map(|_| r#"{"success":true}"#.to_string())
        }
        "/cli/heartbeat/fire" | "/cli/heartbeat/launch" => {
            // Manual single-heartbeat launch — does NOT consult schedule
            // window. Routes through the smart-launch decision tree
            // (fresh-fire / inject-into-live / resume-and-fire) so the
            // CLI, the Tauri Launch button, and the cron tick all share
            // one canonical path. `fire` kept as an alias since the
            // existing CLI verb predates `launch`.
            let name = params.get("name").cloned().unwrap_or_default();
            Ok(crate::heartbeat_launch::smart_launch(project_path, &name).to_string())
        }
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
        "/cli/heartbeat/active-session" => {
            // 0036 — heartbeat-active-session lookup. Reads the row's
            // `active_terminal_id` and verifies via session_lookup
            // (covers both legacy session_map and v2_session_map).
            // Returns the agent_name as well so the renderer can pass
            // it to TerminalPane's `attachAgentName` override and
            // /cli/sessions/v2/spawn returns the existing session
            // (reused=true) instead of spawning a duplicate. See
            // `.k2so/prds/heartbeat-active-session-tracking.md`.
            let name = params.get("name").cloned().unwrap_or_default();
            if name.is_empty() {
                return Err("Missing 'name' parameter".to_string());
            }
            let db = k2so_core::db::shared();
            let conn = db.lock();
            let project_id = k2so_core::agents::resolve_project_id(&conn, project_path)
                .ok_or_else(|| format!("Project not found: {project_path}"))?;
            let hb_row =
                k2so_core::db::schema::AgentHeartbeat::get_by_name(&conn, &project_id, &name)
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| format!("no heartbeat '{name}'"))?;
            let mut active_id = hb_row.active_terminal_id.clone();
            // Walk both legacy + v2 session maps so we accept any
            // running PTY (heartbeat fresh-fires today land in v2;
            // legacy chat tabs may still be in session_map).
            let (mut active_agent_name, mut session_alive, mut is_v2) =
                match active_id.as_deref() {
                    Some(tid) => match k2so_core::session::SessionId::parse(tid) {
                        Some(sid) => {
                            let snap = crate::session_lookup::snapshot_all();
                            let found = snap
                                .iter()
                                .find(|(_n, live)| live.session_id() == sid);
                            match found {
                                Some((nm, live)) => {
                                    (Some(nm.clone()), true, live.is_v2())
                                }
                                None => (None, false, false),
                            }
                        }
                        None => (None, false, false),
                    },
                    None => (None, false, false),
                };
            // Lazy cleanup so the next call reflects reality.
            if active_id.is_some() && !session_alive {
                let _ = k2so_core::db::schema::AgentHeartbeat::clear_active_terminal_id(
                    &conn, &project_id, &name,
                );
                active_id = None;
            }
            // Fallback: stamp was null or pointed at a corpse. Scan
            // argv for any live PTY running `--resume <last_session_id>`
            // and surface it. Avoids the duplicate-claude-process
            // problem where clicking a heartbeat row spawns yet
            // another `claude --resume` against an already-running
            // session. When found, stamp the row so subsequent calls
            // go straight through the fast path above.
            if !session_alive {
                if let Some(saved) = hb_row
                    .last_session_id
                    .as_deref()
                    .filter(|s| !s.is_empty())
                {
                    let snap = crate::session_lookup::snapshot_all();
                    // Prefer `tab-*` agent names (visible UI tabs) over
                    // daemon-internal agent names. Same ranking
                    // `find_live_for_resume` uses.
                    let mut matches: Vec<&(String, crate::session_lookup::LiveSession)> =
                        snap.iter()
                            .filter(|(_n, live)| {
                                let args = live.args();
                                let mut i = 0;
                                while i + 1 < args.len() {
                                    if (args[i] == "--session-id"
                                        || args[i] == "--resume")
                                        && args[i + 1] == saved
                                    {
                                        return true;
                                    }
                                    i += 1;
                                }
                                false
                            })
                            .collect();
                    matches.sort_by_key(|(n, _)| if n.starts_with("tab-") { 0 } else { 1 });
                    if let Some((nm, live)) = matches.first() {
                        let new_tid = live.session_id().to_string();
                        let _ = k2so_core::db::schema::AgentHeartbeat::save_active_terminal_id(
                            &conn, &project_id, &name, &new_tid,
                        );
                        active_id = Some(new_tid);
                        active_agent_name = Some(nm.clone());
                        session_alive = true;
                        is_v2 = live.is_v2();
                    }
                }
            }
            Ok(serde_json::json!({
                "name": hb_row.name,
                "claudeSessionId": hb_row.last_session_id,
                "activeTerminalId": if session_alive { active_id.clone() } else { None },
                "activeAgentName": active_agent_name,
                "sessionAlive": session_alive,
                "isV2": is_v2,
            })
            .to_string())
        }
        _ => Err(format!("Unknown heartbeat route: {path}")),
    }
}

/// Thin forwarder to `triage::handle_triage` (read-only summary).
/// Kept as a named fn here because `cli::dispatch` (in main.rs's
/// module tree) references `crate::handle_agents_triage`.
fn handle_agents_triage(project_path: &str) -> String {
    crate::triage::handle_triage(project_path)
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
    // CORS headers on every response so the Tauri WebView (cross-
    // origin from tauri://localhost or http://localhost:5173 to
    // http://127.0.0.1:<port>) can read the body. Token auth
    // gates every real request so permissive origin adds no risk.
    let resp = format!(
        "HTTP/1.1 {status}\r\n\
         Content-Type: {ct}\r\n\
         Content-Length: {}\r\n\
         Access-Control-Allow-Origin: *\r\n\
         Access-Control-Expose-Headers: *\r\n\
         Connection: close\r\n\r\n{}",
        body.len(),
        body,
    );
    let _ = stream.write_all(resp.as_bytes()).await;
}

/// Respond to a CORS preflight (OPTIONS) with permissive headers so
/// the WebView accepts the subsequent GET/POST. 204 No Content is
/// the conventional preflight response status.
async fn send_cors_preflight(stream: &mut TcpStream) {
    let resp = "HTTP/1.1 204 No Content\r\n\
        Access-Control-Allow-Origin: *\r\n\
        Access-Control-Allow-Methods: GET, POST, OPTIONS\r\n\
        Access-Control-Allow-Headers: Content-Type, Authorization\r\n\
        Access-Control-Max-Age: 600\r\n\
        Content-Length: 0\r\n\
        Connection: close\r\n\r\n";
    let _ = stream.write_all(resp.as_bytes()).await;
}

/// Read the body of a POST request. Consumes the request line and
/// headers from the peeked stream, then returns whatever bytes
/// follow the `\r\n\r\n` separator up to the Content-Length header.
///
/// MVP implementation — assumes the full request arrived in the
/// 4KB peek buffer (fine for the single JSON AgentSignal payloads
/// E7 + E8 handle). Production-grade Content-Length-driven
/// streaming is deferred; the largest signal we expect is ~1KB, so
/// 4KB is 4× the headroom.
async fn read_post_body(stream: &mut TcpStream, buf: &mut [u8]) -> Vec<u8> {
    // Phase 4.5: the old single-read version worked with curl (which
    // batches headers + body into one TCP send) but broke with
    // browser fetch, which sends headers in one packet and body in
    // a separate packet. A single `stream.read()` would only return
    // the headers, leaving the body unread — and the JSON parser
    // got "EOF at column 0".
    //
    // Loop until we have the full body: read headers (first chunk),
    // parse Content-Length, then keep reading until we've got that
    // many body bytes or EOF.
    let mut accumulated: Vec<u8> = Vec::new();
    let mut header_end: Option<usize> = None;
    let mut content_length: Option<usize> = None;

    loop {
        // Read into `buf` and append to `accumulated` until headers
        // end is found.
        let n = match stream.read(buf).await {
            Ok(0) => break, // EOF
            Ok(n) => n,
            Err(_) => return Vec::new(),
        };
        accumulated.extend_from_slice(&buf[..n]);

        if header_end.is_none() {
            if let Some(pos) = accumulated
                .windows(4)
                .position(|w| w == b"\r\n\r\n")
            {
                header_end = Some(pos + 4);
                let headers_str =
                    std::str::from_utf8(&accumulated[..pos]).unwrap_or("");
                content_length = headers_str.lines().find_map(|line| {
                    let lower = line.to_ascii_lowercase();
                    lower
                        .strip_prefix("content-length:")
                        .and_then(|v| v.trim().parse::<usize>().ok())
                });
            }
        }

        // Once headers end is known, check if we have the whole body.
        if let (Some(body_start), Some(clen)) = (header_end, content_length) {
            if accumulated.len() >= body_start + clen {
                return accumulated[body_start..body_start + clen].to_vec();
            }
        }
        // Without Content-Length, fall back to "one read gave us
        // everything" heuristic once we've seen the headers.
        if let (Some(body_start), None) = (header_end, content_length) {
            return accumulated[body_start..].to_vec();
        }
    }

    // EOF before we got the full body (or before headers ended).
    // Return whatever we have between header end and EOF; caller's
    // parser will surface a helpful error if it's incomplete.
    if let Some(body_start) = header_end {
        if accumulated.len() > body_start {
            return accumulated[body_start..].to_vec();
        }
    }
    Vec::new()
}

/// Boot-time sweep that runs the 0.37.0 workspace–agent unification
/// migration once per registered workspace. Idempotent — workspaces
/// that already carry the sentinel `.k2so/.unification-0.37.0-done`
/// no-op in milliseconds. The migration archives originals to
/// `.k2so/migration/legacy/` before mutating anything, so worst-case
/// recovery is a manual restore from there.
///
/// Failures are logged and skipped — a single bad workspace must not
/// keep the daemon from booting and serving healthy ones.
fn run_workspace_unification_sweep() {
    use k2so_core::agents::unification;

    let projects = {
        let db = k2so_core::db::shared();
        let conn = db.lock();
        match k2so_core::db::schema::Project::list(&conn) {
            Ok(rows) => rows,
            Err(e) => {
                log_debug!("[daemon/unification] WARN: list projects: {e}");
                return;
            }
        }
    };

    if projects.is_empty() {
        return;
    }

    let total = projects.len();
    let mut migrated = 0usize;
    let mut already_done = 0usize;
    let mut errors = 0usize;
    for project in &projects {
        if !std::path::Path::new(&project.path).exists() {
            // Workspace path no longer on disk (deleted folder, ejected
            // drive). Don't fail the sweep on this.
            continue;
        }
        match unification::run_unification(&project.path, &project.agent_mode) {
            Ok(outcome) if outcome.already_done => {
                already_done += 1;
            }
            Ok(outcome) => {
                migrated += 1;
                log_debug!(
                    "[daemon/unification] migrated {} ({}): primary={:?} templates={} archived={} merged={} conflicts={}",
                    project.name,
                    project.path,
                    outcome.primary_migrated,
                    outcome.templates_migrated.len(),
                    outcome.legacy_archived.len(),
                    outcome.work_items_merged,
                    outcome.conflicts.len(),
                );
            }
            Err(e) => {
                errors += 1;
                log_debug!(
                    "[daemon/unification] FAILED for {} ({}): {e}",
                    project.name,
                    project.path,
                );
            }
        }
    }
    log_debug!(
        "[daemon/unification] swept {total} workspace(s): migrated={migrated} already_done={already_done} errors={errors}",
    );

    // Rewrite stale `wakeup_path` rows in workspace_heartbeats. Pre-
    // 0.37.0 every heartbeat row pointed at
    // `.k2so/agents/<primary>/heartbeats/<sched>/WAKEUP.md`; the
    // unification migration moved those files to
    // `.k2so/heartbeats/<sched>/WAKEUP.md` but doesn't touch the DB
    // (the migration code is filesystem-only, kept testable without
    // a DB). Run a single-pass rewrite here so heartbeat fires after
    // first 0.37.0 boot find their wakeup files at the new location.
    rewrite_legacy_heartbeat_wakeup_paths();

    // Migrate heartbeat WAKEUP.md files written to the wrong
    // location. Between the unification migration shipping (which
    // moves heartbeats to .k2so/heartbeats/) and the heartbeat
    // write-path fix landing, K2SO's heartbeat scaffolding code
    // wrote new WAKEUP.md files to .k2so/agent/heartbeats/<sched>/
    // (because agent_dir's layout-aware probe correctly resolved
    // to .k2so/agent/ post-migration, but the heartbeat code was
    // still constructing path = agent_dir + "heartbeats" instead
    // of the workspace-level .k2so/heartbeats/). DB rows correctly
    // point at .k2so/heartbeats/, so any file at the agent-relative
    // path is "orphaned" — the runtime can't find it on a fire.
    // Sweep moves them into place once at boot.
    migrate_orphaned_agent_heartbeats();
}

/// Move heartbeat WAKEUP.md files from `.k2so/agent/heartbeats/<sched>/`
/// (the agent-relative path that 0.37.0's incomplete heartbeat
/// write-path fix used) to the workspace-level
/// `.k2so/heartbeats/<sched>/`. DB rows are already pointed at the
/// workspace-level path, so this aligns disk with DB.
///
/// Idempotent — workspaces with no orphaned files are no-ops. A
/// workspace where the destination already exists keeps the
/// existing file and leaves the orphan in place (user resolves).
fn migrate_orphaned_agent_heartbeats() {
    let projects = {
        let db = k2so_core::db::shared();
        let conn = db.lock();
        match k2so_core::db::schema::Project::list(&conn) {
            Ok(rows) => rows,
            Err(_) => return,
        }
    };

    let mut moved = 0usize;
    for project in &projects {
        if !std::path::Path::new(&project.path).exists() {
            continue;
        }
        let project_root = std::path::Path::new(&project.path);
        let orphan_root = project_root.join(".k2so/agent/heartbeats");
        if !orphan_root.exists() {
            continue;
        }
        let workspace_hb_root = project_root.join(".k2so/heartbeats");
        if let Err(e) = fs::create_dir_all(&workspace_hb_root) {
            log_debug!(
                "[daemon/unification] WARN: create {workspace_hb_root:?}: {e}"
            );
            continue;
        }
        let Ok(entries) = fs::read_dir(&orphan_root) else { continue };
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else { continue };
            if !file_type.is_dir() {
                continue;
            }
            let sched_name = entry.file_name();
            let from = orphan_root.join(&sched_name);
            let to = workspace_hb_root.join(&sched_name);
            if to.exists() {
                log_debug!(
                    "[daemon/unification] orphaned heartbeat dir at {from:?} \
                     left in place (workspace-level dir already exists at {to:?})"
                );
                continue;
            }
            match fs::rename(&from, &to) {
                Ok(_) => {
                    log_debug!(
                        "[daemon/unification] moved orphan heartbeat: {from:?} → {to:?}"
                    );
                    moved += 1;
                }
                Err(e) => log_debug!(
                    "[daemon/unification] WARN: move orphan heartbeat {from:?} → {to:?}: {e}"
                ),
            }
        }
        // Best-effort cleanup of the now-empty .k2so/agent/heartbeats/.
        let _ = fs::remove_dir(&orphan_root);
    }
    if moved > 0 {
        log_debug!(
            "[daemon/unification] moved {moved} orphaned heartbeat dir(s) from \
             .k2so/agent/heartbeats/ to workspace-level .k2so/heartbeats/"
        );
    }
}

/// Rewrite any `workspace_heartbeats.wakeup_path` rows whose paths
/// reference a legacy nested-under-agent layout. Two patterns:
///
///   1. Pre-0.37.0: `.k2so/agents/<primary>/heartbeats/<sched>/...`
///      (plural `agents/`, named per-agent subdir).
///   2. 0.36.x→0.37.0 half-state: `.k2so/agent/heartbeats/<sched>/...`
///      (singular `agent/`, post-unification but still nested).
///
/// Post-0.37.0 the canonical layout is `.k2so/heartbeats/<sched>/`.
/// This sweep walks every row, detects either legacy pattern, and
/// rewrites in place. Idempotent — rows already pointing at the
/// canonical layout are untouched.
fn rewrite_legacy_heartbeat_wakeup_paths() {
    let db = k2so_core::db::shared();
    let conn = db.lock();
    let rows: Vec<(String, String, String)> = match conn.prepare(
        "SELECT id, name, wakeup_path FROM workspace_heartbeats \
         WHERE wakeup_path LIKE '%/.k2so/agents/%/heartbeats/%' \
            OR wakeup_path LIKE '.k2so/agents/%/heartbeats/%' \
            OR wakeup_path LIKE '%/.k2so/agent/heartbeats/%' \
            OR wakeup_path LIKE '.k2so/agent/heartbeats/%'",
    ) {
        Ok(mut stmt) => stmt
            .query_map([], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
            })
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default(),
        Err(e) => {
            log_debug!("[daemon/unification] WARN: scan stale wakeup_path: {e}");
            return;
        }
    };

    if rows.is_empty() {
        return;
    }

    let mut rewritten = 0usize;
    for (id, name, old) in &rows {
        // Two legacy patterns to normalize to `.k2so/heartbeats/<sched>/...`:
        //
        //   1. `.k2so/agents/<primary>/heartbeats/<sched>/...`
        //      (pre-0.37.0; named per-agent subdir before plural→singular)
        //   2. `.k2so/agent/heartbeats/<sched>/...`
        //      (0.36.x→0.37.0 half-state; singular `agent` but still nested)
        //
        // Try the plural pattern first (longer match), then fall back to
        // the singular form. Either way the result strips the agent dir
        // and leaves `.k2so/heartbeats/<sched>/...`.
        let new_path = if let Some(start) = old.find(".k2so/agents/") {
            let after = &old[start + ".k2so/agents/".len()..];
            let Some(slash) = after.find('/') else { continue };
            let rest = &after[slash..];
            if !rest.starts_with("/heartbeats/") {
                continue;
            }
            let prefix = &old[..start];
            format!("{prefix}.k2so{rest}")
        } else if let Some(start) = old.find(".k2so/agent/heartbeats/") {
            // Singular `agent` doesn't have a per-name subdir — the path
            // segment to drop is just `agent`.
            let prefix = &old[..start];
            let rest = &old[start + ".k2so/agent".len()..]; // /heartbeats/<sched>/...
            format!("{prefix}.k2so{rest}")
        } else {
            continue;
        };
        match conn.execute(
            "UPDATE workspace_heartbeats SET wakeup_path = ?1 WHERE id = ?2",
            rusqlite::params![new_path, id],
        ) {
            Ok(_) => {
                log_debug!(
                    "[daemon/unification] rewrote wakeup_path for hb={name}: {old} → {new_path}"
                );
                rewritten += 1;
            }
            Err(e) => log_debug!("[daemon/unification] WARN: rewrite wakeup_path for hb={name}: {e}"),
        }
    }
    log_debug!(
        "[daemon/unification] rewrote {rewritten} heartbeat wakeup_path row(s) to post-migration layout"
    );
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

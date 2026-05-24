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
mod canonical_session;
mod chat_routes;
mod claude_auth_host;
mod cli;
mod cli_response;
mod companion_host;
mod companion_routes;
mod db_routes;
mod events;
mod fs_routes;
mod git_routes;
mod heartbeat_launch;
mod heartbeat_launchd_routes;
mod inbox_routes;
mod llm_host;
mod llm_routes;
mod pending_live;
mod project_config_routes;
mod providers;
mod review_checklist_routes;
mod session_events;
mod session_events_ws;
mod session_lookup;
mod sessions_bytes_ws;
mod sessions_grid_ws;
mod sessions_ws;
mod settings_routes;
mod signal_format;
mod skill_layers_routes;
mod spawn;
mod terminal_event_sink;
mod terminal_lifecycle_routes;
mod terminal_routes;
mod themes_routes;
mod triage;
mod v2_session_map;
mod v2_spawn;
mod wake_headless;
mod watchdog;
mod workspace_layouts_dedup;
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

fn main() {
    // Phase 2 Unit 2 — LLM worker subprocess fork. Daemon spawns
    // itself as `k2so-daemon --llm-worker <payload_path>` to run one
    // inference pass in an isolated child process. Worker exits via
    // `libc::_exit(0)` so it never returns from here. Must be the
    // very first thing we do — before tokio runtime init, before
    // rustls provider install, before anything that allocates GPU
    // resources we'd rather the parent daemon own. See
    // `llm_host::worker_main` for the protocol contract.
    let args: Vec<String> = std::env::args().collect();
    if args.len() == 3 && args[1] == "--llm-worker" {
        llm_host::worker_main(&args[2]);
        // unreachable — worker_main calls libc::_exit. The `!`
        // return type prevents fall-through.
    }

    // Real daemon path — boot the multi-thread tokio runtime and
    // run the async main body.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("failed to build tokio runtime");
    rt.block_on(async_main());
}

async fn async_main() {
    // Force a reference to k2so-core so the crate boundary is exercised
    // at build-time until we actually use it for real work.
    k2so_core::__scaffolding_marker();

    // 0.37.9 — raise RLIMIT_NOFILE so the daemon can hold enough fds
    // for many concurrent PTYs / WS sockets / file watchers. launchd
    // gives the daemon a 256/1024 soft limit by default, which gets
    // saturated quickly with 10+ terminal sessions (each takes 2+
    // fds for the PTY pair plus WS sockets per attached client).
    // No-op when already at the kernel hard limit.
    #[cfg(unix)]
    k2so_core::raise_nofile_limit();

    // launchd hands us a sparse PATH; enrich from the user's login shell
    // BEFORE anything else, so child posix_spawn calls (alacritty's
    // tty::new for v2 sessions, plus any Command::new in handlers) can
    // resolve user-installed tools like `claude`, `cursor-agent`,
    // homebrew binaries, etc. See docs in k2so_core::enrich_path_from_login_shell.
    #[cfg(unix)]
    k2so_core::enrich_path_from_login_shell();

    // Phase 2 Unit 1 — install a rustls CryptoProvider so the
    // daemon-owned companion ngrok tunnel can negotiate TLS. Rustls
    // 0.23 compiles both aws-lc-rs (via reqwest rustls-tls) and ring
    // (via ngrok) into the binary; it refuses to auto-pick and
    // panics on first TLS use unless a provider is explicitly
    // installed. Mirrors the same call in src-tauri/src/lib.rs.
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

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

    // Phase 2 Unit 7b — boot-time per-workspace legacy migrations +
    // SKILL.md refresh. Each helper is idempotent: archive_orphan
    // returns immediately when there are no orphans, the heartbeat
    // migrators no-op once rows exist, ensure_all_skills_up_to_date
    // is checksum-gated. Pre-Phase-2 these ran from Tauri's setup
    // hook in `src-tauri/src/lib.rs`; relocating to the daemon means
    // they fire on `launchctl bootstrap` boots even when Tauri is
    // closed, and on remote daemons that have no Tauri at all.
    run_workspace_legacy_migrations_sweep();

    // Phase 2 Unit 4 — workspace_layouts one-shot migration moved
    // from `src-tauri/src/lib.rs::migrate_workspace_layouts_to_db`.
    // Reads `~/.k2so/settings.json#workspaceLayouts`, inserts each
    // pre-existing layout into the `workspace_layouts` table, then
    // strips the key from settings.json. Idempotent on the second
    // boot (the key is gone). Running daemon-side means a remote
    // daemon (K2SO Connect) picks it up without Tauri being present.
    k2so_core::db_ops::migrate_workspace_layouts_to_db();

    // Phase 2.5 follow-up: prefer the port stored in `~/.k2so/daemon.port`
    // from the previous boot so the renderer's `daemon_ws_url` cache + the
    // CLI's port-file readers don't get stale-port traffic on daemon
    // restart. Algorithm + tests in `k2so_core::port_claim`.
    let daemon_port_path = k2so_dir.join("daemon.port");
    let claimed = match k2so_core::port_claim::claim_port(&daemon_port_path) {
        Some(c) => c,
        None => {
            log_debug!("[daemon] FATAL: failed to bind any loopback port");
            std::process::exit(2);
        }
    };
    let port = claimed.port;
    if claimed.reused {
        log_debug!(
            "[daemon] reused previously-published port {} (stable across restarts)",
            port
        );
    } else {
        log_debug!(
            "[daemon] bound new ephemeral port {} (no prior port or port taken)",
            port
        );
    }
    // The std listener has to be set non-blocking before tokio can adopt it.
    if let Err(e) = claimed.listener.set_nonblocking(true) {
        log_debug!("[daemon] FATAL: set_nonblocking on claimed listener: {e}");
        std::process::exit(2);
    }
    let listener = match TcpListener::from_std(claimed.listener) {
        Ok(l) => l,
        Err(e) => {
            log_debug!("[daemon] FATAL: tokio adopt claimed listener: {e}");
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

    // Phase 2 Unit 1 — register daemon-side companion bridges so
    // k2so-core's companion module can run without Tauri being
    // present. The terminal/settings/event bridges all read from
    // daemon-owned state (TerminalManager singleton, ~/.k2so/settings.json,
    // the broadcast event channel created above). Must precede the
    // autostart fire below so the tunnel start sees a configured
    // settings provider.
    companion_host::register((*event_tx).clone());

    // Phase 2 Unit 3 — daemon-side terminal event sink. Wires
    // `k2so_core::terminal::TerminalManager`'s event emissions onto
    // the daemon's broadcast channel as `WireEvent` frames named
    // `terminal:{title,bell,exit,grid}:<id>`. Tauri's
    // `daemon_events.rs` re-emits them via `AppHandle::emit` so the
    // renderer's existing `listen('terminal:grid:<id>')` subscribers
    // keep working. Must run BEFORE any `/cli/terminal/create`
    // request can land, which means before `start_listener` accepts
    // its first connection (we register synchronously here).
    terminal_event_sink::register((*event_tx).clone());

    // First-boot companion autostart. If `companion.auto_start` is
    // true and credentials are present, kick off `start_companion()`
    // on a detached thread (the tunnel takes a few seconds to come
    // up and we don't want to block daemon boot on ngrok). Replaces
    // the pre-Phase-2 Tauri-side autostart in `src-tauri/src/lib.rs`
    // — the daemon now owns the tunnel lifecycle so Mobile Companion
    // + K2SO Connect can reach it even with Tauri closed.
    companion_host::maybe_autostart();

    // Phase 2 Unit 2 — LLM first-boot discovery. Cheap synchronous
    // check that the default model file exists, then caches its
    // path on `llm_host::shared()`. We do NOT load the model in the
    // daemon process — Metal/ggml stays in the subprocess worker so
    // a crash never takes the daemon down. If the model is missing
    // the daemon still boots cleanly; clients call
    // `/cli/llm/download-default` to fetch.
    llm_host::maybe_first_boot_discover();

    // Phase 3.1 F3 — boot-time pending-live replay. Previous
    // 0.37.5 migration: legacy `<sanitized_pid>_<agent>/` queue dirs
    // (pre-0.37.5 keying) merged into bare `<sanitized_pid>/`. MUST
    // run BEFORE replay_all so the in-memory `pending_state` counter
    // is built from the post-migration shape. Idempotent.
    pending_live::migrate_legacy_dirs_to_bare_pid();

    // 0.38.0 — heal corrupt `workspace_layouts.layout_json` rows
    // written by pre-0.38.0 builds whose mount-time sync:tabs-request
    // broadcast race appended duplicate tab entries that all pointed
    // at the same daemon paneGroup. Gated by a code_migrations marker
    // so it's a one-shot pass. See `workspace_layouts_dedup` for
    // signature derivation and unit tests.
    workspace_layouts_dedup::run_once();

    // 0.38.0 — migrate `version: 1` layouts to v2 (metadata-only for
    // daemon-backed tabs). Independent code_migrations marker
    // (`0.38.0-layout-v2-emit`) so the dedup and the v2 emit can be
    // re-run independently in future versions. Renderer also performs
    // the same migration on read (`tabs.ts::migrateLayoutToV2`).
    workspace_layouts_dedup::run_v2_emit_once();

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
            | "/cli/sessions/v2/spawn"
            | "/cli/sessions/v2/close"
            // Phase 2 Unit 1 — body-bearing companion control routes.
            // Password and session-token live in the body so they
            // don't end up in URL-logged form on shared/loopback
            // intermediaries.
            | "/cli/companion/set-password"
            | "/cli/companion/disconnect-session"
            // Phase 2 Unit 5 — Claude Auth mutating routes. POST
            // (not GET) so they're not idempotent-cached by any
            // future proxy and so they parallel Unit 1's pattern
            // for "this writes state". The status read-side stays
            // a GET and goes through `cli::dispatch`.
            | "/cli/claude-auth/refresh-now"
            | "/cli/claude-auth/install-scheduler"
            | "/cli/claude-auth/uninstall-scheduler"
            // Phase 2 Unit 2 — LLM control + chat. Chat body
            // carries the user message + workspace context. Load
            // takes a path. Download-default takes no body but is
            // a write-side operation so we accept POST for it too.
            | "/cli/llm/chat"
            | "/cli/llm/load-model"
            | "/cli/llm/download-default"
            // Phase 2 Unit 7a — settings writes. Partial settings
            // payloads live in the body; `reset` is POST so it can't
            // be reached via a stray GET / browser refresh.
            | "/cli/settings/update"
            | "/cli/settings/reset"
            // Phase 2 Unit 6 — filesystem mutations + chat history
            // mutations + theme/skill-layer/review-checklist
            // mutations. JSON bodies carry the arguments (paths,
            // file contents, source/destination tuples) so they
            // aren't URL-encoded in proxy logs.
            | "/cli/fs/search-tree"
            | "/cli/fs/write-file"
            | "/cli/fs/move"
            | "/cli/fs/copy"
            | "/cli/fs/delete"
            | "/cli/fs/rename"
            | "/cli/fs/create"
            | "/cli/fs/duplicate"
            | "/cli/fs/open-finder"
            | "/cli/fs/open-external"
            | "/cli/chat/rename"
            | "/cli/chat/toggle-pin"
            | "/cli/chat/migrate-ide"
            | "/cli/themes/create-template"
            | "/cli/themes/delete"
            | "/cli/skill-layers/create"
            | "/cli/skill-layers/delete"
            | "/cli/review-checklist/write"
            | "/cli/review-checklist/toggle"
            | "/cli/review-checklist/init"
            // Phase 2 Unit 3 — terminal PTY lifecycle. JSON-bodied
            // mutating routes; method-gated per-handler below.
            | "/cli/terminal/create"
            | "/cli/terminal/kill"
            | "/cli/terminal/resize"
            | "/cli/terminal/kill-foreground"
            | "/cli/terminal/scroll"
            | "/cli/terminal/log"
            | "/cli/terminal/lifecycle-write"
            | "/cli/terminal/set-focus"
            // Phase 2 Unit 7c — heartbeat-launchd installer + orphan-
            // agents sweep. Body-bearing writes; method-gated below.
            | "/cli/heartbeat/install-launchd"
            | "/cli/heartbeat/uninstall-launchd"
            | "/cli/heartbeat/apply-wake-scheduler"
            | "/cli/agents/archive-orphans"
            // Phase 2 Unit 4 — DB-writing routes (states / workspaces /
            // focus-groups / sections / workspace-layouts / timer /
            // presets / window-state / projects / git). JSON-bodied
            // writes — implicit method gate via the `starts_with`
            // dispatch arm in handle_connection that runs Unit 4's
            // POST dispatch. Listed explicitly here so the top-level
            // 405 guard never short-circuits them.
            | "/cli/states/create" | "/cli/states/update" | "/cli/states/delete"
            | "/cli/workspaces/create" | "/cli/workspaces/delete" | "/cli/workspaces/set-nav-visible"
            | "/cli/focus-groups/create" | "/cli/focus-groups/update" | "/cli/focus-groups/delete"
            | "/cli/focus-groups/assign" | "/cli/focus-groups/reconcile"
            | "/cli/sections/create" | "/cli/sections/update" | "/cli/sections/delete"
            | "/cli/sections/reorder" | "/cli/sections/assign"
            | "/cli/workspace-layouts/save" | "/cli/workspace-layouts/delete"
            | "/cli/timer/create" | "/cli/timer/delete"
            | "/cli/presets/create" | "/cli/presets/update" | "/cli/presets/delete"
            | "/cli/presets/reorder" | "/cli/presets/reset"
            | "/cli/window-state/set"
            | "/cli/projects/create" | "/cli/projects/update" | "/cli/projects/delete"
            | "/cli/projects/reorder" | "/cli/projects/touch-interaction"
            | "/cli/projects/touch-interaction-clear" | "/cli/projects/add-from-path"
            | "/cli/projects/add-without-git" | "/cli/projects/init-git-and-open"
            | "/cli/projects/enable-worktrees" | "/cli/projects/detect-icon"
            | "/cli/projects/set-icon" | "/cli/projects/clear-icon"
            | "/cli/projects/open-in-finder" | "/cli/projects/open-in-editor"
            | "/cli/projects/open-in-terminal" | "/cli/projects/refresh-editors"
            | "/cli/git/create-worktree" | "/cli/git/remove-worktree"
            | "/cli/git/reopen-worktree" | "/cli/git/stage" | "/cli/git/unstage"
            | "/cli/git/stage-all" | "/cli/git/commit" | "/cli/git/merge-branch"
            | "/cli/git/abort-merge" | "/cli/git/resolve" | "/cli/git/delete-branch"
            | "/cli/git/prune-worktrees"
            // Phase 2.1 — workspace inbox mutating routes (A22.1).
            // Query-string POSTs (no JSON body); dispatched via
            // `inbox_routes::dispatch_post`. The `/cli/inbox/migrate`
            // route is a one-shot helper for tests / explicit
            // re-migration triggers — daemon first-boot also auto-
            // invokes (Phase 2.1b wiring).
            | "/cli/inbox/compose"
            | "/cli/inbox/move"
            | "/cli/inbox/archive"
            | "/cli/inbox/delete"
            | "/cli/inbox/respond"
            | "/cli/inbox/migrate"
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
        // 0.38.0 Commit 4: daemon-authoritative session lifecycle
        // stream. Pushes `session_added`/`session_removed` JSON
        // frames to subscribers whose `path=` matches the affected
        // session's cwd. Renderer + mobile companion consume the
        // same wire format. See `.k2so/prds/daemon-authoritative-tabs.md`.
        "/cli/sessions/events" => {
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
            session_events_ws::serve_session_events_connection(stream, params).await;
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
        // POST /cli/companion/set-password — Phase 2 Unit 1.
        // Body: `{"password": "..."}`. Hashes argon2id, stores in
        // macOS Keychain (preferred) or settings.json (fallback),
        // then invalidates every live companion session so the old
        // token can't be replayed.
        //
        // Method gate: see the long-form note on /cli/claude-auth/
        // refresh-now below — the top-level dispatch lets a GET
        // through on POST-allowlisted routes. Mirror Unit 5's
        // explicit `if !is_post` guard so a GET against this route
        // can't trigger the password rotation. Especially important
        // here because this is one of the routes Mobile Companion
        // and K2SO Connect will hit over the ngrok tunnel.
        "/cli/companion/set-password" => {
            if !is_post {
                let _ = stream.read(&mut buf).await;
                send_response(
                    &mut stream,
                    "405 Method Not Allowed",
                    "application/json",
                    r#"{"error":"POST required"}"#,
                )
                .await;
                return;
            }
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
            let result = companion_routes::handle_companion_set_password(&body_bytes);
            send_response(&mut stream, result.status, "application/json", &result.body)
                .await;
        }
        // ── Phase 2 Unit 2 — /cli/llm/* ────────────────────────────
        // GET routes are cheap; POST routes go through llm_routes
        // which owns the supervisor's subprocess machinery. All
        // five routes are token-gated by the standard query-string
        // check so callers must pass `?token=<token>` like every
        // other /cli/* endpoint.
        "/cli/llm/check" => {
            let _ = stream.read(&mut buf).await;
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
            let r = llm_routes::handle_check();
            send_response(&mut stream, r.status, r.content_type, &r.body).await;
        }
        "/cli/llm/status" => {
            let _ = stream.read(&mut buf).await;
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
            let r = llm_routes::handle_status();
            send_response(&mut stream, r.status, r.content_type, &r.body).await;
        }
        "/cli/llm/chat" => {
            // Method gate (see feedback_post_only_route_guards memory + the
            // /cli/claude-auth/refresh-now comment): the top-level dispatch
            // lets a GET through on POST-allowlisted routes. Reject explicitly.
            if !is_post {
                let _ = stream.read(&mut buf).await;
                send_response(
                    &mut stream,
                    "405 Method Not Allowed",
                    "application/json",
                    r#"{"error":"POST required"}"#,
                )
                .await;
                return;
            }
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
            // Inference is CPU/GPU heavy and may block for tens of
            // seconds. Run on a blocking worker so the runtime's
            // accept-loop threads stay free.
            let r = tokio::task::spawn_blocking(move || {
                llm_routes::handle_chat(&body_bytes)
            })
            .await
            .unwrap_or_else(|e| crate::cli_response::CliResponse {
                status: "500 Internal Server Error",
                content_type: "application/json",
                body: serde_json::json!({ "error": format!("worker join: {e}") })
                    .to_string(),
            });
            send_response(&mut stream, r.status, r.content_type, &r.body).await;
        }
        "/cli/llm/load-model" => {
            if !is_post {
                let _ = stream.read(&mut buf).await;
                send_response(
                    &mut stream,
                    "405 Method Not Allowed",
                    "application/json",
                    r#"{"error":"POST required"}"#,
                )
                .await;
                return;
            }
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
            let r = tokio::task::spawn_blocking(move || {
                llm_routes::handle_load_model(&body_bytes)
            })
            .await
            .unwrap_or_else(|e| crate::cli_response::CliResponse {
                status: "500 Internal Server Error",
                content_type: "application/json",
                body: serde_json::json!({ "error": format!("worker join: {e}") })
                    .to_string(),
            });
            send_response(&mut stream, r.status, r.content_type, &r.body).await;
        }
        "/cli/llm/download-default" => {
            if !is_post {
                let _ = stream.read(&mut buf).await;
                send_response(
                    &mut stream,
                    "405 Method Not Allowed",
                    "application/json",
                    r#"{"error":"POST required"}"#,
                )
                .await;
                return;
            }
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
            // Body is currently empty; read+drop to flush.
            let _ = read_post_body(&mut stream, &mut buf).await;
            let r = llm_routes::handle_download_default(&state.event_tx);
            send_response(&mut stream, r.status, r.content_type, &r.body).await;
        }
        // POST /cli/companion/disconnect-session — Phase 2 Unit 1.
        // Body: `{"sessionToken": "..."}`. Removes the session row
        // and any WS clients still attached to it.
        //
        // Method gate: same rationale as /cli/companion/set-password
        // above. Don't let a GET disconnect a live session.
        "/cli/companion/disconnect-session" => {
            if !is_post {
                let _ = stream.read(&mut buf).await;
                send_response(
                    &mut stream,
                    "405 Method Not Allowed",
                    "application/json",
                    r#"{"error":"POST required"}"#,
                )
                .await;
                return;
            }
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
            let result =
                companion_routes::handle_companion_disconnect_session(&body_bytes);
            send_response(&mut stream, result.status, "application/json", &result.body)
                .await;
        }
        // POST /cli/claude-auth/refresh-now — Phase 2 Unit 5.
        // No body required (the refresh token comes from the local
        // Keychain / credentials file). POST instead of GET because
        // it mutates token state. Returns the same status payload
        // shape as GET /cli/claude-auth/status.
        //
        // NOTE on method gating: the top-level dispatch only rejects
        // non-GET/non-POST methods; it doesn't reject GET on a
        // POST-allowlisted route (most routes accept both today and
        // gate behavior on body-presence). For Unit 5's mutating
        // routes — which have no body — we must explicitly reject
        // GET in the handler, or a curl GET would silently install /
        // refresh / uninstall the user's launchd scheduler. Caught
        // during smoke testing.
        "/cli/claude-auth/refresh-now" => {
            if !is_post {
                let _ = stream.read(&mut buf).await;
                send_response(
                    &mut stream,
                    "405 Method Not Allowed",
                    "application/json",
                    r#"{"error":"POST required"}"#,
                )
                .await;
                return;
            }
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
            // Drain whatever body the client sent so the socket
            // doesn't get half-read state. We don't use it.
            let _ = read_post_body(&mut stream, &mut buf).await;
            let result = claude_auth_host::handle_refresh_now();
            send_response(&mut stream, result.status, "application/json", &result.body)
                .await;
        }
        // POST /cli/claude-auth/install-scheduler — Phase 2 Unit 5.
        // Writes ~/.k2so/claude-auth-refresh.sh + loads the
        // launchd plist (macOS) or installs the crontab entry
        // (linux). Idempotent. POST-only (see /refresh-now comment).
        "/cli/claude-auth/install-scheduler" => {
            if !is_post {
                let _ = stream.read(&mut buf).await;
                send_response(
                    &mut stream,
                    "405 Method Not Allowed",
                    "application/json",
                    r#"{"error":"POST required"}"#,
                )
                .await;
                return;
            }
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
            let _ = read_post_body(&mut stream, &mut buf).await;
            let result = claude_auth_host::handle_install_scheduler();
            send_response(&mut stream, result.status, "application/json", &result.body)
                .await;
        }
        // POST /cli/claude-auth/uninstall-scheduler — Phase 2 Unit 5.
        // Unloads + removes the plist (macOS) or strips the
        // crontab entry (linux). Idempotent. POST-only.
        "/cli/claude-auth/uninstall-scheduler" => {
            if !is_post {
                let _ = stream.read(&mut buf).await;
                send_response(
                    &mut stream,
                    "405 Method Not Allowed",
                    "application/json",
                    r#"{"error":"POST required"}"#,
                )
                .await;
                return;
            }
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
            let _ = read_post_body(&mut stream, &mut buf).await;
            let result = claude_auth_host::handle_uninstall_scheduler();
            send_response(&mut stream, result.status, "application/json", &result.body)
                .await;
        }
        // POST /cli/settings/update — Phase 2 Unit 7a.
        // Body: arbitrary JSON object deep-merged into settings.json.
        // F3 closure runs inside `app_settings::update()` — companion-
        // credential changes invalidate live sessions server-side, in
        // the same process that owns the live companion runtime.
        // Method gate per feedback_post_only_route_guards memory.
        "/cli/settings/update" => {
            if !is_post {
                let _ = stream.read(&mut buf).await;
                send_response(
                    &mut stream,
                    "405 Method Not Allowed",
                    "application/json",
                    r#"{"error":"POST required"}"#,
                )
                .await;
                return;
            }
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
            let result = settings_routes::handle_settings_update(&body_bytes);
            send_response(&mut stream, result.status, "application/json", &result.body)
                .await;
        }
        // POST /cli/settings/reset — Phase 2 Unit 7a.
        // Restores `AppSettings::default()`, deletes Keychain hash,
        // invalidates every live companion session. POST (not GET)
        // so a browser refresh can't accidentally trigger it.
        // Method gate per feedback_post_only_route_guards memory.
        "/cli/settings/reset" => {
            if !is_post {
                let _ = stream.read(&mut buf).await;
                send_response(
                    &mut stream,
                    "405 Method Not Allowed",
                    "application/json",
                    r#"{"error":"POST required"}"#,
                )
                .await;
                return;
            }
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
            let _ = stream.read(&mut buf).await;
            let result = settings_routes::handle_settings_reset();
            send_response(&mut stream, result.status, "application/json", &result.body)
                .await;
        }
        // Phase 2 Unit 3 — terminal PTY lifecycle (POST routes).
        // Each handler runs through the process-wide
        // `k2so_core::terminal::shared()` TerminalManager so daemon
        // ownership is uniform. The blocking create handler is
        // dispatched via `tokio::task::spawn_blocking` (F5) since
        // posix_spawn + alacritty Term::new can stall briefly under
        // load. The non-blocking handlers (kill/resize/scroll/etc.)
        // are cheap mutex+method calls and run inline.
        //
        // Method gate: see the long-form comment on
        // `/cli/claude-auth/refresh-now`. The top-level dispatch
        // does NOT reject GET on POST-allowlisted routes — without
        // the explicit `if !is_post` guard, a curl GET could
        // silently spawn / kill a PTY.
        "/cli/terminal/create" => {
            if !is_post {
                let _ = stream.read(&mut buf).await;
                send_response(
                    &mut stream,
                    "405 Method Not Allowed",
                    "application/json",
                    r#"{"error":"POST required"}"#,
                )
                .await;
                return;
            }
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
            // F5: posix_spawn + alacritty Term::new can block; run
            // off the accept-loop thread pool.
            let r = tokio::task::spawn_blocking(move || {
                terminal_lifecycle_routes::handle_create(&body_bytes)
            })
            .await
            .unwrap_or_else(|e| crate::cli_response::CliResponse {
                status: "500 Internal Server Error",
                content_type: "application/json",
                body: serde_json::json!({ "error": format!("worker join: {e}") })
                    .to_string(),
            });
            send_response(&mut stream, r.status, r.content_type, &r.body).await;
        }
        "/cli/terminal/kill" => {
            if !is_post {
                let _ = stream.read(&mut buf).await;
                send_response(
                    &mut stream,
                    "405 Method Not Allowed",
                    "application/json",
                    r#"{"error":"POST required"}"#,
                )
                .await;
                return;
            }
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
            // Kill can block briefly waiting on child reap; F5.
            let r = tokio::task::spawn_blocking(move || {
                terminal_lifecycle_routes::handle_kill(&body_bytes)
            })
            .await
            .unwrap_or_else(|e| crate::cli_response::CliResponse {
                status: "500 Internal Server Error",
                content_type: "application/json",
                body: serde_json::json!({ "error": format!("worker join: {e}") })
                    .to_string(),
            });
            send_response(&mut stream, r.status, r.content_type, &r.body).await;
        }
        "/cli/terminal/resize" => {
            if !is_post {
                let _ = stream.read(&mut buf).await;
                send_response(
                    &mut stream,
                    "405 Method Not Allowed",
                    "application/json",
                    r#"{"error":"POST required"}"#,
                )
                .await;
                return;
            }
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
            let r = terminal_lifecycle_routes::handle_resize(&body_bytes);
            send_response(&mut stream, r.status, r.content_type, &r.body).await;
        }
        "/cli/terminal/kill-foreground" => {
            if !is_post {
                let _ = stream.read(&mut buf).await;
                send_response(
                    &mut stream,
                    "405 Method Not Allowed",
                    "application/json",
                    r#"{"error":"POST required"}"#,
                )
                .await;
                return;
            }
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
            let r = terminal_lifecycle_routes::handle_kill_foreground(&body_bytes);
            send_response(&mut stream, r.status, r.content_type, &r.body).await;
        }
        "/cli/terminal/scroll" => {
            if !is_post {
                let _ = stream.read(&mut buf).await;
                send_response(
                    &mut stream,
                    "405 Method Not Allowed",
                    "application/json",
                    r#"{"error":"POST required"}"#,
                )
                .await;
                return;
            }
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
            let r = terminal_lifecycle_routes::handle_scroll(&body_bytes);
            send_response(&mut stream, r.status, r.content_type, &r.body).await;
        }
        "/cli/terminal/log" => {
            if !is_post {
                let _ = stream.read(&mut buf).await;
                send_response(
                    &mut stream,
                    "405 Method Not Allowed",
                    "application/json",
                    r#"{"error":"POST required"}"#,
                )
                .await;
                return;
            }
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
            let r = terminal_lifecycle_routes::handle_log(&body_bytes);
            send_response(&mut stream, r.status, r.content_type, &r.body).await;
        }
        // POST /cli/terminal/lifecycle-write — byte-level write for
        // TerminalManager-owned terminals. The existing
        // /cli/terminal/write (GET, in terminal_routes.rs) operates on
        // the session_map's UUID-keyed sessions; the legacy
        // arbitrary-string TerminalManager IDs need a parallel path.
        // Body: `{"id":"...","data":"..."}`.
        "/cli/terminal/lifecycle-write" => {
            if !is_post {
                let _ = stream.read(&mut buf).await;
                send_response(
                    &mut stream,
                    "405 Method Not Allowed",
                    "application/json",
                    r#"{"error":"POST required"}"#,
                )
                .await;
                return;
            }
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
            let r = terminal_lifecycle_routes::handle_lifecycle_write(&body_bytes);
            send_response(&mut stream, r.status, r.content_type, &r.body).await;
        }
        "/cli/terminal/set-focus" => {
            if !is_post {
                let _ = stream.read(&mut buf).await;
                send_response(
                    &mut stream,
                    "405 Method Not Allowed",
                    "application/json",
                    r#"{"error":"POST required"}"#,
                )
                .await;
                return;
            }
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
            let r = terminal_lifecycle_routes::handle_set_focus(&body_bytes);
            send_response(&mut stream, r.status, r.content_type, &r.body).await;
        }
        // Phase 2 Unit 7c — heartbeat-launchd installer routes.
        // Daemon owns its own `com.k2so.agent-heartbeat.plist` so
        // K2SO Connect (remote daemon without Tauri) can install +
        // remove the scheduler under its own GUI session. Method
        // gates are inline so a stray GET can't trigger a
        // launchctl bootstrap. See `crates/k2so-core/src/agents/
        // heartbeat_install.rs` for the install/uninstall bodies.
        "/cli/heartbeat/install-launchd" => {
            if !is_post {
                let _ = stream.read(&mut buf).await;
                send_response(
                    &mut stream,
                    "405 Method Not Allowed",
                    "application/json",
                    r#"{"error":"POST required"}"#,
                )
                .await;
                return;
            }
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
            // launchctl bootstrap can stall briefly under load; F5.
            let r = tokio::task::spawn_blocking(move || {
                heartbeat_launchd_routes::handle_install_launchd(&body_bytes)
            })
            .await
            .unwrap_or_else(|e| crate::cli_response::CliResponse {
                status: "500 Internal Server Error",
                content_type: "application/json",
                body: serde_json::json!({ "error": format!("worker join: {e}") })
                    .to_string(),
            });
            send_response(&mut stream, r.status, r.content_type, &r.body).await;
        }
        "/cli/heartbeat/uninstall-launchd" => {
            if !is_post {
                let _ = stream.read(&mut buf).await;
                send_response(
                    &mut stream,
                    "405 Method Not Allowed",
                    "application/json",
                    r#"{"error":"POST required"}"#,
                )
                .await;
                return;
            }
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
            let r = tokio::task::spawn_blocking(move || {
                heartbeat_launchd_routes::handle_uninstall_launchd(&body_bytes)
            })
            .await
            .unwrap_or_else(|e| crate::cli_response::CliResponse {
                status: "500 Internal Server Error",
                content_type: "application/json",
                body: serde_json::json!({ "error": format!("worker join: {e}") })
                    .to_string(),
            });
            send_response(&mut stream, r.status, r.content_type, &r.body).await;
        }
        "/cli/heartbeat/apply-wake-scheduler" => {
            if !is_post {
                let _ = stream.read(&mut buf).await;
                send_response(
                    &mut stream,
                    "405 Method Not Allowed",
                    "application/json",
                    r#"{"error":"POST required"}"#,
                )
                .await;
                return;
            }
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
            let r = tokio::task::spawn_blocking(move || {
                heartbeat_launchd_routes::handle_apply_wake_scheduler(&body_bytes)
            })
            .await
            .unwrap_or_else(|e| crate::cli_response::CliResponse {
                status: "500 Internal Server Error",
                content_type: "application/json",
                body: serde_json::json!({ "error": format!("worker join: {e}") })
                    .to_string(),
            });
            send_response(&mut stream, r.status, r.content_type, &r.body).await;
        }
        // Phase 2 Unit 7c — orphan-agent sweep, refactored out of
        // src-tauri/src/commands/projects.rs's agent_mode-change
        // path. Body: `{"project_path": "/path"}`.
        "/cli/agents/archive-orphans" => {
            if !is_post {
                let _ = stream.read(&mut buf).await;
                send_response(
                    &mut stream,
                    "405 Method Not Allowed",
                    "application/json",
                    r#"{"error":"POST required"}"#,
                )
                .await;
                return;
            }
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
            // fs walk + db lock — F5.
            let r = tokio::task::spawn_blocking(move || {
                handle_archive_orphans(&body_bytes)
            })
            .await
            .unwrap_or_else(|e| crate::cli_response::CliResponse {
                status: "500 Internal Server Error",
                content_type: "application/json",
                body: serde_json::json!({ "error": format!("worker join: {e}") })
                    .to_string(),
            });
            send_response(&mut stream, r.status, r.content_type, &r.body).await;
        }
        // Phase 2 Unit 4 — POST routes for git (libgit2 ops). F5:
        // spawn_blocking because diff/merge/status on large repos
        // can block for 100s of ms.
        p if is_post && post_allowed && p.starts_with("/cli/git/") => {
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
            let p_owned = p.to_string();
            let result = tokio::task::spawn_blocking(move || {
                crate::git_routes::dispatch_unit4_git_post(&p_owned, &body_bytes)
            })
            .await
            .unwrap_or_else(|e| crate::cli_response::CliResponse {
                status: "500 Internal Server Error",
                content_type: "application/json",
                body: serde_json::json!({ "error": format!("worker join: {e}") })
                    .to_string(),
            });
            send_response(&mut stream, result.status, result.content_type, &result.body)
                .await;
        }
        // Phase 2 Unit 4 — POST routes for DB-writing domains. JSON-
        // bodied writes; per-route allowlist + same implicit gate
        // pattern as Unit 6. Dispatch is `dispatch_unit4_post`.
        p if is_post && post_allowed && (
            p.starts_with("/cli/states/")
                || p.starts_with("/cli/workspaces/")
                || p.starts_with("/cli/focus-groups/")
                || p.starts_with("/cli/sections/")
                || p.starts_with("/cli/workspace-layouts/")
                || p.starts_with("/cli/timer/")
                || p.starts_with("/cli/presets/")
                || p.starts_with("/cli/window-state/")
                || p.starts_with("/cli/projects/")
        ) => {
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
            let result = crate::db_routes::dispatch_unit4_post(p, &body_bytes);
            send_response(&mut stream, result.status, result.content_type, &result.body)
                .await;
        }
        // Phase 2 Unit 6 — POST routes for filesystem / chat /
        // themes / skill-layers / review-checklist. All JSON-bodied;
        // delegate to per-domain modules. The match-arm guard
        // (`is_post && post_allowed && starts_with`) is the implicit
        // method gate — a GET on these paths falls through to the
        // generic `/cli/` catchall below, which returns a 404
        // "unknown route" since dispatch doesn't have GET handlers
        // for these paths. Functionally equivalent to Unit 5/7a's
        // explicit 405s; the response code differs but no silent
        // mutation is possible either way.
        p if is_post && post_allowed && (
            p.starts_with("/cli/fs/")
                || p.starts_with("/cli/chat/")
                || p.starts_with("/cli/themes/")
                || p.starts_with("/cli/skill-layers/")
                || p.starts_with("/cli/review-checklist/")
        ) => {
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
            let result = dispatch_unit6_post(p, &body_bytes);
            send_response(&mut stream, result.status, "application/json", &result.body)
                .await;
        }
        // Phase 2.1 — workspace inbox POST routes. Query-string only
        // (no body). Token gate is explicit per method-gate rule;
        // body is drained to keep the connection clean. Filesystem
        // operations run in spawn_blocking per F5 (atomic-rename of
        // a `.md` file isn't slow, but `safe_delete::trash` calls
        // into macOS Finder via AppleScript and CAN block).
        p if is_post && post_allowed && p.starts_with("/cli/inbox/") => {
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
            // Drain body (we don't use it — params come from query).
            let _ = read_post_body(&mut stream, &mut buf).await;
            let params = parse_params(&path, &query);
            let p_owned = p.to_string();
            let result = tokio::task::spawn_blocking(move || {
                crate::inbox_routes::dispatch_post(&p_owned, &params)
            })
            .await
            .unwrap_or_else(|e| crate::cli_response::CliResponse {
                status: "500 Internal Server Error",
                content_type: "application/json",
                body: serde_json::json!({ "error": format!("worker join: {e}") })
                    .to_string(),
            });
            send_response(&mut stream, result.status, result.content_type, &result.body).await;
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
    use k2so_core::heartbeats as hb;

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
        "/cli/heartbeat/set-use-workspace-session" => {
            // 0.37.8 — flip the per-heartbeat opt-in to deliver
            // WAKEUP.md into the workspace's pinned chat session
            // instead of the heartbeat's own saved session.
            let name = params.get("name").cloned().unwrap_or_default();
            let enabled = params
                .get("enabled")
                .map(|v| v == "true" || v == "1")
                .unwrap_or(false);
            if name.is_empty() {
                return Err("Missing 'name' parameter".to_string());
            }
            hb::k2so_heartbeat_set_use_workspace_session(
                project_path.to_string(),
                name,
                enabled,
            )
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
            let project_id = k2so_core::workspace::agent_identity::resolve_project_id(&conn, project_path)
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
            let project_id = k2so_core::workspace::agent_identity::resolve_project_id(&conn, project_path)
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
    let project_id = k2so_core::workspace::agent_identity::resolve_project_id(&conn, project_path)
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
    use k2so_core::migrations::unification_0_37_0 as unification;

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

    // (Pre-0.39.0d: `rewrite_legacy_heartbeat_wakeup_paths()` ran
    // here to rewrite pre-0.37.0 `wakeup_path` rows in
    // `workspace_heartbeats` to the post-unification layout. Removed
    // in 0.39.0d now that the 0.37.0+ version floor is established —
    // any workspace upgrading from pre-0.37.0 must first pass
    // through a 0.37.x build to pick up this one-time migration.)

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

    // (Pre-0.39.0d: `archive_legacy_unification_dirs()` ran here to
    // sweep pre-0.37.0 `.k2so/agents/` + `.k2so/agent/heartbeats/`
    // dirs into `.k2so/.archive/0.37.0-unification/`. Removed in
    // 0.39.0d now that the 0.37.0+ version floor is established —
    // workspaces still on the legacy layout must first upgrade
    // through a 0.37.x build to pick up the one-time migration.)

    // 0.37.5: re-key any legacy `<pid>:<agent>` v2_session_map
    // entries to bare `<pid>` so post-upgrade lookups under the new
    // canonical shape land. Defensive — no-op on cold boot since
    // the map starts empty; meaningful when the daemon binary is
    // upgraded without a restart and old in-memory entries linger.
    // MUST run before `boot_sweep_ensure_canonical_sessions` so the
    // sweep's idempotency check sees the post-migration shape.
    crate::v2_session_map::migrate_legacy_keys_to_bare_pid();

    // 0.37.2: proactively ensure each bot-mode workspace has a
    // canonical session registered. Closes the SMS-bridge race
    // window where a webhook's `--wake` arrives before any caller
    // has spawned the canonical PTY. Best-effort per workspace; a
    // failure on one workspace doesn't stop the sweep. See
    // `canonical_session::boot_sweep_ensure_canonical_sessions`.
    crate::canonical_session::boot_sweep_ensure_canonical_sessions();
}

/// Phase 2 Unit 7b — daemon-side replacement for the per-workspace
/// migration loop that previously lived in `src-tauri/src/lib.rs::setup`.
/// Walks every registered project and runs:
///
///   1. `migrate_filenames_to_uppercase`  — agent.md → AGENT.md, etc.
///   2. `detect_interrupted_regen`        — surface stale .regen-in-flight markers.
///   3. `harvest_per_agent_claude_md_files` — archive pre-0.32.7 per-agent CLAUDE.md.
///   4. `migrate_or_scaffold_lead_heartbeat` — manager workspaces get a triage row.
///   5. `ensure_workspace_wakeups`        — scaffold missing wakeup files.
///   6. `promote_legacy_heartbeat`        — single-slot → multi-heartbeat table.
///   7. `repair_mismigrated_heartbeats`   — fix wakeup_path pointing at wrong agent.
///   8. `archive_orphan_top_tier_agents`  — sweep orphan agent dirs.
///   9. `ensure_all_skills_up_to_date`    — universal SKILL.md refresh.
///  10. `migrate_work_to_inbox`           — 0.39.0f Phase 2.1b: relocate
///      `.k2so/work/{inbox,active,done}/*.md` → `.k2so/inbox/{,active,done}/*.md`
///      then send `.k2so/work/` to the macOS Recycle Bin. Marker-gated so
///      re-running is a no-op.
///  11. `consolidate_skills_v1`           — 0.39.0g Phase 2.5b: collapse
///      `.k2so/agents/<x>/` + `.k2so/agent-templates/<x>/` + bare-md
///      `.k2so/skills/<x>.md` into a single home at
///      `.k2so/skills/<x>/SKILL.md`. Collision priority: instance >
///      template > layer; template suffix `-template01..N` on conflict.
///      Renames legacy AGENT.md → SKILL.md, migrates per-skill heartbeats
///      to the workspace-level `.k2so/heartbeats/` with skill-name prefix,
///      sends both source roots to the macOS Recycle Bin. Marker-gated.
///      Runs AFTER work→inbox so this boot still sees the legacy skill
///      layout in the sweeps above (they exit early if missing anyway,
///      but the ordering keeps the per-boot semantics predictable).
///
/// Every helper is idempotent and gated on its own sentinel / row check;
/// the sweep is cheap on a clean boot (no work to do) and resilient if a
/// single workspace explodes — a failure on one path doesn't stop the
/// daemon from booting and serving the rest.
fn run_workspace_legacy_migrations_sweep() {
    use k2so_core::agents::workspace;

    let projects = {
        let db = k2so_core::db::shared();
        let conn = db.lock();
        match k2so_core::db::schema::Project::list(&conn) {
            Ok(rows) => rows,
            Err(e) => {
                log_debug!("[daemon/migrations] WARN: list projects: {e}");
                return;
            }
        }
    };

    if projects.is_empty() {
        return;
    }

    let total = projects.len();
    let mut work_to_inbox_runs = 0usize;
    let mut skills_consolidation_runs = 0usize;
    for project in &projects {
        // Skip the audit-bucket sentinel rows seeded by
        // `db::seed_audit_sentinels`. Their "path" is a bare token,
        // not a real filesystem path; the migration helpers would
        // happily scaffold `<cwd>/_orphan/.k2so/...` and the file
        // watcher would loop.
        if project.id == "_orphan" || project.id == "_broadcast" {
            continue;
        }
        let project_path = std::path::Path::new(&project.path);
        if !project_path.exists() {
            continue;
        }

        workspace::migrate_filenames_to_uppercase(&project.path);
        workspace::detect_interrupted_regen(&project.path);
        workspace::harvest_per_agent_claude_md_files(&project.path);
        workspace::migrate_or_scaffold_lead_heartbeat(&project.path);
        workspace::ensure_workspace_wakeups(&project.path);
        workspace::promote_legacy_heartbeat(&project.path);
        workspace::repair_mismigrated_heartbeats(&project.path);
        let _ = workspace::archive_orphan_top_tier_agents(&project.path);
        workspace::ensure_all_skills_up_to_date(&project.path);

        // 0.39.0f Phase 2.1b: relocate .k2so/work → .k2so/inbox. Marker
        // file (`.k2so/.work-to-inbox-migration-v1-done`) gates re-runs;
        // first-boot per upgraded workspace is the only path that does
        // real work. Per-file errors are logged but don't halt the sweep
        // — the report's `errors` vec carries them for debugging.
        let report = k2so_core::inbox::migrate_work_to_inbox(project_path);
        if !report.already_migrated {
            work_to_inbox_runs += 1;
            log_debug!(
                "[daemon/migrations] migrate_work_to_inbox({}): top={} active={} done={} trashed_root={} errors={}",
                project.path,
                report.moved_top_level,
                report.moved_active,
                report.moved_done,
                report.trashed_work_root,
                report.errors.len(),
            );
            for err in &report.errors {
                log_debug!("[daemon/migrations]   work→inbox err: {err}");
            }
        }

        // 0.39.0g Phase 2.5b: collapse the three legacy skill folders
        // into `.k2so/skills/<x>/SKILL.md`. Marker-gated; first boot
        // per workspace is the only path that does real work. Per-step
        // errors are logged but never abort the sweep.
        let skill_outcome = k2so_core::skills::consolidation::consolidate_skills_v1(
            project_path,
        );
        if !skill_outcome.already_migrated {
            skills_consolidation_runs += 1;
            log_debug!(
                "[daemon/migrations] consolidate_skills_v1({}): bare={} inst={} tmpl={} suffixed={} agent_md_renamed={} agent_md_discarded={} hb={} trashed_agents={} trashed_templates={} errors={}",
                project.path,
                skill_outcome.bare_md_normalized,
                skill_outcome.instances_moved,
                skill_outcome.templates_moved,
                skill_outcome.templates_suffixed,
                skill_outcome.agent_md_renamed,
                skill_outcome.agent_md_discarded,
                skill_outcome.heartbeats_migrated,
                skill_outcome.trashed_agents,
                skill_outcome.trashed_agent_templates,
                skill_outcome.errors.len(),
            );
            for err in &skill_outcome.errors {
                log_debug!("[daemon/migrations]   skills-consolidation err: {err}");
            }
        }
    }
    log_debug!(
        "[daemon/migrations] swept {total} workspace(s) for legacy heartbeat + SKILL migrations; work→inbox ran on {work_to_inbox_runs}; skills-consolidation ran on {skills_consolidation_runs}"
    );
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

// (Pre-0.39.0d: `archive_legacy_unification_dirs()` and
// `rewrite_legacy_heartbeat_wakeup_paths()` lived here as one-time
// pre-0.37.0 → post-0.37.0 layout migrations. Both were removed in
// 0.39.0d once the 0.37.0+ version floor was established — any
// workspace upgrading from pre-0.37.0 must first pass through a
// 0.37.x build to pick up these migrations.)

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

/// Phase 2 Unit 7c — orphan top-tier agent sweep. Inlined handler
/// (instead of a routes module) because the body is two lines of
/// JSON parse + a direct call into `k2so_core::agents::workspace`.
/// Returns `{"success":true,"archived":["<name>", ...]}`.
fn handle_archive_orphans(body: &[u8]) -> cli::CliResponse {
    #[derive(serde::Deserialize)]
    struct Req {
        project_path: String,
    }
    let req: Req = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(e) => return cli::CliResponse::bad_request(format!("invalid body: {e}")),
    };
    let archived = k2so_core::agents::workspace::archive_orphan_top_tier_agents(
        &req.project_path,
    );
    cli::CliResponse::ok_json(
        serde_json::json!({ "success": true, "archived": archived }).to_string(),
    )
}

/// Dispatch a Phase 2 Unit 6 POST request body to the right
/// per-domain handler. Path matching is exact — unknown paths fall
/// through to a 404 so the renderer surfaces "route not found"
/// instead of a silent success.
fn dispatch_unit6_post(path: &str, body: &[u8]) -> cli::CliResponse {
    match path {
        // Filesystem
        "/cli/fs/search-tree" => fs_routes::handle_search_tree(body),
        "/cli/fs/write-file" => fs_routes::handle_write_file(body),
        "/cli/fs/move" => fs_routes::handle_move(body),
        "/cli/fs/copy" => fs_routes::handle_copy(body),
        "/cli/fs/delete" => fs_routes::handle_delete(body),
        "/cli/fs/rename" => fs_routes::handle_rename(body),
        "/cli/fs/create" => fs_routes::handle_create(body),
        "/cli/fs/duplicate" => fs_routes::handle_duplicate(body),
        "/cli/fs/open-finder" => fs_routes::handle_open_finder(body),
        "/cli/fs/open-external" => fs_routes::handle_open_external(body),
        // Chat history (state-mutating)
        "/cli/chat/rename" => chat_routes::handle_rename(body),
        "/cli/chat/toggle-pin" => chat_routes::handle_toggle_pin(body),
        "/cli/chat/migrate-ide" => chat_routes::handle_migrate_ide(body),
        // Themes
        "/cli/themes/create-template" => themes_routes::handle_create_template(body),
        "/cli/themes/delete" => themes_routes::handle_delete(body),
        // Skill layers
        "/cli/skill-layers/create" => skill_layers_routes::handle_create(body),
        "/cli/skill-layers/delete" => skill_layers_routes::handle_delete(body),
        // Review checklist
        "/cli/review-checklist/write" => review_checklist_routes::handle_write(body),
        "/cli/review-checklist/toggle" => review_checklist_routes::handle_toggle(body),
        "/cli/review-checklist/init" => review_checklist_routes::handle_init(body),
        _ => cli::CliResponse::not_found(),
    }
}

/// 32-hex-char cryptographically random token. Same shape as the
/// agent_hooks server's `generate_token` so a future unification is a
/// trivial move.
fn generate_token() -> String {
    let mut buf = [0u8; 16];
    getrandom::getrandom(&mut buf).expect("getrandom failed");
    buf.iter().map(|b| format!("{:02x}", b)).collect()
}

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
mod heartbeat_routes;
mod inbox_routes;
mod llm_host;
mod llm_routes;
mod pending_live;
mod project_config_routes;
mod providers;
mod review_checklist_routes;
mod routes;
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

use tokio::net::TcpListener;
use tokio::sync::broadcast;

use k2so_core::log_debug;

use crate::events::{DaemonBroadcastSink, WireEvent, EVENT_CHANNEL_CAP};

pub(crate) const BANNER: &str = concat!(
    "k2so-daemon ",
    env!("CARGO_PKG_VERSION"),
    " — scaffolding build (tokio)",
);

/// Shared per-process state pulled into every connection task. Cheap to
/// clone: all fields are either `Copy`, `&'static`, or `Arc`-wrapped.
#[derive(Clone)]
pub(crate) struct DaemonState {
    pub token: Arc<String>,
    pub started_at: Instant,
    pub port: u16,
    /// Broadcast channel the daemon's `AgentHookEventSink` publishes into.
    /// Every `/events` WS subscriber takes a `Receiver` off this sender.
    pub event_tx: Arc<broadcast::Sender<WireEvent>>,
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
    // and `k2so_core::migrations::unification_0_37_0`.
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

    // 0.39.0 K2 Connect prep: `legacy_agent_types_v1` migration moved
    // from `src-tauri/src/lib.rs` (where it ran only on Tauri startup).
    // Headless daemons / K2 Connect now pick up the pre-0.34 pod-
    // vocabulary frontmatter rewrite without Tauri ever booting. Gated
    // by the `code_migrations` table so it's a one-shot pass per DB.
    run_legacy_agent_types_v1_migration();

    // 0.39.0 sidebar polish — auto-pin every workspace that was in
    // agent mode pre-0.39.0, so the retirement of the auto-promote-to-
    // top behavior doesn't make users think their agents disappeared.
    // Gated by `code_migrations` so it's a one-shot pass per DB.
    run_auto_pin_existing_agents_migration();

    // 0.39.1 correction — the 0.39.0 ship of the above migration had
    // an over-broad filter that pinned manager/coordinator/pod
    // workspaces too. This corrective migration unpins those for
    // users who already ran the buggy version. Gated by its own
    // `code_migrations` ID so it runs once per DB; no-op for fresh
    // 0.39.1 installs (where the corrected filter pinned only
    // agent + custom from the start).
    run_correct_auto_pin_filter_migration();

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
                                _ = routes::dispatcher::dispatch(stream, st) => {}
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

/// 0.39.0 sidebar polish — daemon-side runner for
/// `auto_pin_existing_agents_0_39_0`.
///
/// Pre-0.39.0 the sidebar auto-promoted every agent-mode workspace
/// into a dedicated "Agents & Pinned" section above the user's
/// manually-pinned workspaces. 0.39.0 retired that auto-promotion —
/// agent-mode workspaces now flow through the same Pinned / focus
/// group / ungrouped sections as any other workspace.
///
/// Without this migration, users upgrading from <0.39.0 would see
/// every workspace that previously appeared in the Agents section
/// vanish from the top of their nav on first 0.39.0 launch. The
/// workspaces themselves are unchanged — they just move from the
/// auto-promoted section to wherever the user's focus group / pinned
/// state placed them. To avoid the surprise, this one-shot migration
/// flips `pinned = 1` for any workspace currently in an agent mode
/// (agent / custom / manager / coordinator / pod) that isn't already
/// pinned. Existing pins are untouched; users can unpin via the
/// existing UI affordance if they don't want agent-mode workspaces
/// in their Pinned section.
///
/// Future workspaces switched into agent mode (post-migration) do
/// NOT auto-pin — they flow through the normal sections like any
/// other workspace. The auto-promote behavior is permanently gone;
/// this migration is just the one-shot bridge.
///
/// Errors are swallowed (logged via `log_debug!`) — a SQL failure
/// here must not stop the daemon from booting.
fn run_auto_pin_existing_agents_migration() {
    use k2so_core::migrations::auto_pin_existing_agents_0_39_0 as mig;

    let db_arc = k2so_core::db::shared();
    let conn = db_arc.lock();

    if k2so_core::db::has_code_migration_applied(&conn, mig::MIGRATION_ID) {
        return;
    }

    let outcome = match mig::run(&conn) {
        Ok(o) => o,
        Err(e) => {
            log_debug!(
                "[daemon/migrations] auto_pin_existing_agents_0_39_0 failed: {e}; will retry on next boot"
            );
            return;
        }
    };

    k2so_core::db::mark_code_migration_applied(
        &conn,
        mig::MIGRATION_ID,
        Some(&format!("pinned {} agent-mode workspaces", outcome.pinned_count)),
    );
    log_debug!(
        "[daemon/migrations] auto_pin_existing_agents_0_39_0: pinned {} agent-mode workspaces; future boots will skip",
        outcome.pinned_count
    );
}

/// 0.39.1 corrective — daemon-side runner for
/// `correct_auto_pin_filter_0_39_1`.
///
/// 0.39.0 shipped with [`auto_pin_existing_agents_0_39_0`] using an
/// over-broad filter that pinned `manager` / `coordinator` / `pod`
/// workspaces in addition to the correct `agent` + `custom`. The
/// pre-0.39.0 sidebar only auto-promoted `agent` + `custom` — the
/// manager family never appeared in the auto-promoted Agents section.
/// This corrective migration unpins the over-pinned manager-family
/// workspaces for users who already ran the buggy 0.39.0 version.
///
/// Gated by its own `code_migrations` ID; one-shot per DB. No-op for
/// fresh 0.39.1 installs where the 0.39.0 migration's corrected
/// filter pinned only agent + custom from the start.
///
/// Errors swallowed (logged via `log_debug!`) — boot must not fail.
fn run_correct_auto_pin_filter_migration() {
    use k2so_core::migrations::correct_auto_pin_filter_0_39_1 as mig;

    let db_arc = k2so_core::db::shared();
    let conn = db_arc.lock();

    if k2so_core::db::has_code_migration_applied(&conn, mig::MIGRATION_ID) {
        return;
    }

    let outcome = match mig::run(&conn) {
        Ok(o) => o,
        Err(e) => {
            log_debug!(
                "[daemon/migrations] correct_auto_pin_filter_0_39_1 failed: {e}; will retry on next boot"
            );
            return;
        }
    };

    k2so_core::db::mark_code_migration_applied(
        &conn,
        mig::MIGRATION_ID,
        Some(&format!(
            "unpinned {} over-pinned manager-family workspaces",
            outcome.unpinned_count
        )),
    );
    log_debug!(
        "[daemon/migrations] correct_auto_pin_filter_0_39_1: unpinned {} workspaces",
        outcome.unpinned_count
    );
}

/// 0.39.0 K2 Connect prep — daemon-side runner for the
/// `legacy_agent_types_v1` AGENT.md frontmatter rewrite.
///
/// Pre-0.34 (pod vocabulary era) workspaces store agent type strings
/// as `type: pod-member` / `type: pod-leader` / `pod_leader: true`.
/// This migration walks every registered project's `.k2so/agents/<n>/`
/// dirs and rewrites those tokens to the post-0.34 vocabulary
/// (`agent-template` / `manager`). Gated by the `code_migrations`
/// table so it's a one-shot pass per local DB.
///
/// Previously lived in `src-tauri/src/lib.rs::setup` and ran only
/// when the Tauri app launched. Moving it daemon-side closes the
/// K2 Connect gap: a remote daemon serving headless workspaces now
/// rewrites legacy frontmatter on its own first boot, so type-aware
/// surfaces don't render stale labels until someone happens to
/// launch K2SO.app.
///
/// Errors are swallowed (logged via `log_debug!`) — a single bad
/// workspace must not stop the daemon from booting.
fn run_legacy_agent_types_v1_migration() {
    use k2so_core::migrations::legacy_agent_types_v1 as mig;

    // DB gating: only run when the marker hasn't landed yet.
    let needs_run = {
        let db_arc = k2so_core::db::shared();
        let conn = db_arc.lock();
        !k2so_core::db::has_code_migration_applied(&conn, mig::MIGRATION_ID)
    };
    if !needs_run {
        return;
    }

    // Snapshot project paths (drops the lock before we start touching
    // the filesystem — the per-file rewrites can be slow if a
    // workspace has hundreds of agent dirs).
    let project_paths: Vec<String> = {
        let db_arc = k2so_core::db::shared();
        let conn = db_arc.lock();
        k2so_core::db::schema::Project::list(&conn)
            .map(|rows| rows.into_iter().map(|p| p.path).collect())
            .unwrap_or_default()
    };

    let outcome = mig::run(project_paths.iter().map(std::path::PathBuf::from));

    // Mark applied unconditionally — matches the pre-move Tauri
    // behavior. A partial failure mid-sweep means the per-file write
    // errors got logged at debug level and we move on. Bumping the
    // migration ID is the escape hatch for "force a re-run."
    {
        let db_arc = k2so_core::db::shared();
        let conn = db_arc.lock();
        k2so_core::db::mark_code_migration_applied(
            &conn,
            mig::MIGRATION_ID,
            Some(&format!("rewrote {} AGENT.md files", outcome.rewritten_count)),
        );
    }
    log_debug!(
        "[daemon/migrations] legacy_agent_types_v1: rewrote {} AGENT.md files; future boots will skip this scan",
        outcome.rewritten_count
    );
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
    // Phase 2.5d: agents::workspace.rs was split into four canonical
    // homes. Pull in the migration helpers + the skill_regen entry
    // point under short aliases that mirror the pre-split call sites
    // below. (Renamed from skill_writer in 0.39.0.)
    use k2so_core::workspace::migrations as workspace;
    use k2so_core::workspace::skill_regen::ensure_all_skills_up_to_date;

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
        ensure_all_skills_up_to_date(&project.path);

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

/// 32-hex-char cryptographically random token. Same shape as the
/// agent_hooks server's `generate_token` so a future unification is a
/// trivial move.
fn generate_token() -> String {
    let mut buf = [0u8; 16];
    getrandom::getrandom(&mut buf).expect("getrandom failed");
    buf.iter().map(|b| format!("{:02x}", b)).collect()
}

// ─────────────────────────────────────────────────────────────────────
// Inline unit tests — boot-only helpers
// ─────────────────────────────────────────────────────────────────────
//
// All HTTP/dispatch tests now live alongside their code (token_ok,
// project_param, parse_params under `routes::http::tests`;
// dispatch_unit6_post under `routes::dispatcher::tests`). What's left
// here is `generate_token`, which is boot-time-only and stays in
// main.rs alongside `async_main`'s consumer.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_token_is_32_hex_chars() {
        let t = generate_token();
        assert_eq!(t.len(), 32, "token should be 32 chars, got {t:?}");
        assert!(
            t.chars().all(|c| c.is_ascii_hexdigit()),
            "token contains non-hex char: {t:?}",
        );
    }

    #[test]
    fn generate_token_produces_unique_values() {
        // 16 random bytes → collision probability astronomically low.
        // Two calls back-to-back must differ.
        let a = generate_token();
        let b = generate_token();
        assert_ne!(a, b, "tokens should not collide: {a} vs {b}");
    }
}

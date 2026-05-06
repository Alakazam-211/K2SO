#[cfg(target_os = "macos")]
#[macro_use]
extern crate objc;

// Bring k2so-core's `log_debug!` + `perf_timer!` + `perf_hist!` into scope
// across every child module, matching the previous behavior of having
// them defined inline in this file.
#[macro_use]
extern crate k2so_core;

/// Flag to skip _exit(0) during relaunch (set by the frontend before process::relaunch)
static RELAUNCH_MODE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);


mod agent_hooks;
mod commands;
mod tray;
// `companion` now lives in k2so-core. Re-exported so existing
// `crate::companion::*` paths (commands/companion.rs, agent_hooks.rs,
// commands/settings.rs) keep working.
pub use k2so_core::companion;
// Modules opened for the benches at src-tauri/benches/perf.rs — the k2so_lib
// crate is not published, so this is a no-op for real consumers. Revert to
// `mod` once the perf pass is over if we decide the benches' existence
// doesn't justify open modules.
// `db` now lives in k2so-core. Re-exported so callers can keep using
// `crate::db::shared()` etc. unchanged. Migrations are bundled into the
// k2so-core binary via include_str! from crates/k2so-core/drizzle_sql/.
pub use k2so_core::db;
// `editors`, `fs_abstract`, `fs_atomic`, `project_config` now live in
// k2so-core (pure std + serde; no Tauri dep). Re-exported so existing
// `crate::editors::*` / `crate::fs_abstract::*` / etc. call sites keep
// working unchanged.
pub use k2so_core::{editors, fs_abstract, fs_atomic, project_config};
// `git` module (libgit2 wrappers, worktree/branch/diff/merge) moved to
// k2so-core so k2so_agents_delegate + the daemon's future supervised-
// launch path can share the same code. Re-exported at the historical
// `crate::git::*` path so all existing call sites resolve unchanged.
pub use k2so_core::git;
// `llm` now lives in k2so-core. Downstream callers keep their
// `crate::llm::*` paths working through this re-export.
pub use k2so_core::llm;
mod menu;
// `perf` now lives in the k2so-core crate. Re-exported so existing
// `crate::perf_timer!` / `crate::perf_hist!` / `crate::perf::*` call sites
// keep working unchanged. See crates/k2so-core/src/perf.rs.
pub use k2so_core::{perf, perf_hist, perf_timer};
mod state;
// Tauri-side HTTP client for the k2so-daemon. Routes state-mutating
// commands through the daemon's loopback HTTP instead of running them
// in-process. Small for now (ping + status); grows as daemon handlers
// land.
mod daemon_client;
// Tauri-backed provider for k2so-core::companion::settings_bridge,
// registered in setup() before the companion module reads credentials.
mod companion_settings_provider;
// Tauri-backed providers for k2so-core::companion's terminal /
// event-sink / app-event-source bridges. Needs an AppHandle so lives
// behind a register() call in setup().
mod companion_host;
// Tauri-backed AgentHookEventSink impl registered in setup() — routes
// agent-hook events (agent:lifecycle, agent:reply, sync:projects, …)
// back onto Tauri's event bus.
mod agent_hook_sink;
// Tauri-backed WorkspaceRegenProvider impl — lets the core
// build_launch path eagerly regen workspace SKILL.md through the
// src-tauri scaffolding orchestrator.
mod workspace_regen_provider;
// Background subscriber for the daemon's /events WebSocket. Spawned in
// setup() once; reconnects forever so we survive daemon restarts.
mod daemon_events;
// `terminal` now lives in k2so-core. Re-exported so existing
// `crate::terminal::*` paths keep working.
pub use k2so_core::terminal;
// Local Tauri-backed implementation of k2so_core::terminal::TerminalEventSink.
mod terminal_event_sink;
mod watcher;
mod window;

use state::AppState;
use std::collections::HashMap;
use parking_lot::Mutex;
use tauri::{Emitter, Manager};

/// H7: sync `k2so_core::hook_config` with the daemon's port + token so
/// in-process Alacritty children emit `/hook/complete` requests at the
/// daemon (the sole HTTP server post-H7). Reads `~/.k2so/daemon.port`
/// + `~/.k2so/daemon.token` — written by the daemon on startup. Runs
/// off-thread with a small retry loop for the cold-start case where
/// Tauri wins the race against launchd's daemon spawn.
///
/// Best-effort: if the daemon files stay missing for the 5 retries,
/// Tauri boots without hook-config wired. New sessions will still
/// reach the daemon's `/cli/*` routes via CLI tools that read the
/// files dynamically; only the in-process Alacritty hook-script path
/// would be deaf, which is a degraded-but-usable state.
fn prime_hook_config_from_daemon() {
    std::thread::spawn(|| {
        use std::time::Duration;
        let Some(home) = dirs::home_dir() else { return };
        let port_path = home.join(".k2so/daemon.port");
        let token_path = home.join(".k2so/daemon.token");
        for attempt in 0..5 {
            let port_ok = std::fs::read_to_string(&port_path)
                .ok()
                .and_then(|s| s.trim().parse::<u16>().ok());
            let token_ok = std::fs::read_to_string(&token_path)
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            if let (Some(port), Some(token)) = (port_ok, token_ok) {
                k2so_core::hook_config::set_port(port);
                k2so_core::hook_config::set_token(token);
                log_debug!(
                    "[h7/hook-config] primed from daemon (port={}, attempt={})",
                    port,
                    attempt
                );
                return;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        log_debug!(
            "[h7/hook-config] daemon port/token files unavailable after 5 retries; \
             Alacritty children will boot with empty hook_config"
        );
    });
}

/// Detect daemon/Tauri version mismatch on app startup and bounce the
/// running daemon when they disagree.
///
/// Background: the daemon is launchd-managed (`KeepAlive=true`), so a
/// drag-replace install of K2SO.app overwrites the binary on disk while
/// launchd keeps the OLD daemon process running with the deleted inode
/// — meaning a freshly-installed K2SO talks to last-version's daemon
/// until the user reboots or manually clicks Settings → Restart Daemon.
/// `daemon_restart()` exists for that manual path; this function is
/// the automatic version of the same idea.
///
/// Runs on a background thread (mirrors `prime_hook_config_from_daemon`)
/// because we must wait for the daemon to be reachable + we don't want
/// to block Tauri's setup hook on a synchronous HTTP round-trip. Polls
/// up to 10× at 500ms intervals; if the daemon stays unreachable we
/// log and bow out — bigger problem than version skew at that point.
fn check_daemon_version_and_restart() {
    use std::time::Duration;
    std::thread::spawn(|| {
        let app_version = env!("CARGO_PKG_VERSION");
        for attempt in 0..10 {
            std::thread::sleep(Duration::from_millis(500));
            let client = match crate::daemon_client::DaemonClient::try_connect() {
                Ok(c) => c,
                Err(_) => continue,
            };
            let status = match client.status() {
                Ok(s) => s,
                Err(_) => continue,
            };
            if status.version == app_version {
                log_debug!(
                    "[version-check] daemon v{} matches app v{} (attempt={})",
                    status.version,
                    app_version,
                    attempt
                );
                return;
            }
            log_debug!(
                "[version-check] MISMATCH daemon=v{} app=v{} (attempt={}); restarting daemon via launchctl kickstart",
                status.version,
                app_version,
                attempt
            );
            match crate::commands::daemon::kickstart_daemon() {
                Ok(()) => log_debug!("[version-check] launchctl kickstart succeeded"),
                Err(e) => log_debug!("[version-check] launchctl kickstart failed: {e}"),
            }
            return;
        }
        log_debug!(
            "[version-check] daemon unreachable after 10 attempts; skipping version check"
        );
    });
}

/// Entry point for the LLM worker subprocess.
/// Loads the model, runs inference, prints the result to stdout, then exits.
pub fn llm_worker_main(payload_path: &str) {
    let raw = match std::fs::read_to_string(payload_path) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Failed to read payload: {e}");
            std::process::exit(1);
        }
    };

    let payload: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Failed to parse payload: {e}");
            std::process::exit(1);
        }
    };

    let model_path = payload["model"].as_str().unwrap_or("");
    let system_prompt = payload["system"].as_str().unwrap_or("");
    let user_message = payload["message"].as_str().unwrap_or("");

    let mut manager = llm::LlmManager::new();
    if let Err(e) = manager.load_model(model_path) {
        eprintln!("Failed to load model: {e}");
        std::process::exit(1);
    }

    match manager.generate(system_prompt, user_message) {
        Ok(output) => {
            // Write to stdout and flush BEFORE _exit (which skips cleanup)
            use std::io::Write;
            let _ = std::io::stdout().write_all(output.as_bytes());
            let _ = std::io::stdout().flush();
            // Force-exit to skip Metal cleanup (same _exit trick as shutdown)
            unsafe { libc::_exit(0); }
        }
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}

/// Re-export so `main.rs` can fire the reqwest warmup thread BEFORE
/// `run()` starts Tauri's window initialization. See
/// `commands::kessel::warm_http_pool_async` for rationale — in one
/// sentence, the first `reqwest::blocking::Client::send()` call takes
/// ~500-800ms to spin up a tokio runtime, and we want that cost
/// absorbed by daemon-creds polling instead of the first Cmd+T.
pub fn warm_http_pool_async() {
    commands::kessel::warm_http_pool_async();
}

pub fn run() {
    // Ignore SIGPIPE so writing to a dead PTY returns EPIPE instead of
    // killing the entire process.
    #[cfg(unix)]
    terminal::ignore_sigpipe();

    // launchd-launched .app processes inherit a sparse PATH that lacks
    // ~/.local/bin, /opt/homebrew/bin, and other user-installed prefixes.
    // Source the user's login shell once and adopt its PATH so legacy
    // alacritty spawns + every Command::new call site can resolve user
    // tools. See docs in k2so_core::enrich_path_from_login_shell.
    #[cfg(unix)]
    k2so_core::enrich_path_from_login_shell();

    // Rustls 0.23 compiles both aws-lc-rs (via reqwest rustls-tls) and ring
    // (via ngrok) into the binary; it refuses to auto-pick and panics on
    // first TLS use unless a provider is explicitly installed.
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    // Wire up k2so-core's companion settings bridge to this app's
    // settings.json. Must happen before the companion tunnel ever
    // starts or any WS client authenticates.
    k2so_core::companion::settings_bridge::set_provider(Box::new(
        companion_settings_provider::TauriCompanionSettingsProvider,
    ));

    let db_handle = perf_timer!("startup_db_init", {
        match db::init_database() {
            Ok(c) => c,
            Err(e) => {
                log_debug!("[k2so] FATAL: Failed to initialize database: {}", e);
                log_debug!("[k2so] The app will now exit. Check disk permissions and space at ~/.k2so/");
                std::process::exit(1);
            }
        }
    });

    let app_state = AppState {
        // Same Arc lives in AppState and in db::SHARED — Tauri commands
        // and HTTP endpoints take the same write lock on the same
        // physical SQLite connection.
        db: db_handle,
        // Arc clone of the k2so-core singletons. AppState is now a
        // handle collection, not the owner — companion + future
        // agent_hooks in core see the same underlying managers.
        terminal_manager: terminal::shared(),
        llm_manager: llm::shared(),
        watchers: Mutex::new(HashMap::new()),
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_drag::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .manage(app_state)
        .menu(|handle| menu::create_menu(handle))
        .on_menu_event(menu::handle_menu_event)
        .setup(|app| {
            let __setup_start = std::time::Instant::now();
            struct SetupGuard(std::time::Instant);
            impl Drop for SetupGuard {
                fn drop(&mut self) {
                    if crate::perf::is_enabled() {
                        use std::io::Write;
                        let _ = writeln!(
                            std::io::stderr(),
                            "[perf] startup_setup_total — {}µs",
                            self.0.elapsed().as_micros()
                        );
                    }
                }
            }
            let _setup_guard = SetupGuard(__setup_start);

            // Register Tauri-side impls for k2so-core's companion
            // terminal / event-sink / app-event-source bridges. Must
            // happen before any companion code runs or subscribes.
            companion_host::register(app.handle().clone());

            // Agent-hook event sink: routes k2so_core::agent_hooks::emit
            // onto AppHandle::emit. Registered before any hook HTTP
            // request can land.
            k2so_core::agent_hooks::set_sink(Box::new(
                agent_hook_sink::TauriAgentHookEventSink::new(app.handle().clone()),
            ));

            // Workspace regen bridge: lets
            // k2so_core::agents::build_launch invoke the src-tauri-
            // resident workspace-SKILL.md scaffolding orchestrator
            // (`k2so_agents_generate_workspace_claude_md`). Daemon
            // + test contexts run without a provider and silently
            // skip the eager regen; freshness arrives on next Tauri
            // startup.
            k2so_core::agents::workspace_regen::set_provider(Box::new(
                workspace_regen_provider::TauriWorkspaceRegenProvider,
            ));

            // Subscribe to the daemon's /events WebSocket. Daemon-
            // originated hook events arrive here and are re-emitted via
            // AppHandle::emit exactly as if agent_hooks.rs had handled
            // them locally. No-op until the daemon is running; reconnects
            // forever so we survive launchctl unload/load cycles.
            daemon_events::spawn_subscriber(app.handle().clone());

            // Migrate old JSON window state to SQLite (one-time migration)
            perf_timer!("startup_migrate_window_state", {
                window::migrate_json_window_state(app.handle());
            });

            // Migrate workspace_layouts from settings.json → SQLite (one-time)
            perf_timer!("startup_migrate_workspace_layouts", {
                migrate_workspace_layouts_to_db(app.handle());
            });

            // Create skill layer template directories if they don't exist
            if let Some(home) = dirs::home_dir() {
                let templates = home.join(".k2so/templates");
                let _ = std::fs::create_dir_all(templates.join("manager"));
                let _ = std::fs::create_dir_all(templates.join("agent-template"));
                let _ = std::fs::create_dir_all(templates.join("custom-agent"));
            }

            // Migrate legacy agent types in AGENT.md files (pod-member → agent-template,
            // pod-leader → manager). Gated via the `code_migrations` table so this
            // only runs the first time post-upgrade; subsequent launches skip entirely
            // instead of rescanning every AGENT.md in every project.
            perf_timer!("startup_migrate_legacy_agent_types", {
                const MIGRATION_ID: &str = "legacy_agent_types_v1";
                let needs_run = {
                    let state = app.state::<AppState>();
                    let db = state.db.lock();
                    !db::has_code_migration_applied(&db, MIGRATION_ID)
                };
                if needs_run {
                    let paths: Vec<String> = {
                        let state = app.state::<AppState>();
                        let db = state.db.lock();
                        let mut p = Vec::new();
                        if let Ok(mut stmt) = db.prepare("SELECT path FROM projects") {
                            if let Ok(rows) = stmt.query_map([], |row| row.get::<_, String>(0)) {
                                for row in rows.flatten() { p.push(row); }
                            }
                        }
                        p
                    };
                    let mut rewritten_count = 0usize;
                    for path in &paths {
                        let agents_dir = std::path::PathBuf::from(path).join(".k2so/agents");
                        if !agents_dir.exists() { continue; }
                        if let Ok(entries) = std::fs::read_dir(&agents_dir) {
                            for entry in entries.flatten() {
                                let agent_md = entry.path().join("AGENT.md");
                                if !agent_md.exists() { continue; }
                                if let Ok(content) = std::fs::read_to_string(&agent_md) {
                                    let mut updated = content.clone();
                                    let mut changed = false;
                                    if updated.contains("type: pod-member") {
                                        updated = updated.replace("type: pod-member", "type: agent-template");
                                        changed = true;
                                    }
                                    if updated.contains("type: pod-leader") {
                                        updated = updated.replace("type: pod-leader", "type: manager");
                                        changed = true;
                                    }
                                    if updated.contains("pod_leader: true") {
                                        updated = updated.replace("pod_leader: true", "manager: true");
                                        changed = true;
                                    }
                                    if changed {
                                        let _ = std::fs::write(&agent_md, &updated);
                                        rewritten_count += 1;
                                    }
                                }
                            }
                        }
                    }
                    // Record completion — idempotent via INSERT OR IGNORE.
                    let state = app.state::<AppState>();
                    let db = state.db.lock();
                    db::mark_code_migration_applied(
                        &db,
                        MIGRATION_ID,
                        Some(&format!("rewrote {} AGENT.md files", rewritten_count)),
                    );
                    log_debug!(
                        "[k2so] legacy_agent_types_v1: rewrote {} AGENT.md files; future launches will skip this scan",
                        rewritten_count
                    );
                }
            });

            // Install the k2so-daemon launchd agent if it isn't already
            // installed AND the daemon binary is bundled next to us.
            // Gated via `code_migrations` so this runs exactly once per
            // upgrade. In debug builds we opt out by default — the
            // `target/debug/k2so-daemon` path is volatile, and a dev
            // with `K2SO_INSTALL_DAEMON=1` can override.
            perf_timer!("startup_install_daemon_plist", {
                // v2 bump: v1 was burned during 0.33.0 RC testing when
                // dev launches with `K2SO_INSTALL_DAEMON=1` marked it
                // applied against an earlier k2so-daemon binary path.
                // Bumping the ID forces a re-install against the current
                // bundled daemon binary on the first 0.33.0 launch for
                // anyone carrying a stale v1 row. Safe for fresh users
                // (neither row present → runs as usual).
                const MIGRATION_ID: &str = "install_daemon_plist_v2";
                let needs_run = {
                    let state = app.state::<AppState>();
                    let db = state.db.lock();
                    !db::has_code_migration_applied(&db, MIGRATION_ID)
                };
                let opted_in = !cfg!(debug_assertions)
                    || std::env::var("K2SO_INSTALL_DAEMON").is_ok();
                if needs_run && opted_in {
                    // Locate k2so-daemon next to the current Tauri binary
                    // (inside K2SO.app/Contents/MacOS/). Skip install if
                    // it isn't bundled yet — earlier 0.33.x dev builds
                    // may ship without it.
                    let maybe_daemon = std::env::current_exe()
                        .ok()
                        .and_then(|p| p.parent().map(|d| d.join("k2so-daemon")))
                        .filter(|p| p.exists());
                    match maybe_daemon {
                        Some(daemon_bin) => {
                            let plist = k2so_core::wake::DaemonPlist::canonical(daemon_bin.clone());
                            match k2so_core::wake::install(&plist) {
                                Ok(path) => {
                                    log_debug!(
                                        "[k2so] installed daemon plist at {} pointing at {}",
                                        path.display(),
                                        daemon_bin.display()
                                    );
                                    let state = app.state::<AppState>();
                                    let db = state.db.lock();
                                    db::mark_code_migration_applied(
                                        &db,
                                        MIGRATION_ID,
                                        Some(&format!("installed from {}", daemon_bin.display())),
                                    );
                                }
                                Err(e) => {
                                    // Don't mark applied — next launch will
                                    // retry. Common failure: launchctl
                                    // complaining about "Load failed: 5:
                                    // Input/output error" which usually
                                    // means a stale plist is already
                                    // loaded. User can resolve via
                                    // `launchctl unload ~/Library/LaunchAgents/com.k2so.k2so-daemon.plist`.
                                    log_debug!("[k2so] daemon plist install failed: {e}");
                                }
                            }
                        }
                        None => {
                            // Bundled daemon missing — common in pre-0.33
                            // dev builds. Leave the migration unapplied
                            // so a later launch (with the daemon bundled)
                            // completes it.
                            log_debug!(
                                "[k2so] daemon binary not found next to current exe; skipping plist install"
                            );
                        }
                    }
                }
            });

            // Autostart: on every Tauri launch (not just first-install
            // migration), make sure the daemon plist is loaded. Covers
            // the case where the user red-buttoned with "keep server
            // running" OFF (which unloads the plist) and then relaunched
            // the app — they expect the daemon to be back without having
            // to click "Restart" in Settings. Fires regardless of the
            // toggle, in both debug and release builds.
            perf_timer!("startup_ensure_daemon_loaded", {
                let plist = k2so_core::wake::DaemonPlist::canonical(
                    std::path::PathBuf::from("/unused"),
                );
                match k2so_core::wake::ensure_loaded(&plist) {
                    Ok(k2so_core::wake::LoadOutcome::AlreadyLoaded) => {
                        log_debug!("[k2so] daemon already loaded in launchctl");
                    }
                    Ok(k2so_core::wake::LoadOutcome::Loaded) => {
                        log_debug!("[k2so] daemon plist loaded (was unloaded)");
                    }
                    Ok(k2so_core::wake::LoadOutcome::NotInstalled) => {
                        log_debug!(
                            "[k2so] daemon plist not installed — install migration will handle it"
                        );
                    }
                    Err(e) => {
                        log_debug!("[k2so] daemon autostart failed: {e}");
                    }
                }
            });

            // SKILL.md regeneration for all workspaces. 0.32.13 changes:
            //
            // 1. Version gate — only regen when the project's last-regen
            //    K2SO version differs from the current binary. Binary
            //    upgrades trigger one regen; subsequent launches at the
            //    same version skip the entire pass (baseline: 3.8 s →
            //    ~few ms for the DB read).
            // 2. Background deferral — the queue of projects that do
            //    need regen runs on a post-UI thread. The window shows
            //    immediately; skill writes complete asynchronously and
            //    emit `startup:skill_regen_complete` when done.
            perf_timer!("startup_skill_regen_gate", {
                const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
                let stale_projects: Vec<(String, String)> = {
                    let state = app.state::<AppState>();
                    let db = state.db.lock();
                    let mut projects = Vec::new();
                    if let Ok(mut stmt) = db.prepare(
                        "SELECT path, agent_mode, skill_regen_version FROM projects",
                    ) {
                        if let Ok(rows) = stmt.query_map([], |row| {
                            let path: String = row.get(0)?;
                            let mode: String = row.get(1)?;
                            let last_ver: Option<String> = row.get(2)?;
                            Ok((path, mode, last_ver))
                        }) {
                            for row in rows.flatten() {
                                let (path, mode, last_ver) = row;
                                // Stale if never regen'd OR the binary
                                // version has moved since last regen.
                                let stale = last_ver
                                    .as_deref()
                                    .map(|v| v != CURRENT_VERSION)
                                    .unwrap_or(true);
                                if stale {
                                    projects.push((path, mode));
                                }
                            }
                        }
                    }
                    projects
                };

                if stale_projects.is_empty() {
                    log_debug!(
                        "[k2so] SKILL regen: all projects current at {} — skipping",
                        CURRENT_VERSION
                    );
                } else {
                    log_debug!(
                        "[k2so] SKILL regen: {} project(s) stale, deferring to background",
                        stale_projects.len()
                    );
                    let handle_for_thread = app.handle().clone();
                    std::thread::spawn(move || {
                        let bg_start = std::time::Instant::now();
                        for (path, mode) in &stale_projects {
                            if mode != "off" {
                                let _ = commands::k2so_agents::k2so_agents_regenerate_skills(path.clone());
                                let _ = commands::k2so_agents::k2so_agents_generate_workspace_claude_md(path.clone());
                            }
                            commands::k2so_agents::write_workspace_skill_file(path);
                            // Record completion for this project so the
                            // next launch skips it.
                            let state = handle_for_thread.state::<AppState>();
                            let db = state.db.lock();
                            let _ = db.execute(
                                "UPDATE projects SET skill_regen_version = ?1 WHERE path = ?2",
                                rusqlite::params![CURRENT_VERSION, path],
                            );
                        }
                        if crate::perf::is_enabled() {
                            use std::io::Write;
                            let _ = writeln!(
                                std::io::stderr(),
                                "[perf] startup_skill_regen_background — {}µs ({} projects)",
                                bg_start.elapsed().as_micros(),
                                stale_projects.len()
                            );
                        }
                        let _ = handle_for_thread.emit(
                            "startup:skill_regen_complete",
                            serde_json::json!({
                                "projectCount": stale_projects.len(),
                                "durationMs": bg_start.elapsed().as_millis(),
                            }),
                        );
                    });
                }
            });

            // Apply saved window state on startup
            if let Some(saved) = window::load_window_state(app.handle()) {
                if let Some(win) = app.get_webview_window("main") {
                    use tauri::PhysicalPosition;
                    use tauri::PhysicalSize;
                    let _ = win.set_position(PhysicalPosition::new(saved.x, saved.y));
                    let _ = win.set_size(PhysicalSize::new(saved.width, saved.height));
                    if saved.is_maximized {
                        let _ = win.maximize();
                    }
                }
            }
            // Native WebKit zoom is disabled via zoomHotkeysEnabled:false in tauri.conf.json.
            // App zoom is handled by transform:scale() in the frontend (App.tsx).

            // Save window state and clean up terminals on close
            let app_handle = app.handle().clone();
            if let Some(win) = app.get_webview_window("main") {
                let win_for_hide = win.clone();
                win.on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                        // Red close-button behavior is controlled by
                        // the "Keep Agent & Companion server running
                        // when K2SO quits" preference:
                        //
                        //   ON  → hide the window, keep Tauri + server
                        //         alive. Menubar icon stays visible so
                        //         the user can see what's still running.
                        //         Full quit happens only via Cmd+Q or
                        //         the menubar "Quit K2SO" item.
                        //   OFF → behave like a normal app quit: tear
                        //         down in-app companion + daemon plist
                        //         (if installed), then proceed to
                        //         destroy.
                        //
                        // Cmd+Q is deliberately NOT routed through here
                        // (NSApplication terminate: goes straight to
                        // RunEvent::ExitRequested) — it always closes
                        // everything regardless of the toggle.
                        let keep_running =
                            commands::settings::read_settings().keep_daemon_on_quit;
                        if keep_running {
                            window::save_window_state(&app_handle);
                            api.prevent_close();
                            let _ = win_for_hide.hide();
                            log_debug!("[window] Close intercepted — keeping server alive per settings");
                            return;
                        }
                        // Toggle OFF — user wants red-dot to take
                        // everything down. Unload the daemon plist (if
                        // installed) so launchd stops respawning it,
                        // then fall through to the normal cleanup +
                        // destroy path below.
                        let plist = k2so_core::wake::DaemonPlist::canonical(
                            std::path::PathBuf::from("/unused"),
                        );
                        if let Some(path) = plist.plist_path() {
                            if path.exists() {
                                let _ = k2so_core::wake::launchctl_unload(&path);
                            }
                        }
                        window::save_window_state(&app_handle);

                        // Parallelize LLM unload and terminal kill with a 5-second timeout.
                        // These have no dependency on each other and can run concurrently.
                        // Zed pattern: log panics instead of silently swallowing them.
                        let handle_for_llm = app_handle.clone();
                        let handle_for_term = app_handle.clone();

                        let llm_thread = std::thread::spawn(move || {
                            if let Err(panic) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                if let Some(state) = handle_for_llm.try_state::<AppState>() {
                                    // Use try_lock to avoid blocking if model is still loading
                                    if let Some(mut manager) = state.llm_manager.try_lock() {
                                        manager.unload();
                                    } else {
                                        log_debug!("[shutdown] LLM lock busy (model loading?) — skipping unload");
                                    }
                                }
                            })) {
                                let msg = panic.downcast_ref::<String>()
                                    .map(|s| s.as_str())
                                    .or_else(|| panic.downcast_ref::<&str>().copied())
                                    .unwrap_or("unknown panic");
                                log_debug!("[shutdown] LLM unload panicked: {}", msg);
                            }
                        });

                        let term_thread = std::thread::spawn(move || {
                            if let Err(panic) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                if let Some(state) = handle_for_term.try_state::<AppState>() {
                                    let mut manager = state.terminal_manager.lock();
                                    manager.kill_all();
                                }
                            })) {
                                let msg = panic.downcast_ref::<String>()
                                    .map(|s| s.as_str())
                                    .or_else(|| panic.downcast_ref::<&str>().copied())
                                    .unwrap_or("unknown panic");
                                log_debug!("[shutdown] Terminal kill panicked: {}", msg);
                            }
                        });

                        // Wait up to 5 seconds for both to complete (increased from 2s).
                        // LLM Metal cleanup and terminal process reaping can take time.
                        let timeout = std::time::Duration::from_secs(5);
                        let (done_tx, done_rx) = std::sync::mpsc::channel();
                        let done_tx2 = done_tx.clone();

                        std::thread::spawn(move || {
                            let _ = llm_thread.join();
                            let _ = done_tx.send("llm");
                        });
                        std::thread::spawn(move || {
                            let _ = term_thread.join();
                            let _ = done_tx2.send("term");
                        });

                        let start = std::time::Instant::now();
                        let mut completed = 0u32;
                        while completed < 2 {
                            let remaining = timeout.saturating_sub(start.elapsed());
                            if remaining.is_zero() {
                                log_debug!("[shutdown] Cleanup timed out after 5s — exiting anyway");
                                break;
                            }
                            match done_rx.recv_timeout(remaining) {
                                Ok(_) => completed += 1,
                                Err(_) => {
                                    log_debug!("[shutdown] Cleanup timed out after 5s — exiting anyway");
                                    break;
                                }
                            }
                        }

                        // Phase 4 H7: the daemon is the sole writer of
                        // ~/.k2so/heartbeat.port since Tauri retired
                        // its HTTP listener. Do NOT remove the file on
                        // Tauri quit — the daemon process keeps running
                        // (launchd-managed) and the CLI still needs a
                        // valid port file to find it.

                        // Force-drop the LLM model to release Metal/GPU resources.
                        if let Some(state) = app_handle.try_state::<AppState>() {
                            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                let mut manager = state.llm_manager.lock();
                                manager.unload();
                            }));
                        }

                        // Use _exit() to skip C++ static destructors (ggml_metal).
                        // Without this, __cxa_finalize_ranges runs ggml's Metal cleanup
                        // which races against macOS Metal device teardown → SIGABRT.
                        // Skip _exit during relaunch so the process plugin can spawn
                        // the new process before this one exits.
                        if RELAUNCH_MODE.load(std::sync::atomic::Ordering::Relaxed) {
                            log_debug!("[shutdown] Relaunch mode — using normal exit");
                            std::process::exit(0);
                        } else {
                            unsafe { libc::_exit(0); }
                        }
                    }
                });
            }
            // Phase 4 H7: Tauri no longer runs its own HTTP listener.
            // The k2so-daemon (launchd-managed) is the sole server for
            // every /cli/* route + /hook/complete + /events WS, and
            // writes heartbeat.port + heartbeat.token itself on startup.
            //
            // What Tauri still does here:
            //   1. Write the notify.sh hook script to disk. The script
            //      reads heartbeat.port at exec time, so it doesn't
            //      need Tauri's port — just needs to exist on disk for
            //      `register_all_hooks` to point ~/.claude/settings.json
            //      at it.
            //   2. Register those hooks with claude/cursor/etc so their
            //      lifecycle events curl into the daemon's /hook/complete.
            //   3. Sync hook_config so in-process Alacritty children
            //      inject the daemon's port + token into child envs —
            //      handled by `prime_hook_config_from_daemon` below.
            //
            // What Tauri no longer does (moved to daemon):
            //   - Bind a TCP listener. The old `agent_hooks::start_server`
            //     call is gone; its 60+ /cli/* routes are all served by
            //     k2so-daemon now.
            //   - Write heartbeat.port / heartbeat.token. Daemon does
            //     it eagerly on startup.
            //   - Clean up stale heartbeat.port files. Same reasoning:
            //     daemon is the owner, not us.
            match agent_hooks::write_hook_script(0) {
                Ok(script_path) => {
                    agent_hooks::register_all_hooks(&app.handle().clone(), &script_path);
                    log_debug!("[agent-hooks] Hook scripts registered at {}", script_path);
                }
                Err(e) => {
                    log_debug!("[agent-hooks] Failed to write hook script: {}", e);
                    let _ = app.handle().emit(
                        "hook-injection-failed",
                        serde_json::json!({
                            "failures": [{"cli": "notify-script", "error": e}]
                        }),
                    );
                }
            }
            prime_hook_config_from_daemon();
            check_daemon_version_and_restart();
            {
                // One-shot migration: ensure every registered project has
                // a workspace `.k2so/wakeup.md` and every existing agent
                // has its own `wakeup.md` (if its type supports wake-up).
                // Runs on every launch but is a no-op for projects already
                // migrated — `ensure_*_wakeup` never overwrites an existing
                // file. Spawns off the main thread so a slow filesystem
                // can't delay startup.
                {
                    let handle = app.handle().clone();
                    std::thread::spawn(move || {
                        use tauri::Manager;
                        let projects = match handle.try_state::<crate::state::AppState>() {
                            Some(state) => {
                                let conn = state.db.lock();
                                crate::db::schema::Project::list(&conn).unwrap_or_default()
                            }
                            None => return,
                        };
                        for project in &projects {
                            // Skip audit-bucket sentinels (`_orphan`,
                            // `_broadcast`) — these are SQL-only rows
                            // seeded by `db::seed_audit_sentinels` and
                            // their "path" is a bare token, not a real
                            // filesystem path. Treating them as projects
                            // made every migration write scaffolds to
                            // `<cwd>/_orphan/` and `<cwd>/_broadcast/`,
                            // which under `tauri dev` (CWD=src-tauri)
                            // caused the file watcher to detect those
                            // writes and restart the app in an infinite
                            // loop (Phase 4.5 diagnostic).
                            if project.id == "_orphan" || project.id == "_broadcast" {
                                continue;
                            }
                            // 0.32.7 filename standardization: rename all lowercase
                            // agent.md / wakeup.md on disk → AGENT.md / WAKEUP.md.
                            // Must run BEFORE the heartbeat migrations below so those
                            // find the UPPERCASE filenames on disk. Idempotent.
                            crate::commands::k2so_agents::migrate_filenames_to_uppercase(&project.path);
                            // 0.32.7 CLAUDE.md harvest: archive any per-agent
                            // Detect whether a previous SKILL.md regen crashed
                            // mid-way. Doesn't auto-repair — subsequent regens
                            // are idempotent — but surfaces a diagnostic so the
                            // user can inspect .k2so/migration/ if they hit
                            // unexpected staleness.
                            crate::commands::k2so_agents::detect_interrupted_regen(&project.path);
                            // CLAUDE.md files left behind by the pre-0.32.7
                            // generator into .k2so/migration/ so nothing the
                            // user (or Claude `# memory`) authored is lost.
                            // Root ./CLAUDE.md is handled by the workspace
                            // skill writer later in the boot path. Idempotent.
                            crate::commands::k2so_agents::harvest_per_agent_claude_md_files(&project.path);
                            // Multi-heartbeat migration / scaffold for __lead__. Must run
                            // before `ensure_workspace_wakeups` so the legacy
                            // `.k2so/wakeup.md` content gets picked up before any new
                            // scaffold writes over it. Idempotent.
                            crate::commands::k2so_agents::migrate_or_scaffold_lead_heartbeat(&project.path);
                            crate::commands::k2so_agents::ensure_workspace_wakeups(&project.path);
                            // One-time promote of legacy single-slot heartbeat_schedule
                            // into the multi-heartbeat agent_heartbeats table. Idempotent.
                            crate::commands::k2so_agents::promote_legacy_heartbeat(&project.path);
                            // Repair any mis-migrated rows from earlier 0.32.0 runs where
                            // find_primary_agent picked an orphan agent dir. Idempotent.
                            crate::commands::k2so_agents::repair_mismigrated_heartbeats(&project.path);
                            // Archive orphan top-tier agents left behind by prior agent-mode
                            // swaps. Templates preserved. Idempotent.
                            crate::commands::k2so_agents::archive_orphan_top_tier_agents(&project.path);
                            // Universal skill refresh. Drives the managed-markers upgrade
                            // protocol for EVERY skill (workspace + every agent's),
                            // so future skill version bumps roll out automatically
                            // without adding a new migration helper. See
                            // ensure_all_skills_up_to_date for the contract.
                            crate::commands::k2so_agents::ensure_all_skills_up_to_date(&project.path);
                        }
                    });
                }

                // Phase 4 H7: the old 60s heartbeat.port watchdog used
                // to periodically rewrite heartbeat.port with Tauri's
                // own port. Post-H7 the daemon owns heartbeat.port and
                // runs its own re-claim loop (see
                // `run_heartbeat_port_watchdog` in k2so-daemon). Tauri
                // re-writing the file would fight the daemon for
                // ownership, so this loop is gone.
            }

            // Auto-start companion API if configured
            {
                let settings = commands::settings::read_settings();
                if settings.companion.auto_start
                    && !settings.companion.username.is_empty()
                    && !settings.companion.password_hash.is_empty()
                    && !settings.companion.ngrok_auth_token.is_empty()
                {
                    let handle = app.handle().clone();
                    std::thread::spawn(move || {
                        // Wait for hook server to initialize
                        std::thread::sleep(std::time::Duration::from_secs(3));
                        // Retry with backoff — ngrok free tier allows one session at a time.
                        // If the app was killed, the old session lingers for ~30-60s on ngrok's side.
                        let delays = [0, 5, 10, 20]; // seconds between retries
                        for (i, delay) in delays.iter().enumerate() {
                            if *delay > 0 {
                                log_debug!("[companion] Auto-start retry {} in {}s...", i + 1, delay);
                                std::thread::sleep(std::time::Duration::from_secs(*delay));
                            }
                            match companion::start_companion() {
                                Ok(url) => {
                                    log_debug!("[companion] Auto-started: {}", url);
                                    return;
                                }
                                Err(e) => {
                                    log_debug!("[companion] Auto-start attempt {} failed: {}", i + 1, e);
                                    if e.contains("already running") || e.contains("cancelled") {
                                        return;
                                    }
                                }
                            }
                        }
                        log_debug!("[companion] Auto-start gave up after {} attempts", delays.len());
                    });
                }
            }

            // Clean up any stale .tmp files from interrupted model downloads
            llm::download::cleanup_stale_downloads();

            // Auto-download AI model on first launch if not present
            {
                let app_handle_for_download = app.handle().clone();
                std::thread::spawn(move || {
                    match llm::download::default_model_exists() {
                        Ok(false) => {
                            log_debug!("[llm] Default model not found, starting download...");
                            if let Some(state) = app_handle_for_download.try_state::<AppState>() {
                                let manager = state.llm_manager.lock();
                                manager.downloading.store(true, std::sync::atomic::Ordering::Relaxed);
                            }
                            let dest = match llm::download::default_model_path() {
                                Ok(p) => p,
                                Err(e) => { log_debug!("[llm] Error getting model path: {e}"); return; }
                            };
                            let dest_str = dest.to_string_lossy().to_string();
                            let progress_app = app_handle_for_download.clone();
                            let result = llm::download::download_model(
                                llm::download::DEFAULT_MODEL_URL,
                                &dest_str,
                                move |p| {
                                    let _ = progress_app.emit("assistant:download-progress", p);
                                },
                            );
                            if let Some(state) = app_handle_for_download.try_state::<AppState>() {
                                let mut manager = state.llm_manager.lock();
                                manager.downloading.store(false, std::sync::atomic::Ordering::Relaxed);
                                if result.is_ok() {
                                    let _ = manager.load_model(&dest_str);
                                    log_debug!("[llm] Model downloaded and loaded successfully");
                                }
                            }
                            if let Err(e) = result {
                                log_debug!("[llm] Auto-download failed: {e}");
                            }
                        }
                        Ok(true) => {
                            // Model exists, try to load it
                            log_debug!("[llm] Default model found, loading...");
                            if let Some(state) = app_handle_for_download.try_state::<AppState>() {
                                let mut manager = state.llm_manager.lock();
                                if let Ok(path) = llm::download::default_model_path() {
                                    let _ = manager.load_model(&path.to_string_lossy());
                                    log_debug!("[llm] Model loaded successfully");
                                }
                            }
                        }
                        Err(e) => log_debug!("[llm] Error checking model: {e}"),
                    }
                });
            }

            // Menubar / system tray icon. Pairs with the persistent-
            // agents feature: once Cmd+Q leaves the daemon running,
            // users need a surface that shows what's still active.
            // Failures here are non-fatal — the app works without a
            // tray, users just lose visibility into the daemon from
            // outside the main window.
            if let Err(e) = tray::install(&app.handle().clone()) {
                log_debug!("[tray] install failed: {e} (continuing without tray)");
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // Projects
            commands::projects::projects_list,
            commands::projects::projects_create,
            commands::projects::projects_update,
            commands::projects::projects_delete,
            commands::projects::projects_reorder,
            commands::projects::workspace_set_nav_visible,
            commands::projects::projects_add_from_path,
            commands::projects::projects_add_without_git,
            commands::projects::projects_init_git_and_open,
            commands::projects::projects_pick_folder,
            commands::projects::projects_open_in_finder,
            commands::projects::projects_get_icon,
            commands::projects::projects_detect_icon,
            commands::projects::projects_upload_icon,
            commands::projects::projects_clear_icon,
            commands::projects::projects_touch_interaction,
            commands::projects::projects_touch_interaction_clear,
            commands::projects::projects_open_in_editor,
            commands::projects::projects_open_in_terminal,
            commands::projects::projects_get_editors,
            commands::projects::projects_get_all_editors,
            commands::projects::projects_refresh_editors,
            commands::projects::projects_open_focus_window,
            commands::projects::projects_enable_worktrees,
            // Workspaces
            commands::workspaces::workspaces_list,
            commands::workspaces::workspaces_create,
            commands::workspaces::workspaces_delete,
            // Focus Groups
            commands::focus_groups::focus_groups_list,
            commands::focus_groups::focus_groups_create,
            commands::focus_groups::focus_groups_update,
            commands::focus_groups::focus_groups_delete,
            commands::focus_groups::focus_groups_assign_project,
            commands::focus_groups::focus_groups_reconcile_project,
            // Workspace Sections
            commands::workspace_sections::sections_list,
            commands::workspace_sections::sections_create,
            commands::workspace_sections::sections_update,
            commands::workspace_sections::sections_delete,
            commands::workspace_sections::sections_reorder,
            commands::workspace_sections::sections_assign_workspace,
            // Agent Presets
            commands::agents::presets_list,
            commands::agents::presets_create,
            commands::agents::presets_update,
            commands::agents::presets_delete,
            commands::agents::presets_reorder,
            commands::agents::presets_reset_built_ins,
            // Filesystem
            commands::filesystem::fs_read_dir,
            commands::filesystem::fs_open_in_finder,
            commands::filesystem::fs_copy_path,
            commands::filesystem::fs_search_tree,
            commands::filesystem::clipboard_read_file_paths,
            commands::filesystem::fs_read_file,
            commands::filesystem::fs_read_binary_file,
            commands::filesystem::fs_write_file,
            commands::filesystem::fs_move_files,
            commands::filesystem::fs_copy_files,
            commands::filesystem::fs_delete,
            commands::filesystem::fs_rename,
            commands::filesystem::fs_create_entry,
            commands::filesystem::fs_duplicate,
            commands::filesystem::open_external,
            // Filesystem watcher
            watcher::fs_watch_dir,
            watcher::fs_unwatch_dir,
            // Settings
            commands::settings::settings_get,
            commands::settings::settings_update,
            commands::settings::settings_reset,
            commands::settings::cli_install_status,
            commands::settings::cli_install,
            commands::settings::cli_uninstall,
            commands::settings::set_document_edited,
            commands::settings::set_relaunch_mode,
            commands::settings::relaunch_via_open,
            // Project Config
            commands::project_config::project_config_get,
            commands::project_config::project_config_has_run_command,
            commands::project_config::project_config_run_command,
            // Terminal
            commands::terminal::terminal_create,
            commands::terminal::terminal_write,
            commands::terminal::terminal_resize,
            commands::terminal::terminal_kill,
            commands::terminal::terminal_active_count_for_path,
            commands::terminal::terminal_kill_foreground,
            commands::terminal::terminal_get_foreground_command,
            commands::terminal::terminal_exists,
            commands::terminal::terminal_get_grid,
            commands::terminal::terminal_scroll,
            commands::terminal::terminal_log,
            commands::terminal::terminal_set_font_size,
            commands::terminal::terminal_get_cell_metrics,
            commands::terminal::terminal_set_focus,
            commands::terminal::terminal_get_selection_text,
            commands::terminal::terminal_read_lines,
            commands::terminal::terminal_list_running_agents,
            // Git
            commands::git::git_info,
            commands::git::git_branches,
            commands::git::git_worktrees,
            commands::git::git_create_worktree,
            commands::git::git_remove_worktree,
            commands::git::git_reopen_worktree,
            commands::git::git_changes,
            // Git Diff
            commands::git::git_diff_file,
            commands::git::git_diff_summary,
            commands::git::git_diff_between_branches,
            commands::git::git_file_content_at_ref,
            // Git Staging
            commands::git::git_stage_file,
            commands::git::git_unstage_file,
            commands::git::git_stage_all,
            // Git Commit
            commands::git::git_commit,
            // Git Merge
            commands::git::git_merge_branch,
            commands::git::git_merge_status,
            commands::git::git_abort_merge,
            commands::git::git_resolve_conflict,
            commands::git::git_delete_branch,
            commands::git::git_prune_worktrees,
            // Workspace Ops
            commands::workspace_ops::workspace_split_pane,
            commands::workspace_ops::workspace_close_pane,
            commands::workspace_ops::workspace_open_document,
            commands::workspace_ops::workspace_open_terminal,
            commands::workspace_ops::workspace_new_tab,
            commands::workspace_ops::workspace_close_tab,
            commands::workspace_ops::workspace_arrange,
            // Assistant (LLM)
            commands::assistant::assistant_chat,
            commands::assistant::assistant_status,
            commands::assistant::assistant_load_model,
            commands::assistant::assistant_download_default_model,
            commands::assistant::assistant_check_model,
            // Chat History
            commands::chat_history::chat_history_list,
            commands::chat_history::chat_history_list_for_project,
            commands::chat_history::chat_history_detect_active_session,
            commands::chat_history::chat_history_session_exists,
            commands::chat_history::chat_history_get_storage_paths,
            commands::chat_history::chat_history_get_custom_names,
            commands::chat_history::chat_history_rename_session,
            commands::chat_history::chat_history_get_pinned,
            commands::chat_history::chat_history_toggle_pin,
            commands::chat_history::chat_history_discover_ide_sessions,
            commands::chat_history::chat_history_migrate_ide_sessions,
            // Timer
            commands::timer::timer_entries_list,
            commands::timer::timer_entry_create,
            commands::timer::timer_entry_delete,
            commands::timer::timer_entries_export,
            // Updater
            commands::updater::check_for_update,
            commands::updater::get_current_version,
            commands::updater::broadcast_sync,
            // Workspace Layouts (per-(project, workspace) pane/tab JSON; renamed from workspace_sessions in 0.37.0)
            commands::workspace_layouts::workspace_layout_save,
            commands::workspace_layouts::workspace_layout_load,
            commands::workspace_layouts::workspace_layout_load_all,
            commands::workspace_layouts::workspace_layout_delete,
            // Claude Auth
            commands::claude_auth::claude_auth_status,
            commands::claude_auth::claude_auth_refresh,
            commands::claude_auth::claude_auth_install_scheduler,
            commands::claude_auth::claude_auth_uninstall_scheduler,
            commands::claude_auth::claude_auth_scheduler_installed,
            // K2SO Agents
            commands::k2so_agents::k2so_agents_list,
            commands::k2so_agents::k2so_agents_create,
            commands::k2so_agents::k2so_agents_delete,
            commands::k2so_agents::k2so_agents_update_field,
            commands::k2so_agents::k2so_agents_get_heartbeat,
            commands::k2so_agents::k2so_agents_set_heartbeat,
            commands::k2so_agents::k2so_agents_scheduler_tick,
            commands::k2so_agents::k2so_agents_heartbeat_noop,
            commands::k2so_agents::k2so_agents_heartbeat_action,
            commands::k2so_agents::k2so_agents_save_session_id,
            commands::k2so_agents::k2so_session_set_surfaced,
            commands::k2so_agents::k2so_agents_clear_session_id,
            // Workspace States
            commands::states::states_list,
            commands::states::states_get,
            commands::states::states_create,
            commands::states::states_update,
            commands::states::states_delete,
            commands::k2so_agents::k2so_agents_work_list,
            commands::k2so_agents::k2so_agents_work_create,
            commands::k2so_agents::k2so_agents_delegate,
            commands::k2so_agents::k2so_agents_work_move,
            commands::k2so_agents::k2so_agents_get_profile,
            commands::k2so_agents::k2so_agents_update_profile,
            commands::k2so_agents::k2so_agents_regenerate_agent_context,
            commands::k2so_agents::k2so_agents_preview_agent_context,
            commands::k2so_agents::k2so_agents_regenerate_workspace_skill,
            commands::k2so_agents::k2so_onboarding_scan,
            commands::k2so_agents::k2so_onboarding_adopt,
            commands::k2so_agents::k2so_onboarding_skip,
            commands::k2so_agents::k2so_onboarding_start_fresh,
            // Back-compat aliases — retained during the 0.33.0 rename window so
            // stale React `invoke()` names keep working until every call site
            // has migrated to the canonical new names above.
            commands::k2so_agents::k2so_agents_generate_claude_md,
            commands::k2so_agents::k2so_agents_teardown_workspace,
            commands::k2so_agents::k2so_agents_preview_workspace_ingest,
            commands::k2so_agents::k2so_agents_run_workspace_ingest,
            commands::k2so_agents::k2so_agents_generate_workspace_claude_md,
            commands::k2so_agents::k2so_agents_disable_workspace_claude_md,
            commands::k2so_agents::k2so_agents_build_launch,
            commands::k2so_agents::k2so_agents_resume_chat_args,
            commands::k2so_agents::k2so_agents_review_queue,
            commands::k2so_agents::k2so_agents_review_approve,
            commands::k2so_agents::k2so_agents_review_reject,
            commands::k2so_agents::k2so_agents_review_request_changes,
            commands::k2so_agents::k2so_agents_workspace_inbox_list,
            commands::k2so_agents::k2so_agents_workspace_inbox_create,
            commands::k2so_agents::k2so_agents_lock,
            commands::k2so_agents::k2so_agents_unlock,
            commands::k2so_agents::k2so_agents_triage_summary,
            commands::k2so_agents::k2so_agents_triage_decide,
            commands::k2so_agents::k2so_agents_install_heartbeat,
            commands::k2so_agents::k2so_agents_uninstall_heartbeat,
            commands::k2so_agents::k2so_agents_apply_wake_scheduler,
            commands::k2so_agents::k2so_agents_update_heartbeat_projects,
            commands::k2so_agents::k2so_agents_preview_schedule,
            // Multi-heartbeat (agent_heartbeats table)
            commands::k2so_agents::k2so_heartbeat_add,
            commands::k2so_agents::k2so_heartbeat_list,
            commands::k2so_agents::k2so_heartbeat_list_archived,
            commands::k2so_agents::k2so_heartbeat_archive,
            commands::k2so_agents::k2so_heartbeat_unarchive,
            commands::k2so_agents::k2so_heartbeat_remove,
            commands::k2so_agents::k2so_workspace_get_show_heartbeat_sessions,
            commands::k2so_agents::k2so_workspace_set_show_heartbeat_sessions,
            commands::k2so_agents::k2so_heartbeat_set_enabled,
            commands::k2so_agents::k2so_heartbeat_edit,
            commands::k2so_agents::k2so_heartbeat_rename,
            commands::k2so_agents::k2so_heartbeat_fires_list,
            agent_hooks::k2so_heartbeat_force_fire,
            agent_hooks::k2so_heartbeat_smart_launch,
            agent_hooks::k2so_heartbeat_active_session,
            agent_hooks::k2so_session_lookup_by_agent,
            // Agent Sessions (DB-tracked)
            commands::k2so_agents::workspace_session_get,
            // Workspace Relations
            commands::k2so_agents::workspace_relations_list,
            commands::k2so_agents::workspace_relations_list_incoming,
            commands::k2so_agents::workspace_relations_create,
            commands::k2so_agents::workspace_relations_delete,
            // Agent Skills
            commands::k2so_agents::k2so_agents_regenerate_skills,
            // Agent Editor
            commands::k2so_agents::k2so_agents_get_editor_context,
            commands::k2so_agents::k2so_agents_preview_claude_md,
            commands::k2so_agents::k2so_agents_regenerate_claude_md,
            commands::k2so_agents::k2so_agents_save_agent_md,
            // Review Checklist
            commands::review_checklist::review_checklist_read,
            commands::review_checklist::review_checklist_write,
            commands::review_checklist::review_checklist_toggle,
            commands::review_checklist::review_checklist_init,
            // Format
            commands::format::format_file,
            commands::format::format_file_check,
            commands::themes::get_themes_dir,
            commands::themes::themes_ensure_dir,
            commands::themes::themes_create_template,
            commands::themes::themes_list_custom,
            commands::themes::themes_delete,
            // Skill Layers
            commands::skill_layers::skill_layers_list,
            commands::skill_layers::skill_layers_create,
            commands::skill_layers::skill_layers_delete,
            commands::skill_layers::skill_layers_get_content,
            // Companion API
            commands::companion::companion_start,
            commands::companion::companion_stop,
            commands::companion::companion_status,
            commands::companion::companion_set_password,
            commands::companion::companion_disconnect_session,
            // k2so-daemon lifecycle (Settings panel reads this to show
            // "daemon: running / not installed / unreachable") and
            // controls the launch agent install/uninstall/restart.
            commands::daemon::daemon_status,
            commands::daemon::daemon_install,
            commands::daemon::daemon_uninstall,
            commands::daemon::daemon_restart,
            commands::daemon::daemon_log_path,
            commands::daemon::daemon_log_tail,
            commands::daemon::daemon_ws_url,
            commands::daemon::get_keep_daemon_on_quit,
            commands::daemon::set_keep_daemon_on_quit,
            // Kessel — terminal spawn path optimized to skip the
            // browser fetch overhead. See commands/kessel.rs.
            commands::kessel::kessel_spawn,
            commands::kessel::kessel_daemon_ws,
            commands::kessel::kessel_write,
            commands::kessel::kessel_resize,
            commands::kessel::kessel_warm_http,
            commands::kessel::kessel_close,
            // Canvas Plan Phase 4 — client-side alacritty Term
            // driven by the daemon's byte stream.
            commands::kessel_term::kessel_term_attach,
            commands::kessel_term::kessel_term_grid_snapshot,
            commands::kessel_term::kessel_term_resize,
            commands::kessel_term::kessel_term_detach,
        ])
        .build(tauri::generate_context!())
        .unwrap_or_else(|e| {
            // Pre-webview failure — we can't show a GUI error, so write to
            // stderr (visible in Console.app when launched from Finder) and
            // exit non-zero so the OS reports the crash cleanly. Previously
            // this used .expect which panicked and aborted with a stderr
            // message that failed on some sandboxes.
            use std::io::Write;
            let _ = writeln!(std::io::stderr(), "K2SO failed to build Tauri context: {}", e);
            std::process::exit(1);
        })
        .run(|app, event| {
            match event {
                // Cmd+Q / File → Quit / Menubar "Quit K2SO" /
                // NSApplication terminate: all land here. Semantic
                // choice ratified with rosson: these always kill
                // everything, regardless of the keep-running toggle.
                // That toggle ONLY controls the red close-button
                // behavior (handled in on_window_event above).
                //
                // So: unconditionally unload the daemon plist (if
                // installed), then let exit proceed. The in-app
                // companion server dies with the Tauri process.
                tauri::RunEvent::ExitRequested { .. } => {
                    let plist = k2so_core::wake::DaemonPlist::canonical(
                        std::path::PathBuf::from("/unused"),
                    );
                    if let Some(path) = plist.plist_path() {
                        if path.exists() {
                            // Best-effort — errors swallowed so a
                            // hung launchctl can't block the quit.
                            let _ = k2so_core::wake::launchctl_unload(&path);
                        }
                    }
                }
                tauri::RunEvent::Exit => {
                    if RELAUNCH_MODE.load(std::sync::atomic::Ordering::Relaxed) {
                        // Relaunch mode — use normal exit so the spawned process survives
                        std::process::exit(0);
                    } else {
                        // Use _exit() to skip C++ static destructors (ggml_metal).
                        // This handles Cmd+Q (NSApplication terminate:) which bypasses
                        // the window CloseRequested event and goes straight to exit().
                        unsafe { libc::_exit(0); }
                    }
                }
                // macOS: user clicked the Dock icon while the window was
                // hidden (e.g. they had closed it with the red button and
                // we kept the app alive for heartbeat agents). Re-show
                // the main window instead of opening a new one.
                #[cfg(target_os = "macos")]
                tauri::RunEvent::Reopen { has_visible_windows, .. } => {
                    if !has_visible_windows {
                        if let Some(win) = app.get_webview_window("main") {
                            let _ = win.show();
                            let _ = win.set_focus();
                        }
                    }
                }
                _ => {}
            }
        });
}

/// True when at least one project has heartbeat enabled. Used by the
/// window close handler to decide whether to keep the app alive after
/// the user clicks the red button. If heartbeat is fully off, red-button
/// quits normally — we don't force the user to Cmd+Q unless they're
/// actually relying on autonomous wakes.
fn any_heartbeat_enabled() -> bool {
    let db = crate::db::shared();
    let conn = db.lock();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM projects WHERE heartbeat_enabled = 1",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);
    count > 0
}

/// One-time migration: move workspace_layouts from settings.json → workspace_layouts SQLite table.
fn migrate_workspace_layouts_to_db(app: &tauri::AppHandle) {
    let settings_path = dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".k2so")
        .join("settings.json");

    if !settings_path.exists() {
        return;
    }

    let raw = match std::fs::read_to_string(&settings_path) {
        Ok(r) => r,
        Err(_) => return,
    };

    let mut parsed: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(_) => return,
    };

    let layouts = match parsed.get("workspaceLayouts") {
        Some(v) if v.is_object() && !v.as_object().unwrap().is_empty() => {
            v.as_object().unwrap().clone()
        }
        _ => return, // Nothing to migrate
    };

    // Get the DB connection from managed state
    let state = app.state::<AppState>();
    let conn = state.db.lock();

    let mut migrated = 0usize;
    for (key, layout_val) in &layouts {
        // key format: "projectId:workspaceId"
        let parts: Vec<&str> = key.splitn(2, ':').collect();
        if parts.len() != 2 {
            continue;
        }
        let project_id = parts[0];
        let workspace_id = parts[1];

        let layout_json = match serde_json::to_string(layout_val) {
            Ok(j) => j,
            Err(_) => continue,
        };

        let id = key.clone();
        if conn.execute(
            "INSERT OR IGNORE INTO workspace_layouts (id, project_id, workspace_id, layout_json, updated_at)
             VALUES (?1, ?2, ?3, ?4, unixepoch())",
            rusqlite::params![id, project_id, workspace_id, layout_json],
        ).is_ok() {
            migrated += 1;
        }
    }

    if migrated > 0 {
        log_debug!("[k2so] Migrated {} workspace layout(s) from settings.json to SQLite", migrated);

        // Remove workspaceLayouts from settings.json
        if let Some(obj) = parsed.as_object_mut() {
            obj.remove("workspaceLayouts");
        }
        if let Ok(json) = serde_json::to_string_pretty(&parsed) {
            let tmp = settings_path.with_extension("json.tmp");
            if std::fs::write(&tmp, &json).is_ok() {
                std::fs::rename(&tmp, &settings_path).ok();
            }
        }
    }
}

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
// Phase 2 Unit 2 — `llm` re-export removed. LLM inference + model
// management moved entirely to k2so-daemon; Tauri no longer touches
// llama.cpp or the model lifecycle. The renderer calls /cli/llm/* on
// the daemon directly.
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
// Phase 2 Unit 1 — companion bridges (settings + terminal + event
// sink + app event source) moved to k2so-daemon. The daemon now
// owns the ngrok tunnel and registers its own providers against
// k2so-core's companion ambient slots. Tauri no longer needs these
// shims because the renderer talks to `/cli/companion/*` directly.
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

// Phase 2 Unit 2 — `llm_worker_main` moved to k2so-daemon's
// `llm_host::worker_main`. The daemon now spawns itself with
// `--llm-worker <payload_path>` for inference. Tauri is no longer
// an LLM host; deleted alongside `commands/assistant.rs` and the
// `--llm-worker` arm in `src-tauri/src/main.rs`.

/// 0.39.0: `warm_http_pool_async` (Kessel reqwest-runtime warmup) was
/// removed alongside the Kessel renderer. Kept as an empty stub so
/// `main.rs` doesn't fail to link — it now does nothing, which is
/// correct because the alacritty-v2 WS spawn path doesn't share the
/// blocking reqwest runtime the old Kessel spawn path needed warm.
pub fn warm_http_pool_async() {}

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

    // Phase 2 Unit 1 — companion settings bridge moved to
    // k2so-daemon. The daemon now reads settings.json directly via
    // its own `DaemonCompanionSettingsProvider`. Tauri no longer
    // registers a provider; the renderer reaches the companion
    // tunnel via `/cli/companion/*` on the daemon, so this process
    // doesn't need one.

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
        // Phase 2 Unit 2 — `llm_manager` field deleted; LLM lives
        // in the daemon now.
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

            // Phase 2 Unit 1 — companion terminal / event-sink /
            // app-event-source bridges moved to k2so-daemon. The
            // daemon registers daemon-owned impls
            // (`crates/k2so-daemon/src/companion_host.rs`) since the
            // tunnel + WS clients now live there. Tauri no longer
            // wires these bridges.

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

                        // Phase 2 Unit 2 — the LLM lives in the daemon
                        // now; Tauri no longer owns a model handle, so
                        // there's nothing to unload here. We only need
                        // to kill terminals before exit. Spawn the
                        // terminal kill on a worker thread so a stuck
                        // PTY reaper can't hang the quit indefinitely.
                        // Zed pattern: log panics instead of silently
                        // swallowing them.
                        let handle_for_term = app_handle.clone();

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

                        // Wait up to 5 seconds for terminal cleanup to
                        // complete. Terminal process reaping can take
                        // time on macOS when launchd-managed children
                        // haven't drained their pipes yet.
                        let timeout = std::time::Duration::from_secs(5);
                        let (done_tx, done_rx) = std::sync::mpsc::channel();

                        std::thread::spawn(move || {
                            let _ = term_thread.join();
                            let _ = done_tx.send("term");
                        });

                        match done_rx.recv_timeout(timeout) {
                            Ok(_) => {}
                            Err(_) => {
                                log_debug!("[shutdown] Cleanup timed out after 5s — exiting anyway");
                            }
                        }

                        // Phase 4 H7: the daemon is the sole writer of
                        // ~/.k2so/heartbeat.port since Tauri retired
                        // its HTTP listener. Do NOT remove the file on
                        // Tauri quit — the daemon process keeps running
                        // (launchd-managed) and the CLI still needs a
                        // valid port file to find it.

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
            // Phase 2 Unit 7b: the per-workspace legacy migrations
            // (filename uppercase, CLAUDE.md harvest, heartbeat
            // promote/repair, orphan archive) + `ensure_all_skills_up_to_date`
            // now run in `k2so-daemon::main::run_workspace_legacy_migrations_sweep`.
            // The daemon executes the same idempotent sweep on its
            // own boot, so this Tauri-side thread is gone. Remote
            // daemons (K2SO Connect) and headless boots now pick up
            // these migrations without Tauri being present.
            //
            // Phase 4 H7: the old 60s heartbeat.port watchdog used to
            // periodically rewrite heartbeat.port with Tauri's own port.
            // Post-H7 the daemon owns heartbeat.port and runs its own
            // re-claim loop (see `run_heartbeat_port_watchdog` in
            // k2so-daemon). Tauri re-writing the file would fight the
            // daemon for ownership, so this loop is gone.

            // Phase 2 Unit 1 — companion ngrok tunnel autostart moved
            // to k2so-daemon. The daemon now reads `companion.auto_start`
            // from `~/.k2so/settings.json` on its own first boot and
            // brings the tunnel up itself (see
            // `crates/k2so-daemon/src/companion_host.rs::maybe_autostart`).
            //
            // Owning the tunnel daemon-side closes the K2SO Connect +
            // Mobile Companion gap: with Tauri closed (or running on a
            // different machine entirely), the daemon's tunnel stays
            // reachable.

            // Phase 2 Unit 2 — LLM stale-tmp cleanup, default-model
            // auto-download, and model auto-load moved to k2so-daemon's
            // first-boot pass (see `llm_host::maybe_first_boot_discover`).
            // The renderer kicks off downloads via /cli/llm/download-default
            // and polls /cli/llm/status for readiness. Tauri is no longer
            // involved in the LLM lifecycle.

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
            // Phase 2 Unit 6 — `commands::filesystem::*` shims
            // deleted. Renderer hits `/cli/fs/*` on the daemon.
            // 0.37.9 — macOS permissions surface for Settings UI
            commands::permissions::permissions_get_status,
            commands::permissions::permissions_request_full_disk_access,
            commands::permissions::permissions_request_accessibility,
            commands::permissions::permissions_request_microphone,
            commands::permissions::permissions_request_apple_events,
            commands::permissions::permissions_request_local_network,
            // Phase 2 Unit 6 — `commands::whats_new::*` shims
            // deleted. Renderer hits `/cli/whats_new*` on the daemon.
            // Memory watcher (0.38.12 — leak telemetry)
            commands::memory_watcher::renderer_memory_status,
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
            // Phase 2 Unit 6 — `commands::project_config::*` shims
            // deleted. Renderer hits `/cli/project-config/*` on the
            // daemon.
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
            // Phase 2 Unit 2 — `assistant_*` commands deleted. LLM
            // moved to k2so-daemon (/cli/llm/*); renderer calls the
            // daemon directly.
            // Phase 2 Unit 6 — `commands::chat_history::*` shims
            // deleted. Renderer hits `/cli/chat/*` on the daemon.
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
            // Claude Auth: Phase 2 Unit 5 — moved to daemon
            // (`/cli/claude-auth/*`). Renderer hits the daemon
            // directly; no Tauri command surface remains.
            // K2SO Agents
            commands::k2so_agents::k2so_agents_list,
            commands::k2so_agents::k2so_agents_create,
            commands::k2so_agents::k2so_workspace_agent_display_name,
            commands::k2so_agents::k2so_workspace_set_agent_display_name,
            commands::k2so_agents::k2so_agents_delete,
            commands::k2so_agents::k2so_agents_update_field,
            commands::k2so_agents::k2so_agents_get_heartbeat,
            commands::k2so_agents::k2so_agents_set_heartbeat,
            commands::k2so_agents::k2so_agents_scheduler_tick,
            commands::k2so_agents::k2so_agents_heartbeat_noop,
            commands::k2so_agents::k2so_agents_heartbeat_action,
            commands::k2so_agents::k2so_agents_save_session_id,
            commands::k2so_agents::k2so_session_set_surfaced,
            commands::k2so_agents::k2so_chat_refresh_broadcast,
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
            commands::k2so_agents::k2so_heartbeat_list_all,
            commands::k2so_agents::k2so_heartbeat_fires_list_all,
            commands::k2so_agents::k2so_heartbeat_list_archived,
            commands::k2so_agents::k2so_heartbeat_archive,
            commands::k2so_agents::k2so_heartbeat_unarchive,
            commands::k2so_agents::k2so_heartbeat_remove,
            commands::k2so_agents::k2so_workspace_get_show_heartbeat_sessions,
            commands::k2so_agents::k2so_workspace_set_show_heartbeat_sessions,
            commands::k2so_agents::k2so_heartbeat_set_enabled,
            commands::k2so_agents::k2so_heartbeat_set_use_workspace_session,
            commands::k2so_agents::k2so_heartbeat_edit,
            commands::k2so_agents::k2so_heartbeat_rename,
            commands::k2so_agents::k2so_heartbeat_fires_list,
            agent_hooks::k2so_heartbeat_force_fire,
            agent_hooks::k2so_heartbeat_smart_launch,
            agent_hooks::k2so_heartbeat_active_session,
            agent_hooks::k2so_session_lookup_by_agent,
            agent_hooks::k2so_sessions_list_for_workspace,
            // Agent Sessions (DB-tracked)
            commands::k2so_agents::workspace_session_get,
            commands::k2so_agents::workspace_session_set_session_id,
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
            // Phase 2 Unit 6 — `commands::review_checklist::*` shims
            // deleted. Renderer hits `/cli/review-checklist/*` on the
            // daemon.
            // Format
            commands::format::format_file,
            commands::format::format_file_check,
            // Phase 2 Unit 6 — `commands::themes::*` shims deleted.
            // Renderer hits `/cli/themes/*` on the daemon.
            // Phase 2 Unit 6 — `commands::skill_layers::*` shims
            // deleted. Renderer hits `/cli/skill-layers/*` on the
            // daemon.
            // Phase 2 Unit 1 — companion API commands deleted; the
            // renderer now calls `/cli/companion/{start,stop,status,
            // set-password,disconnect-session}` on the daemon
            // directly (see `CompanionSection.tsx`).
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

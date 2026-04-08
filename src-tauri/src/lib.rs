#[cfg(target_os = "macos")]
#[macro_use]
extern crate objc;

/// Flag to skip _exit(0) during relaunch (set by the frontend before process::relaunch)
static RELAUNCH_MODE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Safe eprintln that silently ignores write failures.
/// When launched from Finder (no tty), stderr writes can fail and the default
/// `eprintln!` panics, which cascades into abort(). This macro catches that.
macro_rules! log_debug {
    ($($arg:tt)*) => {{
        use std::io::Write;
        let _ = writeln!(std::io::stderr(), $($arg)*);
    }};
}

mod agent_hooks;
mod commands;
mod companion;
mod db;
mod editors;
mod git;
mod llm;
mod menu;
mod project_config;
mod state;
mod terminal;
mod watcher;
mod window;

use state::AppState;
use std::collections::HashMap;
use parking_lot::Mutex;
use tauri::Manager;

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

pub fn run() {
    // Ignore SIGPIPE so writing to a dead PTY returns EPIPE instead of
    // killing the entire process.
    #[cfg(unix)]
    terminal::ignore_sigpipe();

    let conn = match db::init_database() {
        Ok(c) => c,
        Err(e) => {
            log_debug!("[k2so] FATAL: Failed to initialize database: {}", e);
            log_debug!("[k2so] The app will now exit. Check disk permissions and space at ~/.k2so/");
            std::process::exit(1);
        }
    };

    let app_state = AppState {
        db: Mutex::new(conn),
        terminal_manager: Mutex::new(terminal::TerminalManager::new()),
        llm_manager: Mutex::new(llm::LlmManager::new()),
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
            // Migrate old JSON window state to SQLite (one-time migration)
            window::migrate_json_window_state(app.handle());

            // Migrate workspace_layouts from settings.json → SQLite (one-time)
            migrate_workspace_layouts_to_db(app.handle());

            // Regenerate SKILL.md files for all workspaces (v0.26 migration)
            {
                let all_projects: Vec<(String, String)> = {
                    let state = app.state::<AppState>();
                    let db = state.db.lock();
                    let mut projects = Vec::new();
                    if let Ok(mut stmt) = db.prepare("SELECT path, agent_mode FROM projects") {
                        if let Ok(rows) = stmt.query_map([], |row| {
                            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                        }) {
                            for row in rows.flatten() {
                                projects.push(row);
                            }
                        }
                    }
                    projects
                };
                for (path, mode) in &all_projects {
                    // Agent-enabled workspaces: regenerate per-agent SKILL.md + CLAUDE.md
                    if mode != "off" {
                        let _ = commands::k2so_agents::k2so_agents_regenerate_skills(path.clone());
                        // Regenerate workspace root CLAUDE.md with current skill protocol
                        let _ = commands::k2so_agents::k2so_agents_generate_workspace_claude_md(path.clone());
                    }
                    // All workspaces: write workspace-level skill to all harness locations
                    commands::k2so_agents::write_workspace_skill_file(path);
                }
            }

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
                win.on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { .. } = event {
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

                        // Remove stale port file so heartbeat script won't curl a dead port
                        if let Some(home) = dirs::home_dir() {
                            let port_file = home.join(".k2so/heartbeat.port");
                            let _ = std::fs::remove_file(&port_file);
                        }

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
            // Start agent lifecycle notification server and register hooks
            {
                let hook_port = agent_hooks::start_server(app.handle().clone());
                match agent_hooks::write_hook_script(hook_port) {
                    Ok(script_path) => {
                        agent_hooks::register_all_hooks(&script_path);
                        log_debug!("[agent-hooks] Hook system ready on port {} with script {}", hook_port, script_path);
                    }
                    Err(e) => {
                        log_debug!("[agent-hooks] Failed to write hook script: {}", e);
                    }
                }

                // Clean stale port file on startup — if K2SO crashed previously,
                // the old port file persists and CLI/heartbeat scripts connect to a dead port.
                let home = dirs::home_dir().unwrap_or_default();
                let k2so_dir = home.join(".k2so");
                let _ = std::fs::create_dir_all(&k2so_dir);
                {
                    let stale_port_file = k2so_dir.join("heartbeat.port");
                    if stale_port_file.exists() {
                        // Check if the old port is still alive; if not, remove it
                        if let Ok(old_port_str) = std::fs::read_to_string(&stale_port_file) {
                            if let Ok(old_port) = old_port_str.trim().parse::<u16>() {
                                if old_port != hook_port {
                                    // Different port — old instance is dead, clean up
                                    let _ = std::fs::remove_file(&stale_port_file);
                                    log_debug!("[startup] Removed stale port file (was port {})", old_port);
                                }
                            } else {
                                let _ = std::fs::remove_file(&stale_port_file);
                            }
                        }
                    }
                }

                // Write port and token to ~/.k2so/ for external heartbeat scripts and CLI
                // Atomic write: tmp file + rename to prevent partial reads
                let port_file = k2so_dir.join("heartbeat.port");
                let port_tmp = k2so_dir.join("heartbeat.port.tmp");
                if let Err(e) = std::fs::write(&port_tmp, hook_port.to_string())
                    .and_then(|_| std::fs::rename(&port_tmp, &port_file))
                {
                    log_debug!("[heartbeat] Failed to write port file: {}", e);
                    // Fallback: try direct write without atomic rename
                    let _ = std::fs::write(&port_file, hook_port.to_string());
                }
                // Verify the port file was actually written
                match std::fs::read_to_string(&port_file) {
                    Ok(contents) if contents.trim() == hook_port.to_string() => {
                        log_debug!("[heartbeat] Port file verified: {}", port_file.display());
                    }
                    Ok(contents) => {
                        log_debug!("[heartbeat] WARNING: Port file has wrong content: '{}' (expected {})", contents.trim(), hook_port);
                        let _ = std::fs::write(&port_file, hook_port.to_string());
                    }
                    Err(e) => {
                        log_debug!("[heartbeat] WARNING: Port file not readable after write: {}", e);
                    }
                }
                let token_file = k2so_dir.join("heartbeat.token");
                let token_str = crate::agent_hooks::get_token();
                if !token_str.is_empty() {
                    // Create token file with restricted permissions from the start (0600)
                    // to avoid a race window where the file is world-readable between
                    // write and chmod. Uses OpenOptions to set permissions atomically.
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::OpenOptionsExt;
                        use std::io::Write;
                        let token_tmp = k2so_dir.join("heartbeat.token.tmp");
                        match std::fs::OpenOptions::new()
                            .write(true)
                            .create(true)
                            .truncate(true)
                            .mode(0o600)
                            .open(&token_tmp)
                        {
                            Ok(mut f) => {
                                if let Err(e) = f.write_all(token_str.as_bytes())
                                    .and_then(|_| f.flush())
                                {
                                    log_debug!("[heartbeat] Failed to write token file: {}", e);
                                } else {
                                    let _ = std::fs::rename(&token_tmp, &token_file);
                                }
                            }
                            Err(e) => log_debug!("[heartbeat] Failed to create token file: {}", e),
                        }
                    }
                    #[cfg(not(unix))]
                    {
                        let token_tmp = k2so_dir.join("heartbeat.token.tmp");
                        if let Err(e) = std::fs::write(&token_tmp, token_str)
                            .and_then(|_| std::fs::rename(&token_tmp, &token_file))
                        {
                            log_debug!("[heartbeat] Failed to write token file: {}", e);
                        }
                    }
                }

                // Watchdog: periodically verify the port file exists and is correct.
                // If another process deletes it or it gets corrupted, recreate it so
                // the heartbeat script can always find the running K2SO instance.
                {
                    let watchdog_port = hook_port;
                    let watchdog_dir = k2so_dir.clone();
                    std::thread::spawn(move || {
                        loop {
                            std::thread::sleep(std::time::Duration::from_secs(60));
                            let port_file = watchdog_dir.join("heartbeat.port");
                            let needs_write = match std::fs::read_to_string(&port_file) {
                                Ok(contents) => contents.trim() != watchdog_port.to_string(),
                                Err(_) => true,
                            };
                            if needs_write {
                                log_debug!("[heartbeat] Port file missing or stale — recreating");
                                let _ = std::fs::write(&port_file, watchdog_port.to_string());
                            }
                        }
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
                            let result = llm::download::download_model(
                                llm::download::DEFAULT_MODEL_URL,
                                &dest_str,
                                app_handle_for_download.clone(),
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
            // Workspace Sessions
            commands::workspace_sessions::workspace_session_save,
            commands::workspace_sessions::workspace_session_load,
            commands::workspace_sessions::workspace_session_load_all,
            commands::workspace_sessions::workspace_session_delete,
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
            commands::k2so_agents::k2so_agents_generate_claude_md,
            commands::k2so_agents::k2so_agents_generate_workspace_claude_md,
            commands::k2so_agents::k2so_agents_disable_workspace_claude_md,
            commands::k2so_agents::k2so_agents_build_launch,
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
            commands::k2so_agents::k2so_agents_update_heartbeat_projects,
            commands::k2so_agents::k2so_agents_preview_schedule,
            // Agent Sessions (DB-tracked)
            commands::k2so_agents::agent_sessions_list,
            commands::k2so_agents::agent_sessions_get,
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
            // Companion API
            commands::companion::companion_start,
            commands::companion::companion_stop,
            commands::companion::companion_status,
            commands::companion::companion_set_password,
            commands::companion::companion_disconnect_session,
        ])
        .build(tauri::generate_context!())
        .expect("error while building K2SO")
        .run(|_app, event| {
            if let tauri::RunEvent::Exit = event {
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
        });
}

/// One-time migration: move workspace_layouts from settings.json → workspace_sessions SQLite table.
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
            "INSERT OR IGNORE INTO workspace_sessions (id, project_id, workspace_id, layout_json, updated_at)
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

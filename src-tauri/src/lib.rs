mod commands;
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
use std::sync::Mutex;
use tauri::Manager;

pub fn run() {
    // Ignore SIGPIPE so writing to a dead PTY returns EPIPE instead of
    // killing the entire process.
    #[cfg(unix)]
    terminal::ignore_sigpipe();

    let conn = match db::init_database() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[k2so] FATAL: Failed to initialize database: {}", e);
            eprintln!("[k2so] The app will now exit. Check disk permissions and space at ~/.k2so/");
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
        .manage(app_state)
        .menu(|handle| menu::create_menu(handle))
        .on_menu_event(menu::handle_menu_event)
        .setup(|app| {
            // Migrate old JSON window state to SQLite (one-time migration)
            window::migrate_json_window_state(app.handle());

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
            // Save window state and clean up terminals on close
            let app_handle = app.handle().clone();
            if let Some(win) = app.get_webview_window("main") {
                win.on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { .. } = event {
                        window::save_window_state(&app_handle);

                        // Parallelize LLM unload and terminal kill with a 2-second timeout.
                        // These have no dependency on each other and can run concurrently.
                        let handle_for_llm = app_handle.clone();
                        let handle_for_term = app_handle.clone();

                        let llm_thread = std::thread::spawn(move || {
                            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                if let Some(state) = handle_for_llm.try_state::<AppState>() {
                                    // Use try_lock to avoid blocking if model is still loading
                                    if let Ok(mut manager) = state.llm_manager.try_lock() {
                                        manager.unload();
                                    } else {
                                        eprintln!("[shutdown] LLM lock busy (model loading?) — skipping unload");
                                    }
                                }
                            }));
                        });

                        let term_thread = std::thread::spawn(move || {
                            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                if let Some(state) = handle_for_term.try_state::<AppState>() {
                                    if let Ok(mut manager) = state.terminal_manager.lock() {
                                        manager.kill_all();
                                    }
                                }
                            }));
                        });

                        // Wait up to 2 seconds for both to complete.
                        // Use a parking thread to implement join-with-timeout since
                        // JoinHandle::join_timeout is not yet stable.
                        let timeout = std::time::Duration::from_secs(2);
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
                                eprintln!("[shutdown] Cleanup timed out after 2s — exiting anyway");
                                break;
                            }
                            match done_rx.recv_timeout(remaining) {
                                Ok(_) => completed += 1,
                                Err(_) => {
                                    eprintln!("[shutdown] Cleanup timed out after 2s — exiting anyway");
                                    break;
                                }
                            }
                        }
                    }
                });
            }
            // Clean up any stale .tmp files from interrupted model downloads
            llm::download::cleanup_stale_downloads();

            // Auto-download AI model on first launch if not present
            {
                let app_handle_for_download = app.handle().clone();
                std::thread::spawn(move || {
                    match llm::download::default_model_exists() {
                        Ok(false) => {
                            eprintln!("[llm] Default model not found, starting download...");
                            if let Some(state) = app_handle_for_download.try_state::<AppState>() {
                                if let Ok(manager) = state.llm_manager.lock() {
                                    manager.downloading.store(true, std::sync::atomic::Ordering::Relaxed);
                                }
                            }
                            let dest = match llm::download::default_model_path() {
                                Ok(p) => p,
                                Err(e) => { eprintln!("[llm] Error getting model path: {e}"); return; }
                            };
                            let dest_str = dest.to_string_lossy().to_string();
                            let result = llm::download::download_model(
                                llm::download::DEFAULT_MODEL_URL,
                                &dest_str,
                                app_handle_for_download.clone(),
                            );
                            if let Some(state) = app_handle_for_download.try_state::<AppState>() {
                                if let Ok(mut manager) = state.llm_manager.lock() {
                                    manager.downloading.store(false, std::sync::atomic::Ordering::Relaxed);
                                    if result.is_ok() {
                                        let _ = manager.load_model(&dest_str);
                                        eprintln!("[llm] Model downloaded and loaded successfully");
                                    }
                                }
                            }
                            if let Err(e) = result {
                                eprintln!("[llm] Auto-download failed: {e}");
                            }
                        }
                        Ok(true) => {
                            // Model exists, try to load it
                            eprintln!("[llm] Default model found, loading...");
                            if let Some(state) = app_handle_for_download.try_state::<AppState>() {
                                if let Ok(mut manager) = state.llm_manager.lock() {
                                    if let Ok(path) = llm::download::default_model_path() {
                                        let _ = manager.load_model(&path.to_string_lossy());
                                        eprintln!("[llm] Model loaded successfully");
                                    }
                                }
                            }
                        }
                        Err(e) => eprintln!("[llm] Error checking model: {e}"),
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
            commands::projects::projects_add_from_path,
            commands::projects::projects_add_without_git,
            commands::projects::projects_init_git_and_open,
            commands::projects::projects_pick_folder,
            commands::projects::projects_open_in_finder,
            commands::projects::projects_get_icon,
            commands::projects::projects_detect_icon,
            commands::projects::projects_upload_icon,
            commands::projects::projects_clear_icon,
            commands::projects::projects_open_in_editor,
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
            // Filesystem watcher
            watcher::fs_watch_dir,
            watcher::fs_unwatch_dir,
            // Settings
            commands::settings::settings_get,
            commands::settings::settings_update,
            commands::settings::settings_reset,
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
            commands::terminal::terminal_get_buffer,
            commands::terminal::terminal_log,
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
            // Updater
            commands::updater::check_for_update,
            commands::updater::get_current_version,
            commands::updater::broadcast_sync,
        ])
        .run(tauri::generate_context!())
        .expect("error while running K2SO");
}

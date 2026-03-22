use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Emitter, Manager};

pub fn create_menu(handle: &AppHandle) -> Result<Menu<tauri::Wry>, tauri::Error> {
    let menu = Menu::new(handle)?;

    // App submenu (macOS)
    let app_menu = Submenu::with_items(
        handle,
        "K2SO",
        true,
        &[
            &PredefinedMenuItem::about(handle, Some("About K2SO"), None)?,
            &PredefinedMenuItem::separator(handle)?,
            &MenuItem::with_id(handle, "settings", "Settings...", true, Some("CmdOrCtrl+,"))?,
            &PredefinedMenuItem::separator(handle)?,
            &PredefinedMenuItem::services(handle, None)?,
            &PredefinedMenuItem::separator(handle)?,
            &PredefinedMenuItem::hide(handle, None)?,
            &PredefinedMenuItem::hide_others(handle, None)?,
            &PredefinedMenuItem::show_all(handle, None)?,
            &PredefinedMenuItem::separator(handle)?,
            &PredefinedMenuItem::quit(handle, None)?,
        ],
    )?;

    // Edit submenu
    let edit_menu = Submenu::with_items(
        handle,
        "Edit",
        true,
        &[
            &PredefinedMenuItem::undo(handle, None)?,
            &PredefinedMenuItem::redo(handle, None)?,
            &PredefinedMenuItem::separator(handle)?,
            &PredefinedMenuItem::cut(handle, None)?,
            &PredefinedMenuItem::copy(handle, None)?,
            &PredefinedMenuItem::paste(handle, None)?,
            &PredefinedMenuItem::select_all(handle, None)?,
        ],
    )?;

    // View submenu
    let view_menu = Submenu::with_items(
        handle,
        "View",
        true,
        &[
            &MenuItem::with_id(handle, "new-window", "New Window", true, Some("CmdOrCtrl+Shift+N"))?,
            &PredefinedMenuItem::separator(handle)?,
            &MenuItem::with_id(handle, "app-zoom-in", "Zoom In", true, Some("CmdOrCtrl+Equal"))?,
            &MenuItem::with_id(handle, "app-zoom-out", "Zoom Out", true, Some("CmdOrCtrl+-"))?,
            &MenuItem::with_id(handle, "app-zoom-reset", "Zoom Reset", true, Some("CmdOrCtrl+0"))?,
            &PredefinedMenuItem::separator(handle)?,
            &MenuItem::with_id(handle, "terminal-zoom-in", "Terminal Zoom In", true, Some("CmdOrCtrl+Shift+Equal"))?,
            &MenuItem::with_id(handle, "terminal-zoom-out", "Terminal Zoom Out", true, Some("CmdOrCtrl+Shift+-"))?,
            &PredefinedMenuItem::separator(handle)?,
            &PredefinedMenuItem::fullscreen(handle, None)?,
        ],
    )?;

    // Window submenu
    let window_menu = Submenu::with_items(
        handle,
        "Window",
        true,
        &[
            &PredefinedMenuItem::minimize(handle, None)?,
            &PredefinedMenuItem::maximize(handle, None)?,
            &PredefinedMenuItem::separator(handle)?,
            &PredefinedMenuItem::close_window(handle, None)?,
        ],
    )?;

    menu.append(&app_menu)?;
    menu.append(&edit_menu)?;
    menu.append(&view_menu)?;
    menu.append(&window_menu)?;

    Ok(menu)
}

pub fn handle_menu_event(app: &AppHandle, event: MenuEvent) {
    let id = event.id().as_ref();
    match id {
        "settings" => {
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.emit("menu:open-settings", ());
            }
        }
        "app-zoom-in" => {
            emit_to_focused(app, "app:zoom-in");
        }
        "app-zoom-out" => {
            emit_to_focused(app, "app:zoom-out");
        }
        "app-zoom-reset" => {
            emit_to_focused(app, "app:zoom-reset");
        }
        "terminal-zoom-in" => {
            emit_to_focused(app, "terminal:zoom-in");
        }
        "terminal-zoom-out" => {
            emit_to_focused(app, "terminal:zoom-out");
        }
        "new-window" => {
            use tauri::WebviewWindowBuilder;

            // Generate a unique label for the new window
            let label = format!("window-{}", uuid::Uuid::new_v4());
            let webview_url = if cfg!(debug_assertions) {
                tauri::WebviewUrl::External(
                    url::Url::parse("http://localhost:5173").unwrap(),
                )
            } else {
                tauri::WebviewUrl::App("index.html".into())
            };

            let _ = WebviewWindowBuilder::new(app, &label, webview_url)
                .title("K2SO")
                .inner_size(1400.0, 900.0)
                .min_inner_size(800.0, 600.0)
                .hidden_title(true)
                .title_bar_style(tauri::TitleBarStyle::Overlay)
                .build();
        }
        _ => {}
    }
}

fn emit_to_focused(app: &AppHandle, event: &str) {
    // Emit to all windows — the focused one will handle it
    for (_, win) in app.webview_windows() {
        let _ = win.emit(event, ());
    }
}

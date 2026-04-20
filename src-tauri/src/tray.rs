//! macOS menubar icon for the persistent-agents feature.
//!
//! Gives the user visibility into "what's running when K2SO is closed"
//! — the key UX gap that persistent agents introduces. The default
//! "keep daemon running on quit" preference is only safe because the
//! menubar icon surfaces the daemon's state outside the main window.
//!
//! Menu structure:
//!
//! ```text
//!   K2SO Daemon · Running  (PID 1234, up 2h 14m)
//!   ─────────────────────────────────────
//!   Connected sessions (2):
//!     · iPhone 15 Pro  2 min ago
//!     · iPad Pro       47 sec ago
//!   ─────────────────────────────────────
//!   Show K2SO
//!   Settings…
//!   ─────────────────────────────────────
//!   Quit K2SO                          (respects keep-daemon setting)
//!   Quit K2SO and stop daemon          (force-stops regardless)
//! ```
//!
//! The menu is rebuilt every 10s with fresh status + session info.
//! Click → action. All actions go through existing Tauri commands
//! so the menubar + Settings + CLI all agree on behavior.

use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::{TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Wry};

/// Refresh interval for the tray menu contents. 10s is slow enough
/// that we don't spin on `daemon_status()` (which hits the daemon's
/// HTTP surface) but fast enough that "Running → Stopped" transitions
/// feel immediate to users who open the menu.
const REFRESH_INTERVAL: Duration = Duration::from_secs(10);

/// Menu-item IDs we listen for in the tray's `on_menu_event` handler.
/// Using `&'static str` constants rather than match-arms on dynamic
/// IDs so typos are caught at compile time.
mod ids {
    pub const SHOW: &str = "k2so.tray.show";
    pub const SETTINGS: &str = "k2so.tray.settings";
    pub const QUIT: &str = "k2so.tray.quit";
    pub const QUIT_AND_STOP: &str = "k2so.tray.quit-and-stop";
}

/// Handle stored in Tauri state so the refresh loop can rebuild the
/// menu. `Arc<Mutex<>>` because both the refresh thread and the
/// menu-event handler need write access.
pub struct TrayState {
    pub app: AppHandle<Wry>,
}

/// Install the tray icon + wire the refresh loop. Called once from
/// `lib.rs::setup()` AFTER the main window is created (so "Show K2SO"
/// has something to target).
pub fn install(app: &AppHandle<Wry>) -> Result<(), String> {
    let menu = build_menu(app)?;

    let app_clone = app.clone();
    let _tray = TrayIconBuilder::with_id("k2so-main")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(move |app, event| handle_menu_event(app, event.id().as_ref()))
        .on_tray_icon_event(|_tray, event| {
            // Left-click shows the menu (already wired via
            // show_menu_on_left_click=true). We don't do anything
            // special on other event kinds.
            if let TrayIconEvent::Click { .. } = event {
                // no-op — menu opens automatically
            }
        })
        .build(app)
        .map_err(|e| format!("build tray icon: {e}"))?;

    // Periodic refresh. The daemon state ("Running (PID xxx, up Xh)")
    // and connected-session list drift as time passes and clients
    // connect/disconnect. Rebuilding the whole menu is cheap (a
    // handful of `MenuItem::new` calls) and it's the simplest way to
    // keep it current without hand-wiring per-item updates.
    let refresh_app = app.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(REFRESH_INTERVAL);
        if let Ok(menu) = build_menu(&refresh_app) {
            if let Some(tray) = refresh_app.tray_by_id("k2so-main") {
                let _ = tray.set_menu(Some(menu));
            }
        }
    });

    // Stash a handle so future callers can trigger an immediate
    // refresh (e.g., right after daemon_install completes).
    app.manage(Arc::new(Mutex::new(TrayState {
        app: app_clone,
    })));
    Ok(())
}

/// Force an immediate menu rebuild. Called from Tauri command
/// handlers after state-changing actions (daemon install/uninstall/
/// restart) so the user doesn't have to wait for the next 10s tick.
#[allow(dead_code)]
pub fn refresh(app: &AppHandle<Wry>) {
    if let Ok(menu) = build_menu(app) {
        if let Some(tray) = app.tray_by_id("k2so-main") {
            let _ = tray.set_menu(Some(menu));
        }
    }
}

fn build_menu(app: &AppHandle<Wry>) -> Result<Menu<Wry>, String> {
    let status_label = compose_status_label();
    let header = MenuItem::with_id(app, "k2so.tray.header", status_label, false, None::<&str>)
        .map_err(|e| format!("build header: {e}"))?;

    let sep1 = PredefinedMenuItem::separator(app).map_err(|e| format!("sep1: {e}"))?;

    // Sessions block — only rendered if the companion is up AND has
    // at least one connected client. Otherwise the whole submenu is
    // skipped to keep the tray menu tight.
    let (sessions_header, sessions_items): (Option<MenuItem<Wry>>, Vec<MenuItem<Wry>>) =
        compose_sessions(app);

    let sep2 = PredefinedMenuItem::separator(app).map_err(|e| format!("sep2: {e}"))?;

    let show = MenuItem::with_id(app, ids::SHOW, "Show K2SO", true, Some("CmdOrCtrl+O"))
        .map_err(|e| format!("show: {e}"))?;
    let settings_item =
        MenuItem::with_id(app, ids::SETTINGS, "Settings…", true, Some("CmdOrCtrl+,"))
            .map_err(|e| format!("settings: {e}"))?;

    let sep3 = PredefinedMenuItem::separator(app).map_err(|e| format!("sep3: {e}"))?;

    let quit = MenuItem::with_id(app, ids::QUIT, "Quit K2SO", true, Some("CmdOrCtrl+Q"))
        .map_err(|e| format!("quit: {e}"))?;
    let quit_and_stop = MenuItem::with_id(
        app,
        ids::QUIT_AND_STOP,
        "Quit K2SO and stop daemon",
        true,
        None::<&str>,
    )
    .map_err(|e| format!("quit-and-stop: {e}"))?;

    // Assemble. Manual item-list construction (rather than
    // Menu::with_items) so we can conditionally include the sessions
    // block without empty submenus cluttering the tray.
    let menu = Menu::new(app).map_err(|e| format!("menu new: {e}"))?;
    menu.append(&header).map_err(|e| format!("header: {e}"))?;
    menu.append(&sep1).map_err(|e| format!("sep1: {e}"))?;

    if let Some(hdr) = sessions_header {
        menu.append(&hdr).map_err(|e| format!("sess hdr: {e}"))?;
        for item in &sessions_items {
            menu.append(item).map_err(|e| format!("sess item: {e}"))?;
        }
        menu.append(&sep2).map_err(|e| format!("sep2: {e}"))?;
    }

    menu.append(&show).map_err(|e| format!("show: {e}"))?;
    menu.append(&settings_item)
        .map_err(|e| format!("settings: {e}"))?;
    menu.append(&sep3).map_err(|e| format!("sep3: {e}"))?;
    menu.append(&quit).map_err(|e| format!("quit: {e}"))?;
    menu.append(&quit_and_stop)
        .map_err(|e| format!("quit-and-stop: {e}"))?;

    // Touch `Submenu` so the import isn't flagged unused on platforms
    // we don't use a submenu on yet (keeping the import available for
    // the "Recent sessions ▶" submenu we may add later).
    let _ = std::marker::PhantomData::<Submenu<Wry>>;

    Ok(menu)
}

/// Human-readable status line for the tray menu's disabled header.
/// Reads `daemon_status` directly — cheap HTTP call to localhost.
fn compose_status_label() -> String {
    match crate::commands::daemon::daemon_status() {
        crate::commands::daemon::DaemonStatusResponse::Running {
            pid, uptime_secs, ..
        } => {
            format!(
                "K2SO Daemon · Running  (PID {}, up {})",
                pid,
                format_uptime(uptime_secs)
            )
        }
        crate::commands::daemon::DaemonStatusResponse::NotInstalled { .. } => {
            "K2SO Daemon · Not installed".to_string()
        }
        crate::commands::daemon::DaemonStatusResponse::Unreachable { .. } => {
            "K2SO Daemon · Unreachable (crashed?)".to_string()
        }
    }
}

/// Build the "Connected sessions" block. Returns `(header, items)` so
/// the caller can decide whether to render (skip if zero sessions).
///
/// Reads from `k2so_core::companion::companion_status()` which
/// returns the same `connectedClients` count the Settings pane's
/// companion row displays. Per-session detail (device name, last
/// activity) is surfaced from `sessions` in that response when the
/// daemon knows about clients; absent → just a count header.
fn compose_sessions(app: &AppHandle<Wry>) -> (Option<MenuItem<Wry>>, Vec<MenuItem<Wry>>) {
    let status = k2so_core::companion::companion_status();
    let count = status
        .get("connectedClients")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let sessions_json = status.get("sessions").and_then(|v| v.as_array());

    if count == 0 {
        return (None, Vec::new());
    }

    let header_text = if count == 1 {
        "Connected sessions (1):".to_string()
    } else {
        format!("Connected sessions ({}):", count)
    };
    let header = match MenuItem::with_id(
        app,
        "k2so.tray.sessions-header",
        header_text,
        false,
        None::<&str>,
    ) {
        Ok(h) => h,
        Err(_) => return (None, Vec::new()),
    };

    let mut items = Vec::new();
    if let Some(arr) = sessions_json {
        for (i, s) in arr.iter().take(6).enumerate() {
            let device = s
                .get("device")
                .and_then(|v| v.as_str())
                .or_else(|| s.get("label").and_then(|v| v.as_str()))
                .unwrap_or("Unknown device");
            let ago = s
                .get("lastActivityAgo")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let label = if ago.is_empty() {
                format!("   · {}", device)
            } else {
                format!("   · {}  {}", device, ago)
            };
            if let Ok(item) = MenuItem::with_id(
                app,
                format!("k2so.tray.session.{i}"),
                label,
                false,
                None::<&str>,
            ) {
                items.push(item);
            }
        }
    }

    (Some(header), items)
}

fn format_uptime(secs: u64) -> String {
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86400 {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    } else {
        format!("{}d {}h", secs / 86400, (secs % 86400) / 3600)
    }
}

fn handle_menu_event(app: &AppHandle<Wry>, id: &str) {
    match id {
        ids::SHOW => {
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.show();
                let _ = win.unminimize();
                let _ = win.set_focus();
            }
        }
        ids::SETTINGS => {
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.show();
                let _ = win.unminimize();
                let _ = win.set_focus();
                // Emit an event the React side listens for to switch
                // the current workspace view to Settings. Matches the
                // existing `settings:open` pattern used by the
                // command palette + keyboard shortcut.
                use tauri::Emitter;
                let _ = win.emit("settings:open", serde_json::Value::Null);
            }
        }
        ids::QUIT => {
            // Normal quit — honors the "keep daemon running" setting
            // via the ExitRequested handler in lib.rs.
            app.exit(0);
        }
        ids::QUIT_AND_STOP => {
            // Force-stop variant. Ignore the user's preference and
            // unload the plist directly. This is the "I know what I'm
            // doing, kill everything" path for when the user wants a
            // clean stop without changing their default.
            let plist = k2so_core::wake::DaemonPlist::canonical(
                std::path::PathBuf::from("/unused"),
            );
            if let Some(path) = plist.plist_path() {
                if path.exists() {
                    let _ = k2so_core::wake::launchctl_unload(&path);
                }
            }
            app.exit(0);
        }
        _ => {}
    }
}

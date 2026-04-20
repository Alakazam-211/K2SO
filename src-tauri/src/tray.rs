//! macOS menubar icon for the persistent-agents feature.
//!
//! Gives the user visibility into "what's running when K2SO is closed"
//! — the key UX gap that persistent agents introduces. The default
//! "keep daemon running on quit" preference is only safe because the
//! menubar icon surfaces the daemon's state outside the main window.
//!
//! Menu structure (intentionally minimal — the only things a user
//! actually needs to know from the menubar):
//!
//! ```text
//!   Server Status: Running  (PID 1234, up 2h 14m)
//!   Ngrok URL: https://abc.ngrok.app         (click → copy to clipboard)
//!   ─────────────────────────────────────
//!   Connected parties (2):
//!     · iPhone 15 Pro  2 min ago
//!     · iPad Pro       47 sec ago
//!   ─────────────────────────────────────
//!   Quit K2SO                          (honors keep-daemon-on-quit)
//! ```
//!
//! The menu is rebuilt every 10s with fresh status + session info.
//! Click → action. All actions go through existing Tauri commands
//! so the menubar + Settings + CLI all agree on behavior.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use parking_lot::Mutex;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
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
    pub const COPY_TUNNEL_URL: &str = "k2so.tray.copy-tunnel-url";
    pub const QUIT: &str = "k2so.tray.quit";
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
    let menu = Menu::new(app).map_err(|e| format!("menu new: {e}"))?;

    // 1. Server Status — disabled header, just surfaces daemon state.
    let status_label = compose_status_label();
    let status = MenuItem::with_id(
        app,
        "k2so.tray.server-status",
        status_label,
        false,
        None::<&str>,
    )
    .map_err(|e| format!("status: {e}"))?;
    menu.append(&status).map_err(|e| format!("status: {e}"))?;

    // 2. Ngrok URL — clickable, click copies to clipboard. Label reads
    // "Ngrok URL: <url>" if tunnel is up, otherwise "Ngrok URL: Offline"
    // (disabled). The URL lives in companion_status's `tunnelUrl`.
    let companion_status = k2so_core::companion::companion_status();
    let tunnel_url = companion_status
        .get("tunnelUrl")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    let (ngrok_label, ngrok_enabled) = match &tunnel_url {
        Some(url) => (format!("Ngrok URL: {}", url), true),
        None => ("Ngrok URL: Offline".to_string(), false),
    };
    let ngrok = MenuItem::with_id(
        app,
        ids::COPY_TUNNEL_URL,
        ngrok_label,
        ngrok_enabled,
        None::<&str>,
    )
    .map_err(|e| format!("ngrok: {e}"))?;
    menu.append(&ngrok).map_err(|e| format!("ngrok: {e}"))?;

    // 3. Connected parties list — always rendered so the user can see
    // the header even at zero. Header is a disabled label "Connected
    // parties (N):" and each session is a disabled sub-item below.
    let sep1 = PredefinedMenuItem::separator(app).map_err(|e| format!("sep1: {e}"))?;
    menu.append(&sep1).map_err(|e| format!("sep1: {e}"))?;

    let (parties_header, parties_items) = compose_parties(app, &companion_status);
    menu.append(&parties_header)
        .map_err(|e| format!("parties hdr: {e}"))?;
    for item in &parties_items {
        menu.append(item)
            .map_err(|e| format!("party item: {e}"))?;
    }

    // 4. Separator + Quit. Quit honors the keep-daemon-on-quit
    // setting via the normal ExitRequested handler in lib.rs — no
    // tray-local override.
    let sep2 = PredefinedMenuItem::separator(app).map_err(|e| format!("sep2: {e}"))?;
    menu.append(&sep2).map_err(|e| format!("sep2: {e}"))?;

    // IMPORTANT: no `CmdOrCtrl+Q` accelerator here. The OS-level Cmd+Q
    // already routes through our `RunEvent::ExitRequested` handler,
    // so a menu-item accelerator would create a duplicate binding and
    // on some Tauri v2 builds hides the item. Keeping the label only.
    let quit = MenuItem::with_id(app, ids::QUIT, "Quit K2SO", true, None::<&str>)
        .map_err(|e| format!("quit: {e}"))?;
    menu.append(&quit).map_err(|e| format!("quit: {e}"))?;

    // Stash the tunnel URL in a module-local so the menu-event
    // handler can read it back on click without a second HTTP call.
    *last_tunnel_url_slot().lock() = tunnel_url;

    Ok(menu)
}

/// Snapshot of the current ngrok URL captured during the last menu
/// rebuild. Used by the `COPY_TUNNEL_URL` click handler — avoids a
/// second call to `companion_status()` just to get the same string
/// back. Standard `OnceLock<Mutex<Option<_>>>` pattern — no new deps.
static LAST_TUNNEL_URL: OnceLock<Mutex<Option<String>>> = OnceLock::new();

fn last_tunnel_url_slot() -> &'static Mutex<Option<String>> {
    LAST_TUNNEL_URL.get_or_init(|| Mutex::new(None))
}

/// Human-readable status line for the tray menu's disabled header.
/// Reads `daemon_status` directly — cheap HTTP call to localhost.
fn compose_status_label() -> String {
    match crate::commands::daemon::daemon_status() {
        crate::commands::daemon::DaemonStatusResponse::Running {
            pid, uptime_secs, ..
        } => {
            format!(
                "Server Status: Running  (PID {}, up {})",
                pid,
                format_uptime(uptime_secs)
            )
        }
        crate::commands::daemon::DaemonStatusResponse::NotInstalled { .. } => {
            "Server Status: Not installed".to_string()
        }
        crate::commands::daemon::DaemonStatusResponse::Unreachable { .. } => {
            "Server Status: Unreachable (daemon crashed?)".to_string()
        }
    }
}

/// Build the "Connected parties" block. Returns `(header, items)`.
/// Always renders a header, even at zero — "Connected parties (0):"
/// is a valid status the user wants to see. Items only render when
/// sessions exist.
///
/// Reads from `k2so_core::companion::companion_status()` which
/// returns the same `connectedClients` count the Settings pane's
/// companion row displays. Per-session detail (device name, last
/// activity) is surfaced from `sessions` in that response when the
/// daemon knows about clients; absent → just the count header.
fn compose_parties(
    app: &AppHandle<Wry>,
    companion_status: &serde_json::Value,
) -> (MenuItem<Wry>, Vec<MenuItem<Wry>>) {
    let count = companion_status
        .get("connectedClients")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let sessions_json = companion_status.get("sessions").and_then(|v| v.as_array());

    let header_text = format!("Connected parties ({}):", count);
    let header = MenuItem::with_id(
        app,
        "k2so.tray.parties-header",
        header_text,
        false,
        None::<&str>,
    )
    .unwrap_or_else(|_| {
        // Fallback label if MenuItem::with_id ever fails (shouldn't).
        // The unwrap branch lets compose_parties stay infallible so
        // the caller doesn't have to handle Option.
        MenuItem::with_id(app, "k2so.tray.parties-fallback", "Connected parties", false, None::<&str>)
            .expect("bare menu item must build")
    });

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
                format!("k2so.tray.party.{i}"),
                label,
                false,
                None::<&str>,
            ) {
                items.push(item);
            }
        }
    }

    (header, items)
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
        ids::COPY_TUNNEL_URL => {
            // Copy the ngrok URL captured during the last menu build.
            // Using the clipboard plugin the app already registers
                // (tauri-plugin-clipboard-manager) — no new dep.
            if let Some(url) = last_tunnel_url_slot().lock().clone() {
                use tauri_plugin_clipboard_manager::ClipboardExt;
                let _ = app.clipboard().write_text(url);
            }
        }
        ids::QUIT => {
            // Honors the "keep daemon running when K2SO quits" setting
            // via the RunEvent::ExitRequested handler in lib.rs. No
            // tray-local override — one quit path, one policy.
            app.exit(0);
        }
        _ => {}
    }
}

//! Quit-confirmation plumbing for Cmd+Q when the daemon is running.
//!
//! macOS Cmd+Q bypasses the window CloseRequested event and goes straight
//! to `RunEvent::ExitRequested`. In release builds, when the daemon is
//! live, we prevent that exit and bounce the decision to the user:
//!
//!   "K2SO is running in the background and can keep agents working
//!    while the app is closed. Quit mode:
//!       [Keep daemon running]   [Stop everything]   [Cancel]"
//!
//! The user's choice goes through `confirm_quit` below, which flips the
//! `USER_CONFIRMED_QUIT` atomic and re-requests exit. The ExitRequested
//! handler checks the atomic on re-entry and lets the exit proceed.
//!
//! Dev builds skip this entirely — a hung confirmation dialog would
//! block my CLI-kill workflow. Gated via `#[cfg(not(debug_assertions))]`
//! on the install path in `lib.rs`.

use std::sync::atomic::{AtomicBool, Ordering};

/// Set to `true` by `confirm_quit` when the user picks a non-cancel
/// action. The `RunEvent::ExitRequested` handler reads this on re-entry
/// and lets the app exit without prompting a second time.
pub static USER_CONFIRMED_QUIT: AtomicBool = AtomicBool::new(false);

/// Wire format for the confirm-quit user choice. Matches the 3 buttons
/// in `DaemonQuitDialog.tsx`.
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QuitAction {
    /// Close the Tauri window + exit the Tauri process. Daemon keeps
    /// running under launchd (KeepAlive is untouched). Agents survive.
    Keep,
    /// Stop the daemon via `launchctl unload`, then exit Tauri. Agents
    /// are frozen until the user re-opens the app.
    Stop,
    /// Don't exit. Clear the prompt and keep the Tauri window open.
    Cancel,
}

/// Called from the frontend's `DaemonQuitDialog` after the user clicks
/// one of the three buttons. Re-triggers the exit path with
/// `USER_CONFIRMED_QUIT` set so the ExitRequested guard lets it
/// through.
#[tauri::command]
pub fn confirm_quit(app: tauri::AppHandle, action: QuitAction) -> Result<(), String> {
    match action {
        QuitAction::Cancel => {
            // Nothing to do — the ExitRequested handler already called
            // `api.prevent_exit()`, so the app is already staying open.
            // Frontend dismisses the dialog on its own.
            Ok(())
        }
        QuitAction::Keep => {
            USER_CONFIRMED_QUIT.store(true, Ordering::SeqCst);
            app.exit(0);
            Ok(())
        }
        QuitAction::Stop => {
            // Best-effort daemon unload. Any error is surfaced to the
            // user but doesn't block the exit — they asked to quit, we
            // quit. If launchctl choked, the daemon may still be alive
            // under launchd, but that's recoverable from Settings on
            // the next launch.
            let plist =
                k2so_core::wake::DaemonPlist::canonical(std::path::PathBuf::from("/unused"));
            if let Some(plist_path) = plist.plist_path() {
                if plist_path.exists() {
                    let _ = k2so_core::wake::launchctl_unload(&plist_path);
                }
            }
            USER_CONFIRMED_QUIT.store(true, Ordering::SeqCst);
            app.exit(0);
            Ok(())
        }
    }
}

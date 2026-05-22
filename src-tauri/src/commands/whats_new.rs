//! Tauri commands for the "what's new" popup.
//!
//! Three thin wrappers around the daemon's `/cli/whats_new*` routes:
//!
//! - `whats_new_check` — GETs the current/last-seen/has_new/content
//!   payload. Called from the renderer on app mount; the renderer
//!   shows the popup iff `has_new == true`.
//! - `whats_new_mark_seen` — POSTed when the user dismisses the popup.
//! - `whats_new_reset` — clears the dismissal marker (forces re-show
//!   next launch). Exposed for testing + a future "show me what's new
//!   again" Settings affordance.
//!
//! Daemon-first per the architecture invariant: this file owns no
//! logic of its own — parsing, version compare, state I/O all live
//! in `k2so_core::whats_new`. Tauri is a thin client over the daemon
//! HTTP surface.

use std::time::Duration;

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub struct WhatsNewPayload {
    pub current_version: String,
    pub last_seen_version: Option<String>,
    pub has_new: bool,
    pub content: String,
}

/// `whats_new_check` fires on Tauri app mount — right when launchd is
/// still bringing the daemon up. If we hit a single connect-and-fail
/// at that moment the popup silently misses (the 0.38.7 launch-day
/// regression). Poll for daemon reachability up to 10× at 500ms
/// intervals (5s total) — same pattern as `check_daemon_version_and_restart`
/// in lib.rs. Returns Ok on first success; the renderer treats any
/// final Err as silent-skip (popup just doesn't show this launch).
#[tauri::command]
pub fn whats_new_check() -> Result<WhatsNewPayload, String> {
    let mut last_err = String::from("daemon unreachable: no attempts made");
    for attempt in 0..10 {
        if attempt > 0 {
            std::thread::sleep(Duration::from_millis(500));
        }
        match crate::daemon_client::DaemonClient::try_connect() {
            Ok(client) => match client.cli_get("/cli/whats_new", &[]) {
                Ok(body) => {
                    return serde_json::from_str::<WhatsNewPayload>(&body)
                        .map_err(|e| format!("malformed whats_new response: {e} ({body})"));
                }
                Err(e) => last_err = format!("daemon /cli/whats_new: {e}"),
            },
            Err(e) => last_err = format!("daemon unreachable: {e}"),
        }
    }
    Err(last_err)
}

#[tauri::command]
pub fn whats_new_mark_seen() -> Result<(), String> {
    let client = crate::daemon_client::DaemonClient::try_connect()
        .map_err(|e| format!("daemon unreachable: {e}"))?;
    client.cli_get("/cli/whats_new/mark_seen", &[])?;
    Ok(())
}

#[tauri::command]
pub fn whats_new_reset() -> Result<(), String> {
    let client = crate::daemon_client::DaemonClient::try_connect()
        .map_err(|e| format!("daemon unreachable: {e}"))?;
    client.cli_get("/cli/whats_new/reset", &[])?;
    Ok(())
}

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

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub struct WhatsNewPayload {
    pub current_version: String,
    pub last_seen_version: Option<String>,
    pub has_new: bool,
    pub content: String,
}

#[tauri::command]
pub fn whats_new_check() -> Result<WhatsNewPayload, String> {
    let client = crate::daemon_client::DaemonClient::try_connect()
        .map_err(|e| format!("daemon unreachable: {e}"))?;
    let body = client.cli_get("/cli/whats_new", &[])?;
    serde_json::from_str::<WhatsNewPayload>(&body)
        .map_err(|e| format!("malformed whats_new response: {e} ({body})"))
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

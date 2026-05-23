//! Phase 2 Unit 7c — daemon-owned heartbeat-launchd installer.
//!
//! Pre-Unit-7c the heartbeat plist (`com.k2so.agent-heartbeat.plist`)
//! was installed + removed by Tauri commands in
//! `src-tauri/src/commands/k2so_agents.rs`. With Unit 7c the daemon
//! owns its own scheduler plist so K2SO Connect (remote daemon
//! without Tauri) can install + remove its launchd agent under its
//! own GUI session. Same architectural pattern Unit 1 established for
//! `com.k2so.daemon.plist` and Unit 5 for `com.k2so.claude-auth-refresh.plist`.
//!
//! Routes:
//!
//! - `POST /cli/heartbeat/install-launchd` — body:
//!   `{"interval_seconds": <u32>, "wake_system": <bool>}`. Writes
//!   `~/.k2so/heartbeat.sh` + writes/loads the plist.
//! - `POST /cli/heartbeat/uninstall-launchd` — body: ignored.
//!   Unloads + deletes the plist + removes `heartbeat.sh`.
//! - `POST /cli/heartbeat/apply-wake-scheduler` — body:
//!   `{"mode": "off"|"on_demand"|"heartbeat", "interval_minutes": <u32>,
//!     "wake_system": <bool>}`. Routes to the matching install/uninstall.
//!
//! The actual install/uninstall logic lives in
//! `k2so_core::agents::heartbeat_install` so headless callers (CLI,
//! daemon boot, future CLI verb) share one implementation. These
//! handlers parse the JSON body, dispatch, and return a uniform
//! response shape:
//!
//! ```json
//! { "success": true, "message": "...optional human summary..." }
//! ```
//!
//! Method gates live in `main.rs::handle_connection`'s per-route
//! match arm (NOT in `dispatch_unit6_post`) so they short-circuit
//! GET requests before this module even gets called.

use serde::Deserialize;

use k2so_core::agents::heartbeat_install;

use crate::cli_response::CliResponse;

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct InstallLaunchdBody {
    /// Seconds between heartbeat fires. Defaults to 60 if missing.
    interval_seconds: Option<u32>,
    /// Whether the plist should set `WakeSystem=true`. Defaults to false.
    wake_system: Option<bool>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct ApplyWakeSchedulerBody {
    /// One of "off", "on_demand", "heartbeat".
    mode: Option<String>,
    /// Minutes between fires (in "heartbeat" mode). Defaults to 5.
    interval_minutes: Option<u32>,
    /// WakeSystem flag (in "heartbeat" mode). Defaults to false.
    wake_system: Option<bool>,
}

/// Handler for `POST /cli/heartbeat/install-launchd`.
///
/// Writes `~/.k2so/heartbeat.sh` + installs the launchd plist (macOS)
/// or crontab entry (Linux). Idempotent.
pub fn handle_install_launchd(body: &[u8]) -> CliResponse {
    let parsed: InstallLaunchdBody = if body.is_empty() {
        InstallLaunchdBody::default()
    } else {
        match serde_json::from_slice(body) {
            Ok(b) => b,
            Err(e) => return CliResponse::bad_request(format!("invalid body: {e}")),
        }
    };
    let interval_seconds = parsed.interval_seconds.unwrap_or(60).max(60);
    let wake_system = parsed.wake_system.unwrap_or(false);

    match heartbeat_install::install_heartbeat_scheduler(interval_seconds, wake_system) {
        Ok(msg) => CliResponse::ok_json(
            serde_json::json!({ "success": true, "message": msg }).to_string(),
        ),
        Err(e) => CliResponse::bad_request(e),
    }
}

/// Handler for `POST /cli/heartbeat/uninstall-launchd`.
///
/// Unloads + deletes the plist (macOS) or strips the crontab entry
/// (Linux). Idempotent — success on a host that never had the
/// scheduler installed.
pub fn handle_uninstall_launchd(_body: &[u8]) -> CliResponse {
    match heartbeat_install::uninstall_heartbeat_scheduler() {
        Ok(()) => CliResponse::ok_json(r#"{"success":true}"#.to_string()),
        Err(e) => CliResponse::bad_request(e),
    }
}

/// Handler for `POST /cli/heartbeat/apply-wake-scheduler`.
///
/// Reads the user's Wake Scheduler settings and either installs or
/// uninstalls the heartbeat plist accordingly. Mode "off" /
/// "on_demand" uninstall, "heartbeat" installs with the user's
/// interval + wake_system.
pub fn handle_apply_wake_scheduler(body: &[u8]) -> CliResponse {
    let parsed: ApplyWakeSchedulerBody = if body.is_empty() {
        ApplyWakeSchedulerBody::default()
    } else {
        match serde_json::from_slice(body) {
            Ok(b) => b,
            Err(e) => return CliResponse::bad_request(format!("invalid body: {e}")),
        }
    };
    let mode = parsed.mode.unwrap_or_else(|| "off".to_string());
    let interval_minutes = parsed.interval_minutes.unwrap_or(5).max(1);
    let wake_system = parsed.wake_system.unwrap_or(false);

    match heartbeat_install::apply_wake_scheduler(&mode, interval_minutes, wake_system) {
        Ok(msg) => CliResponse::ok_json(
            serde_json::json!({ "success": true, "message": msg }).to_string(),
        ),
        Err(e) => CliResponse::bad_request(e),
    }
}

//! Tauri command surface for the k2so-daemon.
//!
//! Exposes a tiny, frontend-facing view of the persistent-agent daemon's
//! lifecycle — enough for a Settings panel to show "daemon: running
//! (v0.33.0-dev, uptime 4m)" or "daemon: not installed" without the
//! frontend needing to know how the daemon is discovered or
//! authenticated. Wraps `crate::daemon_client::DaemonClient`.

use serde::Serialize;

use crate::daemon_client::{DaemonClient, DaemonStatus};

/// Shape returned to the frontend. Distinct variants rather than
/// Option<DaemonStatus> so the renderer can distinguish "daemon not
/// installed" (port file missing) from "daemon installed but crashed"
/// (port file stale).
#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum DaemonStatusResponse {
    /// Daemon is reachable and authenticated; status payload included.
    Running {
        version: String,
        uptime_secs: u64,
        pid: u32,
        port: u16,
    },
    /// `~/.k2so/daemon.port` or `~/.k2so/daemon.token` not found. The
    /// install migration hasn't run yet, or the user uninstalled the
    /// plist.
    NotInstalled { reason: String },
    /// Files exist but the daemon didn't answer — almost certainly
    /// crashed between launchd restarts. Tauri could offer a "re-install
    /// daemon" button when this state appears.
    Unreachable { reason: String },
}

/// Returns the daemon's current state for the frontend Settings panel.
/// Never returns `Err` — instead encodes each failure mode in the
/// response variant so the frontend can render the right UI without
/// splitting try/catch.
#[tauri::command]
pub fn daemon_status() -> DaemonStatusResponse {
    let client = match DaemonClient::try_connect() {
        Ok(c) => c,
        Err(e) => return DaemonStatusResponse::NotInstalled { reason: e },
    };
    match client.status() {
        Ok(DaemonStatus {
            version,
            uptime_secs,
            pid,
            port,
        }) => DaemonStatusResponse::Running {
            version,
            uptime_secs,
            pid,
            port,
        },
        Err(e) => DaemonStatusResponse::Unreachable { reason: e },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Meaty integration lives in scripts/ — these tests cover the pure
    // Serialize shape so frontend contract changes are loud at build.
    #[test]
    fn running_variant_serializes_with_state_tag() {
        let r = DaemonStatusResponse::Running {
            version: "0.33.0-dev".to_string(),
            uptime_secs: 42,
            pid: 1234,
            port: 58211,
        };
        let json = serde_json::to_string(&r).expect("serialize");
        assert!(json.contains("\"state\":\"running\""), "got: {json}");
        assert!(json.contains("\"version\":\"0.33.0-dev\""), "got: {json}");
        assert!(json.contains("\"uptime_secs\":42"), "got: {json}");
    }

    #[test]
    fn not_installed_variant_serializes_with_state_tag() {
        let r = DaemonStatusResponse::NotInstalled {
            reason: "port file missing".to_string(),
        };
        let json = serde_json::to_string(&r).expect("serialize");
        assert!(json.contains("\"state\":\"not_installed\""), "got: {json}");
        assert!(json.contains("\"reason\":\"port file missing\""), "got: {json}");
    }

    #[test]
    fn unreachable_variant_serializes_with_state_tag() {
        let r = DaemonStatusResponse::Unreachable {
            reason: "connection refused".to_string(),
        };
        let json = serde_json::to_string(&r).expect("serialize");
        assert!(json.contains("\"state\":\"unreachable\""), "got: {json}");
    }

    #[test]
    fn daemon_status_with_no_files_returns_not_installed() {
        // `daemon_status()` reads ~/.k2so/daemon.port. If it's not
        // there (common in dev — nobody's installed the plist), the
        // command should quietly return NotInstalled instead of
        // panicking or returning an Err.
        let k2so_dir = dirs::home_dir().unwrap().join(".k2so");
        let port_file = k2so_dir.join("daemon.port");
        let token_file = k2so_dir.join("daemon.token");
        // Only run if neither file happens to exist in the dev env.
        if port_file.exists() || token_file.exists() {
            eprintln!("[test] daemon files present; skipping NotInstalled assertion");
            return;
        }
        match daemon_status() {
            DaemonStatusResponse::NotInstalled { .. } => {}
            other => panic!("expected NotInstalled, got {other:?}"),
        }
    }
}

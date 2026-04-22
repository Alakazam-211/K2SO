//! Tauri commands for Kessel terminal spawning.
//!
//! **Why this module exists.** The browser-side `KesselTerminal.tsx`
//! used to call `fetch('http://127.0.0.1:<port>/cli/sessions/spawn')`
//! directly. That's ~3-8ms of pure browser overhead per spawn:
//!
//! - Tauri's fetch pipeline + CSP check
//! - JSON body serialization (in JS)
//! - URL parsing + options walk
//! - Network layer hop to the daemon
//!
//! Moving the POST into a Tauri command lets us:
//!
//! 1. Read port/token ONCE at startup via `DaemonClient` and reuse
//!    a persistent `reqwest::blocking::Client` with keep-alive — no
//!    TCP handshake per spawn after the first.
//! 2. Skip the browser-side JSON serialization round-trip — `serde`
//!    on the Rust side is faster than `JSON.stringify` + network
//!    body encoding.
//! 3. Surface a typed result back to the frontend in one IPC hop,
//!    instead of two layers (fetch response → .json() parse).
//!
//! Combined with the `daemon_ws_url` cache already in place, a
//! warm Kessel spawn is now: one Tauri IPC → one POST (persistent
//! connection) → SpawnResponse → React state update.
//!
//! The endpoint served by the daemon is unchanged —
//! `POST /cli/sessions/spawn` in `crates/k2so-daemon/src/awareness_ws.rs`.

use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

/// Cached daemon credentials. Populated lazily on first call;
/// invalidated when a request comes back 401/403 (daemon restarted
/// and rotated the token).
#[derive(Clone)]
struct DaemonCreds {
    port: u16,
    token: String,
}

static DAEMON_CREDS: OnceLock<RwLock<Option<DaemonCreds>>> = OnceLock::new();
static HTTP_CLIENT: OnceLock<reqwest::blocking::Client> = OnceLock::new();

fn creds_cache() -> &'static RwLock<Option<DaemonCreds>> {
    DAEMON_CREDS.get_or_init(|| RwLock::new(None))
}

fn http_client() -> &'static reqwest::blocking::Client {
    HTTP_CLIENT.get_or_init(|| {
        reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(10))
            // pool_max_idle_per_host default is usize::MAX; connections
            // to 127.0.0.1 are kept warm so the 2nd+ spawn in a session
            // doesn't pay the ~100-500µs TCP handshake cost.
            .pool_idle_timeout(Duration::from_secs(60))
            .build()
            .expect("build kessel http client")
    })
}

fn load_creds() -> Result<DaemonCreds, String> {
    if let Some(c) = creds_cache().read().clone() {
        return Ok(c);
    }
    // Miss — read from ~/.k2so/heartbeat.{port,token}. Same files
    // the legacy daemon_ws_url command reads; avoids a second
    // file-reading helper in the codebase.
    let home = dirs::home_dir().ok_or_else(|| "home dir unavailable".to_string())?;
    let port_path = home.join(".k2so/heartbeat.port");
    let token_path = home.join(".k2so/heartbeat.token");
    let port: u16 = fs::read_to_string(&port_path)
        .map_err(|e| format!("read {}: {e}", port_path.display()))?
        .trim()
        .parse()
        .map_err(|e| format!("parse port: {e}"))?;
    let token = fs::read_to_string(&token_path)
        .map_err(|e| format!("read {}: {e}", token_path.display()))?
        .trim()
        .to_string();
    if token.is_empty() {
        return Err("daemon token file empty".into());
    }
    let creds = DaemonCreds { port, token };
    *creds_cache().write() = Some(creds.clone());
    Ok(creds)
}

fn invalidate_creds() {
    *creds_cache().write() = None;
}

// ── Request / response shapes mirroring the browser-side path ──

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KesselSpawnResponse {
    pub session_id: String,
    pub agent_name: String,
    pub port: u16,
    pub token: String,
    /// Whole-operation duration in ms, measured in Rust. Lets the
    /// frontend show "spawned in Nms" without another clock read.
    pub spawn_ms: u64,
    /// Fine-grained breakdown of the spawn path. Only populated in
    /// debug builds; release builds still fill spawn_ms but set all
    /// sub-timings to zero to avoid the extra Instant::now() calls.
    /// Units: microseconds.
    pub timing_us: KesselSpawnTiming,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KesselSpawnTiming {
    /// Time to resolve cached daemon creds (0 after first call).
    pub creds_us: u64,
    /// Time to serialize the JSON request body.
    pub serialize_us: u64,
    /// Time spent inside `reqwest::Client::post().send()` — this is
    /// the daemon's actual work (HTTP accept + spawn_agent_session +
    /// response) plus TCP round-trip overhead. On loopback with
    /// keep-alive, TCP overhead is ~100-500µs, so the bulk of this
    /// number is the daemon-side spawn.
    pub http_us: u64,
    /// Time to read the response body bytes off the socket.
    pub response_read_us: u64,
    /// Time to deserialize the response JSON.
    pub deserialize_us: u64,
}

/// Internal daemon response body. Mirror of `awareness_ws.rs`'s
/// `handle_sessions_spawn` output.
#[derive(Debug, Deserialize)]
struct DaemonSpawnBody {
    #[serde(rename = "sessionId")]
    session_id: String,
    #[serde(rename = "agentName")]
    agent_name: String,
    #[allow(dead_code)]
    #[serde(rename = "pendingDrained", default)]
    pending_drained: usize,
}

/// Spawn a Kessel session via the daemon. Frontend calls this
/// instead of `fetch()`-ing the spawn endpoint directly.
///
/// Returns `{sessionId, agentName, port, token, spawnMs}` — the
/// port/token so `SessionStreamView` can open the WS without a
/// second `daemon_ws_url` round trip, and `spawnMs` for devtools
/// timing display.
///
/// Invalidates the cached token on 401/403 so a daemon restart
/// doesn't wedge the user — next call retries from disk.
#[tauri::command]
pub fn kessel_spawn(
    terminal_id: String,
    cwd: String,
    command: Option<String>,
    args: Option<Vec<String>>,
    cols: Option<u16>,
    rows: Option<u16>,
) -> Result<KesselSpawnResponse, String> {
    let started = std::time::Instant::now();
    let mut t = started;
    let mut timing = KesselSpawnTiming::default();

    let creds = load_creds()?;
    timing.creds_us = t.elapsed().as_micros() as u64;
    t = std::time::Instant::now();

    let body = serde_json::json!({
        "agent_name": format!("tab-{}", terminal_id),
        "cwd": cwd,
        "command": command,
        "args": args,
        "cols": cols.unwrap_or(80),
        "rows": rows.unwrap_or(24),
    });
    let body_bytes = serde_json::to_vec(&body).map_err(|e| format!("serialize: {e}"))?;
    timing.serialize_us = t.elapsed().as_micros() as u64;
    t = std::time::Instant::now();

    let url = format!(
        "http://127.0.0.1:{}/cli/sessions/spawn?token={}",
        creds.port, creds.token
    );

    let response = http_client()
        .post(&url)
        .header("Content-Type", "application/json")
        .body(body_bytes)
        .send()
        .map_err(|e| {
            // Network-level failure — daemon may have been restarted.
            // Invalidate so the next call re-reads the credential files.
            invalidate_creds();
            format!("post /cli/sessions/spawn: {e}")
        })?;
    timing.http_us = t.elapsed().as_micros() as u64;
    t = std::time::Instant::now();

    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED
        || status == reqwest::StatusCode::FORBIDDEN
    {
        invalidate_creds();
    }

    let body_text = response
        .text()
        .map_err(|e| format!("read body: {e}"))?;
    timing.response_read_us = t.elapsed().as_micros() as u64;
    t = std::time::Instant::now();

    if !status.is_success() {
        return Err(format!("daemon {}: {}", status.as_u16(), body_text));
    }

    let spawn: DaemonSpawnBody = serde_json::from_str(&body_text)
        .map_err(|e| format!("decode body: {e}: {body_text}"))?;
    timing.deserialize_us = t.elapsed().as_micros() as u64;

    Ok(KesselSpawnResponse {
        session_id: spawn.session_id,
        agent_name: spawn.agent_name,
        port: creds.port,
        token: creds.token,
        spawn_ms: started.elapsed().as_millis() as u64,
        timing_us: timing,
    })
}

/// Expose the cached daemon creds to the frontend without the
/// fs::read_to_string cost the legacy `daemon_ws_url` command pays
/// per call. Caller semantics match the legacy command — returns
/// `{state: "available", port, token}` on success or
/// `{state: "not_installed", reason}` on failure.
///
/// The frontend's existing `daemon-ws.ts` cache can either keep
/// using the legacy command or migrate to this one; both return the
/// same shape. The win is that subsequent calls hit the in-memory
/// cache on the Rust side too.
#[derive(Debug, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum KesselDaemonWsResponse {
    Available { port: u16, token: String },
    NotInstalled { reason: String },
}

#[tauri::command]
pub fn kessel_daemon_ws() -> KesselDaemonWsResponse {
    match load_creds() {
        Ok(c) => KesselDaemonWsResponse::Available {
            port: c.port,
            token: c.token,
        },
        Err(e) => KesselDaemonWsResponse::NotInstalled { reason: e },
    }
}

/// Write bytes to a Kessel session's PTY. Replaces the browser-side
/// `fetch('/cli/terminal/write?...')` per-keystroke hot path.
///
/// **Why this matters.** Browser fetch carries ~3-15ms of pipeline
/// overhead per call (CSP check, URL parsing, network layer hop).
/// At fast typing speeds (70+ wpm ≈ 7 cps), each keystroke was
/// paying that tax, producing visible lag. Tauri IPC → persistent
/// reqwest → daemon is ~1-3ms. Alacritty uses plain `invoke()` for
/// its write path and feels instant; this brings Kessel to parity.
///
/// Fire-and-forget on the frontend side — the Tauri command returns
/// once the HTTP call is sent; callers don't need to await the PTY
/// acknowledgement.
#[tauri::command]
pub fn kessel_write(session_id: String, text: String) -> Result<(), String> {
    let creds = load_creds()?;
    // URL-encode `text` so control bytes (\r, \n, escape sequences)
    // round-trip correctly. reqwest's query param handling handles
    // this natively.
    let url = format!(
        "http://127.0.0.1:{}/cli/terminal/write",
        creds.port
    );
    let response = http_client()
        .get(&url)
        .query(&[
            ("id", session_id.as_str()),
            ("message", text.as_str()),
            ("token", creds.token.as_str()),
            ("no_submit", "true"),
        ])
        .send()
        .map_err(|e| {
            invalidate_creds();
            format!("write: {e}")
        })?;
    if !response.status().is_success() {
        let s = response.status();
        if s == reqwest::StatusCode::UNAUTHORIZED
            || s == reqwest::StatusCode::FORBIDDEN
        {
            invalidate_creds();
        }
        return Err(format!("daemon write {}: body-skipped", s.as_u16()));
    }
    Ok(())
}

/// Resize a Kessel session's PTY. Same Tauri-IPC pattern as
/// `kessel_write` — eliminates browser fetch overhead from the
/// resize path. Resizes fire on every ResizeObserver callback,
/// typically debounced to ~100ms but still worth routing through
/// the faster path.
#[tauri::command]
pub fn kessel_resize(
    session_id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let creds = load_creds()?;
    let url = format!(
        "http://127.0.0.1:{}/cli/sessions/resize",
        creds.port
    );
    let response = http_client()
        .get(&url)
        .query(&[
            ("session", session_id.as_str()),
            ("cols", cols.to_string().as_str()),
            ("rows", rows.to_string().as_str()),
            ("token", creds.token.as_str()),
        ])
        .send()
        .map_err(|e| {
            invalidate_creds();
            format!("resize: {e}")
        })?;
    if !response.status().is_success() {
        return Err(format!("daemon resize {}: failed", response.status().as_u16()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Confirm the cached-credentials round trip: after `load_creds`
    /// populates the cache, a follow-up read returns the same values
    /// without touching disk. We can't easily unit-test the fs path
    /// without a real daemon, so this just pins the cache behavior.
    #[test]
    fn creds_cache_round_trips() {
        // Force-populate the cache directly (bypassing disk).
        *creds_cache().write() = Some(DaemonCreds {
            port: 12345,
            token: "test-token".into(),
        });
        let c = load_creds().expect("cache hit");
        assert_eq!(c.port, 12345);
        assert_eq!(c.token, "test-token");

        // Invalidate + verify cache clears.
        invalidate_creds();
        assert!(creds_cache().read().is_none());
    }

    /// Tauri's codegen wants a deterministic #[tauri::command]
    /// signature; failing to annotate correctly is a build-time
    /// error. Nothing to test at runtime, but having the command
    /// module compile under test configuration catches signature
    /// drift.
    #[test]
    fn command_module_compiles() {
        let _ = kessel_daemon_ws;
        let _ = kessel_spawn;
    }
}

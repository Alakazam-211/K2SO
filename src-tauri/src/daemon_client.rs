//! Tauri-side HTTP client for the k2so-daemon.
//!
//! The daemon exposes a token-authed HTTP server on `127.0.0.1:<random>` —
//! see `crates/k2so-daemon/src/main.rs`. This module is the Tauri app's
//! counterpart: it discovers the daemon's port + token via the conventional
//! `~/.k2so/daemon.port` / `~/.k2so/daemon.token` files and wraps
//! `reqwest::blocking` calls so command handlers in `src-tauri` can proxy
//! state-mutating work through the daemon instead of running it in-process.
//!
//! Scope of this commit: **connection management + the two endpoints the
//! daemon serves today (`/ping`, `/status`)**. Additional endpoints (state
//! proxies, scheduler calls, etc.) land in later commits as the daemon
//! grows them.
//!
//! Design choices:
//! - **Blocking reqwest** matches the existing K2SO HTTP story (llm::download
//!   + push adapters both use `reqwest::blocking`). No tokio-runtime
//!   ceremony for the handful of ops we make per command.
//! - **Token loaded lazily** — we re-read the port/token files on every
//!   construction because the daemon can be restarted (launchd KeepAlive)
//!   and its port + token rotate. Holding a stale client is a footgun.

use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde::Deserialize;

/// Minimal JSON shape returned by `GET /status` on the daemon.
/// Keep in lockstep with `crates/k2so-daemon/src/main.rs::send_response`
/// body composition.
#[derive(Debug, Clone, Deserialize)]
pub struct DaemonStatus {
    pub version: String,
    pub uptime_secs: u64,
    pub pid: u32,
    pub port: u16,
}

/// A process-global override of WHICH daemon every `DaemonClient` talks
/// to. Set from the renderer (via the `set_active_daemon` Tauri command)
/// whenever the active K2 Connect host changes:
///   - `Some(ActiveDaemon{ base, token })` → all `try_connect()` clients
///     target the REMOTE daemon at `base` with the remote session token.
///   - `None` (the default, and what 'local' restores) → clients fall
///     back to the bundled local daemon discovered via the
///     `~/.k2so/daemon.{port,token}` files, byte-identical to before.
#[derive(Clone)]
struct ActiveDaemon {
    /// Full scheme+authority, NO trailing slash, e.g.
    /// `http://127.0.0.1:58211` or `https://reggie.k2.dev`.
    base: String,
    /// Session token that rides as `?token=` on every request.
    token: String,
}

/// Lazily-initialized holder for the active-daemon override. `None` inside
/// the `Mutex` means "use the local daemon" (the default).
static ACTIVE: OnceLock<Mutex<Option<ActiveDaemon>>> = OnceLock::new();

fn active_cell() -> &'static Mutex<Option<ActiveDaemon>> {
    ACTIVE.get_or_init(|| Mutex::new(None))
}

/// Point every subsequently-constructed `DaemonClient` at a REMOTE daemon
/// (or clear back to local). Called from the `set_active_daemon` Tauri
/// command when the renderer switches the active K2 Connect host.
///
/// Both `base` and `token` must be `Some` to install a remote override;
/// if either is `None` the override is cleared (→ local daemon). `base`
/// is normalized to drop any trailing slash so URL composition stays a
/// simple `format!("{base}{path}")`.
pub fn set_active_daemon(base: Option<String>, token: Option<String>) {
    let next = match (base, token) {
        (Some(b), Some(t)) => {
            let trimmed = b.trim_end_matches('/').to_string();
            if trimmed.is_empty() || t.is_empty() {
                None
            } else {
                Some(ActiveDaemon { base: trimmed, token: t })
            }
        }
        _ => None,
    };
    if let Ok(mut guard) = active_cell().lock() {
        *guard = next;
    }
}

/// Snapshot the current override (clone so we don't hold the lock across
/// the network call).
fn active_daemon() -> Option<ActiveDaemon> {
    active_cell().lock().ok().and_then(|g| g.clone())
}

/// Holds the resolved daemon base URL + token, loaded on construction.
/// Cheap to create — either a clone of the in-memory override or two tiny
/// file reads. Create one per command-handler call; don't cache across
/// commands.
pub struct DaemonClient {
    /// Full scheme+authority, NO trailing slash (e.g.
    /// `http://127.0.0.1:58211` or `https://reggie.k2.dev`). All request
    /// URLs are `format!("{base}{path}...")`.
    base: String,
    token: String,
    http: reqwest::blocking::Client,
}

impl DaemonClient {
    /// Resolve a ready-to-use client.
    ///
    /// If a remote override is installed (see [`set_active_daemon`]) the
    /// client targets that remote daemon. Otherwise it reads
    /// `~/.k2so/daemon.port` + `~/.k2so/daemon.token` and targets the
    /// local bundled daemon at `http://127.0.0.1:<port>`. `Err` only in
    /// the local path when either file is missing or malformed — caller's
    /// responsibility to trigger `launchctl load` and retry.
    pub fn try_connect() -> Result<Self, String> {
        let http = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| format!("build http client: {e}"))?;
        if let Some(remote) = active_daemon() {
            // Remote K2 Connect host: base + session token come straight
            // from the renderer-installed override. reqwest::blocking
            // already links the TLS backend (used by llm::download), so an
            // `https://` base needs no extra ceremony here.
            return Ok(Self {
                base: remote.base,
                token: remote.token,
                http,
            });
        }
        // Local bundled daemon: discover port + token from disk.
        let k2so_dir = k2so_dir()?;
        let port = read_port(&k2so_dir.join("daemon.port"))?;
        let token = read_token(&k2so_dir.join("daemon.token"))?;
        let base = format!("http://127.0.0.1:{port}");
        Ok(Self { base, token, http })
    }

    /// Hit `GET /ping` — no auth required. Returns `true` iff the daemon
    /// is reachable and replied with a 2xx. Intended for the Tauri app's
    /// post-launchd-load handshake.
    pub fn ping(&self) -> bool {
        let url = format!("{}/ping", self.base);
        self.http
            .get(url)
            .send()
            .map(|r| r.status().is_success())
            .unwrap_or(false)
    }

    /// Hit a `/cli/*` route on the daemon, returning the raw response
    /// body (the CLI text/JSON the daemon emits). `params` are
    /// percent-encoded into the query string alongside the auth
    /// token. Any non-2xx status is surfaced as Err with the body.
    pub fn cli_get(&self, path: &str, params: &[(&str, &str)]) -> Result<String, String> {
        let mut url = format!("{}{}?token={}", self.base, path, self.token);
        for (k, v) in params {
            url.push('&');
            url.push_str(&pct_encode(k));
            url.push('=');
            url.push_str(&pct_encode(v));
        }
        let response = self
            .http
            .get(&url)
            .send()
            .map_err(|e| format!("daemon {path}: {e}"))?;
        let status = response.status();
        let body = response.text().unwrap_or_default();
        if !status.is_success() {
            return Err(format!("daemon {path} {}: {body}", status.as_u16()));
        }
        Ok(body)
    }

    /// POST a JSON body to a `/cli/*` route. `body` is serialized as
    /// `application/json`; the auth token rides in the query string
    /// (same wire shape `cli_get` uses, so a single daemon-token
    /// rotation invalidates GET and POST identically). Returns the
    /// raw response body on 2xx; any non-2xx is surfaced as `Err`.
    pub fn cli_post_json(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<String, String> {
        let url = format!("{}{}?token={}", self.base, path, self.token);
        let response = self
            .http
            .post(&url)
            .header("Content-Type", "application/json")
            .body(body.to_string())
            .send()
            .map_err(|e| format!("daemon {path}: {e}"))?;
        let status = response.status();
        let resp_body = response.text().unwrap_or_default();
        if !status.is_success() {
            return Err(format!("daemon {path} {}: {resp_body}", status.as_u16()));
        }
        Ok(resp_body)
    }

    /// Convenience: GET a /cli/* route and decode the JSON body into `T`.
    /// Returns the same error shape as `cli_get` for HTTP failures and
    /// adds a serde decoding error on success-with-bad-body.
    pub fn cli_get_json<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        params: &[(&str, &str)],
    ) -> Result<T, String> {
        let body = self.cli_get(path, params)?;
        serde_json::from_str(&body).map_err(|e| format!("decode {path}: {e}: body={body}"))
    }

    /// Convenience: POST a JSON body and decode the JSON response into `T`.
    pub fn cli_post_json_decode<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<T, String> {
        let resp = self.cli_post_json(path, body)?;
        serde_json::from_str(&resp).map_err(|e| format!("decode {path}: {e}: body={resp}"))
    }

    /// Hit `GET /status?token=<t>` and decode the JSON body.
    pub fn status(&self) -> Result<DaemonStatus, String> {
        let url = format!("{}/status?token={}", self.base, self.token);
        let response = self
            .http
            .get(url)
            .send()
            .map_err(|e| format!("daemon /status: {e}"))?;
        let status = response.status();
        let body = response.text().unwrap_or_default();
        if !status.is_success() {
            return Err(format!("daemon /status {}: {}", status.as_u16(), body));
        }
        serde_json::from_str(&body).map_err(|e| format!("decode /status: {e}: body={body}"))
    }
}

/// Percent-encode a query-string component without pulling a new
/// crate. RFC 3986 unreserved set is letters, digits, `-`, `_`, `.`,
/// `~`. Everything else gets `%HH`-encoded.
fn pct_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

fn k2so_dir() -> Result<PathBuf, String> {
    Ok(dirs::home_dir()
        .ok_or_else(|| "home dir unavailable".to_string())?
        .join(".k2so"))
}

fn read_port(path: &PathBuf) -> Result<u16, String> {
    let raw = fs::read_to_string(path)
        .map_err(|e| format!("read {}: {e}", path.display()))?;
    raw.trim()
        .parse::<u16>()
        .map_err(|e| format!("parse port from {}: {e}", path.display()))
}

fn read_token(path: &PathBuf) -> Result<String, String> {
    let raw = fs::read_to_string(path)
        .map_err(|e| format!("read {}: {e}", path.display()))?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(format!("empty token file at {}", path.display()));
    }
    Ok(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    // The meaty integration test ('boot the daemon, connect, ping,
    // query /status') lives in a manual script rather than here because
    // spawning launchd'd processes from cargo test is fiddly on macOS —
    // the daemon's auth-token file is a per-boot singleton at
    // ~/.k2so/daemon.token, so a test can't pave over a real running
    // daemon without tripping the user's own workflow. Smoke-test
    // workflow is in scripts/perf-load-harness.sh neighborhood.
    //
    // What we CAN test here: the port/token readers and the status
    // deserializer — pure file-I/O + serde.

    #[test]
    fn status_decodes_from_canonical_daemon_body() {
        // Matches the format emitted by k2so-daemon/src/main.rs.
        let json = r#"{"version":"0.33.0-dev","uptime_secs":42,"pid":12345,"port":58211}"#;
        let s: DaemonStatus = serde_json::from_str(json).expect("decode");
        assert_eq!(s.version, "0.33.0-dev");
        assert_eq!(s.uptime_secs, 42);
        assert_eq!(s.pid, 12345);
        assert_eq!(s.port, 58211);
    }

    #[test]
    fn read_port_trims_whitespace() {
        let tmp = std::env::temp_dir().join(format!(
            "k2so-daemon-client-port-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&tmp, "  58211\n").expect("write");
        let p = read_port(&tmp).expect("read");
        assert_eq!(p, 58211);
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn read_port_rejects_malformed() {
        let tmp = std::env::temp_dir().join(format!(
            "k2so-daemon-client-port-bad-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&tmp, "not-a-port").expect("write");
        assert!(read_port(&tmp).is_err());
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn read_token_rejects_empty_file() {
        let tmp = std::env::temp_dir().join(format!(
            "k2so-daemon-client-token-empty-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&tmp, "   \n").expect("write");
        assert!(read_token(&tmp).is_err());
        std::fs::remove_file(&tmp).ok();
    }

    // ── Active-daemon override (K2 Connect host-aware routing) ──────────
    //
    // These mutate the process-global `ACTIVE` cell, so they share state.
    // We keep them in ONE test (sequential, no parallel interleaving) and
    // restore the override to `None` at the end so a `try_connect()`
    // elsewhere in the suite isn't pinned at a stale remote.

    #[test]
    fn active_daemon_override_local_vs_remote() {
        // Default: no override installed → local.
        set_active_daemon(None, None);
        assert!(active_daemon().is_none(), "default override must be None (local)");

        // Installing a remote pins base + token; a trailing slash is
        // trimmed so URL composition is a plain `{base}{path}`.
        set_active_daemon(
            Some("https://reggie.k2.dev/".to_string()),
            Some("sess-tok-123".to_string()),
        );
        let remote = active_daemon().expect("remote override installed");
        assert_eq!(remote.base, "https://reggie.k2.dev");
        assert_eq!(remote.token, "sess-tok-123");

        // A remote DaemonClient builds URLs from the remote base + token,
        // keeping the `?token=` + `&k=v` shape identical to local.
        let http = reqwest::blocking::Client::builder()
            .build()
            .expect("client");
        let client = DaemonClient {
            base: remote.base.clone(),
            token: remote.token.clone(),
            http,
        };
        let url = format!("{}{}?token={}", client.base, "/cli/fs/read-dir", client.token);
        assert_eq!(url, "https://reggie.k2.dev/cli/fs/read-dir?token=sess-tok-123");

        // Missing token clears back to local (never installs a tokenless
        // remote — that would be the source of "Invalid or missing auth
        // token").
        set_active_daemon(Some("https://reggie.k2.dev".to_string()), None);
        assert!(active_daemon().is_none(), "missing token → local");

        // An empty base/token string also clears.
        set_active_daemon(Some(String::new()), Some("t".to_string()));
        assert!(active_daemon().is_none(), "empty base → local");
        set_active_daemon(Some("https://x".to_string()), Some(String::new()));
        assert!(active_daemon().is_none(), "empty token → local");

        // Restore default so the rest of the suite sees local.
        set_active_daemon(None, None);
        assert!(active_daemon().is_none());
    }
}

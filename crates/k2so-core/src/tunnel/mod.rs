//! K2 Connect tunnel connector (CLIENT / open side).
//!
//! This is the **open-core, MIT** half: the daemon-side machinery that
//! exposes the local K2SO daemon to the internet at
//! `https://<user>.k2.dev` by running an `frpc` client that dials the
//! hosted (proprietary) K2 Connect frps server.
//!
//! Pipeline: `frpc (this machine) → frps (Hetzner) → Caddy (*.k2.dev TLS)
//! → https://{user}.k2.dev`.
//!
//! The control plane authorizes each frpc Login by validating the K2SO
//! bearer token carried in the login metas, and *forces* the proxy
//! subdomain to the token's canonical `{user}` namespace. So the client
//! supplies `{ token, requested-subdomain, localPort }`; the server
//! canonicalizes. See the proprietary `k2-connect` repo for the server
//! contract.
//!
//! Modules:
//!   * [`config`]    — `~/.k2so/tunnel.json` (the secret token lives here).
//!   * [`render`]    — frpc v0.61 TOML renderer.
//!   * [`connector`] — spawn / supervise / stop the `frpc` child.
//!
//! Public facade ([`start_tunnel`] / [`stop_tunnel`] / [`tunnel_status`])
//! is what the daemon's `/cli/tunnel/*` routes call.

pub mod config;
pub mod connector;
pub mod render;

pub use config::TunnelConfig;
pub use connector::{FrpcBinary, TunnelStatus};

/// Start the tunnel using the stored config (auto-locating `frpc`).
///
/// * `subdomain` — optional override for the requested subdomain
///   (persisted to the config when present).
/// * `daemon_port` — the live daemon HTTP port to expose when the config
///   doesn't pin a `local_port`.
pub fn start_tunnel(
    subdomain: Option<String>,
    daemon_port: u16,
) -> Result<TunnelStatus, String> {
    connector::start(subdomain, daemon_port, &FrpcBinary::Auto)
}

/// Stop the tunnel (kills the supervised `frpc` child; no restart).
pub fn stop_tunnel() -> Result<(), String> {
    connector::stop()
}

/// Current tunnel status (running? + predicted public URL).
pub fn tunnel_status() -> TunnelStatus {
    connector::status()
}

/// Render the frpc TOML for the stored config + given local port, without
/// spawning anything. Handy for diagnostics / `--dry-run`.
pub fn render_config(local_port: u16) -> Result<String, String> {
    let cfg = config::load()?;
    Ok(render::render_frpc_toml(&cfg, local_port))
}

#[cfg(test)]
pub(crate) mod test_support {
    //! Shared test scaffolding. Tunnel tests touch `$HOME` (config +
    //! frpc.toml + log all live under `~/.k2so/`), so they must
    //! serialize and redirect HOME to a tempdir. We reuse the crate-wide
    //! `themes::HOME_LOCK` so we never race the other HOME-mutating test
    //! suites (app_settings, themes, companion).

    use crate::themes::HOME_LOCK;

    /// Run `f` with `$HOME` pointed at a fresh tempdir, under the global
    /// HOME lock, and clean up afterward. Also clears any prior tunnel
    /// connector singleton state so start/stop tests don't bleed.
    pub fn with_temp_home<F: FnOnce()>(f: F) {
        // parking_lot::Mutex — `lock()` returns the guard directly.
        let _g = HOME_LOCK.lock();
        let prev = std::env::var_os("HOME");
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let tmp = std::env::temp_dir().join(format!("k2so-tunnel-test-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&tmp).expect("create temp HOME");
        std::env::set_var("HOME", &tmp);

        // Ensure a clean connector singleton for this test.
        let _ = super::connector::stop();

        f();

        // Best-effort connector teardown + HOME restore.
        let _ = super::connector::stop();
        match prev {
            Some(p) => std::env::set_var("HOME", p),
            None => std::env::remove_var("HOME"),
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }
}

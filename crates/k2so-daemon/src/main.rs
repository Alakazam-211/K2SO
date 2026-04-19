//! K2SO daemon entry point.
//!
//! Launched by launchd (`~/Library/LaunchAgents/com.k2so.k2so-daemon.plist`,
//! `KeepAlive: true`), this process owns the persistent-agent runtime —
//! SQLite, the heartbeat scheduler, the companion WebSocket + ngrok tunnel,
//! the agent_hooks HTTP server — so that agents keep running while the Tauri
//! app is quit and the laptop lid is closed.
//!
//! Implementation migrates in incrementally behind this entry point; this
//! file is the placeholder for the workspace scaffolding commit.

fn main() {
    // Force the linker to keep k2so-core so `cargo build --workspace`
    // exercises the crate boundary end-to-end from the scaffolding pass
    // onward.
    k2so_core::__scaffolding_marker();

    eprintln!("k2so-daemon 0.33.0-dev: scaffolding only. Not yet functional.");
    std::process::exit(0);
}

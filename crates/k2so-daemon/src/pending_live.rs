//! Pending-live delivery durability.
//!
//! F3 of Phase 3.1. When a `Delivery::Live` signal targets an
//! offline agent, the bus needs to wake the target and inject on
//! session-ready. Without durability, a daemon crash between wake
//! and session-boot would drop the signal. This module persists
//! the signal to `~/.k2so/daemon.pending-live/<agent>/<ts>-<uuid>.json`
//! so it survives restart.
//!
//! The flow:
//!
//!   1. `DaemonWakeProvider::wake` gets called for offline target.
//!      It persists the signal here.
//!   2. (Deferred to a real scheduler-wake primitive: fire the
//!      actual wake. MVP: no wake fires; we rely on the agent's
//!      session being spawned by some other path.)
//!   3. When a session is spawned for the target agent (via
//!      `/cli/sessions/spawn`), `drain_for_agent(name)` reads all
//!      queued signals for that agent, deletes each file, and
//!      returns the signals so the spawn path can inject them.
//!   4. At daemon boot, `replay_all()` scans the queue. Queued
//!      entries stay on disk until either a session spawns for
//!      that agent (drain + inject) or they expire (Phase 3.2 adds
//!      age-based pruning).
//!
//! **Root path.** `~/.k2so/daemon.pending-live/<agent>/`. Lives
//! under the daemon's own `~/.k2so/` — this is the daemon's own
//! queue state, not project-scoped. One daemon, one queue root.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use k2so_core::awareness::AgentSignal;
use k2so_core::fs_atomic::atomic_write;
use k2so_core::log_debug;

/// Resolve the queue root. Daemon's `~/.k2so/daemon.pending-live/`.
/// Tests override via env var; production uses `dirs::home_dir()`.
pub fn queue_root() -> PathBuf {
    if let Ok(override_path) = std::env::var("K2SO_PENDING_LIVE_ROOT") {
        return PathBuf::from(override_path);
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".k2so/daemon.pending-live")
}

/// Persist a signal for eventual inject. Filename uses
/// ns-timestamp + signal uuid so sorted-lex = sorted-by-time and
/// concurrent writers don't collide. Path-traversal-safe: agent
/// name with `/` or `..` is neutralized.
pub fn enqueue(signal: &AgentSignal, target_agent: &str) -> io::Result<PathBuf> {
    let root = queue_root();
    let safe_agent = sanitize(target_agent);
    let dir = root.join(&safe_agent);
    let ns = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let path = dir.join(format!("{ns:020}-{}.json", signal.id));
    let json = serde_json::to_vec_pretty(signal).map_err(|e| {
        io::Error::new(io::ErrorKind::InvalidData, format!("serialize: {e}"))
    })?;
    atomic_write(&path, &json)?;
    log_debug!(
        "[daemon/pending-live] queued signal id={} for agent={} at {:?}",
        signal.id,
        target_agent,
        path
    );
    Ok(path)
}

/// Drain every queued signal for `agent`, deleting each file
/// after successful parse. Used by the spawn path to flush
/// anything that was queued while the agent was offline.
pub fn drain_for_agent(agent: &str) -> Vec<AgentSignal> {
    let root = queue_root();
    let dir = root.join(sanitize(agent));
    drain_directory(&dir)
}

/// Boot-time replay — scan every agent directory under the queue
/// root. Called by the daemon at startup BEFORE the accept loop
/// begins. Returns a `Vec<(agent_name, Vec<AgentSignal>)>` so
/// callers can decide what to do (daemon-at-boot: log + drain as
/// sessions come online; tests: just inspect the contents).
pub fn replay_all() -> Vec<(String, Vec<AgentSignal>)> {
    let root = queue_root();
    let entries = match fs::read_dir(&root) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    for entry in entries.filter_map(|r| r.ok()) {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let agent = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        let signals = drain_directory(&path);
        if !signals.is_empty() {
            out.push((agent, signals));
        }
    }
    out
}

fn drain_directory(dir: &Path) -> Vec<AgentSignal> {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(|r| r.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
        .collect();
    files.sort();
    let mut out = Vec::with_capacity(files.len());
    for p in files {
        match fs::read(&p).and_then(|b| {
            serde_json::from_slice::<AgentSignal>(&b).map_err(|e| {
                io::Error::new(io::ErrorKind::InvalidData, e.to_string())
            })
        }) {
            Ok(signal) => {
                if let Err(e) = fs::remove_file(&p) {
                    log_debug!(
                        "[daemon/pending-live] remove {:?} failed: {}",
                        p,
                        e
                    );
                }
                out.push(signal);
            }
            Err(e) => {
                log_debug!(
                    "[daemon/pending-live] parse {:?} failed: {} — leaving in place",
                    p,
                    e
                );
            }
        }
    }
    out
}

fn sanitize(name: &str) -> String {
    if name.is_empty() || name == "." || name == ".." {
        return "_invalid".to_string();
    }
    let clean: String = name
        .chars()
        .map(|c| match c {
            c if c.is_control() => '_',
            '/' | '\\' | ':' | '\0' => '_',
            _ => c,
        })
        .collect();
    if clean.starts_with('.') {
        format!("_{}", clean)
    } else {
        clean
    }
}

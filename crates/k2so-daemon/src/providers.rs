//! Daemon implementations of the k2so-core awareness provider traits.
//!
//! F1 of Phase 3.1. These are the real `InjectProvider` +
//! `WakeProvider` impls that turn `k2so signal`-style egress from
//! "writes to bus + activity_feed" into "writes to target's live
//! PTY" — the last mile that makes real-time peer-to-peer
//! collaboration actually work.
//!
//! Registered at daemon startup via `register_all()`.

use k2so_core::awareness::{AgentSignal, InjectProvider, WakeProvider};
use k2so_core::log_debug;

use crate::session_map;

/// Looks up the target agent's session handle in `session_map` and
/// writes the rendered signal bytes into its PTY. If no session is
/// registered for the target agent, returns `NotFound` — the
/// egress path sees this as "inject failed" and reports it in the
/// `DeliveryReport`; the signal still lands in activity_feed and
/// the bus, so nothing is silently lost.
pub struct DaemonInjectProvider;

impl InjectProvider for DaemonInjectProvider {
    fn inject(&self, agent: &str, bytes: &[u8]) -> std::io::Result<()> {
        let session = session_map::lookup(agent).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("no live session for agent {agent}"),
            )
        })?;
        session.write(bytes)
    }
}

// NB: the formatter that turns an AgentSignal into bytes-for-PTY
// lives at the crate root (`crate::inject_bytes_for_signal`) —
// both this provider (called by egress::try_inject via k2so-core's
// render function) and the spawn-path drain loop use it. Since
// egress::try_inject formats at its own site and we don't see
// signal context here (only bytes), the provider impl stays
// dead-simple: look up, write.

/// Phase 3.1 MVP wake provider — logs the wake request but doesn't
/// yet trigger a real scheduler wake. The real scheduler-wake
/// primitive ("agent is offline, launch its session") is deferred
/// to follow-up work; the F3 pending-live-delivery queue holds
/// the signal until the session comes online on its own (next
/// user-triggered spawn, next heartbeat, etc.) and injects on
/// that path.
///
/// This preserves the PRD's "Live to offline target = wake + inject"
/// invariant in principle — the inject part gets queued for when
/// the target is next live. The "wake" part is the TODO.
pub struct DaemonWakeProvider;

impl WakeProvider for DaemonWakeProvider {
    fn wake(&self, agent: &str, signal: &AgentSignal) -> std::io::Result<()> {
        // F3 — persist the signal to the pending-live queue so it
        // survives daemon restart. When a session for `agent`
        // spawns (via /cli/sessions/spawn), the spawn path drains
        // the queue and injects each signal in order.
        match crate::pending_live::enqueue(signal, agent) {
            Ok(path) => {
                log_debug!(
                    "[daemon/wake] queued signal id={} for {agent} at {:?}",
                    signal.id,
                    path
                );
                Ok(())
            }
            Err(e) => {
                log_debug!(
                    "[daemon/wake] failed to queue signal for {agent}: {e}"
                );
                Err(e)
            }
        }
    }
}

/// Register both providers on the k2so-core ambient singletons.
/// Called once at daemon startup before the accept loop.
pub fn register_all() {
    k2so_core::awareness::set_inject_provider(Box::new(DaemonInjectProvider));
    k2so_core::awareness::set_wake_provider(Box::new(DaemonWakeProvider));
    log_debug!(
        "[daemon/providers] registered DaemonInjectProvider + DaemonWakeProvider"
    );
}

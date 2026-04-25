//! Daemon implementations of the k2so-core awareness provider traits.
//!
//! F1 of Phase 3.1 — `DaemonInjectProvider` + `DaemonWakeProvider` —
//! plus G4 of Phase 3.2, which teaches `DaemonWakeProvider` to
//! ACTUALLY LAUNCH the target agent's session (via the shared
//! `spawn::spawn_agent_session` helper) when an `AGENT.md` launch
//! profile exists for it. Before G4, wake just enqueued to
//! pending-live and waited for a user-triggered spawn; G4 closes
//! that gap so offline agents get auto-spawned on demand.
//!
//! Registered at daemon startup via `register_all()`.

use k2so_core::agents::launch_profile::{load_launch_profile, resolve_cwd, LaunchProfile};
use k2so_core::awareness::{AgentAddress, AgentSignal, InjectProvider, WakeProvider};
use k2so_core::log_debug;

use crate::session_lookup;
use crate::spawn::{spawn_agent_session_v2_blocking, SpawnAgentSessionRequest};

/// Looks up the target agent's session handle across BOTH the
/// legacy `session_map` (Kessel-T0) and `v2_session_map`
/// (Alacritty_v2) and writes the rendered signal bytes into its
/// PTY. If no session is registered for the target agent in
/// either map, returns `NotFound` — the egress path sees this as
/// "inject failed" and reports it in the `DeliveryReport`; the
/// signal still lands in activity_feed and the bus, so nothing is
/// silently lost.
pub struct DaemonInjectProvider;

impl InjectProvider for DaemonInjectProvider {
    fn inject(&self, agent: &str, bytes: &[u8]) -> std::io::Result<()> {
        let session = session_lookup::lookup_any(agent).ok_or_else(|| {
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

/// Phase 3.2 G4 wake provider. Two-stage delivery:
///
/// 1. **Durability first:** enqueue the signal to the F3 pending-
///    live queue on disk so it survives daemon restart, OS crash,
///    and spawn failure. Queueing is the unconditional baseline —
///    even if auto-launch fails (no profile, bad spawn config),
///    the signal stays on disk and the next user-triggered spawn
///    will drain it.
///
/// 2. **Auto-launch (G4):** if the target agent has a
///    `<project>/.k2so/agents/<agent>/AGENT.md` launch profile
///    (G3's `launch_profile::load_launch_profile`), spawn a fresh
///    session using that profile. The spawn path then drains the
///    pending-live queue in order, and the just-enqueued signal
///    becomes the session's first byte of input.
///
/// Race-safe single-flight: before spawning, re-check
/// `session_map::lookup(agent)`. If a concurrent path (another
/// signal's wake, a `/cli/sessions/spawn` arriving mid-flight)
/// already registered the session, we skip — the queued signal
/// will be drained by the now-live session path naturally.
///
/// The `from.workspace` field on the signal supplies the project
/// id used to locate `AGENT.md`. After G0's CLI resolver, this
/// is a UUID matching `projects.id`; we look it up to get the
/// filesystem path. Signals with a stale or unknown workspace id
/// fall back to queue-only delivery with a log line.
pub struct DaemonWakeProvider;

impl WakeProvider for DaemonWakeProvider {
    fn wake(&self, agent: &str, signal: &AgentSignal) -> std::io::Result<()> {
        // Stage 1: always enqueue so durability is preserved
        // regardless of what the auto-launch path does next.
        match crate::pending_live::enqueue(signal, agent) {
            Ok(path) => {
                log_debug!(
                    "[daemon/wake] queued signal id={} for {agent} at {:?}",
                    signal.id,
                    path
                );
            }
            Err(e) => {
                log_debug!(
                    "[daemon/wake] failed to queue signal for {agent}: {e}"
                );
                return Err(e);
            }
        }

        // Stage 2: G4 auto-launch. Best-effort — any failure here
        // (profile absent, project path unknown, spawn error) leaves
        // the signal queued for a future user-triggered spawn.
        if let Err(reason) = try_auto_launch(agent, signal) {
            log_debug!(
                "[daemon/wake] auto-launch skipped for {agent}: {reason} \
                 (signal stays in queue)"
            );
        }
        Ok(())
    }
}

/// Inner auto-launch path. Lifted out of the trait impl so the
/// "couldn't launch, here's why" branch is explicit and
/// testable.
///
/// Returns `Ok(())` when the spawn succeeded (session_map now has
/// an entry for the agent; the pending queue has been drained).
/// `Err(reason)` covers every skipped-auto-launch path, each with
/// a short reason the caller logs:
///   - agent already has a live session (someone else spawned)
///   - no AGENT.md launch profile on disk
///   - workspace id doesn't map to a known project path
///   - spawn itself failed (bad command, PTY exhaustion, etc.)
fn try_auto_launch(agent: &str, signal: &AgentSignal) -> Result<(), String> {
    // Single-flight: if a concurrent path already registered the
    // session in EITHER map, skip. The pending-live drain on that
    // session will pick up our enqueued signal. Pre-A9 this only
    // checked the legacy map and would re-spawn against live v2
    // sessions, producing a duplicate child for the same agent.
    if session_lookup::lookup_any(agent).is_some() {
        return Err("session already live".into());
    }

    // Resolve the signal's workspace id → projects.id → projects.path.
    // Without a real path we can't locate AGENT.md.
    let workspace_id = match &signal.from {
        AgentAddress::Agent { workspace, .. }
        | AgentAddress::Workspace { workspace } => workspace.0.as_str(),
        AgentAddress::Broadcast => {
            return Err("broadcast signal has no attributable workspace".into());
        }
        // `AgentAddress` is `#[non_exhaustive]`; any future variant
        // falls through to queue-only — the safe default when we
        // don't know how to derive a project id.
        _ => return Err("unhandled AgentAddress variant for from".into()),
    };
    let project_path = lookup_project_path(workspace_id)
        .ok_or_else(|| format!("no project registered for workspace id {workspace_id:?}"))?;

    // G3 launch profile lookup. Absent profile = stay in queue-only
    // mode. Malformed profile = log + skip (bad YAML shouldn't kill
    // the wake path).
    let profile = match load_launch_profile(&project_path, agent) {
        Ok(Some(p)) => p,
        Ok(None) => return Err("no AGENT.md launch profile".into()),
        Err(e) => return Err(format!("launch profile parse failed: {e}")),
    };

    // Spawn. Shared helper tags agent_name on the SessionEntry,
    // registers in session_map, and drains the pending-live queue.
    // The signal we just enqueued is part of that drain and becomes
    // the target's first byte of input.
    let req = launch_request_for(agent, &project_path, &profile);
    // Heartbeat-driven headless wake produces v2 sessions per A9.
    // The Tauri-open path goes through `BackgroundTerminalSpawner`
    // → v2_spawn (also v2 since A8). Both wake paths now converge.
    let outcome = spawn_agent_session_v2_blocking(req)
        .map_err(|e| format!("spawn failed: {e}"))?;

    log_debug!(
        "[daemon/wake] auto-launched session={} agent={agent} pending_drained={} \
         via AGENT.md launch profile",
        outcome.session_id,
        outcome.pending_drained,
    );
    Ok(())
}

/// Lookup `projects.path` by `projects.id`. Returns `None` if the
/// workspace id doesn't match any registered project (signals from
/// an unregistered workspace fall through to queue-only delivery).
fn lookup_project_path(workspace_id: &str) -> Option<String> {
    let db = k2so_core::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT path FROM projects WHERE id = ?1",
        rusqlite::params![workspace_id],
        |row| row.get::<_, String>(0),
    )
    .ok()
}

/// Turn a `LaunchProfile` + project root into a `SpawnAgentSessionRequest`.
/// Applies defaults for every field the profile leaves unset
/// (matching `POST /cli/sessions/spawn`'s defaults for consistency
/// with the explicit-spawn path).
fn launch_request_for(
    agent: &str,
    project_path: &str,
    profile: &LaunchProfile,
) -> SpawnAgentSessionRequest {
    let project_root = std::path::Path::new(project_path);
    let cwd = resolve_cwd(project_root, profile.cwd.as_deref())
        .to_string_lossy()
        .into_owned();

    SpawnAgentSessionRequest {
        agent_name: agent.to_string(),
        cwd,
        command: profile.command.clone(),
        args: profile.args.clone(),
        cols: profile.cols.unwrap_or(80),
        rows: profile.rows.unwrap_or(24),
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

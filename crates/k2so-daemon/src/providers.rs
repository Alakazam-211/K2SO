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
        // Two-phase write — same pattern as
        // `heartbeat_launch::run_inject`. Write the body, settle for
        // 150ms so the TUI input widget commits each character, then
        // write `\r` as a separate syscall so it's interpreted as
        // Enter rather than the tail of a multi-line paste. A
        // combined `body+\r` single write lands typed-but-not-sent
        // because raw-mode input widgets treat fast-arriving bytes
        // as paste content (claude code, codex, gemini all do this).
        session.write(bytes)?;
        std::thread::sleep(std::time::Duration::from_millis(150));
        session.write(b"\r")
    }

    /// Daemon-side liveness probe. Walks both legacy `session_map`
    /// and `v2_session_map` via `session_lookup`. Without this
    /// override, `egress::is_agent_live` would only see legacy
    /// sessions and route every Live signal targeting a v2-only
    /// agent through the wake provider — bypassing the inject
    /// path entirely.
    fn is_live(&self, agent: &str) -> bool {
        session_lookup::lookup_any(agent).is_some()
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
    // Resolve the target workspace id from `signal.to` first
    // (preferred — it's the workspace this signal is FOR), falling
    // back to `signal.from` for legacy callers that don't set
    // `to.workspace` correctly. The pre-0.36.15 code only used
    // `signal.from`, which works for same-workspace messaging
    // (k2so msg from inside a workspace) but is wrong for
    // cross-workspace addressing.
    let workspace_id = match &signal.to {
        AgentAddress::Agent { workspace, .. } => Some(workspace.0.as_str()),
        AgentAddress::Workspace { workspace } => Some(workspace.0.as_str()),
        _ => None,
    }
    .or_else(|| match &signal.from {
        AgentAddress::Agent { workspace, .. }
        | AgentAddress::Workspace { workspace } => Some(workspace.0.as_str()),
        _ => None,
    })
    .ok_or_else(|| "broadcast signal has no attributable workspace".to_string())?;

    // Single-flight: if a concurrent path already registered THIS
    // workspace's session, skip. The pending-live drain on that
    // session will pick up our enqueued signal. Pre-0.36.15 this
    // checked the BARE name only — a stale issue when multiple
    // workspaces share an agent name (lookup returned someone
    // else's session and we incorrectly skipped spawning ours).
    let prefixed_key = format!("{workspace_id}:{agent}");
    if session_lookup::lookup_any(&prefixed_key).is_some() {
        return Err("session already live for this workspace".into());
    }

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

    // Spawn under the project-namespaced key so the next
    // workspace-scoped inject (egress::try_inject's prefixed lookup)
    // finds it. v2_session_map::register also mirrors to the bare
    // name for legacy bare-keyed callers (heartbeat-surfaced
    // sessions, k2so msg without a workspace context).
    let req = launch_request_for(&prefixed_key, &project_path, &profile);
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

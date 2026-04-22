//! Shared session-spawn helper used by every code path that
//! launches a `SessionStreamSession` under daemon control.
//!
//! Before G4, spawn lived inline in `awareness_ws::handle_sessions_spawn`.
//! G4's real scheduler-wake needed to call the SAME spawn flow from
//! `DaemonWakeProvider`, so this module extracts it. Both call sites
//! now delegate here; behavior is identical regardless of whether
//! the spawn was triggered by:
//!
//! - `POST /cli/sessions/spawn` — explicit user request
//! - `DaemonWakeProvider::wake` — auto-launch when a signal arrives
//!   for an offline agent that has an `AGENT.md` launch profile
//!
//! Every spawn does the same six things:
//!
//! 1. Allocate a fresh `SessionId`.
//! 2. Call `spawn_session_stream` (PTY open + child spawn +
//!    dual-emit reader + archive writer).
//! 3. Tag the `SessionEntry` with `agent_name` so liveness lookups
//!    (roster, `egress::is_agent_live`) find it.
//! 4. Register in the daemon's `session_map` so
//!    `DaemonInjectProvider` can reach the PTY by agent name.
//! 5. Drain any pending-live signals for this agent (F3 durability)
//!    — queued signals become the session's first input.
//! 6. Return the session id + drain count so the caller can
//!    report it (HTTP response, log line, activity_feed row).
//!
//! **Single-flight is the caller's problem.** If two callers both
//! observe `session_map::lookup(agent)` returning None and both
//! call `spawn_agent_session`, both will succeed — the second
//! overwrites the first's entry in `session_map`. Upstream
//! registrars should `session_map::lookup` first and skip the
//! spawn if an entry already exists.

use std::sync::Arc;

use k2so_core::log_debug;
use k2so_core::session::{registry, SessionId};
use k2so_core::terminal::{spawn_session_stream, SessionStreamSession, SpawnConfig};

use crate::pending_live;
use crate::session_map;
use crate::signal_format;

/// Input shape for a spawn. Shared by the HTTP handler
/// (`handle_sessions_spawn`) and the scheduler-wake path
/// (`DaemonWakeProvider`). Every field mirrors the `SpawnConfig`
/// portable-pty layer expects, with defaults for fields callers
/// might not care about.
#[derive(Debug, Clone)]
pub struct SpawnAgentSessionRequest {
    pub agent_name: String,
    pub cwd: String,
    pub command: Option<String>,
    pub args: Option<Vec<String>>,
    pub cols: u16,
    pub rows: u16,
}

/// Output shape. The HTTP handler serializes this as JSON; the
/// scheduler-wake path logs it. No `Debug` derive because
/// `SessionStreamSession` holds non-Debug portable-pty handles;
/// downstream callers that need diagnostic output format the
/// individual fields explicitly.
#[derive(Clone)]
pub struct SpawnAgentSessionOutcome {
    pub session_id: SessionId,
    pub agent_name: String,
    pub pending_drained: usize,
    pub session: Arc<SessionStreamSession>,
}

/// Execute the shared spawn flow. Returns the new session's id +
/// how many pending-live signals were drained into it, plus the
/// owning Arc (so callers can hold it alive past this function if
/// needed — `session_map` also holds a clone).
pub fn spawn_agent_session(
    req: SpawnAgentSessionRequest,
) -> Result<SpawnAgentSessionOutcome, String> {
    if req.agent_name.is_empty() {
        return Err("agent_name required".into());
    }

    let session_id = SessionId::new();
    let cfg = SpawnConfig {
        session_id,
        cwd: req.cwd.clone(),
        command: req.command.clone(),
        args: req.args.clone(),
        cols: req.cols,
        rows: req.rows,
        // Production: Kessel renders from Frames, doesn't use the
        // alacritty Term grid. Skipping the Term dual-parse halves
        // the reader thread's CPU cost per chunk, letting the PTY
        // drain at full speed.
        track_alacritty_term: false,
    };

    let session = spawn_session_stream(cfg)?;

    // Tag the SessionEntry so liveness lookups (roster,
    // egress::is_agent_live) find this session under its agent name.
    if let Some(entry) = registry::lookup(&session_id) {
        entry.set_agent_name(&req.agent_name);
    }

    // Register in session_map BEFORE draining pending signals so a
    // concurrent inject finds the session immediately on spawn, not
    // after the drain completes.
    let arc = Arc::new(session);
    session_map::register(&req.agent_name, arc.clone());

    // F3 drain: inject any pending-live signals queued while this
    // agent was offline. Rendered with the same formatter live
    // inject uses so the target sees identical bytes regardless of
    // whether the sender had to wait for the wake-and-inject path.
    let pending = pending_live::drain_for_agent(&req.agent_name);
    let pending_count = pending.len();
    for signal in pending {
        let bytes = signal_format::inject_bytes(&signal);
        if let Err(e) = arc.write(bytes.as_bytes()) {
            log_debug!(
                "[daemon/spawn] drain-inject for {} signal id={} failed: {e}",
                req.agent_name,
                signal.id
            );
        }
    }

    Ok(SpawnAgentSessionOutcome {
        session_id,
        agent_name: req.agent_name,
        pending_drained: pending_count,
        session: arc,
    })
}

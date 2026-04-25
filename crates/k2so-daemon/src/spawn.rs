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

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use k2so_core::log_debug;
use k2so_core::session::{registry, SessionId};
use k2so_core::terminal::{
    spawn_session_stream_and_grow, DaemonPtyConfig, DaemonPtySession, SpawnConfig,
};

use crate::pending_live;
use crate::session_map;
use crate::signal_format;
use crate::v2_session_map;

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

/// Output shape returned by both legacy and v2 spawn variants.
/// Renderer-agnostic on purpose — the caller only needs the
/// session id (which is unique across both maps), the agent name
/// it ended up registered as, and how many pending-live signals
/// were drained on boot. The owning `Arc` lives in whichever map
/// (`session_map` for legacy, `v2_session_map` for v2) registered
/// the session.
#[derive(Clone, Debug)]
pub struct SpawnAgentSessionOutcome {
    pub session_id: SessionId,
    pub agent_name: String,
    pub pending_drained: usize,
}

/// Execute the shared spawn flow. Returns the new session's id +
/// how many pending-live signals were drained into it, plus the
/// owning Arc (so callers can hold it alive past this function if
/// needed — `session_map` also holds a clone).
///
/// Async because the grow-then-shrink orchestration
/// (`spawn_session_stream_and_grow`) awaits the settle watcher
/// before returning. The HTTP response doesn't go out until the
/// session's replay ring is seeded with the full initial paint and
/// the PTY has been SIGWINCHed down to the user's real rows.
pub async fn spawn_agent_session(
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

    let session = spawn_session_stream_and_grow(cfg).await?;

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
    })
}

/// Alacritty_v2 spawn helper. The architectural counterpart to
/// `spawn_agent_session`: takes the same `SpawnAgentSessionRequest`
/// and returns the same `SpawnAgentSessionOutcome`, but the session
/// it produces is a `DaemonPtySession` (registered in
/// `v2_session_map`) instead of a `SessionStreamSession` (legacy
/// `session_map`). End-state target after A9: every system-driven
/// spawn flows through this. Only the explicit Kessel-T0 endpoint
/// (`POST /cli/sessions/spawn` → `awareness_ws::handle_sessions_spawn`)
/// stays on the legacy variant.
///
/// Synchronous: alacritty's IO thread is started in the background
/// by `DaemonPtySession::spawn`; this fn returns as soon as the PTY
/// is open and the Term + event loop are wired. No grow-then-shrink
/// dance is needed (v2 doesn't have the replay-ring seeding the
/// legacy path requires).
pub fn spawn_agent_session_v2_blocking(
    req: SpawnAgentSessionRequest,
) -> Result<SpawnAgentSessionOutcome, String> {
    if req.agent_name.is_empty() {
        return Err("agent_name required".into());
    }

    // Convert the request shape into DaemonPtyConfig. v2 takes its
    // working directory as `Option<PathBuf>` rather than a String,
    // and stores `program: Option<String>` (vs legacy `command`),
    // but the wire shape is otherwise identical.
    let cfg = DaemonPtyConfig {
        session_id: SessionId::new(),
        cols: req.cols,
        rows: req.rows,
        cwd: Some(PathBuf::from(&req.cwd)),
        program: req.command.clone(),
        args: req.args.clone().unwrap_or_default(),
        env: HashMap::new(),
        drain_on_exit: true,
    };
    let session_id = cfg.session_id;

    let session = DaemonPtySession::spawn(cfg)
        .map_err(|e| format!("v2 spawn failed: {e}"))?;
    v2_session_map::register(req.agent_name.clone(), session.clone());

    // Drain any pending-live signals queued for this agent so they
    // become input to the fresh session — same contract as the
    // legacy spawn drain. Without this, signals enqueued by
    // `DaemonWakeProvider::wake` while the agent was offline get
    // silently dropped on v2 boot.
    let pending = pending_live::drain_for_agent(&req.agent_name);
    let pending_drained = pending.len();
    for signal in pending {
        let bytes = signal_format::inject_bytes(&signal);
        session.write(bytes.into_bytes());
    }

    log_debug!(
        "[daemon/spawn] v2 session={} agent={} pending_drained={}",
        session_id,
        req.agent_name,
        pending_drained,
    );

    Ok(SpawnAgentSessionOutcome {
        session_id,
        agent_name: req.agent_name,
        pending_drained,
    })
}

/// Synchronous wrapper around the async `spawn_agent_session`, for
/// callers that live inside the daemon's sync `/cli/*` dispatch
/// path. Uses `tokio::task::block_in_place` to `block_on` the
/// async spawn without deadlocking the multi-thread tokio runtime.
///
/// Only safe inside a multi-thread tokio runtime — the daemon main
/// is `#[tokio::main(flavor = "multi_thread")]`, which is the only
/// production call site. Unit tests that want sync semantics should
/// use a `#[tokio::test(flavor = "multi_thread")]` runtime.
///
/// **Currently unused** — A9 migrated all sync callers to
/// `spawn_agent_session_v2_blocking`. Kept available for future
/// callers that need the legacy spawn from a sync context (none in
/// the production daemon today). `awareness_ws::handle_sessions_spawn`
/// is the only direct legacy spawn site and is already async, so it
/// calls the async `spawn_agent_session` directly.
#[allow(dead_code)]
pub fn spawn_agent_session_blocking(
    req: SpawnAgentSessionRequest,
) -> Result<SpawnAgentSessionOutcome, String> {
    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(spawn_agent_session(req))
    })
}

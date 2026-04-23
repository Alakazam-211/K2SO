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
use k2so_core::terminal::{
    spawn_session_stream_and_grow, GrowHandle, SessionStreamSession, SpawnConfig,
};

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
/// `SessionStreamSession` holds non-Debug portable-pty handles.
///
/// **Canvas Plan Phase A semantic note:** `pending_drained` now
/// reflects the count of pending-live signals QUEUED for async
/// drain, not the count actually written to the PTY at return
/// time. Drain happens on a background task that waits for
/// grow-settle to complete before writing. Callers that need to
/// know when drain has actually landed (tests, diagnostic
/// tooling) should poll the pending-live queue directory or
/// watch the session byte stream for the injected signal bytes.
#[derive(Clone)]
pub struct SpawnAgentSessionOutcome {
    pub session_id: SessionId,
    pub agent_name: String,
    pub pending_drained: usize,
    pub session: Arc<SessionStreamSession>,
}

/// Execute the shared spawn flow. Returns the new session's id +
/// how many pending-live signals were queued for async drain, plus
/// the owning Arc.
///
/// **Phase A: returns in ~20 ms.** The PTY is open, the child
/// process is running, the session is registered in
/// `session::registry`. `session_map::register` happens right
/// before return, so `DaemonInjectProvider` can find the session
/// the instant any caller asks.
///
/// Grow-settle, APC boundary injection, SIGWINCH, and the
/// pending-live drain ALL run on background tokio tasks that live
/// past this function's return. Heartbeat-critical ordering
/// preserved:
///
/// 1. `session_map::register` synchronous (before return).
/// 2. Signals arriving via `DaemonInjectProvider` DURING grow find
///    the session and land via `session.write()` (writer mutex
///    serializes concurrent writes with the drain task).
/// 3. Pending-live drain waits for grow-settle to complete before
///    writing to the PTY — matches pre-Phase-A ordering so
///    harnesses see signals AFTER their cold-start paint, not
///    interleaved with it.
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
        track_alacritty_term: false,
    };

    // Phase A: spawn returns fast; `grow_handle` resolves when the
    // background grow-settle + SIGWINCH sequence finishes.
    let (session_arc, grow_handle) =
        spawn_session_stream_and_grow(cfg).await?;

    // Tag the SessionEntry so liveness lookups (roster,
    // egress::is_agent_live) find this session by agent name.
    if let Some(entry) = registry::lookup(&session_id) {
        entry.set_agent_name(&req.agent_name);
    }

    // Register in session_map BEFORE spawning the drain task so
    // any signal arriving from this moment forward finds a live
    // session. Order-preserving: if a concurrent egress-inject
    // lands in the microsecond between `register` and
    // `spawn(drain)`, its write queues on the writer mutex; the
    // drain task's writes queue behind it. Either ordering is
    // correct — signals land whole regardless.
    session_map::register(&req.agent_name, session_arc.clone());

    // Count queued signals now so we can report the number in the
    // outcome, but don't drain synchronously. Drain runs on a
    // background task that awaits grow-settle first — matches
    // pre-Phase-A ordering.
    let queued_count = pending_live::count_for_agent(&req.agent_name);

    spawn_pending_live_drain_task(
        req.agent_name.clone(),
        session_arc.clone(),
        grow_handle,
    );

    Ok(SpawnAgentSessionOutcome {
        session_id,
        agent_name: req.agent_name,
        pending_drained: queued_count,
        session: session_arc,
    })
}

/// Spawn the background task that drains the pending-live queue
/// into a newly-spawned session AFTER grow-settle has completed.
/// The task holds an Arc clone of the session so the write can
/// happen even if the caller drops their reference between now
/// and grow-completion.
///
/// Separated into its own function for readability — the spawn
/// path's main function should be readable as "register session,
/// kick off drain task, return."
fn spawn_pending_live_drain_task(
    agent_name: String,
    session: Arc<SessionStreamSession>,
    grow_handle: GrowHandle,
) {
    tokio::spawn(async move {
        // Wait for grow to finish before draining. Pre-Phase-A
        // ordering: the harness's cold-start paint should
        // complete first, then pending-live signals arrive as
        // "post-startup user input" rather than getting
        // interleaved into the initial-paint byte stream.
        let _ = grow_handle.wait().await;

        let pending = pending_live::drain_for_agent(&agent_name);
        if pending.is_empty() {
            return;
        }
        let count = pending.len();
        for signal in pending {
            let bytes = signal_format::inject_bytes(&signal);
            if let Err(e) = session.write(bytes.as_bytes()) {
                log_debug!(
                    "[daemon/spawn] post-grow drain for {} signal id={} \
                     failed: {e}",
                    agent_name,
                    signal.id
                );
            }
        }
        log_debug!(
            "[daemon/spawn] post-grow drain for {} completed: {} signals",
            agent_name,
            count
        );
    });
}

/// Synchronous wrapper around the async `spawn_agent_session`, for
/// callers that live inside the daemon's sync `/cli/*` dispatch
/// path (`terminal_routes`, `agents_routes`). Uses
/// `tokio::task::block_in_place` to `block_on` the async spawn
/// without deadlocking the multi-thread tokio runtime.
///
/// Only safe inside a multi-thread tokio runtime — the daemon main
/// is `#[tokio::main(flavor = "multi_thread")]`, which is the only
/// production call site. Unit tests that want sync semantics should
/// use a `#[tokio::test(flavor = "multi_thread")]` runtime.
pub fn spawn_agent_session_blocking(
    req: SpawnAgentSessionRequest,
) -> Result<SpawnAgentSessionOutcome, String> {
    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(spawn_agent_session(req))
    })
}

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
/// (`DaemonWakeProvider`).
///
/// 0.37.0 canonicalization: every spawn carries `project_id` so the
/// v2 spawn helper can register under the unique `<project_id>:<agent>`
/// key. Pre-0.37.0, callers passed bare `agent_name` and registered
/// under the bare key — `agents launch` and `--wake`'s auto-launch
/// path then ended up writing to different slots in the same map
/// (bare vs prefixed) and accumulating duplicate sessions per
/// workspace. With `project_id` mandatory, both paths converge on
/// the same key and the canonical-session invariant holds.
///
/// `project_id` is `Option<String>` only for the legacy
/// Kessel-T0 spawn path (`POST /cli/sessions/spawn`), which doesn't
/// have workspace context by design — that path stays on bare-name
/// keying in the legacy `session_map`. v2 callers MUST set
/// `project_id` or the function returns an error.
#[derive(Debug, Clone)]
pub struct SpawnWorkspaceSessionRequest {
    pub agent_name: String,
    pub project_id: Option<String>,
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
///
/// `reused = true` when the v2 spawn helper found an existing
/// session under the canonical `<project_id>:<agent>` key and
/// returned it instead of spawning a duplicate (idempotent
/// `agents launch`). When `reused` is true, `pending_drained` is
/// 0 (the session was already alive; any pending-live signals
/// were drained on its original spawn).
#[derive(Clone, Debug)]
pub struct SpawnWorkspaceSessionOutcome {
    pub session_id: SessionId,
    pub agent_name: String,
    pub pending_drained: usize,
    pub reused: bool,
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
    req: SpawnWorkspaceSessionRequest,
) -> Result<SpawnWorkspaceSessionOutcome, String> {
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

    Ok(SpawnWorkspaceSessionOutcome {
        session_id,
        agent_name: req.agent_name,
        pending_drained: pending_count,
        reused: false,
    })
}

/// Alacritty_v2 spawn helper. The architectural counterpart to
/// `spawn_agent_session`: takes the same `SpawnWorkspaceSessionRequest`
/// and returns the same `SpawnWorkspaceSessionOutcome`, but the session
/// it produces is a `DaemonPtySession` (registered in
/// `v2_session_map`) instead of a `SessionStreamSession` (legacy
/// `session_map`). After A9, every system-driven spawn flows through
/// this. Only the explicit Kessel-T0 endpoint
/// (`POST /cli/sessions/spawn` → `awareness_ws::handle_sessions_spawn`)
/// stays on the legacy variant.
///
/// 0.37.0 canonicalization: when `req.project_id` is set, the session
/// registers under the prefixed key `<project_id>:<agent_name>` and
/// the function performs an **idempotency check** first — if a session
/// is already registered under that key and its child PID is still
/// alive, return the existing session_id with `reused=true` instead
/// of spawning a duplicate. This is what makes `agents launch`
/// idempotent end-to-end and closes Baden's "session A vs B
/// accumulating" bug class.
///
/// When `req.project_id` is None (legacy bare-name callers, vanishingly
/// rare post-0.37.0), registration falls back to the bare agent_name
/// — the function works but the canonicalization invariant doesn't
/// apply. Production daemon callers always set project_id.
///
/// Synchronous: alacritty's IO thread is started in the background
/// by `DaemonPtySession::spawn`; this fn returns as soon as the PTY
/// is open and the Term + event loop are wired. No grow-then-shrink
/// dance is needed (v2 doesn't have the replay-ring seeding the
/// legacy path requires).
pub fn spawn_agent_session_v2_blocking(
    req: SpawnWorkspaceSessionRequest,
) -> Result<SpawnWorkspaceSessionOutcome, String> {
    if req.agent_name.is_empty() {
        return Err("agent_name required".into());
    }

    // Canonical key construction — every workspace+agent has exactly
    // one slot in v2_session_map. project_id-less callers register
    // under the bare name (legacy fallback).
    let canonical_key = match req.project_id.as_deref() {
        Some(pid) if !pid.is_empty() => format!("{pid}:{}", req.agent_name),
        _ => req.agent_name.clone(),
    };

    // Idempotency: if a session is already registered under the
    // canonical key AND its child PID is alive, return it. The map
    // is normally kept authoritative by the child-exit observer
    // (`v2_spawn::spawn_child_exit_observer`), but the `is_child_alive`
    // double-check closes the small race window between ChildExit
    // and unregister — and gracefully handles the rare case where
    // the observer task panicked or the broadcast channel closed
    // before ChildExit landed.
    if let Some(existing) = v2_session_map::lookup_by_agent_name(&canonical_key) {
        if existing.is_child_alive() {
            log_debug!(
                "[daemon/spawn] v2 reuse session={} canonical_key={}",
                existing.session_id,
                canonical_key,
            );
            return Ok(SpawnWorkspaceSessionOutcome {
                session_id: existing.session_id,
                agent_name: req.agent_name,
                pending_drained: 0,
                reused: true,
            });
        }
        // Stale entry — child has exited but the unregister hadn't
        // fired yet (or the observer dropped its Arc). Clean up and
        // continue to spawn fresh.
        log_debug!(
            "[daemon/spawn] reaping stale v2 entry under canonical_key={} (child exited)",
            canonical_key,
        );
        v2_session_map::unregister(&canonical_key);
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
    v2_session_map::register(canonical_key.clone(), session.clone());

    // Wire the child-exit observer so the v2_session_map slot
    // releases when the child PID dies — keeps the idempotency
    // check above accurate without a separate liveness probe.
    crate::v2_spawn::spawn_child_exit_observer(canonical_key.clone(), session.clone());

    // Drain any pending-live signals queued for this canonical key
    // so they become input to the fresh session. Awareness bus
    // enqueues under the prefixed key (post-0.36.15); this drain
    // matches that. Without this, signals enqueued by
    // `DaemonWakeProvider::wake` while the agent was offline get
    // silently dropped on v2 boot.
    let pending = pending_live::drain_for_agent(&canonical_key);
    let pending_drained = pending.len();
    for signal in pending {
        let bytes = signal_format::inject_bytes(&signal);
        session.write(bytes.into_bytes());
    }

    log_debug!(
        "[daemon/spawn] v2 session={} canonical_key={} pending_drained={}",
        session_id,
        canonical_key,
        pending_drained,
    );

    Ok(SpawnWorkspaceSessionOutcome {
        session_id,
        agent_name: req.agent_name,
        pending_drained,
        reused: false,
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
    req: SpawnWorkspaceSessionRequest,
) -> Result<SpawnWorkspaceSessionOutcome, String> {
    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(spawn_agent_session(req))
    })
}

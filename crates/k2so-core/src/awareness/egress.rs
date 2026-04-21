//! Egress composer — where the Awareness Bus actually delivers.
//!
//! E4 of Phase 3. Takes a signal, resolves liveness, plans delivery
//! via `routing::resolve`, and fires every channel the plan asks
//! for: inbox file, PTY-inject, scheduler wake, bus broadcast,
//! activity_feed row.
//!
//! **Ambient providers.** PTY-inject and scheduler-wake are
//! per-deployment concerns — the daemon knows how to reach live
//! sessions and trigger wake, tests want mocks, the smoke-test
//! example wants in-process calls. This module exposes two
//! provider traits registered at startup via `set_inject_provider`
//! / `set_wake_provider`. With no provider registered, the
//! relevant paths become no-ops but audit (bus + activity_feed)
//! still fires.
//!
//! **Liveness source.** `session::registry` — we walk registered
//! sessions looking for one tagged with the target agent's name.
//! Fast, in-process, no DB round-trip.
//!
//! **activity_feed row.** Always fired (PRD: "always, for audit").
//! Uses `db::shared()` to insert via `ActivityFeedEntry::insert`.
//! `project_id` comes from the signal's `from.workspace` if it
//! matches a registered project row; otherwise falls back to the
//! `_orphan` sentinel (seeded by `db::seed_audit_sentinels` at
//! startup). `AgentAddress::Broadcast` senders use the `_broadcast`
//! sentinel. The original unresolved workspace id is preserved in
//! the row's `to_project_id` column when falling back, so audit
//! consumers can still see what the sender claimed.

use std::path::PathBuf;
use std::sync::OnceLock;

use parking_lot::Mutex;

use crate::awareness::{
    bus, inbox, resolve, routing::DeliveryPlan, AgentAddress, AgentSignal,
    TargetState,
};
use crate::db::schema::ActivityFeedEntry;
use crate::log_debug;
use crate::session::registry;

// ─────────────────────────────────────────────────────────────────────
// Provider traits — host registers implementations at startup
// ─────────────────────────────────────────────────────────────────────

/// Implemented by the host to inject bytes into a live agent's
/// session. Daemon's E7 impl will look up the
/// `SessionStreamSession` by agent_name and call `write(bytes)` on
/// it. Test impls can just collect the calls in a vec.
pub trait InjectProvider: Send + Sync {
    /// Best-effort inject. `Ok` on success; error paths logged by
    /// the caller. If the agent has no live session, return `Err` —
    /// egress degrades to audit-only for that signal.
    fn inject(&self, agent: &str, bytes: &[u8]) -> std::io::Result<()>;
}

/// Implemented by the host to trigger a scheduler wake for an
/// offline agent. Daemon's E7 impl queues a pending-live-delivery
/// file and fires the existing heartbeat-wake pathway.
pub trait WakeProvider: Send + Sync {
    /// Best-effort wake. `Ok` means "wake request accepted"; the
    /// actual session boot happens asynchronously. Errors on
    /// unknown agent / invalid state.
    fn wake(&self, agent: &str, signal: &AgentSignal) -> std::io::Result<()>;
}

static INJECT_PROVIDER: OnceLock<Mutex<Option<Box<dyn InjectProvider>>>> =
    OnceLock::new();
static WAKE_PROVIDER: OnceLock<Mutex<Option<Box<dyn WakeProvider>>>> =
    OnceLock::new();

fn inject_slot() -> &'static Mutex<Option<Box<dyn InjectProvider>>> {
    INJECT_PROVIDER.get_or_init(|| Mutex::new(None))
}

fn wake_slot() -> &'static Mutex<Option<Box<dyn WakeProvider>>> {
    WAKE_PROVIDER.get_or_init(|| Mutex::new(None))
}

/// Register the host's inject provider. Idempotent — second call
/// overwrites. Test helpers use this to install a mock.
pub fn set_inject_provider(p: Box<dyn InjectProvider>) {
    *inject_slot().lock() = Some(p);
}

/// Register the host's wake provider. Idempotent.
pub fn set_wake_provider(p: Box<dyn WakeProvider>) {
    *wake_slot().lock() = Some(p);
}

/// Test helper — clear provider registrations so tests see a
/// clean slate. Mirror of the `registry::clear_for_tests` pattern.
#[cfg(any(test, feature = "test-util"))]
pub fn clear_providers_for_tests() {
    *inject_slot().lock() = None;
    *wake_slot().lock() = None;
}

// ─────────────────────────────────────────────────────────────────────
// DeliveryReport — what the composer did
// ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeliveryReport {
    /// `Delivery::Live` sender whose bytes reached the target's PTY
    /// directly. False if the target was offline or inject failed.
    pub injected_to_pty: bool,
    /// `Delivery::Live` sender whose target was offline; wake
    /// provider fired successfully.
    pub woke_offline_target: bool,
    /// `Delivery::Inbox` sender whose file landed at this path.
    pub inbox_path: Option<PathBuf>,
    /// Row id of the activity_feed entry written. 0 if no DB was
    /// available (tests without `test-util`) — not an error, just
    /// means audit couldn't persist.
    pub activity_feed_row_id: i64,
    /// Bus publish always fires; marker exists for symmetry and to
    /// make "everything succeeded" assertions easy.
    pub published_to_bus: bool,
}

// ─────────────────────────────────────────────────────────────────────
// deliver — the entry point
// ─────────────────────────────────────────────────────────────────────

/// Deliver `signal`. Returns what actually happened; callers (the
/// `k2so signal` CLI, APC ingress) use the report to decide
/// follow-up (prints, retries, etc.).
///
/// `inbox_root` is the directory under which per-agent inboxes
/// live. Daemon passes `<project>/.k2so/awareness/inbox/` for
/// same-workspace delivery; cross-workspace delivery (Phase 4)
/// rebuilds this per target.
///
/// For `AgentAddress::Broadcast` / `Workspace`, this function
/// handles the "always" channels (bus + activity_feed). Fanout
/// across individual agents is the caller's responsibility — use
/// `roster::query` to enumerate members, then call `deliver_to_agent`
/// for each.
pub fn deliver(signal: &AgentSignal, inbox_root: &std::path::Path) -> DeliveryReport {
    match &signal.to {
        AgentAddress::Agent { name, .. } => {
            deliver_to_agent(signal, name, inbox_root)
        }
        AgentAddress::Workspace { .. } | AgentAddress::Broadcast => {
            // Multi-target paths only run the audit channels here.
            // Caller enumerates and loops.
            let mut report = DeliveryReport::default();
            report.published_to_bus = true;
            bus::publish(signal.clone());
            report.activity_feed_row_id = write_audit(signal, None);
            report
        }
    }
}

/// Deliver to a specific agent name. Resolves liveness from
/// session::registry, plans via routing::resolve_for_agent, fires
/// each channel the plan prescribes.
///
/// Invariants held:
/// - Inbox writes only when sender picked `Delivery::Inbox`. Never
///   as a fallback for offline targets.
/// - Live+live tries inject. On inject failure (provider unregistered,
///   session mid-shutdown, etc.), the report's `injected_to_pty`
///   stays false but audit still fires — the signal isn't lost, it's
///   recorded in activity_feed for later manual inspection.
/// - Every delivery writes activity_feed. Every delivery publishes
///   to bus.
pub fn deliver_to_agent(
    signal: &AgentSignal,
    target_agent: &str,
    inbox_root: &std::path::Path,
) -> DeliveryReport {
    let state = if is_agent_live(target_agent) {
        TargetState::Live
    } else {
        TargetState::Offline
    };
    let plan = resolve(signal, state);
    execute(signal, target_agent, plan, inbox_root)
}

/// Run a pre-computed `DeliveryPlan`. Exposed publicly for callers
/// that already resolved the plan themselves (e.g. a dry-run UI that
/// shows "if you click send, here's what will happen" before actually
/// clicking).
pub fn execute(
    signal: &AgentSignal,
    target_agent: &str,
    plan: DeliveryPlan,
    inbox_root: &std::path::Path,
) -> DeliveryReport {
    let mut report = DeliveryReport::default();

    if plan.inject_to_pty {
        match try_inject(target_agent, signal) {
            Ok(()) => report.injected_to_pty = true,
            Err(e) => {
                log_debug!(
                    "[awareness/egress] inject to {} failed: {} — audit-only",
                    target_agent,
                    e
                );
            }
        }
    }

    if plan.wake_target {
        match try_wake(target_agent, signal) {
            Ok(()) => report.woke_offline_target = true,
            Err(e) => {
                log_debug!(
                    "[awareness/egress] wake {} failed: {} — audit-only",
                    target_agent,
                    e
                );
            }
        }
    }

    if plan.write_to_inbox {
        match inbox::write(inbox_root, target_agent, signal) {
            Ok(path) => report.inbox_path = Some(path),
            Err(e) => {
                log_debug!(
                    "[awareness/egress] inbox write for {} failed: {}",
                    target_agent,
                    e
                );
            }
        }
    }

    if plan.publish_to_bus {
        bus::publish(signal.clone());
        report.published_to_bus = true;
    }

    if plan.write_activity_feed {
        report.activity_feed_row_id = write_audit(signal, Some(target_agent));
    }

    report
}

// ─────────────────────────────────────────────────────────────────────
// Internals
// ─────────────────────────────────────────────────────────────────────

fn is_agent_live(agent: &str) -> bool {
    registry::list_ids()
        .into_iter()
        .filter_map(|id| registry::lookup(&id).and_then(|e| e.agent_name()))
        .any(|n| n == agent)
}

fn try_inject(agent: &str, signal: &AgentSignal) -> std::io::Result<()> {
    let slot = inject_slot().lock();
    let provider = slot.as_ref().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotConnected,
            "no InjectProvider registered",
        )
    })?;
    let bytes = render_signal_for_inject(signal);
    provider.inject(agent, bytes.as_bytes())
}

fn try_wake(agent: &str, signal: &AgentSignal) -> std::io::Result<()> {
    let slot = wake_slot().lock();
    let provider = slot.as_ref().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotConnected,
            "no WakeProvider registered",
        )
    })?;
    provider.wake(agent, signal)
}

/// Format a signal as the bytes to inject into the target's PTY.
/// Phase 3 MVP: just render the message text with a newline so the
/// harness reads it as a typed line. Future refinements (prefix with
/// `[from foo]`, ANSI formatting, agent-speak control codes) live in
/// per-harness integrations (Phase 6).
fn render_signal_for_inject(signal: &AgentSignal) -> String {
    use crate::awareness::SignalKind;
    let body = match &signal.kind {
        SignalKind::Msg { text } => text.clone(),
        SignalKind::Status { text } => format!("[status] {text}"),
        SignalKind::Presence { state } => {
            format!("[presence {state:?}]")
        }
        SignalKind::Reservation { paths, action } => {
            format!("[{action:?}] {}", paths.join(", "))
        }
        SignalKind::TaskLifecycle { phase, task_ref } => {
            let r = task_ref.as_deref().unwrap_or("");
            format!("[task {phase:?}] {r}")
        }
        SignalKind::Custom { kind, payload } => {
            format!("[{kind}] {payload}")
        }
    };
    let from = match &signal.from {
        AgentAddress::Agent { name, .. } => name.clone(),
        AgentAddress::Workspace { .. } => "workspace".into(),
        AgentAddress::Broadcast => "broadcast".into(),
    };
    format!("[{from}] {body}\n")
}

fn write_audit(signal: &AgentSignal, target_agent: Option<&str>) -> i64 {
    let project_id = match &signal.from {
        AgentAddress::Agent { workspace, .. }
        | AgentAddress::Workspace { workspace } => workspace.0.as_str(),
        AgentAddress::Broadcast => "_broadcast",
    };
    let from_agent = match &signal.from {
        AgentAddress::Agent { name, .. } => Some(name.as_str()),
        _ => None,
    };
    let event_type = format!("signal:{}", signal_kind_tag(signal));
    let summary = signal_summary(signal);
    // Store the full AgentSignal JSON in the metadata column so
    // callers can reconstruct the entire message from the audit
    // log — including signal id (for bus/inbox correlation), full
    // body (summary truncates at 80 chars), priority, reply_to,
    // exact timestamp. activity_feed is the PRIMITIVE audit
    // surface; higher-level views (conversation threads, per-agent
    // message history, reply chains) are SQL queries on top.
    let full_signal = serde_json::to_string(signal).ok();

    let db = crate::db::shared();
    let conn = db.lock();
    // Primary attempt: write with the sender-supplied project_id.
    match ActivityFeedEntry::insert(
        &conn,
        project_id,
        target_agent,
        &event_type,
        from_agent,
        target_agent,
        None,
        Some(&summary),
        full_signal.as_deref(),
    ) {
        Ok(id) => id,
        Err(e) if is_fk_violation(&e) => {
            // FK miss — the sender's workspace isn't a registered
            // project. Fall back to the `_orphan` sentinel so audit
            // still fires. Primitive promise: activity_feed ALWAYS
            // records every delivered signal; we never silently drop
            // because a caller passed an unregistered project id.
            log_debug!(
                "[awareness/egress] project_id={:?} not registered; \
                 retrying audit insert under _orphan bucket",
                project_id
            );
            match ActivityFeedEntry::insert(
                &conn,
                "_orphan",
                target_agent,
                &event_type,
                from_agent,
                target_agent,
                Some(project_id),
                Some(&summary),
                full_signal.as_deref(),
            ) {
                Ok(id) => id,
                Err(e2) => {
                    log_debug!(
                        "[awareness/egress] _orphan fallback insert failed: {}",
                        e2
                    );
                    0
                }
            }
        }
        Err(e) => {
            log_debug!("[awareness/egress] activity_feed insert failed: {}", e);
            0
        }
    }
}

/// True iff `err` represents a SQLite FOREIGN KEY constraint
/// violation (extended code `SQLITE_CONSTRAINT_FOREIGNKEY`, 787).
/// Used to distinguish "project_id doesn't match any projects.id"
/// from other insert failures (disk full, DB locked, etc.) — only
/// the FK case triggers the `_orphan` fallback.
fn is_fk_violation(err: &rusqlite::Error) -> bool {
    matches!(
        err,
        rusqlite::Error::SqliteFailure(e, _)
            if e.extended_code
                == rusqlite::ffi::SQLITE_CONSTRAINT_FOREIGNKEY
    )
}

fn signal_kind_tag(signal: &AgentSignal) -> &'static str {
    use crate::awareness::SignalKind;
    match &signal.kind {
        SignalKind::Msg { .. } => "msg",
        SignalKind::Status { .. } => "status",
        SignalKind::Reservation { .. } => "reservation",
        SignalKind::Presence { .. } => "presence",
        SignalKind::TaskLifecycle { .. } => "task",
        SignalKind::Custom { .. } => "custom",
    }
}

fn signal_summary(signal: &AgentSignal) -> String {
    use crate::awareness::SignalKind;
    let delivery_tag = match signal.delivery {
        crate::awareness::Delivery::Live => "live",
        crate::awareness::Delivery::Inbox => "inbox",
    };
    let body_snippet = match &signal.kind {
        SignalKind::Msg { text } | SignalKind::Status { text } => {
            text.chars().take(80).collect::<String>()
        }
        SignalKind::Presence { state } => format!("{state:?}"),
        SignalKind::Reservation { paths, .. } => paths.join(","),
        SignalKind::TaskLifecycle { phase, .. } => format!("{phase:?}"),
        SignalKind::Custom { kind, .. } => kind.clone(),
    };
    format!("[{delivery_tag}] {body_snippet}")
}

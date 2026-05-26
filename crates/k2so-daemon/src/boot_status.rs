//! Daemon boot-phase state — the single source of truth behind the
//! `/boot-status` handshake and the dispatcher's readiness gate.
//!
//! ## Why this exists (0.39.5)
//!
//! Pre-0.39.5 the daemon ran every first-boot migration (the
//! 64-workspace unification + skills-consolidation + auto-pin sweep)
//! BEFORE it bound its port. During a 0.38.x → 0.39.x auto-update the
//! NEW daemon was therefore unreachable for the entire migration
//! window — while the OUTGOING old daemon was still bound to the stable
//! port and still answered `/ping` with 200. The renderer's
//! `ConnectionGate` took that false-positive "healthy" ping, mounted
//! the app, and its store fetches landed in the gap where the old
//! daemon had been killed and the new one was still migrating → blank
//! window ("appears to have crashed").
//!
//! The fix: bind the listener FIRST, advertise progress here, and have
//! the dispatcher 503 every real route until [`set_ready`] runs. The
//! renderer reads `phase` + `version` from `/boot-status` and only
//! mounts against a daemon whose version is paired with the app AND
//! whose phase is `ready` — so it can never bind to the outgoing old
//! daemon, and it can SHOW the user the migration is in progress.
//!
//! See `[[project_daemon_handshake_contract]]` and
//! `release-notes-0.39.5.md`.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{LazyLock, RwLock};

/// daemon↔client API compatibility version. Bump this ONLY when a
/// change breaks the routes/contract clients depend on — NOT on every
/// release. K2 Connect range-checks `protocol` to decide whether it can
/// talk to a remote daemon of a different marketing version; the local
/// auto-update path keys off the exact `version` string instead. The
/// two are intentionally decoupled. Starts at 1.
pub const PROTOCOL: u32 = 1;

const STARTING: u8 = 0;
const MIGRATING: u8 = 1;
const READY: u8 = 2;
const ERROR: u8 = 3;

static PHASE: AtomicU8 = AtomicU8::new(STARTING);
static DETAIL: LazyLock<RwLock<String>> = LazyLock::new(|| RwLock::new(String::new()));

/// Enter the `migrating` phase with an initial human-readable detail.
pub fn set_migrating(detail: &str) {
    set_detail(detail);
    PHASE.store(MIGRATING, Ordering::SeqCst);
}

/// Update the human-readable detail string. UI-only — clients MUST NOT
/// parse this for logic; it exists purely so the window can show
/// "Applying updates… (12/64 workspaces)" while we work.
pub fn set_detail(detail: &str) {
    if let Ok(mut d) = DETAIL.write() {
        *d = detail.to_string();
    }
}

/// Mark the daemon fully booted: migrations done, every provider/sink
/// registered, real routes safe to serve. Clears the detail. This is
/// the gate the dispatcher and the renderer both wait on.
pub fn set_ready() {
    set_detail("");
    PHASE.store(READY, Ordering::SeqCst);
}

/// Mark a fatal boot/migration error with a human-readable detail.
/// Reserved for future wiring (e.g. a migration that hard-fails) so the
/// renderer can surface a real error instead of spinning forever.
#[allow(dead_code)]
pub fn set_error(detail: &str) {
    set_detail(detail);
    PHASE.store(ERROR, Ordering::SeqCst);
}

/// True once [`set_ready`] has run. The dispatcher uses this to 503
/// every non-liveness route until first-boot migrations complete.
pub fn is_ready() -> bool {
    PHASE.load(Ordering::SeqCst) == READY
}

/// Lowercase phase string for the `/boot-status` JSON. Unknown future
/// values never appear here, but clients should treat any phase other
/// than `ready` as "not ready" (forward-compatible).
pub fn phase_str() -> &'static str {
    match PHASE.load(Ordering::SeqCst) {
        MIGRATING => "migrating",
        READY => "ready",
        ERROR => "error",
        _ => "starting",
    }
}

/// Current human-readable detail (cloned). UI-only.
pub fn detail() -> String {
    match DETAIL.read() {
        Ok(d) => d.clone(),
        // Poisoned lock should never happen (no panics under the lock),
        // but degrade to an empty detail rather than taking the daemon
        // down over a status string.
        Err(_) => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // These mutate process-global state, so they live in ONE test to
    // run serially and not race the shared AtomicU8 / RwLock across
    // cargo's parallel test threads.
    #[test]
    fn phase_transitions_and_detail_round_trip() {
        // Default before any boot call.
        assert_eq!(phase_str(), "starting");
        assert!(!is_ready());

        set_migrating("Applying updates…");
        assert_eq!(phase_str(), "migrating");
        assert!(!is_ready());
        assert_eq!(detail(), "Applying updates…");

        set_detail("12/64 workspaces");
        assert_eq!(detail(), "12/64 workspaces");
        assert!(!is_ready(), "detail update must not flip readiness");

        set_ready();
        assert_eq!(phase_str(), "ready");
        assert!(is_ready());
        assert_eq!(detail(), "", "set_ready clears the detail");
    }

    #[test]
    fn protocol_is_stable() {
        // Guards against an accidental bump — protocol changes are a
        // deliberate, breaking-contract decision.
        assert_eq!(PROTOCOL, 1);
    }
}

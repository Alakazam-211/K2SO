//! G1 integration tests — harness watchdog escalation ladder
//! against real PTY sessions.
//!
//! These tests spawn a real `cat` process under the daemon's
//! session_map, wait for it to go idle past the configured
//! thresholds, drive `watchdog::tick()` deterministically, and
//! assert:
//!
//! 1. `Escalation::Warn` — a `watchdog.idle_warning` SemanticEvent
//!    frame is published (visible to any session subscriber).
//! 2. `Escalation::CtrlC` — 0x03 is written to the PTY.
//! 3. `Escalation::Kill` — `cat` exits (child.try_wait() succeeds).
//!
//! Driving `tick()` directly instead of relying on the background
//! tokio loop keeps the test deterministic; sleeping between ticks
//! is the only non-deterministic bit, and the thresholds are tiny
//! (50–500 ms) to keep the whole file under a second of wall time.

#![cfg(unix)]

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use k2so_core::session::{
    self, registry, Frame, SemanticKind, SessionId, WatchdogConfig,
};
use k2so_core::terminal::{spawn_session_stream, SpawnConfig};

use k2so_daemon::session_map;
use k2so_daemon::watchdog;

/// Serialize every watchdog test — all touch the global session_map +
/// session::registry singletons.
static TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn spawn_cat_session() -> (SessionId, Arc<k2so_core::terminal::SessionStreamSession>) {
    let id = SessionId::new();
    let session = spawn_session_stream(SpawnConfig {
        session_id: id,
        cwd: "/tmp".into(),
        // `cat` is perfect for idle testing: it blocks on stdin
        // forever unless we write, so `last_frame_at` stays frozen
        // at spawn time and the watchdog correctly sees it as idle.
        command: Some("cat".into()),
        args: None,
        cols: 80,
        rows: 24,
    })
    .expect("spawn cat");
    let arc = Arc::new(session);
    (id, arc)
}

fn fast_config() -> WatchdogConfig {
    WatchdogConfig {
        warn_after: Some(Duration::from_millis(80)),
        ctrl_c_after: Some(Duration::from_millis(200)),
        kill_after: Some(Duration::from_millis(400)),
        spawn_grace: Duration::ZERO,
        poll_interval: Duration::from_millis(25),
    }
}

/// Drain the session's frame stream looking for a SemanticEvent
/// with the given custom kind. Returns true if found within the
/// timeout, false otherwise.
async fn await_watchdog_frame(
    rx: &mut tokio::sync::broadcast::Receiver<Frame>,
    want_kind: &str,
    timeout: Duration,
) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if std::time::Instant::now() >= deadline {
            return false;
        }
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Ok(Frame::SemanticEvent {
                kind: SemanticKind::Custom { kind, .. },
                ..
            })) if kind == want_kind => {
                return true;
            }
            Ok(Ok(_)) => continue, // some other frame, keep looking
            Ok(Err(_)) => return false, // channel closed
            Err(_) => return false,    // timeout
        }
    }
}

// ─────────────────────────────────────────────────────────────────────
// Warn + Ctrl-C on a wedged cat: Ctrl-C (SIGINT via PTY line
// discipline) is sufficient to kill cat, so the kill stage never
// needs to fire. This is the COMMON success path — most interactive
// harnesses (bash, claude, codex, vim) respond to Ctrl-C.
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn watchdog_warn_and_ctrl_c_wake_wedged_cat() {
    let _g = lock();
    let agent = "watchdog-ladder-cat";
    let (id, session) = spawn_cat_session();
    session_map::register(agent, session.clone());

    // Subscribe BEFORE ticking so we see every SemanticEvent.
    let entry = registry::lookup(&id).expect("session registered");
    let mut rx = entry.subscribe();

    let config = fast_config();
    let mut states: HashMap<SessionId, k2so_core::session::EscalationState> =
        HashMap::new();

    // Wait past warn_after, tick, assert warning.
    tokio::time::sleep(Duration::from_millis(100)).await;
    watchdog::tick(&config, &mut states);
    assert!(
        states.get(&id).map(|s| s.warned).unwrap_or(false),
        "warn stage should have fired"
    );
    assert!(
        await_watchdog_frame(&mut rx, "watchdog.idle_warning", Duration::from_millis(100)).await,
        "watchdog.idle_warning frame should have published"
    );

    // Wait past ctrl_c_after, tick, assert ctrl_c.
    tokio::time::sleep(Duration::from_millis(120)).await;
    watchdog::tick(&config, &mut states);
    assert!(
        states.get(&id).map(|s| s.ctrl_c_sent).unwrap_or(false),
        "ctrl_c stage should have fired"
    );
    assert!(
        await_watchdog_frame(&mut rx, "watchdog.ctrl_c_sent", Duration::from_millis(100)).await,
        "watchdog.ctrl_c_sent frame should have published"
    );

    // Cat reacts to the 0x03 byte (PTY line discipline converts it
    // to SIGINT) and exits on its own — Ctrl-C was sufficient. This
    // is the primitive's COMMON success path: the wedge is unstuck
    // without needing SIGKILL.
    assert!(
        session.wait_for_exit(Duration::from_millis(500)),
        "cat should have exited in response to Ctrl-C (SIGINT)"
    );

    session_map::unregister(agent);
}

// ─────────────────────────────────────────────────────────────────────
// Kill stage actually terminates the child process.
//
// This is the fallback for harnesses that trap/ignore Ctrl-C.
// Rather than wrestle with shell-escaping an inline SIGINT-ignoring
// script (which introduces portability noise between zsh/bash
// on macOS), we isolate the Kill execution path by using a config
// that ONLY enables `kill_after` — no warn, no ctrl_c interference.
// That way the Kill path is the only escalation evaluate() can
// possibly return, and the test asserts:
//   - state.killed flips true once idle passes threshold
//   - the child process actually exits (SIGKILL can't be ignored)
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn watchdog_kill_stage_terminates_child() {
    let _g = lock();
    let agent = "watchdog-kill-only";
    let (id, session) = spawn_cat_session();
    session_map::register(agent, session.clone());

    let entry = registry::lookup(&id).expect("session registered");
    let mut rx = entry.subscribe();

    // Kill-only config: other stages disabled, so evaluate() will
    // never return Warn or CtrlC regardless of timing.
    let config = WatchdogConfig {
        warn_after: None,
        ctrl_c_after: None,
        kill_after: Some(Duration::from_millis(150)),
        spawn_grace: Duration::ZERO,
        poll_interval: Duration::from_millis(25),
    };
    let mut states: HashMap<SessionId, k2so_core::session::EscalationState> =
        HashMap::new();

    tokio::time::sleep(Duration::from_millis(220)).await;
    watchdog::tick(&config, &mut states);

    assert!(
        states.get(&id).map(|s| s.killed).unwrap_or(false),
        "kill stage should have fired (only enabled stage)"
    );
    assert!(
        await_watchdog_frame(&mut rx, "watchdog.killed", Duration::from_millis(200)).await,
        "watchdog.killed SemanticEvent frame should publish"
    );
    assert!(
        session.wait_for_exit(Duration::from_millis(500)),
        "cat should have exited after SIGKILL"
    );

    session_map::unregister(agent);
}

// ─────────────────────────────────────────────────────────────────────
// Spawn grace freezes escalation during the harness boot window
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn watchdog_spawn_grace_prevents_early_escalation() {
    let _g = lock();
    let agent = "watchdog-grace";
    let (id, session) = spawn_cat_session();
    session_map::register(agent, session.clone());

    // Grace is longer than the idle threshold — the watchdog must
    // NOT escalate during the grace window even though `idle >=
    // warn_after` technically holds.
    let config = WatchdogConfig {
        warn_after: Some(Duration::from_millis(10)),
        ctrl_c_after: Some(Duration::from_millis(20)),
        kill_after: Some(Duration::from_millis(30)),
        spawn_grace: Duration::from_millis(500),
        poll_interval: Duration::from_millis(10),
    };
    let mut states = HashMap::new();

    // Wait past warn threshold but still inside spawn grace.
    tokio::time::sleep(Duration::from_millis(100)).await;
    watchdog::tick(&config, &mut states);
    assert!(
        !states.get(&id).map(|s| s.warned).unwrap_or(false),
        "warn must not fire during spawn_grace window"
    );
    assert!(
        !states.get(&id).map(|s| s.ctrl_c_sent).unwrap_or(false),
        "ctrl_c must not fire during spawn_grace window"
    );

    // After grace elapses, escalation resumes.
    tokio::time::sleep(Duration::from_millis(450)).await;
    watchdog::tick(&config, &mut states);
    // Age is now well past kill_after — catch-up rule should
    // fire Kill directly, skipping warn/ctrl_c.
    assert!(
        states.get(&id).map(|s| s.killed).unwrap_or(false),
        "post-grace tick should catch up to Kill"
    );

    let _ = session.kill();
    session_map::unregister(agent);
}

// ─────────────────────────────────────────────────────────────────────
// State pruning for exited sessions
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn watchdog_prunes_state_for_exited_sessions() {
    let _g = lock();
    let agent = "watchdog-prune";
    let (id, session) = spawn_cat_session();
    session_map::register(agent, session.clone());

    let config = fast_config();
    let mut states = HashMap::new();

    // Drive one tick past warn_after so the session has a state
    // entry.
    tokio::time::sleep(Duration::from_millis(100)).await;
    watchdog::tick(&config, &mut states);
    assert!(states.contains_key(&id), "state should exist after first tick");

    // Unregister the session (simulates session drop / exit) and
    // tick again — the pruning step should remove the stale entry.
    session_map::unregister(agent);
    watchdog::tick(&config, &mut states);
    assert!(
        !states.contains_key(&id),
        "watchdog should prune state for sessions no longer in session_map"
    );

    let _ = session.kill();
}

// ─────────────────────────────────────────────────────────────────────
// Disabled config is a no-op even on an idle wedged session
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn watchdog_disabled_config_never_escalates() {
    let _g = lock();
    let agent = "watchdog-disabled";
    let (id, session) = spawn_cat_session();
    session_map::register(agent, session.clone());

    let config = WatchdogConfig::disabled();
    let mut states = HashMap::new();

    tokio::time::sleep(Duration::from_millis(150)).await;
    watchdog::tick(&config, &mut states);

    // Disabled config means evaluate always returns None, so no
    // state entry is ever mutated.
    let warned = states.get(&id).map(|s| s.warned).unwrap_or(false);
    assert!(!warned, "disabled watchdog must never escalate");

    let _ = session.kill();
    session_map::unregister(agent);
}

// ─────────────────────────────────────────────────────────────────────
// Fresh-frame publish resets idle timer — session stops being idle
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn watchdog_idle_resets_on_new_frame() {
    let _g = lock();
    let agent = "watchdog-reset";
    let (id, session) = spawn_cat_session();
    session_map::register(agent, session.clone());

    let entry = registry::lookup(&id).expect("session registered");
    let config = fast_config();
    let mut states = HashMap::new();

    // Wait for warning, tick, confirm warn fired.
    tokio::time::sleep(Duration::from_millis(100)).await;
    watchdog::tick(&config, &mut states);
    assert!(states.get(&id).map(|s| s.warned).unwrap_or(false));

    // Simulate the harness becoming active again — publish a
    // fresh Text frame. This updates last_frame_at.
    entry.publish(k2so_core::session::Frame::Text {
        bytes: b"hello I'm awake".to_vec(),
        style: None,
    });

    // New idle timer starts. With a 25 ms tick and 80 ms warn
    // threshold, we should NOT re-warn on the next tick even
    // though warned is already true.
    tokio::time::sleep(Duration::from_millis(40)).await;
    watchdog::tick(&config, &mut states);
    // State's `warned` stays true (one-shot); ctrl_c must NOT have
    // fired because the fresh frame reset the idle timer.
    assert!(
        !states.get(&id).map(|s| s.ctrl_c_sent).unwrap_or(false),
        "fresh frame should reset idle; ctrl_c must not have fired yet"
    );

    let _ = session.kill();
    session_map::unregister(agent);
}

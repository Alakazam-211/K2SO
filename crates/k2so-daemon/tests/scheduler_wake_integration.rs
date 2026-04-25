//! G4 integration tests — `DaemonWakeProvider` auto-launches
//! offline agents via their `AGENT.md` launch profile.
//!
//! Proves the Phase 3.2 goal: an offline agent with a launch
//! profile goes from "signal sent → queued forever" (the F3
//! behavior) to "signal sent → auto-launch fires → queue drains →
//! signal becomes first byte of input" (the G4 behavior).

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use k2so_core::awareness::{
    egress, AgentAddress, AgentSignal, Delivery, SignalKind, WorkspaceId,
};
use k2so_core::db::init_for_tests;

use k2so_daemon::providers;
use k2so_daemon::session_lookup;
use k2so_daemon::session_map;
use k2so_daemon::v2_session_map;

/// Serialize every test — all touch the global session::registry,
/// session_map, awareness providers, and the shared in-memory DB.
static TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Register a project row + return a scratch project directory on
/// disk so AGENT.md files can be written there.
fn setup_project(workspace_id: &str) -> PathBuf {
    let project_path = std::env::temp_dir().join(format!(
        "k2so-scheduler-wake-test-{}-{}-{}",
        workspace_id,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&project_path);
    std::fs::create_dir_all(&project_path).unwrap();

    let db = k2so_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT OR REPLACE INTO projects (id, path, name) VALUES (?1, ?2, ?3)",
        rusqlite::params![
            workspace_id,
            project_path.to_string_lossy().as_ref(),
            "scheduler-wake-test"
        ],
    )
    .unwrap();
    project_path
}

fn write_agent_md(project: &Path, agent: &str, body: &str) {
    let dir = project.join(".k2so/agents").join(agent);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("AGENT.md"), body).unwrap();
}

/// Redirect the pending-live queue root to a per-test scratch dir
/// so parallel runs (and rerun-after-panic) don't share queue state.
fn isolate_pending_root(tag: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "k2so-pending-sched-test-{}-{}-{}",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    std::env::set_var("K2SO_PENDING_LIVE_ROOT", &path);
    path
}

fn live_signal(workspace: &str, target: &str) -> AgentSignal {
    AgentSignal::new(
        AgentAddress::Agent {
            workspace: WorkspaceId(workspace.into()),
            name: "sender".into(),
        },
        AgentAddress::Agent {
            workspace: WorkspaceId(workspace.into()),
            name: target.into(),
        },
        SignalKind::Msg {
            text: "hello auto-launch".into(),
        },
    )
    .with_delivery(Delivery::Live)
}

// ─────────────────────────────────────────────────────────────────────
// Agent with AGENT.md launch profile → auto-launch fires
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn wake_auto_launches_agent_with_launch_profile() {
    let _g = lock();
    init_for_tests();
    providers::register_all();
    let queue_root = isolate_pending_root("auto-launch-profile");

    // Fresh workspace id + project row + AGENT.md on disk.
    let workspace = "scheduler-wake-ws-auto";
    let project = setup_project(workspace);
    write_agent_md(
        &project,
        "auto-target",
        "---\nname: auto-target\nlaunch:\n  command: cat\n  cwd: /tmp\n---\n",
    );

    // Sanity: neither map has the agent yet.
    assert!(session_lookup::lookup_any("auto-target").is_none());

    // Send a Live signal. egress sees target=offline → calls wake.
    // Our G4 wake enqueues + auto-launches via AGENT.md.
    let signal = live_signal(workspace, "auto-target");
    let inbox_root = std::env::temp_dir().join("k2so-g4-inbox-ignored");
    let _ = std::fs::create_dir_all(&inbox_root);
    let report = egress::deliver(&signal, &inbox_root);
    assert!(
        report.woke_offline_target,
        "wake must have fired — target was offline at delivery time"
    );

    // G4 outcome: post-A9 wake auto-launches into v2_session_map
    // (the unified lookup_any handles both legacy + v2).
    assert!(
        session_lookup::lookup_any("auto-target").is_some(),
        "auto-target should have been auto-launched by wake"
    );

    // The pending queue was drained as part of the spawn flow —
    // the signal file should not linger on disk.
    let agent_queue = queue_root.join("auto-target");
    let remaining: Vec<_> = if agent_queue.exists() {
        std::fs::read_dir(&agent_queue)
            .unwrap()
            .filter_map(|r| r.ok())
            .collect()
    } else {
        Vec::new()
    };
    assert!(
        remaining.is_empty(),
        "queue should be empty after auto-launch drain; remaining: {remaining:?}"
    );

    // Let the reader thread start; then clean up. Give a small
    // delay so the PTY has time to fully init before drop. v2
    // sessions teardown via unregister; legacy needs an explicit
    // kill.
    tokio::time::sleep(Duration::from_millis(50)).await;
    if let Some(legacy) = session_map::lookup("auto-target") {
        let _ = legacy.kill();
        session_map::unregister("auto-target");
    } else {
        v2_session_map::unregister("auto-target");
    }
}

// ─────────────────────────────────────────────────────────────────────
// Agent WITHOUT launch profile → stays queued, no auto-launch
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn wake_falls_back_to_queue_only_without_launch_profile() {
    let _g = lock();
    init_for_tests();
    providers::register_all();
    let queue_root = isolate_pending_root("no-profile");

    let workspace = "scheduler-wake-ws-no-profile";
    let project = setup_project(workspace);
    // AGENT.md exists but has NO launch: block.
    write_agent_md(
        &project,
        "profileless-target",
        "---\nname: profileless-target\nrole: no launch block\n---\n\nbody.\n",
    );

    let signal = live_signal(workspace, "profileless-target");
    let inbox_root = std::env::temp_dir().join("k2so-g4-inbox-ignored-2");
    let _ = std::fs::create_dir_all(&inbox_root);
    let report = egress::deliver(&signal, &inbox_root);
    assert!(report.woke_offline_target);

    // Expected: no auto-launch — neither map has the agent, and
    // the queue retains the signal file for a future user-triggered
    // spawn to drain.
    assert!(
        session_lookup::lookup_any("profileless-target").is_none(),
        "agent without launch profile must not be auto-launched"
    );

    let agent_queue = queue_root.join("profileless-target");
    let queued: Vec<_> = std::fs::read_dir(&agent_queue)
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    assert_eq!(
        queued.len(),
        1,
        "signal should still be queued for a future spawn"
    );
}

// ─────────────────────────────────────────────────────────────────────
// Unregistered workspace id → stays queued, no project path lookup
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn wake_falls_back_when_workspace_id_is_unknown() {
    let _g = lock();
    init_for_tests();
    providers::register_all();
    let queue_root = isolate_pending_root("unknown-workspace");

    // Deliberately do NOT register this workspace. wake's project-
    // path lookup will return None → auto-launch skips.
    let signal = live_signal("_not_a_real_workspace_id_", "ghost-agent");
    let inbox_root = std::env::temp_dir().join("k2so-g4-inbox-ignored-3");
    let _ = std::fs::create_dir_all(&inbox_root);
    let _ = egress::deliver(&signal, &inbox_root);

    assert!(
        session_lookup::lookup_any("ghost-agent").is_none(),
        "unknown workspace must not trigger auto-launch"
    );

    let agent_queue = queue_root.join("ghost-agent");
    let queued: Vec<_> = std::fs::read_dir(&agent_queue)
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    assert_eq!(
        queued.len(),
        1,
        "signal should be queued even when auto-launch can't resolve project path"
    );
}

// ─────────────────────────────────────────────────────────────────────
// Already-live agent: wake short-circuits auto-launch (single-flight)
// ─────────────────────────────────────────────────────────────────────
//
// This guards the race where a concurrent `/cli/sessions/spawn`
// already registered the session between egress's liveness check
// and our wake call. We should NOT spawn a second session that'd
// overwrite the first in session_map.

#[tokio::test(flavor = "current_thread")]
async fn wake_skips_auto_launch_when_agent_is_already_live() {
    use k2so_core::session::SessionId;
    use k2so_core::terminal::{spawn_session_stream, SpawnConfig};

    let _g = lock();
    init_for_tests();
    providers::register_all();
    let _queue = isolate_pending_root("already-live");

    let workspace = "scheduler-wake-ws-already-live";
    let project = setup_project(workspace);
    write_agent_md(
        &project,
        "live-target",
        "---\nlaunch:\n  command: cat\n---\n",
    );

    // Manually register a session BEFORE the signal fires — this
    // simulates a race where another spawn path got there first.
    let existing_id = SessionId::new();
    let existing_session = spawn_session_stream(SpawnConfig {
        session_id: existing_id,
        cwd: "/tmp".into(),
        command: Some("cat".into()),
        args: None,
        cols: 80,
        rows: 24,
        track_alacritty_term: false,
    })
    .expect("existing session spawn");
    let existing_arc = std::sync::Arc::new(existing_session);
    session_map::register("live-target", existing_arc.clone());

    let signal = live_signal(workspace, "live-target");
    let inbox_root = std::env::temp_dir().join("k2so-g4-inbox-ignored-4");
    let _ = std::fs::create_dir_all(&inbox_root);
    let _ = egress::deliver(&signal, &inbox_root);

    // session_map still points to the pre-existing session, not a
    // new one spawned by wake.
    let current = session_map::lookup("live-target")
        .expect("session should still be registered");
    assert_eq!(
        current.session_id, existing_id,
        "wake must NOT have replaced the live session with a fresh spawn"
    );

    let _ = existing_arc.kill();
    session_map::unregister("live-target");
}

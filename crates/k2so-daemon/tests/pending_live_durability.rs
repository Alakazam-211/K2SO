//! F3 pending-live-delivery durability tests.
//!
//! Sender emits a Live signal for an offline target → DaemonWakeProvider
//! persists the signal to `pending_live` queue → later, when the
//! target's session spawns, the drain loop injects the queued
//! signal into the fresh PTY.

#![cfg(unix)]

use std::sync::Mutex as StdMutex;
use std::time::Duration;

use k2so_core::awareness::{
    egress, AgentAddress, AgentSignal, Delivery, SignalKind, WorkspaceId,
};
use k2so_core::db::init_for_tests;

use k2so_daemon::awareness_ws;
use k2so_daemon::pending_live;
use k2so_daemon::providers;
use k2so_daemon::session_map;
use k2so_daemon::spawn::{spawn_agent_session_v2_blocking, SpawnWorkspaceSessionRequest};
use k2so_daemon::v2_session_map;

static TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn ensure_project(pid: &str) {
    let db = k2so_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT OR IGNORE INTO projects (id, path, name) VALUES (?1, ?1, ?1)",
        rusqlite::params![pid],
    )
    .unwrap();
}

fn isolate_pending_root(tag: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "k2so-pending-test-{}-{}-{}",
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

#[tokio::test(flavor = "current_thread")]
async fn live_signal_to_offline_target_lands_in_pending_queue() {
    let _g = lock();
    init_for_tests();
    // Deliberately NOT registering "unregistered-ws" in projects.
    // Post-0.37.0 the wake provider has a default launch profile
    // fallback that would otherwise auto-launch and drain the
    // queue immediately. By targeting a workspace not in the
    // projects table, `try_auto_launch` errors at
    // `lookup_project_path` and the signal stays queued — which
    // is exactly the F3 invariant we want to pin.
    providers::register_all();
    let queue_root = isolate_pending_root("offline-queue");

    let signal = AgentSignal::new(
        AgentAddress::Agent {
            workspace: WorkspaceId("unregistered-ws".into()),
            name: "foo".into(),
        },
        AgentAddress::Agent {
            workspace: WorkspaceId("unregistered-ws".into()),
            name: "offline-target".into(),
        },
        SignalKind::Msg {
            text: "F3-queue-test".into(),
        },
    )
    .with_delivery(Delivery::Live);

    let inbox_root = std::env::temp_dir().join("k2so-f3-ignored-inbox");
    let _ = std::fs::create_dir_all(&inbox_root);
    let report = egress::deliver(&signal, &inbox_root);

    assert!(!report.injected_to_pty);
    assert!(
        report.woke_offline_target,
        "wake should have fired and queued the signal"
    );

    // Queue file exists under the canonical key (post-0.37.0:
    // wake provider enqueues under `<workspace_id>:<agent>`,
    // sanitized to `_` for filesystem safety).
    let agent_dir = queue_root.join("unregistered-ws_offline-target");
    assert!(agent_dir.exists(), "agent dir should exist: {agent_dir:?}");
    let queued: Vec<_> = std::fs::read_dir(&agent_dir)
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    assert_eq!(queued.len(), 1, "exactly one queued signal expected");
}

#[tokio::test(flavor = "current_thread")]
async fn spawn_drains_pending_queue_and_injects_on_boot() {
    let _g = lock();
    init_for_tests();
    // Use an UNREGISTERED workspace so the post-0.37.0 default
    // launch profile fallback can't auto-launch — keeps the F3
    // queue-then-drain-on-explicit-spawn semantics intact.
    providers::register_all();
    let _queue = isolate_pending_root("spawn-drain");

    // Step 1: send a Live signal while the target is offline.
    // This queues the signal via DaemonWakeProvider.
    let signal = AgentSignal::new(
        AgentAddress::Agent {
            workspace: WorkspaceId("unregistered-drain-ws".into()),
            name: "foo".into(),
        },
        AgentAddress::Agent {
            workspace: WorkspaceId("unregistered-drain-ws".into()),
            name: "drain-target".into(),
        },
        SignalKind::Msg {
            text: "F3-drain-probe".into(),
        },
    )
    .with_delivery(Delivery::Live);

    let inbox_root = std::env::temp_dir().join("k2so-f3-drain-inbox");
    let _ = std::fs::create_dir_all(&inbox_root);
    let report = egress::deliver(&signal, &inbox_root);
    assert!(report.woke_offline_target);

    // Step 2: spawn a session via the legacy Kessel-T0 endpoint
    // (`/cli/sessions/spawn` → `awareness_ws::handle_sessions_spawn`).
    // Post-0.37.0 the wake provider enqueues under the canonical
    // `<workspace_id>:<agent>` key. The legacy spawn endpoint
    // doesn't auto-canonicalize agent_name, so the test passes the
    // canonical key form directly so the drain side matches.
    let canonical_key = "unregistered-drain-ws:drain-target";
    let spawn_body = serde_json::json!({
        "agent_name": canonical_key,
        "cwd": "/tmp",
        "command": "cat",
        "args": null,
        "cols": 80,
        "rows": 24,
    })
    .to_string();
    let spawn_result =
        awareness_ws::handle_sessions_spawn(spawn_body.as_bytes()).await;
    assert_eq!(spawn_result.status, "200 OK");
    let resp: serde_json::Value =
        serde_json::from_str(&spawn_result.body).unwrap();
    let drained = resp
        .get("pendingDrained")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    assert_eq!(drained, 1, "spawn should have drained 1 queued signal");

    // Queue dir is now empty (sanitized canonical-key path).
    let queue_root = pending_live::queue_root();
    let agent_dir = queue_root.join("unregistered-drain-ws_drain-target");
    if agent_dir.exists() {
        let remaining: Vec<_> = std::fs::read_dir(&agent_dir)
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(remaining.is_empty(), "queue should be empty after drain");
    }

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Cleanup — session was registered under canonical key.
    session_map::unregister(canonical_key);
}

#[tokio::test(flavor = "current_thread")]
async fn replay_all_finds_queued_entries() {
    let _g = lock();
    init_for_tests();
    // Unregistered workspace → wake auto-launch fallback can't
    // fire → signals stay queued for replay.
    providers::register_all();
    let _queue = isolate_pending_root("replay-all");

    // Queue two signals for two different agents.
    for (agent, text) in [("a", "msg-to-a"), ("b", "msg-to-b")] {
        let sig = AgentSignal::new(
            AgentAddress::Agent {
                workspace: WorkspaceId("unregistered-replay-ws".into()),
                name: "sender".into(),
            },
            AgentAddress::Agent {
                workspace: WorkspaceId("unregistered-replay-ws".into()),
                name: agent.to_string(),
            },
            SignalKind::Msg {
                text: text.to_string(),
            },
        )
        .with_delivery(Delivery::Live);
        let inbox = std::env::temp_dir().join("k2so-f3-replay-inbox");
        let _ = std::fs::create_dir_all(&inbox);
        egress::deliver(&sig, &inbox);
    }

    let replayed = pending_live::replay_all();
    let mut agents: Vec<_> = replayed.iter().map(|(n, _)| n.clone()).collect();
    agents.sort();
    // Post-0.37.0: wake provider enqueues under canonical
    // `<workspace_id>:<agent>` keys. replay_all reads dir names off
    // disk; dir names are pending_live::sanitize()'d so `:` becomes
    // `_`. So replay surfaces the SANITIZED form. Pre-0.37.0 these
    // were bare ['a', 'b'].
    assert_eq!(
        agents,
        vec!["unregistered-replay-ws_a".to_string(), "unregistered-replay-ws_b".to_string()]
    );
    // Each has 1 signal.
    for (_, sigs) in &replayed {
        assert_eq!(sigs.len(), 1);
    }
}

// ─────────────────────────────────────────────────────────────────────
// 0.37.0 canonical-key keying — wake provider enqueues under
// `<workspace_id>:<agent>` and v2 spawn drains the same key. This
// is the alignment fix that closes the cross-restart durability
// gap: signals queued while a workspace's agent is offline land in
// the right session when it comes back, even if the daemon
// restarted in between.
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn canonical_key_wake_enqueue_drains_on_v2_spawn() {
    let _g = lock();
    init_for_tests();
    // Deliberately UNregistered so the auto-launch fallback errors
    // out (no project path → can't load any launch profile). The
    // signal stays queued, ready for the manual drain below to
    // exercise the canonical-key alignment.
    providers::register_all();
    v2_session_map::clear_for_tests();
    let _queue = isolate_pending_root("canonical-key-drain");

    // Step 1: send a Live signal to an offline target. The wake
    // provider should enqueue under the canonical key
    // `<workspace_id>:<agent>` (post-0.37.0 alignment with the
    // v2 spawn helper's drain key). Pre-0.37.0 the provider used
    // bare `agent` and the v2 drain used the prefixed form — they
    // never met. This test pins the alignment.
    let signal = AgentSignal::new(
        AgentAddress::Agent {
            workspace: WorkspaceId("canonical-ws".into()),
            name: "sender".into(),
        },
        AgentAddress::Agent {
            workspace: WorkspaceId("canonical-ws".into()),
            name: "canonical-target".into(),
        },
        SignalKind::Msg {
            text: "canonical-key-probe".into(),
        },
    )
    .with_delivery(Delivery::Live);

    let inbox_root = std::env::temp_dir().join("k2so-canonical-drain-inbox");
    let _ = std::fs::create_dir_all(&inbox_root);
    egress::deliver(&signal, &inbox_root);

    // Verify enqueue happened under the CANONICAL key, not bare.
    // pending_live::enqueue sanitizes `:` → `_` for filesystem
    // safety, so the on-disk dir for canonical key
    // "canonical-ws:canonical-target" is "canonical-ws_canonical-target".
    let canonical_key = "canonical-ws:canonical-target";
    let canonical_dir_name = "canonical-ws_canonical-target";
    let queue_root = pending_live::queue_root();
    let canonical_dir = queue_root.join(canonical_dir_name);
    let bare_dir = queue_root.join("canonical-target");
    assert!(
        canonical_dir.exists(),
        "queue dir for canonical key {canonical_key:?} (sanitized: {canonical_dir_name:?}) should exist"
    );
    assert!(
        !bare_dir.exists() || std::fs::read_dir(&bare_dir).map(|d| d.count()).unwrap_or(0) == 0,
        "queue dir for bare 'canonical-target' should be empty/absent — pre-0.37.0 keying regression"
    );

    // Step 2: simulate "daemon restarted, in-memory map is empty,
    // queue is intact on disk" by NOT spawning yet. Then spawn via
    // `spawn_agent_session_v2_blocking` with the workspace_id
    // attached. The helper should construct the same canonical key
    // and drain the queue.
    let outcome = spawn_agent_session_v2_blocking(SpawnWorkspaceSessionRequest {
        agent_name: "canonical-target".to_string(),
        project_id: Some("canonical-ws".to_string()),
        cwd: "/tmp".to_string(),
        command: Some("cat".to_string()),
        args: None,
        cols: 80,
        rows: 24,
    })
    .expect("v2 spawn should succeed");

    assert_eq!(
        outcome.pending_drained, 1,
        "spawn under canonical key should drain the queued signal"
    );
    assert!(!outcome.reused, "fresh spawn (map cleared above), not reused");

    // Queue dir under the canonical key is now drained.
    if canonical_dir.exists() {
        let remaining: Vec<_> = std::fs::read_dir(&canonical_dir)
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(
            remaining.is_empty(),
            "canonical-key queue dir should be empty post-drain; remaining: {remaining:?}"
        );
    }

    // Cleanup.
    tokio::time::sleep(Duration::from_millis(50)).await;
    v2_session_map::clear_for_tests();
}

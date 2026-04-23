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
    ensure_project("k2so-ws");
    providers::register_all();
    let queue_root = isolate_pending_root("offline-queue");

    let signal = AgentSignal::new(
        AgentAddress::Agent {
            workspace: WorkspaceId("k2so-ws".into()),
            name: "foo".into(),
        },
        AgentAddress::Agent {
            workspace: WorkspaceId("k2so-ws".into()),
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

    // Queue file exists.
    let agent_dir = queue_root.join("offline-target");
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
    ensure_project("k2so-ws");
    providers::register_all();
    let _queue = isolate_pending_root("spawn-drain");

    // Step 1: send a Live signal while the target is offline.
    // This queues the signal via DaemonWakeProvider.
    let signal = AgentSignal::new(
        AgentAddress::Agent {
            workspace: WorkspaceId("k2so-ws".into()),
            name: "foo".into(),
        },
        AgentAddress::Agent {
            workspace: WorkspaceId("k2so-ws".into()),
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

    // Step 2: now spawn a session for drain-target via the HTTP
    // handler. The spawn path should drain the queue and inject
    // the queued signal into the fresh PTY.
    let spawn_body = serde_json::json!({
        "agent_name": "drain-target",
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
    // Canvas Plan Phase A: `pendingDrained` now reflects how many
    // signals were QUEUED at spawn time (will be drained by the
    // async task). The actual drain runs off the hot path. Both
    // counts agree here because no new signals arrive between
    // queue + spawn.
    assert_eq!(drained, 1, "spawn should have queued 1 signal for drain");

    // Phase A: drain is async. Poll the queue directory for up to
    // 3 seconds waiting for the background drain task to complete.
    let queue_root = pending_live::queue_root();
    let agent_dir = queue_root.join("drain-target");
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    loop {
        let empty = if agent_dir.exists() {
            std::fs::read_dir(&agent_dir)
                .map(|d| d.filter_map(|r| r.ok()).next().is_none())
                .unwrap_or(true)
        } else {
            true
        };
        if empty {
            break;
        }
        if std::time::Instant::now() >= deadline {
            panic!("queue should become empty after async drain, but still contains entries after 3s");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Cleanup.
    session_map::unregister("drain-target");
}

#[tokio::test(flavor = "current_thread")]
async fn replay_all_finds_queued_entries() {
    let _g = lock();
    init_for_tests();
    ensure_project("k2so-ws");
    providers::register_all();
    let _queue = isolate_pending_root("replay-all");

    // Queue two signals for two different agents.
    for (agent, text) in [("a", "msg-to-a"), ("b", "msg-to-b")] {
        let sig = AgentSignal::new(
            AgentAddress::Agent {
                workspace: WorkspaceId("k2so-ws".into()),
                name: "sender".into(),
            },
            AgentAddress::Agent {
                workspace: WorkspaceId("k2so-ws".into()),
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
    assert_eq!(agents, vec!["a".to_string(), "b".to_string()]);
    // Each has 1 signal.
    for (_, sigs) in &replayed {
        assert_eq!(sigs.len(), 1);
    }
}

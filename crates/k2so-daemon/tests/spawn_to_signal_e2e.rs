//! F2 end-to-end integration — the full loop from
//! `POST /cli/sessions/spawn` to `k2so signal` to the target's
//! live PTY.
//!
//! Proves that once a session is spawned via HTTP, it's discoverable
//! by the InjectProvider under its agent name, so a signal
//! published via egress reaches its PTY.

#![cfg(unix)]

use std::sync::Mutex as StdMutex;
use std::time::Duration;

use k2so_core::awareness::{
    egress, AgentAddress, AgentSignal, Delivery, SignalKind, WorkspaceId,
};
use k2so_core::db::init_for_tests;

use k2so_daemon::awareness_ws;
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

#[tokio::test(flavor = "current_thread")]
async fn spawn_via_http_then_signal_reaches_target_pty() {
    let _g = lock();
    init_for_tests();
    ensure_project("k2so-ws");
    providers::register_all();

    // 0.37.0: post-canonicalization, awareness bus lookups for
    // workspace-addressed signals key on `<workspace>:<agent>`. The
    // legacy `/cli/sessions/spawn` (Kessel-T0) path doesn't auto-
    // canonicalize — it registers verbatim under whatever
    // `agent_name` is passed. So the test passes the canonical key
    // form directly to keep the e2e signal-to-PTY path coherent.
    let canonical_key = "k2so-ws:bar";
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
    assert_eq!(spawn_result.status, "200 OK", "spawn body: {}", spawn_result.body);
    let spawn_resp: serde_json::Value =
        serde_json::from_str(&spawn_result.body).expect("spawn response is JSON");
    let agent_name = spawn_resp
        .get("agentName")
        .and_then(|v| v.as_str())
        .expect("agentName in response");
    assert_eq!(agent_name, canonical_key);

    // session_map now has the canonical key.
    assert!(
        session_map::lookup(canonical_key).is_some(),
        "spawn should have registered {canonical_key} in session_map"
    );

    // Reader thread needs a moment to attach.
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Send a Live signal to bar via egress — this goes through
    // DaemonInjectProvider → session_map::lookup → session.write.
    let signal = AgentSignal::new(
        AgentAddress::Agent {
            workspace: WorkspaceId("k2so-ws".into()),
            name: "foo".into(),
        },
        AgentAddress::Agent {
            workspace: WorkspaceId("k2so-ws".into()),
            name: "bar".into(),
        },
        SignalKind::Msg {
            text: "f2-http-spawn-probe".into(),
        },
    )
    .with_delivery(Delivery::Live);

    let inbox_root = std::env::temp_dir().join("k2so-f2-inbox");
    let _ = std::fs::create_dir_all(&inbox_root);
    let report = egress::deliver(&signal, &inbox_root);

    // The whole point: inject succeeded because the daemon spawn
    // populated session_map, which DaemonInjectProvider looked up.
    assert!(
        report.injected_to_pty,
        "expected injected_to_pty=true; got {report:?}"
    );

    // Audit row captured the full signal (F1.5 regression).
    let db = k2so_core::db::shared();
    let conn = db.lock();
    let metadata: String = conn
        .query_row(
            "SELECT metadata FROM activity_feed WHERE id = ?1",
            rusqlite::params![report.activity_feed_row_id],
            |row| row.get(0),
        )
        .expect("activity_feed row");
    let decoded: AgentSignal =
        serde_json::from_str(&metadata).expect("metadata is AgentSignal JSON");
    assert_eq!(decoded.id, signal.id);

    // Cleanup.
    let _ = session_map::unregister(canonical_key);
}

#[tokio::test(flavor = "current_thread")]
async fn spawn_with_missing_agent_name_returns_400() {
    let _g = lock();
    let body = r#"{"cwd":"/tmp"}"#; // missing agent_name
    let result = awareness_ws::handle_sessions_spawn(body.as_bytes()).await;
    assert_eq!(result.status, "400 Bad Request");
    assert!(result.body.contains("agent_name"));
}

#[tokio::test(flavor = "current_thread")]
async fn spawn_with_bad_json_returns_400() {
    let _g = lock();
    let body = b"not json at all";
    let result = awareness_ws::handle_sessions_spawn(body).await;
    assert_eq!(result.status, "400 Bad Request");
    assert!(result.body.contains("parse"));
}

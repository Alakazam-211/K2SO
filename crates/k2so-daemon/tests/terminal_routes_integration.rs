//! H1 of Phase 4 integration tests — daemon-side
//! `/cli/terminal/read` + `/cli/terminal/write` against
//! daemon-owned sessions.
//!
//! Both handlers are pure fns that take a query-params HashMap
//! and return a `CliResponse`, so tests drive them directly (no
//! HTTP plumbing). The integration points we care about:
//!
//! - session::registry lookup by SessionId
//! - session_map lookup by SessionId (for write)
//! - Frame::Text decode → line splitting → last-N selection
//! - `session.write(bytes)` reach through the map
//! - Error paths (missing id, invalid UUID, unknown session)

#![cfg(unix)]

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use k2so_core::session::{registry, Frame, SessionId};
use k2so_core::terminal::{spawn_session_stream, SpawnConfig};

use k2so_daemon::session_map;
use k2so_daemon::terminal_routes;

/// Serialize the tests — all touch session_map + session::registry
/// singletons.
static TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn spawn_cat_session(
    agent: &str,
) -> (SessionId, Arc<k2so_core::terminal::SessionStreamSession>) {
    let id = SessionId::new();
    let session = spawn_session_stream(SpawnConfig {
        session_id: id,
        cwd: "/tmp".into(),
        command: Some("cat".into()),
        args: None,
        cols: 80,
        rows: 24,
    })
    .expect("spawn cat");
    let arc = Arc::new(session);
    // Tag agent_name so liveness lookups would find it (not strictly
    // needed for these H1 tests but mirrors real spawn behavior).
    if let Some(entry) = registry::lookup(&id) {
        entry.set_agent_name(agent);
    }
    session_map::register(agent, arc.clone());
    (id, arc)
}

fn params(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

fn extract_lines(json: &str) -> Vec<String> {
    let v: serde_json::Value = serde_json::from_str(json).unwrap();
    v["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s.as_str().unwrap().to_string())
        .collect()
}

// ─────────────────────────────────────────────────────────────────────
// read: happy path
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn read_returns_last_n_lines_of_frame_text() {
    let _g = lock();
    let agent = "read-happy";
    let (id, session) = spawn_cat_session(agent);

    // Publish a few text frames directly into the session's entry
    // (bypasses the PTY). Simulates what the dual-emit reader
    // would do after cat echoed something.
    let entry = registry::lookup(&id).expect("registered");
    for i in 0..7 {
        entry.publish(Frame::Text {
            bytes: format!("line-{i}\n").into_bytes(),
            style: None,
        });
    }

    let p = params(&[("id", &id.to_string()), ("lines", "3")]);
    let resp = terminal_routes::handle_read(&p);
    assert_eq!(resp.status, "200 OK");
    let lines = extract_lines(&resp.body);
    assert_eq!(lines, vec!["line-4".to_string(), "line-5".to_string(), "line-6".to_string()]);

    let _ = session.kill();
    session_map::unregister(agent);
}

#[tokio::test(flavor = "current_thread")]
async fn read_handles_no_lines_param_default_50() {
    let _g = lock();
    let agent = "read-default";
    let (id, session) = spawn_cat_session(agent);

    let entry = registry::lookup(&id).expect("registered");
    for i in 0..10 {
        entry.publish(Frame::Text {
            bytes: format!("row-{i}\n").into_bytes(),
            style: None,
        });
    }

    let p = params(&[("id", &id.to_string())]);
    let resp = terminal_routes::handle_read(&p);
    let lines = extract_lines(&resp.body);
    // 10 < default 50; all returned.
    assert_eq!(lines.len(), 10);
    assert_eq!(lines[0], "row-0");
    assert_eq!(lines[9], "row-9");

    let _ = session.kill();
    session_map::unregister(agent);
}

#[tokio::test(flavor = "current_thread")]
async fn read_ignores_non_text_frames() {
    let _g = lock();
    let agent = "read-mixed";
    let (id, session) = spawn_cat_session(agent);

    let entry = registry::lookup(&id).expect("registered");
    entry.publish(Frame::Text {
        bytes: b"hello\n".to_vec(),
        style: None,
    });
    entry.publish(Frame::SemanticEvent {
        kind: k2so_core::session::SemanticKind::ToolCall,
        payload: serde_json::json!({"name": "bash"}),
    });
    entry.publish(Frame::Text {
        bytes: b"world\n".to_vec(),
        style: None,
    });

    let resp = terminal_routes::handle_read(&params(&[("id", &id.to_string())]));
    let lines = extract_lines(&resp.body);
    assert_eq!(lines, vec!["hello".to_string(), "world".to_string()]);

    let _ = session.kill();
    session_map::unregister(agent);
}

#[tokio::test(flavor = "current_thread")]
async fn read_strips_crlf_and_preserves_partial_lines() {
    let _g = lock();
    let agent = "read-crlf";
    let (id, session) = spawn_cat_session(agent);

    let entry = registry::lookup(&id).expect("registered");
    entry.publish(Frame::Text {
        bytes: b"first\r\nsecond\r\nthird-in-progress".to_vec(),
        style: None,
    });

    let resp = terminal_routes::handle_read(&params(&[("id", &id.to_string())]));
    let lines = extract_lines(&resp.body);
    assert_eq!(
        lines,
        vec![
            "first".to_string(),
            "second".to_string(),
            "third-in-progress".to_string(),
        ]
    );

    let _ = session.kill();
    session_map::unregister(agent);
}

// ─────────────────────────────────────────────────────────────────────
// read: error paths
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn read_missing_id_is_400() {
    let _g = lock();
    let resp = terminal_routes::handle_read(&params(&[]));
    assert_eq!(resp.status, "400 Bad Request");
    assert!(resp.body.contains("missing id"));
}

#[tokio::test(flavor = "current_thread")]
async fn read_invalid_uuid_is_400() {
    let _g = lock();
    let resp = terminal_routes::handle_read(&params(&[("id", "not-a-uuid")]));
    assert_eq!(resp.status, "400 Bad Request");
    assert!(resp.body.contains("invalid session id"));
}

#[tokio::test(flavor = "current_thread")]
async fn read_unknown_session_is_400() {
    let _g = lock();
    let random = SessionId::new().to_string();
    let resp = terminal_routes::handle_read(&params(&[("id", &random)]));
    assert_eq!(resp.status, "400 Bad Request");
    assert!(resp.body.contains("session not found"));
}

// ─────────────────────────────────────────────────────────────────────
// write: happy path
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn write_injects_bytes_into_session_pty() {
    let _g = lock();
    let agent = "write-happy";
    let (id, session) = spawn_cat_session(agent);

    // no_submit=true so the follow-up \r doesn't fire (keeps the
    // test deterministic — we don't need to wait the 150 ms).
    let p = params(&[
        ("id", &id.to_string()),
        ("message", "hello-from-write"),
        ("no_submit", "true"),
    ]);
    let resp = terminal_routes::handle_write(&p);
    assert_eq!(resp.status, "200 OK");
    assert!(resp.body.contains("success"));

    // Let cat loop the bytes back — they go into the session's
    // Frame stream via the dual-emit reader.
    tokio::time::sleep(Duration::from_millis(100)).await;

    let entry = registry::lookup(&id).expect("registered");
    let frames = entry.replay_snapshot();
    let mut seen_bytes = Vec::<u8>::new();
    for f in frames {
        if let Frame::Text { bytes, .. } = f {
            seen_bytes.extend(bytes);
        }
    }
    let s = String::from_utf8_lossy(&seen_bytes);
    assert!(
        s.contains("hello-from-write"),
        "injected bytes should appear in the session's Frame stream, got: {s:?}"
    );

    let _ = session.kill();
    session_map::unregister(agent);
}

#[tokio::test(flavor = "current_thread")]
async fn write_missing_message_is_400() {
    let _g = lock();
    let agent = "write-miss-msg";
    let (id, session) = spawn_cat_session(agent);

    let resp = terminal_routes::handle_write(&params(&[("id", &id.to_string())]));
    assert_eq!(resp.status, "400 Bad Request");
    assert!(resp.body.contains("missing message"));

    let _ = session.kill();
    session_map::unregister(agent);
}

#[tokio::test(flavor = "current_thread")]
async fn write_unknown_session_is_400() {
    let _g = lock();
    let random = SessionId::new().to_string();
    let resp = terminal_routes::handle_write(&params(&[
        ("id", &random),
        ("message", "x"),
        ("no_submit", "true"),
    ]));
    assert_eq!(resp.status, "400 Bad Request");
    assert!(resp.body.contains("session not found"));
}

#[tokio::test(flavor = "current_thread")]
async fn write_invalid_uuid_is_400() {
    let _g = lock();
    let resp = terminal_routes::handle_write(&params(&[
        ("id", "not-a-uuid"),
        ("message", "x"),
    ]));
    assert_eq!(resp.status, "400 Bad Request");
    assert!(resp.body.contains("invalid session id"));
}

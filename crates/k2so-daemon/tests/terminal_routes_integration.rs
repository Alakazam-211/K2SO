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

use k2so_daemon::session_lookup;
use k2so_daemon::session_map;
use k2so_daemon::terminal_routes;
use k2so_daemon::v2_session_map;

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
        track_alacritty_term: false,
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

// ─────────────────────────────────────────────────────────────────────
// H2 — /cli/agents/running enumerates daemon session_map
// ─────────────────────────────────────────────────────────────────────

fn drop_all_sessions(agents: &[&str]) {
    for a in agents {
        if let Some(sess) = session_map::unregister(a) {
            let _ = sess.kill();
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn agents_running_returns_each_live_session() {
    let _g = lock();
    // Clean slate — prior tests may have leaked entries in the
    // shared singleton session_map.
    for name in ["running-alpha", "running-beta", "running-gamma"] {
        let _ = session_map::unregister(name);
    }

    let (id_a, _sess_a) = spawn_cat_session("running-alpha");
    let (id_b, _sess_b) = spawn_cat_session("running-beta");

    let resp = terminal_routes::handle_agents_running(&params(&[]));
    assert_eq!(resp.status, "200 OK");
    let arr: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
    let items = arr.as_array().expect("response is an array");
    // Filter down to the two we care about — other tests can leak.
    let mine: Vec<_> = items
        .iter()
        .filter(|v| {
            let agent = v["agentName"].as_str().unwrap_or("");
            agent == "running-alpha" || agent == "running-beta"
        })
        .collect();
    assert_eq!(
        mine.len(),
        2,
        "expected 2 matching entries: {}",
        serde_json::to_string_pretty(items).unwrap_or_default()
    );

    for v in &mine {
        let agent = v["agentName"].as_str().unwrap();
        let tid = v["terminalId"].as_str().unwrap();
        let expected_id = if agent == "running-alpha" {
            id_a.to_string()
        } else {
            id_b.to_string()
        };
        assert_eq!(tid, expected_id);
        assert_eq!(v["cwd"].as_str(), Some("/tmp"));
        assert_eq!(v["command"].as_str(), Some("cat"));
        // idleMs is a non-negative integer.
        let idle = v["idleMs"].as_u64().expect("idleMs present");
        let _ = idle; // >= 0 by u64 type
        // subscriberCount: 0 is fine (no WS client attached in
        // this test; daemon's own reader thread subscribes to the
        // broadcast channel only via the broadcast::Receiver which
        // is a send side).
        v["subscriberCount"].as_u64().expect("subscriberCount present");
    }

    drop_all_sessions(&["running-alpha", "running-beta"]);
}

// ─────────────────────────────────────────────────────────────────────
// H3 — /cli/terminal/spawn + /cli/terminal/spawn-background
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn terminal_spawn_requires_agent_param() {
    let _g = lock();
    let resp = terminal_routes::handle_terminal_spawn(
        &params(&[("command", "cat")]),
        "/tmp/test-project",
    );
    assert_eq!(resp.status, "400 Bad Request");
    assert!(resp.body.contains("missing agent"));
}

#[tokio::test(flavor = "current_thread")]
async fn terminal_spawn_creates_session_and_registers_in_map() {
    let _g = lock();
    let agent = "h3-spawn-test";
    let _ = v2_session_map::unregister(agent);
    let _ = session_map::unregister(agent);

    let resp = terminal_routes::handle_terminal_spawn(
        &params(&[
            ("agent", agent),
            ("command", "cat"),
            ("cwd", "/tmp"),
        ]),
        "/tmp/test-project",
    );
    assert_eq!(resp.status, "200 OK");
    let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
    assert_eq!(v["success"], serde_json::json!(true));
    assert_eq!(v["agentName"].as_str(), Some(agent));
    let terminal_id = v["terminalId"].as_str().unwrap();
    assert!(!terminal_id.is_empty());

    // Post-A9, /cli/terminal/spawn produces a v2 session — assert via
    // the unified lookup so the test is renderer-agnostic.
    let session = session_lookup::lookup_any(agent).expect("registered");
    assert_eq!(session.session_id().to_string(), terminal_id);
    assert_eq!(session.cwd(), "/tmp");
    assert_eq!(session.command().as_deref(), Some("cat"));

    let _ = v2_session_map::unregister(agent);
    let _ = session_map::unregister(agent);
}

#[tokio::test(flavor = "current_thread")]
async fn terminal_spawn_background_allows_missing_agent() {
    let _g = lock();
    let resp = terminal_routes::handle_terminal_spawn_background(
        &params(&[("command", "cat"), ("cwd", "/tmp")]),
        "/tmp/test-project",
    );
    assert_eq!(resp.status, "200 OK");
    let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
    let agent_name = v["agentName"].as_str().unwrap();
    assert!(
        agent_name.starts_with("terminal-"),
        "agent_name should be synthesized, got: {agent_name}"
    );

    // Synthesized session must be addressable via lookup_any (post-A9
    // the spawn produces a v2 session in v2_session_map).
    assert!(
        session_lookup::lookup_any(agent_name).is_some(),
        "synthesized agent_name {agent_name} not registered"
    );
    let _ = v2_session_map::unregister(agent_name);
    let _ = session_map::unregister(agent_name);
}

#[tokio::test(flavor = "current_thread")]
async fn terminal_spawn_applies_default_cwd_from_project() {
    let _g = lock();
    let agent = "h3-cwd-default";
    let _ = v2_session_map::unregister(agent);
    let _ = session_map::unregister(agent);

    // Create a real directory so `resolve_cwd` (which falls back
    // to $HOME for missing paths) actually uses our project path.
    let project_dir = std::env::temp_dir().join(format!(
        "k2so-h3-cwd-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&project_dir).unwrap();
    let project_str = project_dir.to_string_lossy().into_owned();

    let resp = terminal_routes::handle_terminal_spawn(
        &params(&[("agent", agent), ("command", "cat")]),
        &project_str,
    );
    assert_eq!(resp.status, "200 OK");
    let session = session_lookup::lookup_any(agent).expect("registered");
    assert_eq!(
        session.cwd(),
        project_str,
        "cwd should default to project_path when cwd param is absent"
    );

    let _ = v2_session_map::unregister(agent);
    let _ = session_map::unregister(agent);
    let _ = std::fs::remove_dir_all(&project_dir);
}

#[tokio::test(flavor = "current_thread")]
async fn agents_running_returns_empty_array_when_no_sessions() {
    let _g = lock();
    // Drain any leftover registrations. `snapshot()` is authoritative;
    // registry may still have entries for just-dropped sessions that
    // haven't unregistered yet, but session_map is what this endpoint
    // reads.
    let pre = session_map::snapshot();
    for (name, sess) in pre {
        let _ = sess.kill();
        session_map::unregister(&name);
    }

    let resp = terminal_routes::handle_agents_running(&params(&[]));
    assert_eq!(resp.status, "200 OK");
    let arr: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
    assert_eq!(
        arr.as_array().unwrap().len(),
        0,
        "empty session_map should yield empty JSON array"
    );
}

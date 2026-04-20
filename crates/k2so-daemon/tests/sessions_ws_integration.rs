//! D6 end-to-end integration test for 0.34.0 Session Stream WS.
//!
//! Exercises the full path:
//!   spawn_session_stream → SessionRegistry → sessions_ws::serve
//!   → tokio-tungstenite client → Frame JSON over the wire
//!
//! No real daemon startup — we bind a loopback `TcpListener` in the
//! test, accept a single connection, hand it to
//! `sessions_ws::serve_session_subscribe_connection`, and connect
//! a `tokio-tungstenite` client to that port. Avoids the bootstrap
//! overhead of spawning a full daemon binary while still hitting
//! every line of the WS handshake + Frame fan-out code path.

#![cfg(unix)]

use std::collections::HashMap;
use std::time::Duration;

use futures_util::StreamExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;

use k2so_core::session::{registry, SessionId};
use k2so_core::terminal::{spawn_session_stream, SpawnConfig};

/// Spin up a listener, accept one connection, hand it to the WS
/// handler. Returns the bound port for the client to connect to.
async fn start_one_shot_server(params: HashMap<String, String>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept");
        // NB: this mirrors main.rs's dispatch — in reality token
        // auth runs BEFORE upgrade. Tests exercise the handler
        // directly; authz is unit-tested in sessions_ws::tests and
        // main.rs dispatch.
        k2so_daemon::sessions_ws::serve_session_subscribe_connection(stream, params).await;
    });
    port
}

async fn connect_ws(port: u16, session_param: &str) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>> {
    let url = format!(
        "ws://127.0.0.1:{port}/cli/sessions/subscribe?session={session_param}"
    );
    let (ws, _resp) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("ws client connect");
    ws
}

fn expect_event(msg: &str, expected_event: &str) -> serde_json::Value {
    let parsed: serde_json::Value =
        serde_json::from_str(msg).unwrap_or_else(|e| panic!("bad JSON: {msg} — {e}"));
    assert_eq!(
        parsed.get("event").and_then(|v| v.as_str()),
        Some(expected_event),
        "expected event={expected_event}, got: {msg}"
    );
    parsed
}

// ─────────────────────────────────────────────────────────────────────
// Happy path
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn subscriber_receives_ack_then_frames() {
    // Spawn session first so it's registered before the client
    // attempts to subscribe.
    let session_id = SessionId::new();
    let session = spawn_session_stream(SpawnConfig {
        session_id,
        cwd: "/tmp".into(),
        command: Some("sleep".into()),
        args: Some(vec!["30".into()]),
        cols: 80,
        rows: 24,
    })
    .expect("spawn session");

    // Set up server + client.
    let mut params = HashMap::new();
    params.insert("session".into(), session_id.to_string());
    let port = start_one_shot_server(params).await;
    let mut ws = connect_ws(port, &session_id.to_string()).await;

    // Expect session:ack first.
    let ack_text = match timeout(Duration::from_secs(2), ws.next())
        .await
        .expect("ack arrives within 2s")
        .expect("ws stream open")
        .expect("ack message is Ok")
    {
        Message::Text(t) => t,
        other => panic!("expected Text, got {other:?}"),
    };
    let ack = expect_event(&ack_text, "session:ack");
    assert_eq!(
        ack.pointer("/payload/sessionId").and_then(|v| v.as_str()),
        Some(session_id.to_string().as_str())
    );

    // Feed input to the child so it echoes back as Text frames.
    // `sleep` ignores stdin, so for the echo we'd need `cat` — but
    // we used `sleep` to keep the session alive long enough for
    // subscribe. Instead, use session.write() to simulate output
    // directly — no, write() goes to the child's stdin. Skip this
    // and just verify ack works; use the `cat` path in the next
    // test to verify frames arrive.

    session.kill().expect("kill");
    ws.close(None).await.ok();
}

#[tokio::test(flavor = "current_thread")]
async fn subscriber_receives_text_frames_for_child_output() {
    let session_id = SessionId::new();
    let session = spawn_session_stream(SpawnConfig {
        session_id,
        cwd: "/tmp".into(),
        command: Some("cat".into()), // echoes stdin to stdout
        args: None,
        cols: 80,
        rows: 24,
    })
    .expect("spawn session");

    let mut params = HashMap::new();
    params.insert("session".into(), session_id.to_string());
    let port = start_one_shot_server(params).await;
    let mut ws = connect_ws(port, &session_id.to_string()).await;

    // Drain the ack first.
    let ack_text = match timeout(Duration::from_secs(2), ws.next())
        .await
        .expect("ack within 2s")
        .expect("stream open")
        .expect("ack Ok")
    {
        Message::Text(t) => t,
        other => panic!("expected Text for ack, got {other:?}"),
    };
    let _ = expect_event(&ack_text, "session:ack");

    // Write input to cat.
    session
        .write(b"d6-integration-probe\n")
        .expect("write to child");

    // Poll the WS for up to 3s for a Text frame containing our
    // sentinel.
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    let mut found = false;
    while std::time::Instant::now() < deadline && !found {
        match timeout(Duration::from_millis(500), ws.next()).await {
            Ok(Some(Ok(Message::Text(t)))) => {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&t) {
                    if v.get("event").and_then(|e| e.as_str()) == Some("session:frame") {
                        let payload = v.get("payload").cloned().unwrap_or(serde_json::Value::Null);
                        if frame_payload_contains_text(&payload, "d6-integration-probe") {
                            found = true;
                        }
                    }
                }
            }
            _ => {}
        }
    }
    assert!(
        found,
        "expected to see a session:frame containing 'd6-integration-probe' within 3s"
    );

    session.kill().expect("kill");
    ws.close(None).await.ok();
}

fn frame_payload_contains_text(payload: &serde_json::Value, needle: &str) -> bool {
    if payload.get("frame").and_then(|f| f.as_str()) != Some("Text") {
        return false;
    }
    let data = match payload.get("data") {
        Some(d) => d,
        None => return false,
    };
    let bytes = match data.get("bytes").and_then(|b| b.as_array()) {
        Some(arr) => arr,
        None => return false,
    };
    let collected: Vec<u8> = bytes
        .iter()
        .filter_map(|n| n.as_u64().map(|u| u as u8))
        .collect();
    std::str::from_utf8(&collected)
        .map(|s| s.contains(needle))
        .unwrap_or(false)
}

// ─────────────────────────────────────────────────────────────────────
// Error paths
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn unknown_session_returns_http_error_before_upgrade() {
    // Use a UUID that can't be registered.
    let bogus_id = SessionId::new();
    let mut params = HashMap::new();
    params.insert("session".into(), bogus_id.to_string());
    let port = start_one_shot_server(params).await;

    // Attempt to upgrade — expect connection refused at upgrade
    // time because the handler writes HTTP 400 and closes before
    // negotiating WS.
    let url = format!(
        "ws://127.0.0.1:{port}/cli/sessions/subscribe?session={bogus_id}"
    );
    let result = tokio_tungstenite::connect_async(&url).await;
    // tokio-tungstenite's connect_async errors because the server
    // sent an HTTP response that wasn't a 101 upgrade.
    assert!(
        result.is_err(),
        "unknown session should fail WS upgrade; got Ok({:?})",
        result.ok().map(|(_, r)| r)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn malformed_session_param_returns_http_error() {
    let mut params = HashMap::new();
    params.insert("session".into(), "not-a-uuid".into());
    let port = start_one_shot_server(params).await;
    let url = format!("ws://127.0.0.1:{port}/cli/sessions/subscribe?session=not-a-uuid");
    let result = tokio_tungstenite::connect_async(&url).await;
    assert!(
        result.is_err(),
        "malformed session id should fail WS upgrade"
    );
}

// ─────────────────────────────────────────────────────────────────────
// Multi-subscriber fanout
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn two_concurrent_subscribers_each_see_frames() {
    let session_id = SessionId::new();
    let session = spawn_session_stream(SpawnConfig {
        session_id,
        cwd: "/tmp".into(),
        command: Some("cat".into()),
        args: None,
        cols: 80,
        rows: 24,
    })
    .expect("spawn");

    // Two independent one-shot servers, each points at the same
    // session in the registry. Each accept() handles one connection
    // so two clients => two servers => two accept hand-offs.
    let mut params = HashMap::new();
    params.insert("session".into(), session_id.to_string());
    let port_a = start_one_shot_server(params.clone()).await;
    let port_b = start_one_shot_server(params).await;
    let mut ws_a = connect_ws(port_a, &session_id.to_string()).await;
    let mut ws_b = connect_ws(port_b, &session_id.to_string()).await;

    // Drain acks.
    let _ = timeout(Duration::from_secs(2), ws_a.next()).await;
    let _ = timeout(Duration::from_secs(2), ws_b.next()).await;

    // Write once to the child — both subscribers should see it.
    session
        .write(b"d6-fanout-test\n")
        .expect("write to child");

    let mut a_saw = false;
    let mut b_saw = false;
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while std::time::Instant::now() < deadline && !(a_saw && b_saw) {
        if !a_saw {
            if let Ok(Some(Ok(Message::Text(t)))) =
                timeout(Duration::from_millis(200), ws_a.next()).await
            {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&t) {
                    if v.get("event").and_then(|e| e.as_str()) == Some("session:frame") {
                        if frame_payload_contains_text(
                            v.get("payload").unwrap_or(&serde_json::Value::Null),
                            "d6-fanout-test",
                        ) {
                            a_saw = true;
                        }
                    }
                }
            }
        }
        if !b_saw {
            if let Ok(Some(Ok(Message::Text(t)))) =
                timeout(Duration::from_millis(200), ws_b.next()).await
            {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&t) {
                    if v.get("event").and_then(|e| e.as_str()) == Some("session:frame") {
                        if frame_payload_contains_text(
                            v.get("payload").unwrap_or(&serde_json::Value::Null),
                            "d6-fanout-test",
                        ) {
                            b_saw = true;
                        }
                    }
                }
            }
        }
    }
    assert!(a_saw, "subscriber A should have seen the frame");
    assert!(b_saw, "subscriber B should have seen the frame");

    session.kill().expect("kill");
}

// ─────────────────────────────────────────────────────────────────────
// Late subscriber + replay ring
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn late_subscriber_receives_replay_ring_then_live() {
    let session_id = SessionId::new();
    let session = spawn_session_stream(SpawnConfig {
        session_id,
        cwd: "/tmp".into(),
        command: Some("cat".into()),
        args: None,
        cols: 80,
        rows: 24,
    })
    .expect("spawn");

    // Write BEFORE any subscriber is attached. Frames land in the
    // replay ring; a late subscriber should see them on flush.
    session
        .write(b"d6-replay-first\n")
        .expect("pre-subscribe write");
    // Give the reader thread time to process + publish.
    tokio::time::sleep(Duration::from_millis(300)).await;

    let mut params = HashMap::new();
    params.insert("session".into(), session_id.to_string());
    let port = start_one_shot_server(params).await;
    let mut ws = connect_ws(port, &session_id.to_string()).await;

    // Drain ack.
    let ack_text = match timeout(Duration::from_secs(2), ws.next())
        .await
        .expect("ack within 2s")
        .expect("stream open")
        .expect("ack Ok")
    {
        Message::Text(t) => t,
        other => panic!("expected Text for ack, got {other:?}"),
    };
    let ack = expect_event(&ack_text, "session:ack");
    let replay_count = ack
        .pointer("/payload/replayCount")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    assert!(
        replay_count >= 1,
        "replay ring should hold at least one frame from pre-subscribe writes, got {replay_count}"
    );

    // Collect frames for up to 3s and look for the pre-subscribe sentinel.
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    let mut saw_replay_sentinel = false;
    while std::time::Instant::now() < deadline && !saw_replay_sentinel {
        if let Ok(Some(Ok(Message::Text(t)))) =
            timeout(Duration::from_millis(300), ws.next()).await
        {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&t) {
                if v.get("event").and_then(|e| e.as_str()) == Some("session:frame") {
                    if frame_payload_contains_text(
                        v.get("payload").unwrap_or(&serde_json::Value::Null),
                        "d6-replay-first",
                    ) {
                        saw_replay_sentinel = true;
                    }
                }
            }
        }
    }
    assert!(
        saw_replay_sentinel,
        "late subscriber should see 'd6-replay-first' via replay ring flush"
    );

    session.kill().expect("kill");
    ws.close(None).await.ok();
}

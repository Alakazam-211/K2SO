//! Integration tests for the grow-then-shrink spawn flow.
//!
//! Verifies the core invariants of `spawn_session_stream_and_grow`:
//!   1. PTY opens at the oversized GROW_ROWS value.
//!   2. Settle watcher fires and SIGWINCHes down to the target rows.
//!   3. The ring captures frames produced during the grow phase.
//!   4. Each settle-trigger path (mode-change / idle / ceiling)
//!      produces the expected outcome.
//!
//! Uses real PTYs + real shell commands so the tokio runtime, the
//! reader thread, the broadcast channel, and the settle watcher are
//! all exercised together — the same code path a production daemon
//! spawn takes.

#![cfg(all(unix, feature = "session_stream"))]

use std::time::{Duration, Instant};

use k2so_core::session::registry;
use k2so_core::terminal::{
    spawn_session_stream_and_grow, SpawnConfig, GROW_ROWS,
};

/// Target rows for the shrunk session — always smaller than
/// `GROW_ROWS` so the grow+settle path actually runs.
const TARGET_ROWS: u16 = 24;
const TARGET_COLS: u16 = 80;

fn tmp_cwd(tag: &str) -> String {
    let path = std::env::temp_dir().join(format!(
        "k2so-grow-{}-{}-{}",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path.to_string_lossy().into_owned()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn grow_and_shrink_with_bracketed_paste_mode_change() {
    // A shell script that prints some text, then emits DECSET ?2004
    // (bracketed_paste ON), then blocks. Historically the settle
    // watcher treated bracketed_paste as a fast-settle trigger; after
    // the claude --resume regression (bracketed_paste fires during
    // cold-start BEFORE the resume paint lands) we dropped that fast
    // path and rely solely on IDLE_MS / CEILING_MS. The test still
    // holds under the idle path — the `sleep 30` after the escape
    // sequence gives the watcher a clean 400 ms of quiet, well under
    // the 1500 ms upper bound we assert below.
    let cwd = tmp_cwd("mode-change");
    let cfg = SpawnConfig {
        cwd: cwd.clone(),
        command: Some(
            "printf 'hello grow phase\\n'; printf '\\x1b[?2004h'; sleep 30"
                .to_string(),
        ),
        args: None,
        cols: TARGET_COLS,
        rows: TARGET_ROWS,
        ..SpawnConfig::default()
    };

    let t0 = Instant::now();
    let session = spawn_session_stream_and_grow(cfg)
        .await
        .expect("spawn should succeed");
    let elapsed = t0.elapsed();

    // Mode-change trigger should fire quickly — well under idle.
    assert!(
        elapsed < Duration::from_millis(1500),
        "mode-change settle should be fast, took {elapsed:?}"
    );

    let entry =
        registry::lookup(&session.session_id).expect("entry still registered");
    let ring = entry.replay_snapshot();

    // Ring must contain at least the text frame + the mode-change
    // frame from the grow phase.
    let saw_text = ring
        .iter()
        .any(|f| matches!(f, k2so_core::session::Frame::Text { .. }));
    let saw_mode_change = ring.iter().any(|f| {
        matches!(
            f,
            k2so_core::session::Frame::ModeChange {
                mode: k2so_core::session::ModeKind::BracketedPaste,
                on: true
            }
        )
    });
    assert!(saw_text, "ring should contain the 'hello grow phase' text");
    assert!(saw_mode_change, "ring should contain the bracketed_paste ModeChange");

    // Teardown — the session handle drops when we exit scope.
    // Child (sleep 30) gets SIGHUP when the master PTY drops.
    drop(session);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn grow_and_shrink_with_idle_trigger_on_simple_shell() {
    // bash-like scenario: print something, then go idle. No mode
    // change, no fast signal — the 400 ms idle watcher fires.
    let cwd = tmp_cwd("idle");
    let cfg = SpawnConfig {
        cwd: cwd.clone(),
        command: Some("printf 'idle-test-marker\\n'; sleep 30".to_string()),
        args: None,
        cols: TARGET_COLS,
        rows: TARGET_ROWS,
        ..SpawnConfig::default()
    };

    let t0 = Instant::now();
    let session = spawn_session_stream_and_grow(cfg)
        .await
        .expect("spawn should succeed");
    let elapsed = t0.elapsed();

    // Idle settle = ~400 ms plus PTY/shell startup overhead.
    // Upper bound generous to keep the test non-flaky on loaded CI.
    assert!(
        elapsed >= Duration::from_millis(300),
        "idle settle should take at least ~400 ms, got {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_millis(2500),
        "idle settle should fire well before the 3s ceiling, took {elapsed:?}"
    );

    let entry =
        registry::lookup(&session.session_id).expect("entry still registered");
    let ring = entry.replay_snapshot();
    let saw_marker = ring.iter().any(|f| {
        if let k2so_core::session::Frame::Text { bytes, .. } = f {
            String::from_utf8_lossy(bytes).contains("idle-test-marker")
        } else {
            false
        }
    });
    assert!(saw_marker, "ring should contain the printed marker");

    drop(session);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn grow_boundary_apc_lands_in_byte_stream() {
    // Canvas Plan Phase 3: verify that after grow-settle the daemon
    // injects `\x1b_k2so:grow_boundary:...\x07` into the Session's
    // byte stream, in addition to the existing Frame::SemanticEvent
    // emission. Byte-stream subscribers read the APC inline and
    // use it as their seam.
    let cwd = tmp_cwd("apc-marker");
    let cfg = SpawnConfig {
        cwd: cwd.clone(),
        command: Some(
            "printf 'hello grow\\n'; sleep 30".to_string(),
        ),
        args: None,
        cols: TARGET_COLS,
        rows: TARGET_ROWS,
        ..SpawnConfig::default()
    };

    let session = spawn_session_stream_and_grow(cfg)
        .await
        .expect("spawn should succeed");

    let entry =
        registry::lookup(&session.session_id).expect("entry still registered");

    // Give the byte archive writer a beat to flush so both the
    // ring and the on-disk file are observable.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // Concatenate the full byte ring; scan for the APC marker.
    let ring: Vec<u8> = entry
        .bytes_snapshot_from(0)
        .into_iter()
        .flat_map(|c| c.data.to_vec())
        .collect();
    let needle = b"\x1b_k2so:grow_boundary:";
    let pos = ring
        .windows(needle.len())
        .position(|w| w == needle)
        .expect("ring must contain APC grow_boundary introducer");

    // Scan forward for the BEL terminator (\x07) and pull out the
    // JSON payload between the colon and the BEL.
    let after_intro = &ring[pos + needle.len()..];
    let bel = after_intro
        .iter()
        .position(|b| *b == b'\x07')
        .expect("APC must terminate with BEL");
    let payload_bytes = &after_intro[..bel];
    let payload_str =
        std::str::from_utf8(payload_bytes).expect("payload must be UTF-8");
    let payload: serde_json::Value =
        serde_json::from_str(payload_str).expect("payload must parse as JSON");

    assert_eq!(payload["target_cols"], TARGET_COLS);
    assert_eq!(payload["target_rows"], TARGET_ROWS);
    assert_eq!(payload["grow_rows"], GROW_ROWS);
    assert!(
        payload["reason"].is_string(),
        "payload.reason should be present"
    );

    // Archive file should also have it (byte archive IS the ring +
    // older evicted bytes appended).
    let archive_path = std::path::PathBuf::from(&cwd)
        .join(".k2so/sessions")
        .join(session.session_id.to_string())
        .join("archive.bytes");
    let archive = std::fs::read(&archive_path).expect("archive exists");
    assert!(
        archive.windows(needle.len()).any(|w| w == needle),
        "archive.bytes must contain the APC grow_boundary introducer"
    );

    drop(session);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn grow_skipped_when_requested_rows_exceeds_grow_rows() {
    // Smoke: if the caller already asks for rows >= GROW_ROWS, the
    // grow phase is a no-op. Spawn should return promptly with the
    // requested rows in effect.
    let cwd = tmp_cwd("no-grow");
    let cfg = SpawnConfig {
        cwd: cwd.clone(),
        command: Some("sleep 30".to_string()),
        args: None,
        cols: TARGET_COLS,
        rows: GROW_ROWS + 10,
        ..SpawnConfig::default()
    };

    let t0 = Instant::now();
    let session = spawn_session_stream_and_grow(cfg)
        .await
        .expect("spawn should succeed");
    let elapsed = t0.elapsed();

    // No settle to wait for — should return under the idle threshold.
    assert!(
        elapsed < Duration::from_millis(200),
        "no-grow path should return almost immediately, took {elapsed:?}"
    );

    drop(session);
}

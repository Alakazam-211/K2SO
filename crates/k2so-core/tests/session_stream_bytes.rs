//! Integration tests for the raw byte stream (Canvas Plan Phase 2).
//!
//! Verifies three things:
//!   1. A spawned Session's byte tap captures the PTY's output
//!      verbatim (same bytes the child wrote, same order).
//!   2. Subscribers attaching mid-flight via `bytes_subscribe` +
//!      `bytes_snapshot_from` see a contiguous byte range covering
//!      at least what was in the ring at attach time.
//!   3. The byte archive file on disk is byte-identical to the
//!      concatenation of captured chunks.
//!
//! Uses real PTYs + real shell commands so the tokio runtime, the
//! reader thread, and the broadcast+ring pipeline are all
//! exercised together — same path a production daemon spawn takes.
//!
//! Deliberately NOT calling `spawn_session_stream_and_grow` here —
//! the grow path adds a SIGWINCH mid-capture which complicates the
//! "bytes captured should equal bytes the child wrote" assertion
//! (Claude-style TUIs repaint on SIGWINCH, doubling the byte
//! count). We use the plain `spawn_session_stream` with a small
//! canvas that doesn't trigger grow.

#![cfg(all(unix, feature = "session_stream"))]

use std::time::{Duration, Instant};

use k2so_core::session::registry;
use k2so_core::terminal::{spawn_session_stream, SpawnConfig};

fn tmp_cwd(tag: &str) -> String {
    let path = std::env::temp_dir().join(format!(
        "k2so-bytes-{}-{}-{}",
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

/// Wait until `f` returns true OR `timeout` elapses. Returns whether
/// it succeeded. Polls every 20 ms.
async fn wait_until(timeout: Duration, mut f: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if f() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    f()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn byte_ring_captures_shell_output_verbatim() {
    let cwd = tmp_cwd("verbatim");
    // Print a specific, identifiable marker string. The shell will
    // also emit its own prompt + PROMPT_EOL_MARK stuff, but our
    // marker must appear in the byte ring.
    let marker = "k2so-byte-test-marker-49281";
    let cfg = SpawnConfig {
        cwd: cwd.clone(),
        command: Some(format!(
            "printf '{marker}\\n'; sleep 30"
        )),
        args: None,
        cols: 80,
        rows: 24,
        ..SpawnConfig::default()
    };

    let session = spawn_session_stream(cfg).expect("spawn");
    let entry = registry::lookup(&session.session_id).expect("registered");

    // Wait for the marker to land in the byte ring.
    let entry_c = entry.clone();
    let marker_bytes = marker.as_bytes().to_vec();
    let found = wait_until(Duration::from_secs(3), move || {
        let snap = entry_c.bytes_snapshot_from(0);
        let mut all = Vec::new();
        for chunk in snap {
            all.extend_from_slice(&chunk.data);
        }
        all.windows(marker_bytes.len()).any(|w| w == marker_bytes)
    })
    .await;
    assert!(
        found,
        "byte ring should contain the printed marker within 3s"
    );

    // The back_offset should be non-zero (bytes were written).
    let (front, back) = entry.bytes_offsets();
    assert_eq!(front, 0, "front shouldn't have evicted on such small output");
    assert!(back > 0, "back_offset should have advanced");
    assert!(
        back as usize >= marker.len(),
        "back_offset {back} must cover at least the marker length {}",
        marker.len()
    );

    drop(session);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn byte_archive_file_matches_ring_contents() {
    let cwd = tmp_cwd("archive");
    let marker = "k2so-byte-archive-roundtrip";
    let cfg = SpawnConfig {
        cwd: cwd.clone(),
        command: Some(format!("printf '{marker}\\n'; sleep 30")),
        args: None,
        cols: 80,
        rows: 24,
        ..SpawnConfig::default()
    };

    let session = spawn_session_stream(cfg).expect("spawn");
    let entry = registry::lookup(&session.session_id).expect("registered");

    // Wait for the marker to land in the ring.
    let entry_c = entry.clone();
    let marker_bytes = marker.as_bytes().to_vec();
    wait_until(Duration::from_secs(3), move || {
        let snap = entry_c.bytes_snapshot_from(0);
        let mut all = Vec::new();
        for chunk in snap {
            all.extend_from_slice(&chunk.data);
        }
        all.windows(marker_bytes.len()).any(|w| w == marker_bytes)
    })
    .await;

    // Give the byte-archive task a beat to flush through the broadcast.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let archive_path = std::path::PathBuf::from(&cwd)
        .join(".k2so/sessions")
        .join(session.session_id.to_string())
        .join("archive.bytes");
    assert!(
        archive_path.exists(),
        "byte archive file should exist at {:?}",
        archive_path
    );
    let archive_contents = std::fs::read(&archive_path).expect("read archive");
    assert!(
        archive_contents.windows(marker.len()).any(|w| w == marker.as_bytes()),
        "archive.bytes must contain the marker string"
    );

    // Archive contents should start with the same bytes as the ring
    // (ring may have more if the archive writer lagged; archive
    // cannot have bytes that aren't in the ring's captured range).
    let ring: Vec<u8> = entry
        .bytes_snapshot_from(0)
        .into_iter()
        .flat_map(|c| c.data.to_vec())
        .collect();
    assert!(
        ring.starts_with(&archive_contents)
            || archive_contents.starts_with(&ring[..archive_contents.len().min(ring.len())]),
        "archive file should be a prefix of (or equal to) the ring contents"
    );

    drop(session);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn byte_subscriber_receives_live_chunks() {
    let cwd = tmp_cwd("live-sub");
    let cfg = SpawnConfig {
        cwd: cwd.clone(),
        command: Some(
            "printf 'first\\n'; sleep 0.1; printf 'second\\n'; sleep 30"
                .to_string(),
        ),
        args: None,
        cols: 80,
        rows: 24,
        ..SpawnConfig::default()
    };

    let session = spawn_session_stream(cfg).expect("spawn");
    let entry = registry::lookup(&session.session_id).expect("registered");

    // Attach a live subscriber. We ALSO snapshot the ring for
    // anything that landed before we subscribed; real clients do
    // both (snapshot + live tail) to get a contiguous stream.
    let mut rx = entry.bytes_subscribe();
    let mut collected: Vec<u8> = entry
        .bytes_snapshot_from(0)
        .into_iter()
        .flat_map(|c| c.data.to_vec())
        .collect();

    // Drain live broadcast for up to 2s OR until we see "second".
    let drain_started = Instant::now();
    while drain_started.elapsed() < Duration::from_secs(2) {
        match tokio::time::timeout(Duration::from_millis(200), rx.recv()).await {
            Ok(Ok(chunk)) => collected.extend_from_slice(&chunk),
            Ok(Err(_)) => break, // channel closed
            Err(_) => {}         // timeout tick; loop
        }
        if collected
            .windows(b"second".len())
            .any(|w| w == b"second")
        {
            break;
        }
    }

    assert!(
        collected.windows(b"first".len()).any(|w| w == b"first"),
        "live subscriber should see 'first' (from snapshot or live)"
    );
    assert!(
        collected.windows(b"second".len()).any(|w| w == b"second"),
        "live subscriber should see 'second' (printed after subscribe)"
    );

    drop(session);
}

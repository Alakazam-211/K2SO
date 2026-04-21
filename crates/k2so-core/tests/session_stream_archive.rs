//! E6 tests for `session::archive` — per-session NDJSON writer.
//!
//! Covers:
//!   - Spawn → publish N frames → exit → N lines in archive.ndjson
//!     each deserializes back to the original frame
//!   - File path shape: `<project>/.k2so/sessions/<id>/archive.ndjson`
//!   - Writer exits naturally when SessionEntry's sender drops
//!   - File appended, not truncated (two runs on the same path
//!     coexist)

#![cfg(feature = "session_stream")]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use k2so_core::session::{
    self, archive, Frame, SemanticKind, SessionEntry, SessionId,
};

fn tmp_project(tag: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "k2so-archive-test-{}-{}-{}",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn text_frame(s: &str) -> Frame {
    Frame::Text {
        bytes: s.as_bytes().to_vec(),
        style: None,
    }
}

fn read_archive_lines(path: &PathBuf) -> Vec<String> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    text.lines().map(str::to_string).collect()
}

/// Decode a Frame::Text line and return its UTF-8 body. `None`
/// for other frame types. Frame::Text serializes `bytes` as a
/// JSON array of integers, so the substring `"first-run-1"`
/// doesn't appear literally in the archive line — we have to
/// deserialize and decode to check the content.
fn text_frame_body(line: &str) -> Option<String> {
    let frame: Frame = serde_json::from_str(line).ok()?;
    match frame {
        Frame::Text { bytes, .. } => String::from_utf8(bytes).ok(),
        _ => None,
    }
}

#[tokio::test(flavor = "current_thread")]
async fn spawn_and_publish_creates_ndjson_with_one_line_per_frame() {
    let session_id = SessionId::new();
    let project = tmp_project("basic");
    let entry = Arc::new(SessionEntry::new());

    let handle = archive::spawn(session_id, entry.clone(), project.clone());
    // Yield to let the writer open its file before we publish.
    tokio::time::sleep(Duration::from_millis(50)).await;

    for i in 0..5 {
        entry.publish(text_frame(&format!("line-{i}")));
    }

    // Close the sender by dropping all SessionEntry Arcs.
    drop(entry);

    // Writer exits on RecvError::Closed. Give it a deadline.
    let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;

    let archive_path = project
        .join(".k2so/sessions")
        .join(session_id.to_string())
        .join("archive.ndjson");
    assert!(archive_path.exists(), "archive not created: {archive_path:?}");

    let lines = read_archive_lines(&archive_path);
    assert_eq!(lines.len(), 5, "expected 5 lines, got {}: {lines:?}", lines.len());

    // Each line round-trips back to its Frame.
    for (i, line) in lines.iter().enumerate() {
        let frame: Frame = serde_json::from_str(line)
            .unwrap_or_else(|e| panic!("line {i} bad JSON: {e}\n{line}"));
        match frame {
            Frame::Text { bytes, .. } => {
                let s = String::from_utf8(bytes).unwrap();
                assert_eq!(s, format!("line-{i}"));
            }
            other => panic!("line {i} wrong variant: {other:?}"),
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn archive_survives_across_multiple_writer_lifetimes() {
    // Two separate (session_id, entry) pairs writing to the SAME
    // project_root should each produce their own archive file
    // under their own session_id dir — no cross-contamination.
    let project = tmp_project("multi");
    let id_a = SessionId::new();
    let id_b = SessionId::new();
    let entry_a = Arc::new(SessionEntry::new());
    let entry_b = Arc::new(SessionEntry::new());

    let ha = archive::spawn(id_a, entry_a.clone(), project.clone());
    let hb = archive::spawn(id_b, entry_b.clone(), project.clone());
    tokio::time::sleep(Duration::from_millis(50)).await;

    entry_a.publish(text_frame("from-a-1"));
    entry_a.publish(text_frame("from-a-2"));
    entry_b.publish(text_frame("from-b-1"));

    drop(entry_a);
    drop(entry_b);

    let _ = tokio::time::timeout(Duration::from_secs(2), ha).await;
    let _ = tokio::time::timeout(Duration::from_secs(2), hb).await;

    let a_lines = read_archive_lines(
        &project.join(".k2so/sessions").join(id_a.to_string()).join("archive.ndjson"),
    );
    let b_lines = read_archive_lines(
        &project.join(".k2so/sessions").join(id_b.to_string()).join("archive.ndjson"),
    );
    assert_eq!(a_lines.len(), 2);
    assert_eq!(b_lines.len(), 1);
    assert_eq!(text_frame_body(&a_lines[0]).as_deref(), Some("from-a-1"));
    assert_eq!(text_frame_body(&a_lines[1]).as_deref(), Some("from-a-2"));
    assert_eq!(text_frame_body(&b_lines[0]).as_deref(), Some("from-b-1"));
}

#[tokio::test(flavor = "current_thread")]
async fn archive_appends_across_two_runs_on_same_session_id() {
    // If the daemon restarts mid-session (signal-kill + respawn),
    // the second writer should APPEND to the existing file, not
    // clobber it.
    let project = tmp_project("append");
    let session_id = SessionId::new();

    // First writer's lifetime.
    {
        let entry = Arc::new(SessionEntry::new());
        let handle = archive::spawn(session_id, entry.clone(), project.clone());
        tokio::time::sleep(Duration::from_millis(50)).await;
        entry.publish(text_frame("first-run-1"));
        entry.publish(text_frame("first-run-2"));
        drop(entry);
        let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
    }

    // Second writer, same session_id.
    {
        let entry = Arc::new(SessionEntry::new());
        let handle = archive::spawn(session_id, entry.clone(), project.clone());
        tokio::time::sleep(Duration::from_millis(50)).await;
        entry.publish(text_frame("second-run-1"));
        drop(entry);
        let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
    }

    let archive_path = project
        .join(".k2so/sessions")
        .join(session_id.to_string())
        .join("archive.ndjson");
    let lines = read_archive_lines(&archive_path);
    assert_eq!(lines.len(), 3, "expected 3 total lines across 2 runs: {lines:?}");
    assert_eq!(text_frame_body(&lines[0]).as_deref(), Some("first-run-1"));
    assert_eq!(text_frame_body(&lines[1]).as_deref(), Some("first-run-2"));
    assert_eq!(text_frame_body(&lines[2]).as_deref(), Some("second-run-1"));
}

#[tokio::test(flavor = "current_thread")]
async fn semantic_event_frames_round_trip_through_archive() {
    let project = tmp_project("semantic");
    let session_id = SessionId::new();
    let entry = Arc::new(SessionEntry::new());

    let handle = archive::spawn(session_id, entry.clone(), project.clone());
    tokio::time::sleep(Duration::from_millis(50)).await;

    let semantic = Frame::SemanticEvent {
        kind: SemanticKind::ToolCall,
        payload: serde_json::json!({ "name": "bash", "id": "t_1" }),
    };
    entry.publish(semantic.clone());

    let cursor_op = Frame::CursorOp(k2so_core::session::CursorOp::ClearScreen);
    entry.publish(cursor_op.clone());

    drop(entry);
    let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;

    let path = project
        .join(".k2so/sessions")
        .join(session_id.to_string())
        .join("archive.ndjson");
    let lines = read_archive_lines(&path);
    assert_eq!(lines.len(), 2);

    let decoded_1: Frame = serde_json::from_str(&lines[0]).unwrap();
    let decoded_2: Frame = serde_json::from_str(&lines[1]).unwrap();
    assert_eq!(decoded_1, semantic);
    assert_eq!(decoded_2, cursor_op);
}

#[tokio::test(flavor = "current_thread")]
async fn empty_session_produces_empty_archive_file() {
    // Edge case: a session that registers and immediately drops
    // should produce an empty archive file (the writer ran, opened
    // the file, saw no frames, observed the sender drop, exited).
    let project = tmp_project("empty");
    let session_id = SessionId::new();
    let entry = Arc::new(SessionEntry::new());

    let handle = archive::spawn(session_id, entry.clone(), project.clone());
    tokio::time::sleep(Duration::from_millis(50)).await;

    drop(entry);
    let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;

    let path = project
        .join(".k2so/sessions")
        .join(session_id.to_string())
        .join("archive.ndjson");
    assert!(path.exists(), "even empty sessions should create the file");
    let bytes = std::fs::metadata(&path).unwrap().len();
    assert_eq!(bytes, 0, "empty session should produce zero bytes");
}

#[test]
fn byte_thresholds_match_expected_values() {
    // Lock the MVP thresholds so Phase 3.2 has to bump them
    // deliberately. WARN at 100MB, hard limit at 500MB.
    assert_eq!(session::WARN_BYTES, 100 * 1024 * 1024);
    assert_eq!(session::HARD_LIMIT_BYTES, 500 * 1024 * 1024);
}

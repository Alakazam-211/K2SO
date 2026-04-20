//! C4 line-mux tests: `LineMux::feed()` on synthetic PTY byte chunks.
//!
//! Recorded Claude Code fixtures land alongside C6 when the recognizer
//! needs real data; Phase 1 sticks to hand-crafted byte sequences so
//! the tests can be read end-to-end without inspecting a `.bin` blob.

#![cfg(feature = "session_stream")]

use k2so_core::session::{CursorOp, EraseMode, Frame};
use k2so_core::term::LineMux;

fn text_frame_bytes(frame: &Frame) -> Option<&[u8]> {
    match frame {
        Frame::Text { bytes, .. } => Some(bytes),
        _ => None,
    }
}

fn cursor_op(frame: &Frame) -> Option<&CursorOp> {
    match frame {
        Frame::CursorOp(op) => Some(op),
        _ => None,
    }
}

#[test]
fn two_lines_committed_and_two_text_frames_emitted() {
    let mut mux = LineMux::new();
    let frames = mux.feed(b"hello\nworld\n");

    // Scrollback has 2 committed lines.
    let lines: Vec<_> = mux.lines().collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].text, "hello");
    assert_eq!(lines[1].text, "world");
    assert!(lines[0].seqno < lines[1].seqno);

    // Two Text frames — one per run of printable chars before each
    // newline. No CursorOp emitted for LF in Phase 1.
    let text_frames: Vec<_> = frames.iter().filter_map(text_frame_bytes).collect();
    assert_eq!(text_frames.len(), 2);
    assert_eq!(text_frames[0], b"hello");
    assert_eq!(text_frames[1], b"world");

    // The current (unfinished) line is empty since the chunk ended on LF.
    assert!(mux.current_line_text().is_none());
}

#[test]
fn clear_screen_emits_one_cursor_op() {
    let mut mux = LineMux::new();
    let frames = mux.feed(b"\x1b[2J");
    assert_eq!(frames.len(), 1);
    assert!(matches!(
        cursor_op(&frames[0]),
        Some(CursorOp::ClearScreen)
    ));
}

#[test]
fn cup_goto_parses_row_col() {
    let mut mux = LineMux::new();
    let frames = mux.feed(b"\x1b[10;20H");
    assert_eq!(frames.len(), 1);
    assert!(matches!(
        cursor_op(&frames[0]),
        Some(CursorOp::Goto { row: 10, col: 20 })
    ));
}

#[test]
fn cup_goto_defaults_when_params_omitted() {
    let mut mux = LineMux::new();
    let frames = mux.feed(b"\x1b[H");
    assert_eq!(frames.len(), 1);
    assert!(matches!(
        cursor_op(&frames[0]),
        Some(CursorOp::Goto { row: 1, col: 1 })
    ));
}

#[test]
fn cursor_movement_variants() {
    let cases: [(&[u8], fn(&CursorOp) -> bool); 4] = [
        (b"\x1b[3A", |op| matches!(op, CursorOp::Up(3))),
        (b"\x1b[5B", |op| matches!(op, CursorOp::Down(5))),
        (b"\x1b[7C", |op| matches!(op, CursorOp::Forward(7))),
        (b"\x1b[9D", |op| matches!(op, CursorOp::Back(9))),
    ];
    for (input, check) in cases {
        let mut mux = LineMux::new();
        let frames = mux.feed(input);
        assert_eq!(frames.len(), 1);
        assert!(check(cursor_op(&frames[0]).unwrap()));
    }
}

#[test]
fn erase_in_line_modes() {
    let mut mux = LineMux::new();
    // EL 0 (to end), EL 1 (from start), EL 2 (all).
    let frames = mux.feed(b"\x1b[0K\x1b[1K\x1b[2K");
    let ops: Vec<_> = frames.iter().filter_map(cursor_op).collect();
    assert_eq!(ops.len(), 3);
    assert!(matches!(
        ops[0],
        CursorOp::EraseInLine(EraseMode::ToEnd)
    ));
    assert!(matches!(
        ops[1],
        CursorOp::EraseInLine(EraseMode::FromStart)
    ));
    assert!(matches!(
        ops[2],
        CursorOp::EraseInLine(EraseMode::All)
    ));
}

#[test]
fn backspace_strips_last_char_from_current_line() {
    let mut mux = LineMux::new();
    let _ = mux.feed(b"hello\x08\x08\x08");
    assert_eq!(mux.current_line_text(), Some("he"));
}

#[test]
fn unterminated_line_visible_via_current_line_text() {
    let mut mux = LineMux::new();
    let frames = mux.feed(b"partial");
    // No line committed yet — chunk didn't end with LF.
    assert_eq!(mux.line_count(), 0);
    assert_eq!(mux.current_line_text(), Some("partial"));
    // One Text frame was flushed at chunk end.
    let text_frames: Vec<_> = frames.iter().filter_map(text_frame_bytes).collect();
    assert_eq!(text_frames.len(), 1);
    assert_eq!(text_frames[0], b"partial");
}

#[test]
fn mixed_text_and_control_preserves_ordering() {
    let mut mux = LineMux::new();
    let frames = mux.feed(b"pre\x1b[2Jpost\n");
    // Frames should be: Text("pre"), CursorOp(ClearScreen), Text("post")
    // (terminal line committed + Text frame flushed before LF).
    assert!(frames.len() >= 3);
    assert_eq!(text_frame_bytes(&frames[0]), Some(&b"pre"[..]));
    assert!(matches!(
        cursor_op(&frames[1]),
        Some(CursorOp::ClearScreen)
    ));
    assert_eq!(text_frame_bytes(&frames[2]), Some(&b"post"[..]));

    let lines: Vec<_> = mux.lines().collect();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].text, "prepost");
}

#[test]
fn split_feed_does_not_lose_bytes() {
    // Two `feed()` calls, each a partial CSI sequence — vte's Parser
    // is stateful, so the mux should still emit the one expected
    // CursorOp once the sequence completes.
    let mut mux = LineMux::new();
    let first = mux.feed(b"\x1b[");
    assert_eq!(first.len(), 0, "incomplete CSI should emit nothing");
    let second = mux.feed(b"2J");
    assert_eq!(second.len(), 1);
    assert!(matches!(
        cursor_op(&second[0]),
        Some(CursorOp::ClearScreen)
    ));
}

#[test]
fn scrollback_cap_trims_oldest_lines() {
    let mut mux = LineMux::with_cap(3);
    let _ = mux.feed(b"a\nb\nc\nd\ne\n");
    // Cap is 3 — only last 3 lines remain.
    let lines: Vec<_> = mux.lines().map(|l| l.text.clone()).collect();
    assert_eq!(lines, vec!["c".to_string(), "d".to_string(), "e".to_string()]);
    // Seqnos keep growing; current_seqno reflects how many lines
    // have been committed + the one being built.
    assert!(mux.current_seqno() >= 5);
}

#[test]
fn seqno_is_monotonic_across_line_commits() {
    let mut mux = LineMux::new();
    let _ = mux.feed(b"alpha\nbeta\ngamma\n");
    let lines: Vec<_> = mux.lines().collect();
    assert_eq!(lines.len(), 3);
    assert!(lines[0].seqno < lines[1].seqno);
    assert!(lines[1].seqno < lines[2].seqno);
}

#[test]
fn empty_feed_is_a_noop() {
    let mut mux = LineMux::new();
    let frames = mux.feed(b"");
    assert!(frames.is_empty());
    assert_eq!(mux.line_count(), 0);
    assert!(mux.current_line_text().is_none());
}

#[test]
#[should_panic(expected = "LineMux cap must be >= 1")]
fn zero_cap_panics() {
    let _ = LineMux::with_cap(0);
}

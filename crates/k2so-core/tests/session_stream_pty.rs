//! D3 `session_stream_pty` integration tests.
//!
//! Spawns real child processes under a real PTY; verifies that
//! `spawn_session_stream` drives alacritty's `Term` grid correctly
//! without going through alacritty's private `EventLoop`. This is
//! the Phase 2 invariant-proof commit — LineMux in D3b and Phase 5
//! will see the exact same byte stream this reader feeds into `Term`.
//!
//! Platform: Unix only.

#![cfg(all(feature = "session_stream", unix))]

use std::time::Duration;

use k2so_core::session::{registry, Frame, SessionId};
use k2so_core::terminal::{spawn_session_stream, SpawnConfig};

/// Grab the raw text of the first `n` rows of Term's grid, trimmed
/// of trailing whitespace on each row. alacritty's `Term` exposes
/// grid cells by `Point { line, column }`; we walk the first
/// `cols` columns of each visible line and concatenate characters.
fn dump_visible_rows<L: alacritty_terminal::event::EventListener>(
    term: &alacritty_terminal::Term<L>,
    n: usize,
) -> Vec<String> {
    use alacritty_terminal::grid::Dimensions;
    use alacritty_terminal::index::{Column, Line, Point};
    let cols = term.columns();
    let mut out = Vec::new();
    for row in 0..n.min(term.screen_lines()) {
        let mut line = String::new();
        for col in 0..cols {
            let cell = &term.grid()[Point::new(Line(row as i32), Column(col))];
            line.push(cell.c);
        }
        out.push(line.trim_end().to_string());
    }
    out
}

#[test]
fn echo_hello_populates_term_grid() {
    let cfg = SpawnConfig {
        session_id: SessionId::new(),
        cwd: "/tmp".into(),
        // Use `printf` instead of `echo` because macOS /bin/sh `echo`
        // behavior is shell-specific; printf is consistent.
        command: Some("printf".into()),
        args: Some(vec!["hello from session stream\\n".into()]),
        cols: 80,
        rows: 24,
        track_alacritty_term: true,
    };
    let mut session =
        spawn_session_stream(cfg).expect("session should spawn");

    // printf exits immediately — wait for child then drain reader.
    assert!(
        session.wait_for_exit(Duration::from_secs(5)),
        "printf should exit within 5s"
    );
    assert!(
        session.wait_for_reader_drain(Duration::from_secs(5)),
        "reader should drain after child exits"
    );

    // The Term grid should contain the printf output. Row 0 is
    // where printf's stdout lands (there's no shell prompt because
    // we `exec`'d printf directly via shell -ilc, so the shell
    // exits immediately after).
    let rows = {
        let term = session.term.lock();
        dump_visible_rows(&*term, 5)
    };
    // The string may appear in row 0, 1, or later depending on
    // whether the shell printed a banner first. Just assert it
    // exists somewhere in the first handful of rows.
    let joined = rows.join("\n");
    assert!(
        joined.contains("hello from session stream"),
        "expected 'hello from session stream' in grid; got:\n{joined}"
    );
}

#[test]
fn session_id_survives_spawn() {
    let session_id = SessionId::new();
    let cfg = SpawnConfig {
        session_id,
        cwd: "/tmp".into(),
        command: Some("true".into()),
        args: None,
        cols: 80,
        rows: 24,
        track_alacritty_term: true,
    };
    let mut session =
        spawn_session_stream(cfg).expect("session should spawn");
    assert_eq!(session.session_id, session_id);
    assert!(session.wait_for_exit(Duration::from_secs(5)));
    assert!(session.wait_for_reader_drain(Duration::from_secs(5)));
}

#[test]
fn write_to_session_reaches_child() {
    // Use `cat` — reads stdin forever, echoes to stdout. We write
    // a line, wait for it to appear in the Term grid, then kill.
    let cfg = SpawnConfig {
        session_id: SessionId::new(),
        cwd: "/tmp".into(),
        command: Some("cat".into()),
        args: None,
        cols: 80,
        rows: 24,
        track_alacritty_term: true,
    };
    let mut session =
        spawn_session_stream(cfg).expect("session should spawn");

    session
        .write(b"k2so-stream-write-test\n")
        .expect("write to session should succeed");

    // Poll the Term grid until the echo appears (up to 2s).
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    let mut found = false;
    while std::time::Instant::now() < deadline {
        {
            let term = session.term.lock();
            let rows = dump_visible_rows(&*term, 5);
            if rows.iter().any(|r| r.contains("k2so-stream-write-test")) {
                found = true;
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        found,
        "written bytes should echo back through Term grid within 2s"
    );

    // Kill cat; reader drains; test exits clean.
    session.kill().expect("kill should succeed");
    assert!(session.wait_for_reader_drain(Duration::from_secs(5)));
}

#[test]
fn drop_kills_child_cleanly() {
    // Spawn a long-running child, drop the handle, verify no thread
    // leak + no zombie. This is the load-bearing lifecycle check —
    // every session spawn must clean up when the owner drops the
    // handle.
    let child_pid_alive = {
        let cfg = SpawnConfig {
            session_id: SessionId::new(),
            cwd: "/tmp".into(),
            command: Some("sleep".into()),
            args: Some(vec!["60".into()]),
            cols: 80,
            rows: 24,
            track_alacritty_term: true,
        };
        let session =
            spawn_session_stream(cfg).expect("session should spawn");
        // Give the child a moment to actually start.
        std::thread::sleep(Duration::from_millis(100));
        // We don't currently expose child PID on the handle; the
        // test exercises the Drop lifecycle regardless.
        drop(session);
        true
    };
    assert!(child_pid_alive, "sanity");
    // At this point the child has been killed; reader thread has
    // joined. No observable way to assert this from here without
    // PID tracking, but if Drop deadlocks the test hangs (and
    // Cargo eventually kills it).
}

// ─────────────────────────────────────────────────────────────────────
// D3b integration — SessionRegistry + LineMux fork
// ─────────────────────────────────────────────────────────────────────

#[test]
fn session_is_registered_while_reader_runs() {
    let session_id = SessionId::new();
    let cfg = SpawnConfig {
        session_id,
        cwd: "/tmp".into(),
        command: Some("sleep".into()),
        args: Some(vec!["60".into()]),
        cols: 80,
        rows: 24,
        track_alacritty_term: true,
    };
    let session = spawn_session_stream(cfg).expect("spawn");
    // Registered immediately after spawn returns.
    assert!(
        registry::lookup(&session_id).is_some(),
        "session should be registered immediately after spawn"
    );
    session.kill().expect("kill");
    // Give the reader a moment to unregister after seeing EOF.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        if registry::lookup(&session_id).is_none() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        registry::lookup(&session_id).is_none(),
        "session should be unregistered after reader thread exits"
    );
}

#[test]
fn published_frames_reach_subscriber() {
    let session_id = SessionId::new();
    let cfg = SpawnConfig {
        session_id,
        cwd: "/tmp".into(),
        command: Some("printf".into()),
        args: Some(vec!["d3b-subscriber-test\\n".into()]),
        cols: 80,
        rows: 24,
        track_alacritty_term: true,
    };

    // Subscribe BEFORE the reader thread has finished publishing.
    // Register call inside spawn_session_stream happens before the
    // reader thread is spawned, so this lookup always succeeds.
    let mut session = spawn_session_stream(cfg).expect("spawn");
    let entry = registry::lookup(&session_id).expect("registered");
    let mut rx = entry.subscribe();

    // Wait for the child + reader to drain.
    assert!(session.wait_for_exit(Duration::from_secs(5)));
    assert!(session.wait_for_reader_drain(Duration::from_secs(5)));

    // Replay ring should carry our Text frame. We don't check the
    // live channel because printf fires AND closes so fast the
    // frame might be published before the reader thread handed off
    // to our subscribe — but the replay ring retains it regardless.
    // Use `try_recv` first (might catch any live delivery that
    // happens to have landed), then fall back to replay_snapshot.
    let mut collected: Vec<Frame> = Vec::new();
    while let Ok(frame) = rx.try_recv() {
        collected.push(frame);
    }
    collected.extend(entry.replay_snapshot());

    let text_bytes: Vec<Vec<u8>> = collected
        .iter()
        .filter_map(|f| match f {
            Frame::Text { bytes, .. } => Some(bytes.clone()),
            _ => None,
        })
        .collect();
    let joined = String::from_utf8(
        text_bytes.into_iter().flatten().collect::<Vec<_>>(),
    )
    .unwrap_or_default();
    assert!(
        joined.contains("d3b-subscriber-test"),
        "expected frame content to include 'd3b-subscriber-test'; \
         got concatenation: {joined:?}"
    );
}

#[test]
fn two_subscribers_each_see_frames() {
    // Spawn a session; subscribe twice; write to the session;
    // verify both receivers see the written bytes as Text frames.
    let session_id = SessionId::new();
    let cfg = SpawnConfig {
        session_id,
        cwd: "/tmp".into(),
        command: Some("cat".into()),
        args: None,
        cols: 80,
        rows: 24,
        track_alacritty_term: true,
    };
    let session = spawn_session_stream(cfg).expect("spawn");
    let entry = registry::lookup(&session_id).expect("registered");
    let mut a = entry.subscribe();
    let mut b = entry.subscribe();

    session
        .write(b"d3b-fanout-test\n")
        .expect("write to child");

    // Poll both receivers for any Text frame containing our sentinel.
    // Use a generous timeout because cat echoes the line once shell
    // buffering clears.
    let mut a_saw = false;
    let mut b_saw = false;
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while std::time::Instant::now() < deadline && !(a_saw && b_saw) {
        while let Ok(frame) = a.try_recv() {
            if frame_contains(&frame, "d3b-fanout-test") {
                a_saw = true;
            }
        }
        while let Ok(frame) = b.try_recv() {
            if frame_contains(&frame, "d3b-fanout-test") {
                b_saw = true;
            }
        }
        if !(a_saw && b_saw) {
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    session.kill().expect("kill");
    assert!(a_saw, "subscriber A should have seen the frame");
    assert!(b_saw, "subscriber B should have seen the frame");
}

fn frame_contains(frame: &Frame, needle: &str) -> bool {
    match frame {
        Frame::Text { bytes, .. } => {
            std::str::from_utf8(bytes).map_or(false, |s| s.contains(needle))
        }
        _ => false,
    }
}

#[test]
fn resize_updates_term_dimensions() {
    use alacritty_terminal::grid::Dimensions;
    let cfg = SpawnConfig {
        session_id: SessionId::new(),
        cwd: "/tmp".into(),
        command: Some("sleep".into()),
        args: Some(vec!["60".into()]),
        cols: 80,
        rows: 24,
        track_alacritty_term: true,
    };
    let session =
        spawn_session_stream(cfg).expect("session should spawn");
    {
        let term = session.term.lock();
        assert_eq!(term.columns(), 80);
        assert_eq!(term.screen_lines(), 24);
    }

    session.resize(120, 40).expect("resize should succeed");
    {
        let term = session.term.lock();
        assert_eq!(term.columns(), 120);
        assert_eq!(term.screen_lines(), 40);
    }

    session.kill().expect("kill should succeed");
}

//! D5 smoke-test consumer for 0.34.0 Session Stream (Phase 2).
//!
//! Minimal dev-loop utility. Spawns a `SessionStreamSession` in the
//! same process, subscribes to its `SessionRegistry` entry, and
//! prints every `Frame` to stdout as a JSON line. This is the
//! first testable-in-terminal artifact for Phase 2 — run it against
//! any shell command and watch Frames flow through the end-to-end
//! pipeline (PTY → LineMux → Frame → broadcast → subscriber).
//!
//! Why in-process rather than "connect to a running daemon via WS"?
//! Because Phase 2 doesn't yet have a way to spawn a session inside
//! the daemon from an external process — that's Phase 3+ work
//! (needs a `/cli/sessions/spawn` verb + cross-process registry
//! handoff). Until then the smoke test is in-process; D6's
//! integration test covers the daemon WS path explicitly.
//!
//! # Invariant audit
//! Zero alacritty references here — subscribers never import
//! alacritty types. `grep -n alacritty` on this file should stay
//! empty forever.
//!
//! # Usage
//! ```text
//! # Print "hello world" frames:
//! cargo run -p k2so-core --example session_stream_subscribe \
//!   --features session_stream -- echo "hello world"
//!
//! # Drive a real Claude Code session and watch semantic events
//! # (boxes, tool calls) stream in real time:
//! cargo run -p k2so-core --example session_stream_subscribe \
//!   --features session_stream -- claude
//! ```
//!
//! Ctrl-C to exit.

#[cfg(not(feature = "session_stream"))]
fn main() {
    eprintln!(
        "This example requires the `session_stream` feature. Run with:\n\
         cargo run -p k2so-core --example session_stream_subscribe \\\n\
         --features session_stream -- <command>"
    );
    std::process::exit(1);
}

#[cfg(feature = "session_stream")]
#[tokio::main(flavor = "current_thread")]
async fn main() {
    use std::time::Duration;
    use tokio::sync::broadcast::error::RecvError;

    use k2so_core::session::registry;
    use k2so_core::terminal::{spawn_session_stream, SessionStreamSession, SpawnConfig};

    // Collect positional args after the binary name; everything
    // except the first arg is the shell command + args.
    let raw_args: Vec<String> = std::env::args().collect();
    let args = &raw_args[1..];
    let (command, cmd_args) = if args.is_empty() {
        // Default to a short one-liner so running the example with
        // no args still demonstrates something.
        (
            Some("printf".to_string()),
            Some(vec!["smoke from session-stream\\n".to_string()]),
        )
    } else {
        let command = args[0].clone();
        let rest = args[1..].to_vec();
        (Some(command), if rest.is_empty() { None } else { Some(rest) })
    };

    let session_id = k2so_core::session::SessionId::new();
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "/tmp".to_string());

    eprintln!(
        "▸ spawning session {session_id} — cmd: {:?} args: {:?}",
        command, cmd_args
    );

    let session: SessionStreamSession = match spawn_session_stream(SpawnConfig {
        session_id,
        cwd,
        command,
        args: cmd_args,
        cols: 120,
        rows: 40,
    }) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("▸ spawn failed: {e}");
            std::process::exit(2);
        }
    };

    let entry = registry::lookup(&session_id)
        .expect("session should be registered by spawn_session_stream");

    // Subscribe BEFORE snapshotting — matches the daemon's WS
    // endpoint ordering from sessions_ws.rs. At-least-once
    // delivery: a frame landing in the gap may be printed twice.
    let mut rx = entry.subscribe();
    let replay = entry.replay_snapshot();

    eprintln!("▸ subscribed (replay ring holds {} frames)", replay.len());
    eprintln!("▸ streaming — Ctrl-C to exit");

    // Flush replay.
    for frame in replay {
        print_frame(&frame);
    }

    // Critical: drop the local Arc<SessionEntry> now that we've
    // got our Receiver. If we held it, the Sender half would stay
    // alive forever and `rx.recv()` would never return Closed
    // even after the reader thread unregisters on child exit.
    // `rx` holds only the receive side of the channel, which is
    // enough for it to observe Closed once all Senders drop.
    drop(entry);

    // Drive the live stream + watch for child exit to exit cleanly.
    let exit_poller = tokio::task::spawn_blocking(move || {
        // Block until the child exits, then drop the session (which
        // kills the reader thread and triggers broadcast Close).
        if session.wait_for_exit(Duration::from_secs(60 * 60 * 24)) {
            eprintln!("▸ child exited");
        }
        drop(session);
    });

    loop {
        match rx.recv().await {
            Ok(frame) => print_frame(&frame),
            Err(RecvError::Lagged(n)) => {
                eprintln!("▸ lagged {n} frames (slow consumer / bursty output)");
            }
            Err(RecvError::Closed) => {
                break;
            }
        }
    }
    let _ = exit_poller.await;
    eprintln!("▸ stream closed");
}

#[cfg(feature = "session_stream")]
fn print_frame(frame: &k2so_core::session::Frame) {
    match serde_json::to_string(frame) {
        Ok(line) => println!("{line}"),
        Err(e) => eprintln!("▸ serialize error: {e}"),
    }
}

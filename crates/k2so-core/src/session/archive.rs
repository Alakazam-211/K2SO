//! Per-session NDJSON archive writer.
//!
//! E6 of Phase 3. Sessions' in-memory broadcast + replay ring
//! (Phase 2) are great for live/catch-up delivery but don't
//! survive daemon restart. This module adds the first durable
//! layer: an async tokio task per session that subscribes to the
//! session's `SessionEntry`, serializes each emitted `Frame` as
//! JSON, and appends one line per frame to
//! `<project>/.k2so/sessions/<session-id>/archive.ndjson`.
//!
//! **Decoupled from the hot path.** Writer runs as its own tokio
//! task on the runtime's thread pool. Slow disk writes can't stall
//! the PTY reader thread that's producing frames — backpressure
//! shows up as `RecvError::Lagged(n)` on this task's subscription,
//! which the writer logs and continues past.
//!
//! **Disk-growth guard (MVP).** Byte counter tracks bytes written.
//! At 100MB log a warning. At 500MB **stop writing** (hard fail-
//! open — session stays live, archive just doesn't grow any
//! further). Phase 3.2 replaces this with real rotation.
//!
//! **Lifetime.** Task exits naturally when the SessionEntry's
//! broadcast sender drops (registry unregister → entry Arc drops →
//! last sender drops → rx.recv() returns Closed). No explicit
//! shutdown signal needed.

use std::path::PathBuf;
use std::sync::Arc;

use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;
use tokio::sync::broadcast::error::RecvError;
use tokio::task::JoinHandle;

use crate::log_debug;
use crate::session::{Frame, SessionEntry, SessionId};

/// Log a warning once the archive crosses this size.
pub const WARN_BYTES: u64 = 100 * 1024 * 1024;

/// Stop writing once the archive crosses this size. Session stays
/// alive; archive just freezes. Phase 3.2 adds real rotation.
pub const HARD_LIMIT_BYTES: u64 = 500 * 1024 * 1024;

/// Spawn an archive-writer task for `session_id`. Returns the
/// `JoinHandle` so callers can either await the task's natural
/// shutdown or abort it eagerly on session teardown.
///
/// **Requires a tokio runtime.** Caller must be inside a tokio
/// context (daemon's `#[tokio::main]` runtime, or a `#[tokio::test]`).
/// If called without a runtime, this function panics — matches
/// `tokio::spawn`'s own contract.
pub fn spawn(
    session_id: SessionId,
    entry: Arc<SessionEntry>,
    project_root: PathBuf,
) -> JoinHandle<()> {
    let rx = entry.subscribe();
    tokio::spawn(async move {
        if let Err(e) = run(session_id, rx, project_root).await {
            log_debug!("[session/archive] writer exited with error: {e}");
        }
    })
}

/// Run the archive-writer loop. Public so tests can invoke it
/// directly on a specific runtime + assert on its result.
///
/// Opens the archive file in append mode (creating it + parent
/// dirs as needed), then reads frames until the broadcast channel
/// closes. Writes each frame as `<json>\n`. Tracks byte count;
/// warns at `WARN_BYTES`, stops at `HARD_LIMIT_BYTES`.
pub async fn run(
    session_id: SessionId,
    mut rx: tokio::sync::broadcast::Receiver<Frame>,
    project_root: PathBuf,
) -> std::io::Result<()> {
    let archive_dir = project_root
        .join(".k2so/sessions")
        .join(session_id.to_string());
    tokio::fs::create_dir_all(&archive_dir).await?;
    let archive_path = archive_dir.join("archive.ndjson");

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&archive_path)
        .await?;
    log_debug!(
        "[session/archive] writer started — session={} path={:?}",
        session_id,
        archive_path
    );

    let mut bytes_written: u64 = file.metadata().await?.len();
    let mut warned = bytes_written >= WARN_BYTES;
    let mut frozen = bytes_written >= HARD_LIMIT_BYTES;

    loop {
        match rx.recv().await {
            Ok(frame) => {
                if frozen {
                    continue;
                }
                let mut buf = match serde_json::to_vec(&frame) {
                    Ok(v) => v,
                    Err(e) => {
                        log_debug!(
                            "[session/archive] serialize frame failed: {e}"
                        );
                        continue;
                    }
                };
                buf.push(b'\n');
                if let Err(e) = file.write_all(&buf).await {
                    log_debug!(
                        "[session/archive] write failed: {e} — freezing archive"
                    );
                    frozen = true;
                    continue;
                }
                bytes_written += buf.len() as u64;
                if !warned && bytes_written >= WARN_BYTES {
                    log_debug!(
                        "[session/archive] session={} archive >= {WARN_BYTES} bytes ({} MB); \
                         Phase 3.2 will add rotation",
                        session_id,
                        bytes_written / 1024 / 1024
                    );
                    warned = true;
                }
                if !frozen && bytes_written >= HARD_LIMIT_BYTES {
                    log_debug!(
                        "[session/archive] session={} archive hit hard limit \
                         ({HARD_LIMIT_BYTES} bytes) — freezing writes; session stays live",
                        session_id
                    );
                    frozen = true;
                }
            }
            Err(RecvError::Lagged(n)) => {
                log_debug!(
                    "[session/archive] session={} lagged {n} frames — audit incomplete",
                    session_id
                );
                continue;
            }
            Err(RecvError::Closed) => {
                break;
            }
        }
    }

    let _ = file.flush().await;
    log_debug!(
        "[session/archive] writer exiting — session={} bytes_written={}",
        session_id,
        bytes_written
    );
    Ok(())
}

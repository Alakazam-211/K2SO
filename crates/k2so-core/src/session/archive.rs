//! Per-session NDJSON archive writer with rotation.
//!
//! Each session registered in `session::registry` gets a tokio task
//! that subscribes to its `SessionEntry` broadcast, serializes every
//! emitted `Frame` as JSON, and appends one line per frame to the
//! session's active archive segment on disk.
//!
//! **Decoupled from the hot path.** Writer runs as its own tokio
//! task. Slow disk writes can't stall the PTY reader that's
//! producing frames — backpressure shows up as `RecvError::Lagged(n)`
//! on this task's subscription, which the writer logs and continues
//! past.
//!
//! **File layout** (`<project>/.k2so/sessions/<session-id>/`):
//!
//! ```text
//! archive.ndjson           ← active segment; new frames append here
//! archive.000.ndjson       ← first rotated segment (oldest)
//! archive.001.ndjson       ← second rotated segment
//! archive.000.ndjson.gz    ← compacted by `k2so sessions compact`
//! ```
//!
//! The ACTIVE segment always has the stable name `archive.ndjson`.
//! When the active segment reaches `ROTATE_BYTES`, the writer
//! atomically renames it to the next free `archive.NNN.ndjson` and
//! starts a fresh active file. This keeps the "live tail" pointer
//! stable for consumers that follow the session in real time.
//!
//! **Rotation boundary** is checked before each write — a frame is
//! never split across segments. Worst case, the final frame in a
//! segment exceeds `ROTATE_BYTES` by the frame's size; it still
//! lands as one line.
//!
//! **Aggregate cap** (`HARD_LIMIT_BYTES`, default 5 GB): when the
//! sum of every segment in this session's directory exceeds the
//! cap, the writer freezes — session stays live, archive stops
//! growing. Operators run `k2so sessions compact <id>` to gzip old
//! segments, reducing aggregate usage and unfreezing writes on the
//! next pass.
//!
//! **Lifetime.** Task exits naturally when the SessionEntry's
//! broadcast sender drops (registry unregister → entry Arc drops →
//! last sender drops → rx.recv() returns Closed). No explicit
//! shutdown signal needed.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;
use tokio::sync::broadcast::error::RecvError;
use tokio::task::JoinHandle;

use crate::log_debug;
use crate::session::{Frame, SessionEntry, SessionId};

/// Per-segment rotation boundary. When the active segment reaches
/// this size the writer rotates to a new segment. 50 MB keeps
/// individual files small enough to tail / grep / gzip quickly on
/// any modern machine.
pub const ROTATE_BYTES: u64 = 50 * 1024 * 1024;

/// Aggregate size at which the writer logs a warning. Informational
/// only — writing continues past this threshold, all the way to
/// `HARD_LIMIT_BYTES`.
pub const WARN_BYTES: u64 = 1_024 * 1024 * 1024;

/// Aggregate size at which the writer freezes. Session stays alive;
/// archive stops growing. Operators run `k2so sessions compact
/// <id>` to gzip older segments, which reduces aggregate size and
/// unfreezes the writer on its next pass.
pub const HARD_LIMIT_BYTES: u64 = 5 * 1_024 * 1_024 * 1_024;

/// Zero-padded width for rotated segment indices. 3 digits supports
/// 1 000 segments per session = ~50 GB of raw NDJSON per session at
/// the default ROTATE_BYTES, which is far beyond any realistic
/// session lifetime.
const ROTATION_INDEX_WIDTH: usize = 3;

/// Spawn an archive-writer task for `session_id`. Returns the
/// `JoinHandle` so callers can either await the task's natural
/// shutdown or abort it eagerly on session teardown.
///
/// **Requires a tokio runtime.** Caller must be inside a tokio
/// context (daemon's `#[tokio::main]` runtime, or a
/// `#[tokio::test]`). If called without a runtime, this function
/// panics — matches `tokio::spawn`'s own contract.
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
/// directly on a specific runtime and assert on its result.
///
/// Opens the active segment (`archive.ndjson`) in append mode
/// (creating it + parent dirs as needed). Reads frames until the
/// broadcast channel closes. Writes each frame as `<json>\n`.
/// Rotates active → `archive.NNN.ndjson` at the per-segment
/// boundary; freezes (fail-open) at the aggregate cap.
pub async fn run(
    session_id: SessionId,
    mut rx: tokio::sync::broadcast::Receiver<Frame>,
    project_root: PathBuf,
) -> std::io::Result<()> {
    let archive_dir = project_root
        .join(".k2so/sessions")
        .join(session_id.to_string());
    tokio::fs::create_dir_all(&archive_dir).await?;
    let active_path = archive_dir.join("archive.ndjson");

    let mut file = open_for_append(&active_path).await?;
    log_debug!(
        "[session/archive] writer started — session={} path={:?}",
        session_id,
        active_path
    );

    let mut bytes_in_current = file.metadata().await?.len();
    let mut aggregate = compute_aggregate_bytes(&archive_dir).await?;
    let mut warned = aggregate >= WARN_BYTES;
    let mut frozen = aggregate >= HARD_LIMIT_BYTES;

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
                let write_len = buf.len() as u64;

                // Rotate BEFORE writing if the write would cross
                // the per-segment boundary. A zero-byte active
                // segment accepts the write unconditionally so a
                // single massive frame never livelocks in an infinite
                // "rotate empty file → write to new empty file → it's
                // still too big → rotate again" cycle.
                if bytes_in_current > 0
                    && bytes_in_current + write_len > ROTATE_BYTES
                {
                    if let Err(e) = rotate_active_segment(
                        &mut file,
                        &archive_dir,
                        &active_path,
                        session_id,
                    )
                    .await
                    {
                        log_debug!(
                            "[session/archive] rotation failed: {e} — freezing archive"
                        );
                        frozen = true;
                        continue;
                    }
                    bytes_in_current = 0;
                }

                if let Err(e) = file.write_all(&buf).await {
                    log_debug!(
                        "[session/archive] write failed: {e} — freezing archive"
                    );
                    frozen = true;
                    continue;
                }
                bytes_in_current += write_len;
                aggregate += write_len;

                if !warned && aggregate >= WARN_BYTES {
                    log_debug!(
                        "[session/archive] session={} aggregate >= {} MB ({} MB used); \
                         consider `k2so sessions compact {session_id}`",
                        session_id,
                        WARN_BYTES / 1024 / 1024,
                        aggregate / 1024 / 1024
                    );
                    warned = true;
                }
                if aggregate >= HARD_LIMIT_BYTES {
                    log_debug!(
                        "[session/archive] session={} aggregate hit hard limit \
                         ({} bytes) — freezing writes; run `k2so sessions compact` \
                         to free space",
                        session_id,
                        HARD_LIMIT_BYTES
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
        "[session/archive] writer exiting — session={} bytes_in_current={} aggregate={}",
        session_id,
        bytes_in_current,
        aggregate
    );
    Ok(())
}

/// Open `path` in create+append mode. Extracted so the rotation
/// path and the initial-open path share the same flags.
async fn open_for_append(path: &Path) -> std::io::Result<tokio::fs::File> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await
}

/// Rotate the active segment: close current file, rename
/// `archive.ndjson` → `archive.NNN.ndjson` (NNN = next free index),
/// open a fresh `archive.ndjson` for continued appends.
///
/// Atomic via `rename()` — subscribers following `archive.ndjson`
/// in real time will see the file "move" but never a torn state.
/// Consumers that care about rotation boundaries can watch the
/// directory instead.
async fn rotate_active_segment(
    file: &mut tokio::fs::File,
    archive_dir: &Path,
    active_path: &Path,
    session_id: SessionId,
) -> std::io::Result<()> {
    file.flush().await?;
    let next = next_rotation_index(archive_dir).await?;
    let rotated = archive_dir.join(format!(
        "archive.{:0width$}.ndjson",
        next,
        width = ROTATION_INDEX_WIDTH
    ));
    tokio::fs::rename(active_path, &rotated).await?;
    *file = open_for_append(active_path).await?;
    log_debug!(
        "[session/archive] rotated session={} → {:?}",
        session_id,
        rotated.file_name().unwrap_or_default()
    );
    Ok(())
}

/// Find the next unused rotation index for this session's archive
/// directory. Scans existing `archive.NNN.ndjson` and
/// `archive.NNN.ndjson.gz` files so a compacted segment doesn't
/// get overwritten by a later rotation.
async fn next_rotation_index(dir: &Path) -> std::io::Result<u32> {
    let mut max_seen: Option<u32> = None;
    let mut entries = tokio::fs::read_dir(dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            continue;
        };
        if let Some(idx) = parse_rotation_index(name) {
            max_seen = Some(max_seen.map_or(idx, |m| m.max(idx)));
        }
    }
    Ok(max_seen.map_or(0, |m| m + 1))
}

/// Parse a rotated segment filename to its index. Returns `None`
/// for the active segment, the session metadata files, or anything
/// else that doesn't match `archive.NNN.ndjson[.gz]`.
pub fn parse_rotation_index(filename: &str) -> Option<u32> {
    let rest = filename.strip_prefix("archive.")?;
    let body = rest
        .strip_suffix(".ndjson.gz")
        .or_else(|| rest.strip_suffix(".ndjson"))?;
    // Reject if there's any leftover dot — e.g. `archive.foo.ndjson`
    // isn't a rotated segment, it's someone else's file.
    if body.contains('.') {
        return None;
    }
    body.parse().ok()
}

/// Sum the sizes of every file in `dir`. Used at writer startup so
/// the aggregate byte counter reflects historical segments, not
/// just the active one.
pub async fn compute_aggregate_bytes(dir: &Path) -> std::io::Result<u64> {
    let mut sum: u64 = 0;
    let mut entries = tokio::fs::read_dir(dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        if let Ok(meta) = entry.metadata().await {
            if meta.is_file() {
                sum += meta.len();
            }
        }
    }
    Ok(sum)
}

/// Every rotated (but not yet compacted) segment in `session_dir`,
/// sorted by index ascending. Callers can pass these to a gzip
/// helper — the `k2so sessions compact` CLI walks this list,
/// gzips each, and removes the original.
///
/// Pure enumeration: doesn't touch the active segment, doesn't
/// touch already-compacted (`*.ndjson.gz`) files.
pub fn rotated_uncompressed_segments(session_dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut out: Vec<(u32, PathBuf)> = Vec::new();
    for entry in std::fs::read_dir(session_dir)? {
        let entry = entry?;
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            continue;
        };
        if !name.ends_with(".ndjson") {
            continue;
        }
        if name == "archive.ndjson" {
            continue;
        }
        if let Some(idx) = parse_rotation_index(name) {
            out.push((idx, entry.path()));
        }
    }
    out.sort_by_key(|(idx, _)| *idx);
    Ok(out.into_iter().map(|(_, p)| p).collect())
}

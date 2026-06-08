//! K2SO 0.39.39 — daemon-owned pinned-chat session lifecycle (Phase 1).
//!
//! Pins the `POST /cli/workspace/ensure-pinned-chat` find-or-spawn
//! contract (`crate::pinned_chat::ensure_pinned_chat`):
//!
//!   - A never-chatted workspace spawns FRESH: resumedExisting=false,
//!     argv carries `--session-id <new>`, the session registers under
//!     the canonical `<project_id>` key, and active_terminal_id is
//!     stamped.
//!   - A second ensure WITHOUT forceRespawn returns reused=true with
//!     the SAME session id — no duplicate spawn (the atomic
//!     allocate+register that closes the #682 dup-`--session-id` race).
//!   - An ensure WITH forceRespawn unregisters the existing session
//!     (emitting SessionRemoved) and respawns (emitting SessionAdded),
//!     yielding a NEW session id and re-stamping active_terminal_id.
//!
//! Self-contained: shadows `claude` with a temp-dir shim that `exec`s
//! `cat` (a long-lived stdin reader) so no real claude binary / API
//! call is involved. The daemon PTY spawn enriches the child PATH from
//! the test process's PATH (`daemon_pty.rs` PATH-enrichment), which we
//! prepend the shim dir to.

#![cfg(unix)]

use std::path::PathBuf;
use std::sync::Mutex as StdMutex;

use k2so_core::db::init_for_tests;
use k2so_daemon::pinned_chat::ensure_pinned_chat;
use k2so_daemon::session_events::{self, SessionEvent};
use k2so_daemon::v2_session_map;

static TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Create a `claude` shim that execs `cat` and prepend its dir to the
/// test process's PATH so the daemon PTY spawn (which enriches the
/// child PATH from `std::env::var("PATH")`) finds it first. Returns the
/// shim dir so the caller can keep it alive for the test's duration.
fn install_claude_shim() -> PathBuf {
    let shim_dir = std::env::temp_dir().join(format!(
        "k2so-pinned-chat-shim-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&shim_dir).unwrap();
    let shim = shim_dir.join("claude");
    // `cat` with no args reads stdin until EOF → long-lived PTY child.
    std::fs::write(&shim, "#!/bin/sh\nexec cat\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();

    let prev = std::env::var("PATH").unwrap_or_default();
    std::env::set_var("PATH", format!("{}:{}", shim_dir.display(), prev));
    shim_dir
}

fn setup_project(workspace_id: &str, name: &str) -> PathBuf {
    let project_path = std::env::temp_dir().join(format!(
        "k2so-pinned-chat-test-{}-{}-{}",
        workspace_id,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&project_path);
    std::fs::create_dir_all(&project_path).unwrap();

    let db = k2so_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT OR REPLACE INTO projects (id, path, name, agent_mode) \
         VALUES (?1, ?2, ?3, 'custom')",
        rusqlite::params![
            workspace_id,
            project_path.to_string_lossy().as_ref(),
            name,
        ],
    )
    .unwrap();
    project_path
}

fn active_terminal_id(workspace_id: &str) -> Option<String> {
    let db = k2so_core::db::shared();
    let conn = db.lock();
    k2so_core::db::schema::WorkspaceSession::get(&conn, workspace_id)
        .unwrap()
        .and_then(|row| row.active_terminal_id)
}

// ─────────────────────────────────────────────────────────────────────
// Fresh spawn on a never-chatted workspace.
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ensure_pinned_chat_fresh_spawns_and_registers() {
    let _g = lock();
    init_for_tests();
    v2_session_map::clear_for_tests();
    let _shim = install_claude_shim();

    let workspace_id = "pinned-fresh-ws";
    let project = setup_project(workspace_id, "fresh");
    let project_path = project.to_string_lossy().into_owned();

    let out = ensure_pinned_chat(&project_path, false)
        .expect("ensure should succeed on a never-chatted workspace");

    assert!(!out.reused, "cold workspace must spawn fresh");
    assert!(
        !out.resumed_existing,
        "never-chatted workspace must pre-allocate a fresh session (--session-id), not --resume"
    );
    assert_eq!(out.command, "claude");
    assert!(
        out.args.iter().any(|a| a == "--session-id"),
        "fresh spawn argv must carry --session-id, got: {:?}",
        out.args
    );
    assert!(
        !out.args.iter().any(|a| a == "--resume"),
        "fresh spawn must NOT carry --resume, got: {:?}",
        out.args
    );
    assert!(!out.session_id.is_empty());
    assert!(!out.claude_session_id.is_empty());

    // Registered under the canonical bare-`<project_id>` key.
    let live = v2_session_map::lookup_by_agent_name(workspace_id);
    assert!(
        live.is_some(),
        "v2_session_map missing canonical key {workspace_id} after ensure"
    );
    assert_eq!(live.unwrap().session_id.to_string(), out.session_id);

    // active_terminal_id stamped.
    assert_eq!(
        active_terminal_id(workspace_id).as_deref(),
        Some(out.session_id.as_str()),
        "workspace_sessions.active_terminal_id must equal the fresh session id"
    );

    v2_session_map::clear_for_tests();
}

// ─────────────────────────────────────────────────────────────────────
// Idempotency: second ensure without forceRespawn reuses.
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ensure_pinned_chat_second_call_reuses_no_duplicate() {
    let _g = lock();
    init_for_tests();
    v2_session_map::clear_for_tests();
    let _shim = install_claude_shim();

    let workspace_id = "pinned-reuse-ws";
    let project = setup_project(workspace_id, "reuse");
    let project_path = project.to_string_lossy().into_owned();

    let first = ensure_pinned_chat(&project_path, false).expect("first ensure");
    assert!(!first.reused, "first call must be a fresh spawn");

    let second = ensure_pinned_chat(&project_path, false).expect("second ensure");
    assert!(
        second.reused,
        "second ensure without forceRespawn must reuse the live session"
    );
    assert_eq!(
        second.session_id, first.session_id,
        "reused session id must match the original — no duplicate spawn"
    );

    // Exactly ONE map entry for the canonical key.
    let count = v2_session_map::snapshot()
        .into_iter()
        .filter(|(k, _)| k == workspace_id)
        .count();
    assert_eq!(count, 1, "exactly one canonical entry, got {count}");

    v2_session_map::clear_for_tests();
}

// ─────────────────────────────────────────────────────────────────────
// forceRespawn: unregister (Removed) + respawn (Added), new id, re-stamp.
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ensure_pinned_chat_force_respawn_replaces_and_emits_removed_then_added() {
    let _g = lock();
    init_for_tests();
    v2_session_map::clear_for_tests();
    let _shim = install_claude_shim();

    let workspace_id = "pinned-force-ws";
    let project = setup_project(workspace_id, "force");
    let project_path = project.to_string_lossy().into_owned();

    let first = ensure_pinned_chat(&project_path, false).expect("first ensure");
    assert!(!first.reused);
    let original_session = first.session_id.clone();
    assert_eq!(
        active_terminal_id(workspace_id).as_deref(),
        Some(original_session.as_str())
    );

    // Subscribe BEFORE the force-respawn so we capture both events.
    let mut rx = session_events::subscribe();

    let respawned =
        ensure_pinned_chat(&project_path, true).expect("force_respawn ensure");
    assert!(
        !respawned.reused,
        "force_respawn must spawn fresh, not reuse"
    );
    assert_ne!(
        respawned.session_id, original_session,
        "force_respawn must yield a NEW daemon session id"
    );

    // Drain the bus for ~1s; assert we saw SessionRemoved for the
    // original session id THEN SessionAdded for the new one, in order.
    let mut saw_removed_first = false;
    let mut saw_added_after = false;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Ok(SessionEvent::SessionRemoved { agent_name, .. }))
                if agent_name == workspace_id =>
            {
                if !saw_added_after {
                    saw_removed_first = true;
                }
            }
            Ok(Ok(SessionEvent::SessionAdded {
                agent_name,
                session_id,
                ..
            })) if agent_name == workspace_id => {
                if saw_removed_first && session_id == respawned.session_id {
                    saw_added_after = true;
                    break;
                }
            }
            Ok(Ok(_)) => continue, // contamination from another test
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
            Ok(Err(_)) => break,
            Err(_) => break,
        }
    }
    assert!(
        saw_removed_first,
        "force_respawn must emit SessionRemoved for the canonical key"
    );
    assert!(
        saw_added_after,
        "force_respawn must emit SessionAdded (after Removed) for the new session"
    );

    // active_terminal_id re-stamped to the new session.
    assert_eq!(
        active_terminal_id(workspace_id).as_deref(),
        Some(respawned.session_id.as_str()),
        "active_terminal_id must be re-stamped to the respawned session id"
    );

    // Exactly one canonical entry remains, pointing at the new session.
    let live = v2_session_map::lookup_by_agent_name(workspace_id)
        .expect("canonical entry must exist after respawn");
    assert_eq!(live.session_id.to_string(), respawned.session_id);

    v2_session_map::clear_for_tests();
}

// ─────────────────────────────────────────────────────────────────────
// Error: unregistered workspace.
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ensure_pinned_chat_errors_when_workspace_unregistered() {
    let _g = lock();
    init_for_tests();
    v2_session_map::clear_for_tests();

    let result = ensure_pinned_chat("/tmp/k2so-pinned-chat-not-registered", false);
    assert!(result.is_err());
    assert!(
        result.unwrap_err().contains("project not registered"),
        "error must explain the cause"
    );
}

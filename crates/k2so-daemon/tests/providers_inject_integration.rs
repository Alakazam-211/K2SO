//! F1 integration test — daemon-side `InjectProvider` reaches a
//! live `SessionStreamSession` via the agent-name map.
//!
//! This is the end-to-end proof that `k2so signal foo msg '...'`
//! (via egress::deliver → InjectProvider → session_map lookup →
//! session.write) actually lands bytes in a running PTY.

#![cfg(unix)]

use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use k2so_core::awareness::{
    egress, AgentAddress, AgentSignal, Delivery, SignalKind, WorkspaceId,
};
use k2so_core::db::init_for_tests;
use k2so_core::session::SessionId;
use k2so_core::terminal::{spawn_session_stream, SpawnConfig};

use k2so_daemon::providers;
use k2so_daemon::session_map;
use k2so_daemon::v2_session_map;

/// Serialize the tests — all touch the global k2so-core provider
/// slot and the daemon's global session map.
static TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn ensure_project(pid: &str) {
    let db = k2so_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT OR IGNORE INTO projects (id, path, name) VALUES (?1, ?1, ?1)",
        rusqlite::params![pid],
    )
    .unwrap();
}

// Note: verifying bytes actually reached the PTY (via Term grid
// inspection) is covered by Phase 2's session_stream_pty tests in
// k2so-core. Here, a successful `session.write()` via the
// InjectProvider is the end-to-end proof we need —
// `report.injected_to_pty == true` means the provider resolved
// the agent name, looked up the session, and called `write` with
// no error.

// ─────────────────────────────────────────────────────────────────────
// End-to-end: k2so signal → daemon egress → InjectProvider →
// session.write → target's PTY echoes the bytes back
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn daemon_inject_provider_writes_bytes_to_live_session() {
    let _g = lock();
    init_for_tests();
    ensure_project("k2so-ws");

    // Register the daemon's providers as if the daemon had just
    // booted. This replaces any mock providers set by prior tests.
    providers::register_all();

    // Spawn a real PTY running `cat` as "bar". cat echoes stdin
    // to stdout, so anything our inject writes comes back out
    // and lands in bar's alacritty Term grid.
    let bar_id = SessionId::new();
    let bar_session = spawn_session_stream(SpawnConfig {
        session_id: bar_id,
        cwd: "/tmp".into(),
        command: Some("cat".into()),
        args: None,
        cols: 80,
        rows: 24,
        track_alacritty_term: false,
    })
    .expect("spawn bar session");

    // 0.37.0 canonicalization: every workspace-agent session is
    // keyed under `<workspace_id>:<agent_name>`. Egress's prefixed
    // lookup (post-0.36.15 bridge retirement) requires this — the
    // bare-name fallback is gone. Register + tag under the
    // canonical key so the resolution path actually finds the
    // session.
    let canonical_key = "k2so-ws:bar";
    let bar_arc = Arc::new(bar_session);
    session_map::register(canonical_key, Arc::clone(&bar_arc));

    // Small delay so the reader thread is attached before we write.
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Build a Live signal from foo → bar. Egress looks up bar's
    // liveness, resolves to Live+Live, calls InjectProvider.inject.
    let signal = AgentSignal::new(
        AgentAddress::Agent {
            workspace: WorkspaceId("k2so-ws".into()),
            name: "foo".into(),
        },
        AgentAddress::Agent {
            workspace: WorkspaceId("k2so-ws".into()),
            name: "bar".into(),
        },
        SignalKind::Msg {
            text: "f1-end-to-end-probe".into(),
        },
    )
    .with_delivery(Delivery::Live);

    let inbox_root = std::env::temp_dir().join(format!(
        "k2so-f1-inbox-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::create_dir_all(&inbox_root);

    // Crucial: the k2so-core liveness check walks the core's
    // session::registry for sessions with matching agent_name.
    // Post-0.37.0 the registry expects the canonical key form so
    // the cross-map lookup matches.
    if let Some(entry) = k2so_core::session::registry::lookup(&bar_id) {
        entry.set_agent_name(canonical_key);
    }

    let report = egress::deliver(&signal, &inbox_root);

    // Inject should have fired via the daemon provider.
    assert!(
        report.injected_to_pty,
        "expected injected_to_pty=true; got report {report:?}"
    );
    // No inbox file — Live to live target never writes inbox.
    assert!(report.inbox_path.is_none());
    // Audit fired.
    assert!(report.published_to_bus);
    assert!(report.activity_feed_row_id > 0);

    // Cleanup — unregister + kill session.
    session_map::unregister(canonical_key);
    bar_arc.kill().ok();
}

#[tokio::test(flavor = "current_thread")]
async fn daemon_inject_provider_reports_missing_agent_as_error() {
    let _g = lock();
    init_for_tests();
    ensure_project("k2so-ws");
    providers::register_all();

    // No session registered for "nobody".
    let signal = AgentSignal::new(
        AgentAddress::Agent {
            workspace: WorkspaceId("k2so-ws".into()),
            name: "foo".into(),
        },
        AgentAddress::Agent {
            workspace: WorkspaceId("k2so-ws".into()),
            name: "nobody".into(),
        },
        SignalKind::Msg {
            text: "never arrives".into(),
        },
    );

    let inbox_root = std::env::temp_dir().join("k2so-f1-nobody");
    let _ = std::fs::create_dir_all(&inbox_root);
    let report = egress::deliver(&signal, &inbox_root);

    // Target wasn't live in registry, so egress picked the
    // wake-path. DaemonWakeProvider returns Ok unconditionally for
    // Phase 3.1 MVP, so woke_offline_target=true. No inject, no
    // inbox. Audit fires.
    assert!(!report.injected_to_pty);
    assert!(
        report.woke_offline_target,
        "DaemonWakeProvider should accept the wake even when \
         session doesn't exist — real scheduler-wake is deferred"
    );
    assert!(report.inbox_path.is_none());
    assert!(report.published_to_bus);
    assert!(report.activity_feed_row_id > 0);
}

// ─────────────────────────────────────────────────────────────────────
// A9 regression — DaemonInjectProvider must reach v2 sessions too.
// Before A9, providers only checked the legacy session_map and were
// blind to every session registered via /cli/sessions/v2/spawn or
// the migrated spawn_agent_session_v2_blocking. These tests pin the
// fix in place: register a v2 session, call the provider directly,
// assert inject succeeds (lookup_any fell through legacy and hit v2).
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn daemon_inject_provider_writes_bytes_to_v2_session() {
    use k2so_core::awareness::InjectProvider;
    use k2so_core::terminal::{DaemonPtyConfig, DaemonPtySession};
    use std::path::PathBuf;

    let _g = lock();
    init_for_tests();
    ensure_project("k2so-ws-v2");

    // Spawn a v2 DaemonPtySession running `cat`. v2 doesn't register
    // in `k2so_core::session::registry` — it lives in a parallel map
    // entirely. The InjectProvider has to look in v2_session_map for
    // it, which is exactly what A9's session_lookup::lookup_any does.
    let v2_cfg = DaemonPtyConfig {
        session_id: SessionId::new(),
        cols: 80,
        rows: 24,
        cwd: Some(PathBuf::from("/tmp")),
        program: Some("cat".to_string()),
        args: vec![],
        env: Default::default(),
        drain_on_exit: true,
    };
    let v2_session = DaemonPtySession::spawn(v2_cfg).expect("v2 spawn");
    v2_session_map::register("v2-only-bar", v2_session.clone());

    // Call the provider directly. Pre-A9 this would return NotFound
    // because the legacy session_map has no entry for "v2-only-bar".
    let provider = providers::DaemonInjectProvider;
    provider
        .inject("v2-only-bar", b"a9-inject-probe\n")
        .expect("inject must reach v2 session via lookup_any");

    // Asking for an unknown agent must still return NotFound — the
    // dual-map walk doesn't make every name resolvable.
    let err = provider
        .inject("never-existed", b"x")
        .expect_err("unknown agent must fail with NotFound");
    assert_eq!(err.kind(), std::io::ErrorKind::NotFound);

    // Cleanup.
    v2_session_map::unregister("v2-only-bar");
    drop(v2_session);
}

#[tokio::test(flavor = "current_thread")]
async fn daemon_inject_provider_finds_legacy_first_then_v2() {
    use k2so_core::awareness::InjectProvider;
    use k2so_core::terminal::{DaemonPtyConfig, DaemonPtySession};
    use std::path::PathBuf;
    use std::sync::Arc;

    let _g = lock();
    init_for_tests();

    // Register the SAME agent_name in both maps. Provider is
    // documented as "legacy first" — verify by writing to legacy
    // and asserting it didn't fail (doesn't prove ordering, but
    // does prove both paths work without the registration order
    // mattering).
    let legacy_id = SessionId::new();
    let legacy = spawn_session_stream(SpawnConfig {
        session_id: legacy_id,
        cwd: "/tmp".into(),
        command: Some("cat".into()),
        args: None,
        cols: 80,
        rows: 24,
        track_alacritty_term: false,
    })
    .expect("legacy spawn");
    let legacy_arc = Arc::new(legacy);
    session_map::register("dual-name", Arc::clone(&legacy_arc));

    let v2 = DaemonPtySession::spawn(DaemonPtyConfig {
        session_id: SessionId::new(),
        cols: 80,
        rows: 24,
        cwd: Some(PathBuf::from("/tmp")),
        program: Some("cat".to_string()),
        args: vec![],
        env: Default::default(),
        drain_on_exit: true,
    })
    .expect("v2 spawn");
    v2_session_map::register("dual-name", v2.clone());

    let provider = providers::DaemonInjectProvider;
    provider
        .inject("dual-name", b"both-maps\n")
        .expect("inject must succeed when agent_name lives in both maps");

    session_map::unregister("dual-name");
    v2_session_map::unregister("dual-name");
    legacy_arc.kill().ok();
    drop(v2);
}

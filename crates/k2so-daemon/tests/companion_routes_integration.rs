//! H4 of Phase 4 integration tests — daemon-side
//! `/cli/companion/sessions` + `/cli/companion/projects-summary`.
//!
//! These routes join daemon session_map snapshots against the
//! `projects` + `focus_groups` tables. Tests seed minimal DB
//! rows + register fake sessions, then assert on the JSON shape
//! each route returns.

#![cfg(unix)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use k2so_core::db::init_for_tests;
use k2so_core::session::SessionId;
use k2so_core::terminal::{spawn_session_stream, SpawnConfig};

use k2so_daemon::companion_routes;
use k2so_daemon::session_map;

/// Serialize tests — they all touch the shared in-memory DB +
/// session_map singletons.
static TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn ensure_project(id: &str, path: &str, name: &str) {
    let db = k2so_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT OR REPLACE INTO projects \
         (id, path, name, color, agent_mode, pinned, tab_order) \
         VALUES (?1, ?2, ?3, '#123456', 'off', 0, 0)",
        rusqlite::params![id, path, name],
    )
    .unwrap();
}

fn clear_projects() {
    let db = k2so_core::db::shared();
    let conn = db.lock();
    // Keep sentinel rows (_orphan, _broadcast) so G0 fallbacks
    // don't break any test that writes activity_feed.
    let _ = conn.execute(
        "DELETE FROM projects WHERE id NOT IN ('_orphan', '_broadcast')",
        [],
    );
}

fn tmp_project_dir(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "k2so-h4-{}-{}-{}",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn spawn_cat_in(
    agent: &str,
    cwd: &str,
) -> (SessionId, Arc<k2so_core::terminal::SessionStreamSession>) {
    let id = SessionId::new();
    let session = spawn_session_stream(SpawnConfig {
        session_id: id,
        cwd: cwd.into(),
        command: Some("cat".into()),
        args: None,
        cols: 80,
        rows: 24,
        track_alacritty_term: false,
    })
    .expect("spawn cat");
    let arc = Arc::new(session);
    session_map::register(agent, arc.clone());
    (id, arc)
}

fn drop_all(agents: &[&str]) {
    for a in agents {
        if let Some(s) = session_map::unregister(a) {
            let _ = s.kill();
        }
    }
}

fn params() -> HashMap<String, String> {
    HashMap::new()
}

// ─────────────────────────────────────────────────────────────────────
// /cli/companion/sessions
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn companion_sessions_maps_live_sessions_to_workspaces() {
    let _g = lock();
    init_for_tests();
    clear_projects();
    // Drain any existing session_map registrations from prior tests.
    for (name, s) in session_map::snapshot() {
        let _ = s.kill();
        session_map::unregister(&name);
    }

    let proj_a = tmp_project_dir("companion-a");
    let proj_b = tmp_project_dir("companion-b");
    let a_path = proj_a.to_string_lossy().into_owned();
    let b_path = proj_b.to_string_lossy().into_owned();
    ensure_project("ws-a", &a_path, "Alpha");
    ensure_project("ws-b", &b_path, "Beta");

    let (_ida, _sa) = spawn_cat_in("alpha-agent", &a_path);
    let (_idb, _sb) = spawn_cat_in("beta-agent", &b_path);
    // A session whose cwd doesn't match any workspace should be
    // dropped from the response.
    let (_idx, _sx) = spawn_cat_in("orphan-agent", "/tmp");

    let resp = companion_routes::handle_companion_sessions(&params());
    assert_eq!(resp.status, "200 OK");
    let arr: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
    let items = arr.as_array().expect("array");
    // Only alpha + beta should appear; orphan's /tmp cwd doesn't
    // prefix-match /tmp/k2so-h4-* (but the other test-cases in
    // /tmp owned by OTHER parallel test runs might... which is
    // why `clear_projects` above restricted workspace rows).
    let our: Vec<_> = items
        .iter()
        .filter(|v| {
            let ws_id = v["workspaceId"].as_str().unwrap_or("");
            ws_id == "ws-a" || ws_id == "ws-b"
        })
        .collect();
    assert_eq!(our.len(), 2, "expected alpha + beta only: {items:#?}");

    // Alpha's record is correctly attributed.
    let alpha = our
        .iter()
        .find(|v| v["workspaceId"] == "ws-a")
        .expect("alpha present");
    assert_eq!(alpha["workspaceName"].as_str(), Some("Alpha"));
    assert_eq!(alpha["agentName"].as_str(), Some("alpha-agent"));
    assert_eq!(alpha["label"].as_str(), Some("alpha-agent"));
    assert_eq!(alpha["command"].as_str(), Some("cat"));

    drop_all(&["alpha-agent", "beta-agent", "orphan-agent"]);
    clear_projects();
}

#[tokio::test(flavor = "current_thread")]
async fn companion_sessions_uses_worktree_agent_name() {
    let _g = lock();
    init_for_tests();
    clear_projects();
    for (name, s) in session_map::snapshot() {
        let _ = s.kill();
        session_map::unregister(&name);
    }

    let proj = tmp_project_dir("wt");
    let proj_path = proj.to_string_lossy().into_owned();
    ensure_project("wt-ws", &proj_path, "Worktree Project");
    let wt_path = proj.join(".k2so/worktrees/builder/deep/subdir");
    std::fs::create_dir_all(&wt_path).unwrap();

    // session_map agent name is synthesized (e.g. background
    // spawn); worktree-path rule should override.
    let (_id, _s) = spawn_cat_in(
        "terminal-abc12345",
        &wt_path.to_string_lossy(),
    );

    let resp = companion_routes::handle_companion_sessions(&params());
    let arr: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
    let item = arr
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["workspaceId"] == "wt-ws")
        .expect("worktree session present");
    assert_eq!(item["agentName"].as_str(), Some("builder"));
    assert_eq!(item["label"].as_str(), Some("builder"));

    drop_all(&["terminal-abc12345"]);
    clear_projects();
}

// ─────────────────────────────────────────────────────────────────────
// /cli/companion/projects-summary
// ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn projects_summary_counts_running_per_workspace() {
    let _g = lock();
    init_for_tests();
    clear_projects();
    for (name, s) in session_map::snapshot() {
        let _ = s.kill();
        session_map::unregister(&name);
    }

    let proj_a = tmp_project_dir("sum-a");
    let proj_b = tmp_project_dir("sum-b");
    let a_path = proj_a.to_string_lossy().into_owned();
    let b_path = proj_b.to_string_lossy().into_owned();
    ensure_project("sum-a", &a_path, "SumA");
    ensure_project("sum-b", &b_path, "SumB");

    // Two sessions for SumA, zero for SumB.
    let (_, _) = spawn_cat_in("s-a-one", &a_path);
    let (_, _) = spawn_cat_in("s-a-two", &a_path);

    let resp = companion_routes::handle_companion_projects_summary(&params());
    assert_eq!(resp.status, "200 OK");
    let arr: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
    let items = arr.as_array().expect("array");

    let a = items
        .iter()
        .find(|v| v["id"] == "sum-a")
        .expect("sum-a present");
    assert_eq!(a["name"].as_str(), Some("SumA"));
    assert_eq!(a["agentsRunning"].as_u64(), Some(2));
    assert_eq!(a["focusGroup"], serde_json::Value::Null);

    let b = items
        .iter()
        .find(|v| v["id"] == "sum-b")
        .expect("sum-b present");
    assert_eq!(b["agentsRunning"].as_u64(), Some(0));

    drop_all(&["s-a-one", "s-a-two"]);
    clear_projects();
}

#[tokio::test(flavor = "current_thread")]
async fn projects_summary_counts_pending_reviews_from_filesystem() {
    let _g = lock();
    init_for_tests();
    clear_projects();

    let proj = tmp_project_dir("reviews");
    let proj_path = proj.to_string_lossy().into_owned();
    ensure_project("review-ws", &proj_path, "ReviewsOnly");

    // Seed two .md files in two agents' done/ dirs + one non-.md
    // file (should not count).
    let done_alpha = proj.join(".k2so/agents/alpha/work/done");
    let done_beta = proj.join(".k2so/agents/beta/work/done");
    std::fs::create_dir_all(&done_alpha).unwrap();
    std::fs::create_dir_all(&done_beta).unwrap();
    std::fs::write(done_alpha.join("task-1.md"), "body").unwrap();
    std::fs::write(done_alpha.join("task-2.md"), "body").unwrap();
    std::fs::write(done_alpha.join("notes.txt"), "ignored").unwrap();
    std::fs::write(done_beta.join("task-3.md"), "body").unwrap();

    let resp = companion_routes::handle_companion_projects_summary(&params());
    let arr: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
    let rec = arr
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["id"] == "review-ws")
        .expect("review-ws present");
    assert_eq!(rec["reviewsPending"].as_u64(), Some(3));

    clear_projects();
}

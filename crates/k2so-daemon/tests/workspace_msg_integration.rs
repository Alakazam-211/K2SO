//! 0.37.0 simplified messaging — `k2so msg <workspace> "text" [--wake]`.
//!
//! Pins the workspace-token resolver and the inbox delivery path
//! against the schema after unification. The smart-cascade
//! `deliver_live` path requires spawning `claude` (or a substitute)
//! and is exercised end-to-end through `cli/k2so` against a live
//! daemon — those checks live in CI's smoke harness, not here.
//!
//! What we cover here:
//! - `resolve_workspace` accepts name | absolute path | UUID and
//!   only returns a hit when a `projects` row matches.
//! - `deliver_to_inbox` writes a real markdown file into
//!   `<project>/.k2so/work/inbox/` and the daemon's response
//!   reflects the work-item shape SMS-bridge consumers depend on.

#![cfg(unix)]

use std::path::PathBuf;
use std::sync::Mutex as StdMutex;

use k2so_core::db::init_for_tests;

use k2so_daemon::workspace_msg;

static TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn setup_project(workspace_id: &str, name: &str) -> PathBuf {
    let project_path = std::env::temp_dir().join(format!(
        "k2so-ws-msg-test-{}-{}-{}",
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
         VALUES (?1, ?2, ?3, 'manager')",
        rusqlite::params![
            workspace_id,
            project_path.to_string_lossy().as_ref(),
            name,
        ],
    )
    .unwrap();
    project_path
}

// ─────────────────────────────────────────────────────────────────────
// resolve_workspace
// ─────────────────────────────────────────────────────────────────────

#[test]
fn resolve_workspace_by_name_returns_path() {
    let _g = lock();
    init_for_tests();
    let workspace_id = "ws-msg-resolve-name";
    let project = setup_project(workspace_id, "ResolveTest");

    let resolved = workspace_msg::resolve_workspace("ResolveTest");
    assert_eq!(
        resolved.as_deref(),
        Some(project.to_string_lossy().as_ref()),
        "name lookup should return the project's canonical path"
    );
}

#[test]
fn resolve_workspace_by_absolute_path_returns_path() {
    let _g = lock();
    init_for_tests();
    let workspace_id = "ws-msg-resolve-path";
    let project = setup_project(workspace_id, "PathLookup");
    let path_str = project.to_string_lossy().to_string();

    let resolved = workspace_msg::resolve_workspace(&path_str);
    assert_eq!(
        resolved.as_deref(),
        Some(path_str.as_str()),
        "absolute path lookup should round-trip"
    );
}

#[test]
fn resolve_workspace_by_uuid_returns_path() {
    let _g = lock();
    init_for_tests();
    // Real UUID format (36 chars, 4 dashes) so the resolver's UUID
    // detection branch fires, not the name fallback.
    let workspace_id = "11112222-3333-4444-5555-666677778888";
    let project = setup_project(workspace_id, "UuidLookup");

    let resolved = workspace_msg::resolve_workspace(workspace_id);
    assert_eq!(
        resolved.as_deref(),
        Some(project.to_string_lossy().as_ref()),
        "UUID lookup should return the project's canonical path"
    );
}

#[test]
fn resolve_workspace_unknown_token_returns_none() {
    let _g = lock();
    init_for_tests();
    // Don't set up any project — the resolver must miss cleanly,
    // not panic or return a stale match.
    let resolved = workspace_msg::resolve_workspace("definitely-not-a-real-workspace-name");
    assert!(resolved.is_none(), "missing token should return None");
}

#[test]
fn resolve_workspace_empty_token_returns_none() {
    let _g = lock();
    init_for_tests();
    let resolved = workspace_msg::resolve_workspace("");
    assert!(resolved.is_none(), "empty token must short-circuit, not match every row");
}

// ─────────────────────────────────────────────────────────────────────
// deliver_to_inbox
// ─────────────────────────────────────────────────────────────────────

#[test]
fn deliver_to_inbox_writes_markdown_file() {
    let _g = lock();
    init_for_tests();
    let workspace_id = "ws-msg-inbox";
    let project = setup_project(workspace_id, "InboxTarget");
    let project_path = project.to_string_lossy().into_owned();

    // The body's first line becomes the title (truncated to 80 chars).
    let body = "first line is the title\nsecond line is body content\nthird line";
    let result = workspace_msg::deliver_to_inbox(&project_path, body, "test-sender");

    assert_eq!(
        result.get("success").and_then(|v| v.as_bool()),
        Some(true),
        "deliver_to_inbox should report success: {result}"
    );
    assert_eq!(
        result.get("delivery").and_then(|v| v.as_str()),
        Some("inbox"),
        "delivery field must be 'inbox'"
    );

    // The work-item record should expose the title + sender so SMS
    // bridge consumers can echo back what was filed.
    let inner = result.get("result").expect("result subobject present");
    assert_eq!(
        inner.get("title").and_then(|v| v.as_str()),
        Some("first line is the title"),
        "title must be the body's first line"
    );
    assert_eq!(
        inner.get("assignedBy").and_then(|v| v.as_str()),
        Some("test-sender"),
        "assignedBy must echo the sender argument"
    );

    // Verify the file actually landed on disk.
    let inbox_dir = project.join(".k2so/work/inbox");
    let files: Vec<_> = std::fs::read_dir(&inbox_dir)
        .expect("inbox dir should exist after deliver")
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("md"))
        .collect();
    assert_eq!(files.len(), 1, "exactly one .md file should land per call");

    let written = std::fs::read_to_string(files[0].path()).unwrap();
    assert!(
        written.contains("title: first line is the title"),
        "frontmatter must carry the title"
    );
    assert!(
        written.contains("second line is body content"),
        "body content must be preserved"
    );
}

#[test]
fn deliver_to_inbox_truncates_long_first_line_for_title() {
    let _g = lock();
    init_for_tests();
    let workspace_id = "ws-msg-inbox-long";
    let project = setup_project(workspace_id, "InboxLong");
    let project_path = project.to_string_lossy().into_owned();

    // 100-char first line should truncate to 80 chars in the title.
    let long_first = "x".repeat(100);
    let body = format!("{long_first}\nrest");
    let result = workspace_msg::deliver_to_inbox(&project_path, &body, "cli");

    let title = result
        .get("result")
        .and_then(|r| r.get("title"))
        .and_then(|v| v.as_str())
        .expect("title present");
    assert_eq!(title.chars().count(), 80, "title must truncate to 80 chars");
}

//! Integration-ish tests for the Clone-to unpack route.
//!
//! Each test builds a synthetic bundle (via the k2so-core engine) from a
//! temp source workspace, then unpacks it into a temp DEST with a temp
//! HOME override so the `~/.claude/projects/<slug>/` placement is hermetic
//! and the real home is never touched. Registration goes through the
//! shared in-memory test DB (k2so-core `test-util`).

use super::*;
use k2so_core::clone;
use std::fs;
use std::path::{Path, PathBuf};

// ── temp dir (no tempfile dep; mirrors the core clone tests) ──────────
struct TempDir {
    path: PathBuf,
}
impl TempDir {
    fn new(prefix: &str) -> Self {
        let pid = std::process::id();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        static CTR: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = CTR.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("{prefix}-{pid}-{nanos}-{n}"));
        fs::create_dir_all(&path).expect("create tempdir");
        Self { path }
    }
    fn path(&self) -> &Path {
        &self.path
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("mkdir -p");
    }
    fs::write(path, contents).expect("write file");
}

/// Build a synthetic SOURCE: a workspace tree + a hermetic
/// `<src_home>/.claude/projects/<slug>/` with memory + a session. Returns
/// the bundle path (built with `agent_mode='manager'` settings) and the
/// roots so the caller can keep them alive.
fn build_source_bundle(
    root: &TempDir,
    settings: Option<clone::WorkspaceSettings>,
) -> (PathBuf, String) {
    let project = root.path().join("source").join("My Agent");
    let src_home = root.path().join("src-home");
    fs::create_dir_all(&project).unwrap();
    fs::create_dir_all(&src_home).unwrap();

    write(&project.join("README.md"), "# My Agent\nproject docs\n");
    write(&project.join(".k2so/PROJECT.md"), "# Project\n");
    write(&project.join("src/main.rs"), "fn main() {}\n");

    let canon = fs::canonicalize(&project).unwrap();
    let slug = k2so_core::chat_history::claude_project_hash(&canon.to_string_lossy());
    let slug_dir = src_home.join(".claude").join("projects").join(&slug);
    write(&slug_dir.join("memory/MEMORY.md"), "## Memory Index\nm1\n");
    write(&slug_dir.join("memory/a.md"), "memory a\n");
    write(
        &slug_dir.join("44444444-4444-4444-4444-444444444444.jsonl"),
        "{\"type\":\"live\"}\n",
    );

    let opts = clone::CloneOptions {
        home_override: Some(src_home.clone()),
        ..Default::default()
    };
    let inv = clone::inventory(&canon.to_string_lossy(), opts.clone()).unwrap();
    let bundle = root.path().join("bundle.tar.gz");
    clone::build_bundle(
        &inv,
        &opts,
        "2026-06-05T00:00:00Z".to_string(),
        settings,
        &bundle,
    )
    .unwrap();

    (bundle, slug)
}

fn manager_settings() -> clone::WorkspaceSettings {
    clone::WorkspaceSettings {
        agent_mode: "manager".to_string(),
        agent_enabled: true,
        heartbeat_enabled: true,
        name: "My Agent".to_string(),
        color: "#aa00aa".to_string(),
        worktree_mode: 2,
    }
}

#[test]
fn unpack_places_files_registers_and_applies_settings() {
    let root = TempDir::new("k2so-unpack-test");
    let (bundle, _src_slug) = build_source_bundle(&root, Some(manager_settings()));

    let dest_parent = root.path().join("dest");
    let remote_home = root.path().join("remote-home");
    fs::create_dir_all(&dest_parent).unwrap();
    fs::create_dir_all(&remote_home).unwrap();

    let (project, dest_path) =
        super::unpack_and_register(&bundle, &dest_parent, &remote_home)
            .expect("unpack + register must succeed");

    // dest_path = <dest_parent>/My Agent
    let expected_dest = dest_parent.join("My Agent");
    assert_eq!(
        Path::new(&dest_path),
        expected_dest,
        "dest dir named after the source workspace"
    );

    // workspace files landed at dest_path
    assert!(
        expected_dest.join("README.md").is_file(),
        "workspace README at dest"
    );
    assert!(expected_dest.join(".k2so/PROJECT.md").is_file());
    assert!(expected_dest.join("src/main.rs").is_file());

    // memory + session under the RECOMPUTED remote slug dir
    let remote_slug =
        k2so_core::chat_history::claude_project_hash(&dest_path);
    let remote_slug_dir = remote_home
        .join(".claude")
        .join("projects")
        .join(&remote_slug);
    assert!(
        remote_slug_dir.join("memory/MEMORY.md").is_file(),
        "memory under recomputed slug, looked at {}",
        remote_slug_dir.display()
    );
    assert!(remote_slug_dir.join("memory/a.md").is_file());
    assert!(
        remote_slug_dir
            .join("44444444-4444-4444-4444-444444444444.jsonl")
            .is_file(),
        "live session re-rooted under recomputed slug"
    );

    // project registered at dest_path with the applied settings
    assert_eq!(project.path, dest_path, "registered at dest_path");
    assert_eq!(project.name, "My Agent");
    assert_eq!(project.color, "#aa00aa");
    assert_eq!(project.agent_mode, "manager");
    assert_eq!(project.agent_enabled, 1, "manager → enabled");
    assert_eq!(project.heartbeat_enabled, 1);
    assert_eq!(project.worktree_mode, 2);

    // confirm it's queryable in the DB
    let db = k2so_core::db::shared();
    let conn = db.lock();
    let fetched = Project::get(&conn, &project.id).expect("row exists");
    assert_eq!(fetched.agent_mode, "manager");
    drop(conn);

    // cleanup DB row so the shared in-memory DB stays tidy.
    let _ = pops::projects_delete(&project.id);
}

#[test]
fn unpack_is_collision_safe() {
    let root = TempDir::new("k2so-unpack-collide");
    let (bundle, _slug) = build_source_bundle(&root, Some(manager_settings()));

    let dest_parent = root.path().join("dest");
    let remote_home = root.path().join("remote-home");
    fs::create_dir_all(&dest_parent).unwrap();
    fs::create_dir_all(&remote_home).unwrap();

    // Pre-create the target dir so the first unpack must collision-rename.
    fs::create_dir_all(dest_parent.join("My Agent")).unwrap();

    let (project, dest_path) =
        super::unpack_and_register(&bundle, &dest_parent, &remote_home)
            .expect("unpack succeeds despite collision");

    assert_eq!(
        Path::new(&dest_path),
        dest_parent.join("My Agent (1)"),
        "collision-safe rename to 'name (1)', got {dest_path}"
    );
    assert!(dest_parent.join("My Agent (1)").join("README.md").is_file());

    let _ = pops::projects_delete(&project.id);
}

#[test]
fn unpack_with_no_settings_registers_with_defaults() {
    let root = TempDir::new("k2so-unpack-nosettings");
    let (bundle, _slug) = build_source_bundle(&root, None);

    let dest_parent = root.path().join("dest");
    let remote_home = root.path().join("remote-home");
    fs::create_dir_all(&dest_parent).unwrap();
    fs::create_dir_all(&remote_home).unwrap();

    let (project, _dest) =
        super::unpack_and_register(&bundle, &dest_parent, &remote_home)
            .expect("unpack with no settings still registers");

    // Default registration: agent off, default color, name = folder.
    assert_eq!(project.name, "My Agent");
    assert_eq!(project.agent_mode, "off");

    let _ = pops::projects_delete(&project.id);
}

#[test]
fn unpack_handler_400s_on_missing_bundle() {
    // The token gate lives in the dispatcher, not the handler (the
    // garbage-token → 403 path is covered end-to-end in
    // clone_routes_integration.rs). This asserts the HANDLER itself
    // produces a clean 400 on a nonexistent bundle path rather than
    // panicking.
    let body = serde_json::json!({
        "bundle_path": "/nonexistent/bundle.tar.gz",
        "dest_parent": "/tmp",
    })
    .to_string();
    let resp = super::handle_clone_unpack(body.as_bytes());
    assert_eq!(resp.status, "400 Bad Request", "missing bundle → 400");
}

#[test]
fn bundle_handler_rejects_invalid_json() {
    let resp = super::handle_clone_bundle(b"not json");
    assert_eq!(resp.status, "400 Bad Request");
}

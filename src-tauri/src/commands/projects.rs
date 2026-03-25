use std::fs;
use std::path::Path;
use std::process::Command;
use tauri::{AppHandle, Emitter, State};
use crate::db::schema::{FocusGroup, Project, Workspace};
use crate::editors;
use crate::editors::EditorInfo;
use crate::project_config;
use crate::state::AppState;

// ── Icon detection helpers ──────────────────────────────────────────────

const ICON_BASENAMES: &[&str] = &["favicon", "icon", "logo", "app-icon"];
const ICON_EXTENSIONS: &[&str] = &[".svg", ".png", ".ico", ".jpg", ".jpeg", ".icns"];

fn extension_priority(ext: &str) -> u32 {
    match ext.to_lowercase().as_str() {
        ".svg" => 0,
        ".png" => 1,
        ".ico" => 2,
        ".jpg" | ".jpeg" => 3,
        ".icns" => 4,
        _ => 99,
    }
}

fn is_icon_filename(name: &str) -> bool {
    let lower = name.to_lowercase();
    let ext = Path::new(&lower)
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    let base = lower.strip_suffix(&ext).unwrap_or(&lower);
    ICON_BASENAMES.contains(&base) && ICON_EXTENSIONS.contains(&ext.as_str())
}

fn find_icon_files(dir: &Path, max_depth: u32, current_depth: u32) -> Vec<std::path::PathBuf> {
    let mut results = Vec::new();
    if current_depth > max_depth {
        return results;
    }

    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return results,
    };

    let skip_dirs = [
        "node_modules", ".git", ".next", "dist", "out", "coverage", ".cache",
    ];

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let path = entry.path();

        if let Ok(ft) = entry.file_type() {
            if ft.is_file() && is_icon_filename(&name) {
                results.push(path);
            } else if ft.is_dir() && current_depth < max_depth && !skip_dirs.contains(&name.as_str())
            {
                results.extend(find_icon_files(&path, max_depth, current_depth + 1));
            }
        }
    }

    results
}

fn read_icon_as_data_url(file_path: &Path) -> Option<String> {
    use base64::Engine;

    let ext = file_path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();

    if ext == "svg" {
        let svg = fs::read_to_string(file_path).ok()?;
        let encoded = base64::engine::general_purpose::STANDARD.encode(svg.as_bytes());
        return Some(format!("data:image/svg+xml;base64,{}", encoded));
    }

    // For raster images, read and resize to 48x48
    let data = fs::read(file_path).ok()?;
    let img = image::load_from_memory(&data).ok()?;
    let resized = img.resize_exact(48, 48, image::imageops::FilterType::Lanczos3);
    let mut buf = std::io::Cursor::new(Vec::new());
    resized
        .write_to(&mut buf, image::ImageFormat::Png)
        .ok()?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(buf.into_inner());
    Some(format!("data:image/png;base64,{}", encoded))
}

fn check_package_json_icon(project_path: &Path) -> Option<std::path::PathBuf> {
    let pkg_path = project_path.join("package.json");
    if !pkg_path.exists() {
        return None;
    }
    let raw = fs::read_to_string(&pkg_path).ok()?;
    let pkg: serde_json::Value = serde_json::from_str(&raw).ok()?;

    // Check top-level icon field
    if let Some(icon) = pkg.get("icon").and_then(|v| v.as_str()) {
        let icon_path = project_path.join(icon);
        if icon_path.exists() {
            return Some(icon_path);
        }
    }

    // Check build.icon (electron-builder)
    if let Some(icon) = pkg
        .get("build")
        .and_then(|b| b.get("icon"))
        .and_then(|v| v.as_str())
    {
        let icon_path = project_path.join(icon);
        if icon_path.exists() {
            return Some(icon_path);
        }
    }

    None
}

fn check_manifest_icons(project_path: &Path) -> Option<std::path::PathBuf> {
    let manifest_paths = [
        "manifest.json",
        "site.webmanifest",
        "public/manifest.json",
        "public/site.webmanifest",
        "src/manifest.json",
    ];

    for mp in &manifest_paths {
        let full_path = project_path.join(mp);
        if !full_path.exists() {
            continue;
        }

        let raw = match fs::read_to_string(&full_path) {
            Ok(r) => r,
            Err(_) => continue,
        };
        let manifest: serde_json::Value = match serde_json::from_str(&raw) {
            Ok(m) => m,
            Err(_) => continue,
        };

        if let Some(icons) = manifest.get("icons").and_then(|v| v.as_array()) {
            if icons.is_empty() {
                continue;
            }
            let mut sorted_icons: Vec<&serde_json::Value> = icons.iter().collect();
            sorted_icons.sort_by(|a, b| {
                let size_a: u32 = a
                    .get("sizes")
                    .and_then(|s| s.as_str())
                    .and_then(|s| s.split('x').next())
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
                let size_b: u32 = b
                    .get("sizes")
                    .and_then(|s| s.as_str())
                    .and_then(|s| s.split('x').next())
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
                size_b.cmp(&size_a)
            });

            for icon in sorted_icons {
                if let Some(src) = icon.get("src").and_then(|v| v.as_str()) {
                    let icon_path = project_path.join(src);
                    if icon_path.exists() {
                        return Some(icon_path);
                    }
                }
            }
        }
    }

    None
}

fn detect_project_icon(project_path: &str) -> Option<String> {
    let path = Path::new(project_path);

    // 1. Check package.json icon field
    if let Some(pkg_icon) = check_package_json_icon(path) {
        if let Some(data_url) = read_icon_as_data_url(&pkg_icon) {
            return Some(data_url);
        }
    }

    // 2. Check manifest files
    if let Some(manifest_icon) = check_manifest_icons(path) {
        if let Some(data_url) = read_icon_as_data_url(&manifest_icon) {
            return Some(data_url);
        }
    }

    // 3. Static well-known paths
    let static_paths = [
        "favicon.ico", "favicon.png", "favicon.svg",
        "public/favicon.ico", "public/favicon.png", "public/favicon.svg",
        "static/favicon.ico", "static/favicon.png",
        "icon.png", "icon.svg", "icon.ico",
        "app-icon.png", "logo.png", "logo.svg", "logo.ico",
        ".icon.png",
        "app/favicon.ico", "app/favicon.png", "app/icon.ico", "app/icon.png", "app/icon.svg",
        "app/public/favicon.ico", "app/public/favicon.png",
        "src/favicon.ico", "src/favicon.png",
        "src/assets/icon.png", "src/assets/icon.svg", "src/assets/logo.png", "src/assets/logo.svg",
        "src/assets/favicon.ico", "src/assets/favicon.png",
        "src/app/favicon.ico", "src/app/icon.png",
        "resources/icon.png", "resources/icon.ico",
        "build/icon.png", "build/icon.ico", "build/icon.icns",
        "buildResources/icon.png", "buildResources/icon.ico",
        "assets/icon.png", "assets/logo.png",
    ];

    let mut sorted_static: Vec<&&str> = static_paths.iter().collect();
    sorted_static.sort_by(|a, b| {
        let ext_a = Path::new(a)
            .extension()
            .map(|e| format!(".{}", e.to_string_lossy()))
            .unwrap_or_default();
        let ext_b = Path::new(b)
            .extension()
            .map(|e| format!(".{}", e.to_string_lossy()))
            .unwrap_or_default();
        extension_priority(&ext_a).cmp(&extension_priority(&ext_b))
    });

    for icon_path in &sorted_static {
        let full_path = path.join(icon_path);
        if full_path.exists() {
            if let Some(data_url) = read_icon_as_data_url(&full_path) {
                return Some(data_url);
            }
        }
    }

    // 4. Recursive shallow search (max 2 levels deep)
    let mut found = find_icon_files(path, 2, 0);
    if !found.is_empty() {
        found.sort_by(|a, b| {
            let ext_a = a
                .extension()
                .map(|e| format!(".{}", e.to_string_lossy()))
                .unwrap_or_default();
            let ext_b = b
                .extension()
                .map(|e| format!(".{}", e.to_string_lossy()))
                .unwrap_or_default();
            extension_priority(&ext_a).cmp(&extension_priority(&ext_b))
        });
        if let Some(data_url) = read_icon_as_data_url(&found[0]) {
            return Some(data_url);
        }
    }

    None
}

// ── Transaction helper ───────────────────────────────────────────────────

/// Run a closure inside a SQL transaction. Commits on success, rolls back on error.
fn with_transaction<T, F>(conn: &rusqlite::Connection, f: F) -> Result<T, String>
where
    F: FnOnce() -> Result<T, String>,
{
    conn.execute_batch("BEGIN").map_err(|e| format!("Failed to begin transaction: {}", e))?;
    match f() {
        Ok(val) => {
            conn.execute_batch("COMMIT").map_err(|e| format!("Failed to commit: {}", e))?;
            Ok(val)
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

// ── Helper to get next tab order ─────────────────────────────────────────

fn next_tab_order(conn: &rusqlite::Connection) -> i64 {
    let projects = Project::list(conn).unwrap_or_default();
    projects.iter().map(|p| p.tab_order).max().unwrap_or(-1) + 1
}

// ── Tauri Commands ──────────────────────────────────────────────────────

#[tauri::command]
pub fn projects_list(state: State<'_, AppState>) -> Result<Vec<Project>, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    Project::list(&conn).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn projects_create(
    app: AppHandle,
    state: State<'_, AppState>,
    name: String,
    path: String,
    color: Option<String>,
) -> Result<Project, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let project_id = uuid::Uuid::new_v4().to_string();
    let workspace_id = uuid::Uuid::new_v4().to_string();
    let tab_order = next_tab_order(&conn);
    let color = color.unwrap_or_else(|| "#3b82f6".to_string());

    let result = with_transaction(&conn, || {
        Project::create(&conn, &project_id, &name, &path, &color, tab_order, 1, None, None)
            .map_err(|e| e.to_string())?;

        Workspace::create(&conn, &workspace_id, &project_id, None, "branch", Some("main"), "main", 0, None)
            .map_err(|e| e.to_string())?;

        Project::get(&conn, &project_id).map_err(|e| e.to_string())
    })?;
    let _ = app.emit("sync:projects", ());
    Ok(result)
}

#[tauri::command]
pub fn projects_update(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    name: Option<String>,
    color: Option<String>,
    tab_order: Option<i64>,
    worktree_mode: Option<i64>,
    pinned: Option<i64>,
    manually_active: Option<i64>,
    icon_url: Option<String>,
) -> Result<Project, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;

    // icon_url: Some("data:...") sets it, Some("") clears it, None leaves unchanged
    let icon_param = icon_url.as_ref().map(|url| {
        if url.is_empty() { None } else { Some(url.as_str()) }
    });

    Project::update(
        &conn,
        &id,
        name.as_deref(),
        None,
        color.as_deref(),
        tab_order,
        worktree_mode,
        icon_param,
        None,
        pinned,
        manually_active,
    )
    .map_err(|e| e.to_string())?;
    let result = Project::get(&conn, &id).map_err(|e| e.to_string())?;
    let _ = app.emit("sync:projects", ());
    Ok(result)
}

#[tauri::command]
pub fn projects_enable_worktrees(
    app: AppHandle,
    state: State<'_, AppState>,
    project_id: String,
) -> Result<Project, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;

    // Get the project
    let project = Project::get(&conn, &project_id).map_err(|e| e.to_string())?;

    let result = with_transaction(&conn, || {
        // Set worktree_mode = 1
        Project::update(&conn, &project_id, None, None, None, None, Some(1), None, None, None, None)
            .map_err(|e| e.to_string())?;

        // Get existing workspace records
        let existing_workspaces = Workspace::list(&conn, &project_id).unwrap_or_default();

        // Scan disk for existing git worktrees
        let worktrees = crate::git::list_worktrees(&project.path);

        for wt in &worktrees {
            if wt.is_main {
                if let Some(main_ws) = existing_workspaces.iter().find(|ws| ws.type_ == "branch" || ws.type_ == "default") {
                    if main_ws.branch.as_deref() != Some(&wt.branch) {
                        conn.execute(
                            "UPDATE workspaces SET branch = ?1, name = ?2 WHERE id = ?3",
                            rusqlite::params![wt.branch, wt.branch, main_ws.id],
                        ).map_err(|e| e.to_string())?;
                    }
                    if main_ws.type_ != "branch" {
                        conn.execute(
                            "UPDATE workspaces SET type = 'branch' WHERE id = ?1",
                            rusqlite::params![main_ws.id],
                        ).map_err(|e| e.to_string())?;
                    }
                }
            } else {
                let already_tracked = existing_workspaces.iter().any(|ws| {
                    ws.worktree_path.as_deref() == Some(&wt.path)
                });

                if !already_tracked {
                    let ws_id = uuid::Uuid::new_v4().to_string();
                    let max_order = existing_workspaces.iter().map(|w| w.tab_order).max().unwrap_or(-1) + 1;
                    Workspace::create(
                        &conn, &ws_id, &project_id, None, "worktree",
                        Some(&wt.branch), &wt.branch, max_order, Some(&wt.path),
                    ).map_err(|e| e.to_string())?;
                }
            }
        }

        Project::get(&conn, &project_id).map_err(|e| e.to_string())
    })?;
    let _ = app.emit("sync:projects", ());
    Ok(result)
}

#[tauri::command]
pub fn projects_delete(app: AppHandle, state: State<'_, AppState>, id: String) -> Result<(), String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    // Delete workspaces first (cascade)
    conn.execute("DELETE FROM workspaces WHERE project_id = ?1", rusqlite::params![id])
        .map_err(|e| e.to_string())?;
    Project::delete(&conn, &id).map_err(|e| e.to_string())?;
    let _ = app.emit("sync:projects", ());
    Ok(())
}

#[tauri::command]
pub fn projects_reorder(app: AppHandle, state: State<'_, AppState>, ids: Vec<String>) -> Result<(), String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    for (i, id) in ids.iter().enumerate() {
        Project::update(&conn, id, None, None, None, Some(i as i64), None, None, None, None, None)
            .map_err(|e| e.to_string())?;
    }
    let _ = app.emit("sync:projects", ());
    Ok(())
}

#[derive(serde::Serialize)]
#[serde(untagged)]
pub enum AddFromPathResult {
    NeedsGitInit {
        #[serde(rename = "needsGitInit")]
        needs_git_init: bool,
        path: String,
        name: String,
    },
    Project(Project),
}

#[tauri::command]
pub fn projects_add_from_path(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
) -> Result<AddFromPathResult, String> {
    let p = Path::new(&path);
    let name = p
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.clone());

    // Check if the path is a git repository
    let git_dir = p.join(".git");
    let is_git_repo = if git_dir.exists() {
        true
    } else {
        // Double-check with git rev-parse
        Command::new("git")
            .args(["rev-parse", "--show-toplevel"])
            .current_dir(&path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    };

    if !is_git_repo {
        return Ok(AddFromPathResult::NeedsGitInit {
            needs_git_init: true,
            path,
            name,
        });
    }

    if !p.exists() {
        return Err(format!("Path does not exist: {}", path));
    }

    // Validate no overlap with existing projects or worktrees
    // Use path-with-separator to avoid false positives like "K2SO" matching "K2SO-website"
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let existing_projects = Project::list(&conn).unwrap_or_default();
    let path_with_sep = format!("{}/", path.trim_end_matches('/'));
    for ep in &existing_projects {
        if path == ep.path {
            return Err(format!("This folder is already added as workspace '{}'.", ep.name));
        }
        let ep_with_sep = format!("{}/", ep.path.trim_end_matches('/'));
        if path_with_sep.starts_with(&ep_with_sep) || ep_with_sep.starts_with(&path_with_sep) {
            let workspaces = Workspace::list(&conn, &ep.id).unwrap_or_default();
            for ws in &workspaces {
                if let Some(ref wt_path) = ws.worktree_path {
                    let wt_with_sep = format!("{}/", wt_path.trim_end_matches('/'));
                    if path_with_sep.starts_with(&wt_with_sep) || wt_with_sep.starts_with(&path_with_sep) {
                        return Err(format!(
                            "This folder overlaps with a worktree in workspace '{}'. Remove it first or choose a different folder.",
                            ep.name
                        ));
                    }
                }
            }
        }
    }

    let project_id = uuid::Uuid::new_v4().to_string();
    let workspace_id = uuid::Uuid::new_v4().to_string();
    let tab_order = next_tab_order(&conn);

    // Detect actual current branch
    let current_branch = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(&path)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "main".to_string());

    // Default worktree_mode = 0 (disabled); user can enable in settings
    let result = with_transaction(&conn, || {
        Project::create(
            &conn, &project_id, &name, &path, "#3b82f6", tab_order, 0, None, None,
        )
        .map_err(|e| e.to_string())?;

        Workspace::create(
            &conn, &workspace_id, &project_id, None, "branch", Some(&current_branch), &current_branch, 0, None,
        )
        .map_err(|e| e.to_string())?;

        // Reconcile focus group from .k2so/config.json if present
        let config = project_config::get_project_config(&path);
        if let Some(ref focus_group_name) = config.focus_group_name {
            reconcile_focus_group(&conn, &project_id, focus_group_name)?;
        }

        let project = Project::get(&conn, &project_id).map_err(|e| e.to_string())?;
        Ok(AddFromPathResult::Project(project))
    })?;
    let _ = app.emit("sync:projects", ());
    Ok(result)
}

fn reconcile_focus_group(
    conn: &rusqlite::Connection,
    project_id: &str,
    focus_group_name: &str,
) -> Result<(), String> {
    // Try to find existing group by name
    let groups = FocusGroup::list(conn).map_err(|e| e.to_string())?;
    let existing = groups.iter().find(|g| g.name == focus_group_name);

    let group_id = if let Some(g) = existing {
        g.id.clone()
    } else {
        // Auto-create
        let new_id = uuid::Uuid::new_v4().to_string();
        let max_order = groups.iter().map(|g| g.tab_order).max().unwrap_or(-1) + 1;
        FocusGroup::create(conn, &new_id, focus_group_name, None, max_order)
            .map_err(|e| e.to_string())?;
        new_id
    };

    Project::update(conn, project_id, None, None, None, None, None, None, Some(Some(group_id.as_str())), None, None)
        .map_err(|e| e.to_string())?;

    Ok(())
}

#[tauri::command]
pub fn projects_add_without_git(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
) -> Result<Project, String> {
    let name = Path::new(&path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.clone());

    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let project_id = uuid::Uuid::new_v4().to_string();
    let workspace_id = uuid::Uuid::new_v4().to_string();
    let tab_order = next_tab_order(&conn);

    let result = with_transaction(&conn, || {
        Project::create(
            &conn, &project_id, &name, &path, "#3b82f6", tab_order, 0, None, None,
        )
        .map_err(|e| e.to_string())?;

        Workspace::create(
            &conn, &workspace_id, &project_id, None, "default", None, &name, 0, None,
        )
        .map_err(|e| e.to_string())?;

        Project::get(&conn, &project_id).map_err(|e| e.to_string())
    })?;
    let _ = app.emit("sync:projects", ());
    Ok(result)
}

#[tauri::command]
pub fn projects_init_git_and_open(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
    branch: Option<String>,
) -> Result<Project, String> {
    let branch_name = branch
        .filter(|b| !b.trim().is_empty())
        .unwrap_or_else(|| "main".to_string());

    // Run git init
    let init_output = Command::new("git")
        .args(["init", &format!("--initial-branch={}", branch_name)])
        .current_dir(&path)
        .output()
        .map_err(|e| format!("Failed to run git init: {}", e))?;

    if !init_output.status.success() {
        let stderr = String::from_utf8_lossy(&init_output.stderr);
        return Err(format!("Failed to initialize git: {}", stderr));
    }

    // Create initial empty commit
    let commit_output = Command::new("git")
        .args(["commit", "--allow-empty", "-m", "Initial commit"])
        .current_dir(&path)
        .output()
        .map_err(|e| format!("Failed to run git commit: {}", e))?;

    if !commit_output.status.success() {
        let stderr = String::from_utf8_lossy(&commit_output.stderr);
        if stderr.contains("user.email") || stderr.contains("user.name") {
            return Err(
                "Git user not configured. Run:\n  git config --global user.name \"Your Name\"\n  git config --global user.email \"you@example.com\""
                    .to_string(),
            );
        }
        return Err(format!("Failed to create initial commit: {}", stderr));
    }

    let name = Path::new(&path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.clone());

    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let project_id = uuid::Uuid::new_v4().to_string();
    let workspace_id = uuid::Uuid::new_v4().to_string();
    let tab_order = next_tab_order(&conn);

    // Default worktree_mode = 0 (disabled)
    let result = with_transaction(&conn, || {
        Project::create(
            &conn, &project_id, &name, &path, "#3b82f6", tab_order, 0, None, None,
        )
        .map_err(|e| e.to_string())?;

        Workspace::create(
            &conn, &workspace_id, &project_id, None, "branch", Some(&branch_name), &branch_name, 0, None,
        )
        .map_err(|e| e.to_string())?;

        Project::get(&conn, &project_id).map_err(|e| e.to_string())
    })?;
    let _ = app.emit("sync:projects", ());
    Ok(result)
}

#[tauri::command]
pub async fn projects_pick_folder(app: tauri::AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = std::sync::mpsc::channel();
    app.dialog()
        .file()
        .set_title("Select Project Folder")
        .pick_folder(move |path| {
            let _ = tx.send(path.map(|p| p.to_string()));
        });
    rx.recv()
        .map_err(|e| e.to_string())?
        .map_or(Ok(None), |p| Ok(Some(p)))
}

#[tauri::command]
pub fn projects_open_in_finder(path: String) -> Result<(), String> {
    Command::new("open")
        .arg("-R")
        .arg(&path)
        .spawn()
        .map_err(|e| format!("Failed to open Finder: {}", e))?;
    Ok(())
}

#[derive(serde::Serialize)]
pub struct IconResult {
    pub found: bool,
    pub data_url: Option<String>,
}

#[tauri::command]
pub fn projects_get_icon(
    state: State<'_, AppState>,
    path: String,
    project_id: Option<String>,
) -> Result<IconResult, String> {
    // Check DB first if projectId provided
    if let Some(ref pid) = project_id {
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        if let Ok(project) = Project::get(&conn, pid) {
            if let Some(ref icon_url) = project.icon_url {
                return Ok(IconResult {
                    found: true,
                    data_url: Some(icon_url.clone()),
                });
            }
        }
    }

    // Run filesystem detection
    if let Some(data_url) = detect_project_icon(&path) {
        // Cache in DB if projectId provided
        if let Some(ref pid) = project_id {
            let conn = state.db.lock().map_err(|e| e.to_string())?;
            Project::update(
                &conn, pid, None, None, None, None, None,
                Some(Some(data_url.as_str())), None, None, None,
            )
            .ok();
        }
        return Ok(IconResult {
            found: true,
            data_url: Some(data_url),
        });
    }

    Ok(IconResult {
        found: false,
        data_url: None,
    })
}

#[tauri::command]
pub fn projects_detect_icon(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<IconResult, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let project = Project::get(&conn, &project_id).map_err(|e| e.to_string())?;

    if let Some(data_url) = detect_project_icon(&project.path) {
        Project::update(
            &conn, &project_id, None, None, None, None, None,
            Some(Some(data_url.as_str())), None, None, None,
        )
        .map_err(|e| e.to_string())?;
        Ok(IconResult {
            found: true,
            data_url: Some(data_url),
        })
    } else {
        Ok(IconResult {
            found: false,
            data_url: None,
        })
    }
}

#[tauri::command]
pub async fn projects_upload_icon(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    project_id: String,
) -> Result<IconResult, String> {
    use tauri_plugin_dialog::DialogExt;

    // Verify project exists
    {
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        Project::get(&conn, &project_id).map_err(|_| "Project not found".to_string())?;
    }

    let (tx, rx) = std::sync::mpsc::channel();
    app.dialog()
        .file()
        .set_title("Select Icon Image")
        .add_filter("Images", &["png", "jpg", "jpeg", "svg", "ico", "icns"])
        .pick_file(move |path| {
            let _ = tx.send(path.map(|p| p.to_string()));
        });

    let selected = rx.recv().map_err(|e| e.to_string())?.map_or(Ok(None), |p| Ok::<_, String>(Some(p)))?;

    match selected {
        None => Ok(IconResult { found: false, data_url: None }),
        Some(file_path) => {
            let data_url = read_icon_as_data_url(Path::new(&file_path))
                .ok_or("Could not read the selected image")?;

            let conn = state.db.lock().map_err(|e| e.to_string())?;
            Project::update(
                &conn, &project_id, None, None, None, None, None,
                Some(Some(data_url.as_str())), None, None, None,
            )
            .map_err(|e| e.to_string())?;

            let _ = app.emit("sync:projects", ());
            Ok(IconResult {
                found: true,
                data_url: Some(data_url),
            })
        }
    }
}

#[tauri::command]
pub fn projects_clear_icon(
    app: AppHandle,
    state: State<'_, AppState>,
    project_id: String,
) -> Result<(), String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    Project::update(
        &conn, &project_id, None, None, None, None, None,
        Some(None), None, None, None,
    )
    .map_err(|e| e.to_string())?;
    let _ = app.emit("sync:projects", ());
    Ok(())
}

#[tauri::command]
pub fn projects_touch_interaction(
    state: State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    Project::touch_interaction(&conn, &id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn projects_touch_interaction_clear(
    state: State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    Project::clear_interaction(&conn, &id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn projects_open_in_editor(editor_id: String, path: String) -> Result<(), String> {
    editors::open_in_editor(&editor_id, &path)
}

#[tauri::command]
pub fn projects_get_editors() -> Result<Vec<EditorInfo>, String> {
    Ok(editors::get_installed_editors())
}

#[tauri::command]
pub fn projects_open_in_terminal(terminal_app: String, path: String) -> Result<(), String> {
    editors::open_in_terminal(&terminal_app, &path)
}

#[tauri::command]
pub fn projects_get_all_editors() -> Result<Vec<EditorInfo>, String> {
    Ok(editors::get_all_editors())
}

#[tauri::command]
pub fn projects_refresh_editors() -> Result<Vec<EditorInfo>, String> {
    Ok(editors::clear_editor_cache())
}

#[tauri::command]
pub async fn projects_open_focus_window(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    project_id: String,
) -> Result<serde_json::Value, String> {
    use tauri::Manager;
    use tauri::WebviewWindowBuilder;

    // Look up the project name from DB
    let project_name = {
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        let project = crate::db::schema::Project::get(&conn, &project_id)
            .map_err(|e| e.to_string())?;
        project.name
    };

    // Check if a focus window already exists for this project
    let label = format!("focus-{}", project_id);
    if let Some(existing) = app.get_webview_window(&label) {
        existing.set_focus().map_err(|e| e.to_string())?;
        return Ok(serde_json::json!({ "focused": true }));
    }

    // Build the webview URL with focus hash
    // In dev mode, use the dev server URL; in production, use the default app URL
    let webview_url = if cfg!(debug_assertions) {
        let url_str = format!("http://localhost:5173#focus={}", project_id);
        tauri::WebviewUrl::External(
            url::Url::parse(&url_str).map_err(|e| e.to_string())?,
        )
    } else {
        // In production, use the app's dist URL with a hash fragment
        tauri::WebviewUrl::App(format!("index.html#focus={}", project_id).into())
    };

    // Create a new focus window
    let _window = WebviewWindowBuilder::new(
        &app,
        &label,
        webview_url,
    )
    .title(&project_name)
    .inner_size(1200.0, 800.0)
    .min_inner_size(600.0, 400.0)
    .hidden_title(true)
    .title_bar_style(tauri::TitleBarStyle::Overlay)
    .build()
    .map_err(|e| e.to_string())?;

    Ok(serde_json::json!({ "opened": true }))
}

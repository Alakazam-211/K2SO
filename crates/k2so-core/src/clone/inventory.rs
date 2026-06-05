//! Inventory pass — resolve PROJECT + SLUG and enumerate the three state
//! locations into a structured [`CloneInventory`].

use super::scrub::classify_secret;
use super::{CloneInventory, CloneOptions, DestinationClass, InventoryEntry};
use crate::chat_history::claude_project_hash;
use std::path::{Path, PathBuf};

/// Bulk directories never worth migrating — build outputs, dependency
/// caches, OS junk. Extends the spirit of `projects_ops`/`file_index`'s
/// skip-lists. Applied INSIDE nested git repos too (their `node_modules`,
/// etc.). NOTE: `.git` is intentionally NOT here — a nested repo's `.git`
/// rides along as a direct file copy so it arrives a functioning clone.
const SKIP_DIRS: &[&str] = &[
    "node_modules",
    "dist",
    "out",
    ".next",
    "build",
    ".cache",
    ".turbo",
    ".vercel",
    ".auth", // login/session state (also secret-classified)
    "screenshots",
    "artifacts",
    "coverage",
    "__pycache__",
];

/// Bulk file globs / names dropped everywhere.
fn is_bulk_file(name: &str) -> bool {
    name == ".DS_Store"
        || name == "Thumbs.db"
        || name == "desktop.ini"
        || name.ends_with(".log")
}

/// Resolve PROJECT + SLUG, enumerate the workspace tree (excludes +
/// secret scrub), collect the memory dir, and collect the live session
/// `.jsonl`(s). See module + struct docs for the rules.
///
/// `project_path` is resolved to an absolute, canonical-ish path; SLUG is
/// computed from that resolved path so the on-disk `~/.claude/projects/
/// <slug>/` matches what Claude Code wrote.
pub fn inventory(project_path: &str, opts: CloneOptions) -> Result<CloneInventory, String> {
    let project = resolve_project_path(project_path)?;
    let project_str = project.to_string_lossy().to_string();
    let slug = claude_project_hash(&project_str);

    let mut entries: Vec<InventoryEntry> = Vec::new();
    let mut scrubbed: Vec<String> = Vec::new();

    // ── 1. Workspace tree ────────────────────────────────────────────
    collect_workspace(&project, &opts, &mut entries, &mut scrubbed)?;

    // ── 2 + 3. Memory + sessions (shared slug dir) ───────────────────
    let home = resolve_home(&opts)?;
    let projects_dir = home.join(".claude").join("projects");
    let slug_dir = projects_dir.join(&slug);

    collect_memory(&slug_dir, &mut entries);
    collect_sessions(&projects_dir, &slug, &opts, &mut entries)?;

    scrubbed.sort();
    scrubbed.dedup();

    Ok(CloneInventory {
        project_path: project_str,
        slug,
        entries,
        scrubbed_secrets: scrubbed,
    })
}

/// Canonicalize the project path. Falls back to an absolutized
/// (non-canonical) form if the path doesn't yet exist on disk, so the
/// caller still gets a stable SLUG.
fn resolve_project_path(project_path: &str) -> Result<PathBuf, String> {
    let p = Path::new(project_path);
    if let Ok(canon) = std::fs::canonicalize(p) {
        return Ok(canon);
    }
    if p.is_absolute() {
        Ok(p.to_path_buf())
    } else {
        let cwd = std::env::current_dir()
            .map_err(|e| format!("cannot resolve cwd for relative project path: {e}"))?;
        Ok(cwd.join(p))
    }
}

/// Home dir: explicit override (tests) else `dirs::home_dir()`.
fn resolve_home(opts: &CloneOptions) -> Result<PathBuf, String> {
    if let Some(h) = &opts.home_override {
        return Ok(h.clone());
    }
    dirs::home_dir().ok_or_else(|| "cannot resolve home directory".to_string())
}

/// Walk the PROJECT tree with the `ignore` crate (honors `.gitignore` /
/// `.ignore` / `.k2soignore`), apply the bulk skip-list (also inside
/// nested git repos), and secret-classify every surviving file.
fn collect_workspace(
    project: &Path,
    opts: &CloneOptions,
    entries: &mut Vec<InventoryEntry>,
    scrubbed: &mut Vec<String>,
) -> Result<(), String> {
    if !project.is_dir() {
        return Err(format!(
            "project path is not a directory: {}",
            project.display()
        ));
    }

    let walker = ignore::WalkBuilder::new(project)
        .hidden(false) // we WANT .k2so/.claude/.git etc.
        .git_ignore(true)
        .git_global(false)
        .git_exclude(true)
        .follow_links(false)
        .require_git(false)
        // `.k2soignore` as a custom ignore file name (same syntax as
        // .gitignore); `ignore` reads `.ignore` by default but not this.
        .add_custom_ignore_filename(".k2soignore")
        .filter_entry(|entry| {
            if entry.depth() == 0 {
                return true;
            }
            // Only directory components are bulk-skipped wholesale.
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            if is_dir {
                let name = entry.file_name().to_string_lossy();
                return !SKIP_DIRS.iter().any(|&s| s.eq_ignore_ascii_case(&name));
            }
            true
        })
        .build();

    for entry in walker {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        if entry.depth() == 0 {
            continue;
        }
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if is_dir {
            continue; // entries are files; dirs are recreated implicitly.
        }
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if is_bulk_file(&name) {
            continue;
        }
        let rel = match path.strip_prefix(project) {
            Ok(r) => r.to_string_lossy().to_string(),
            Err(_) => continue,
        };

        // Secret classification (unless carrying secrets).
        if !opts.carry_secrets {
            if let Some(_reason) = classify_secret(&rel, path) {
                scrubbed.push(rel);
                continue;
            }
        }

        entries.push(InventoryEntry {
            abs_path: path.to_path_buf(),
            rel_path: rel,
            class: DestinationClass::Workspace,
        });
    }

    Ok(())
}

/// Collect the ENTIRE `<slug>/memory/` directory (`MEMORY.md` +
/// every `*.md`, recursively). Missing dir → no memory entries.
fn collect_memory(slug_dir: &Path, entries: &mut Vec<InventoryEntry>) {
    let memory_dir = slug_dir.join("memory");
    if !memory_dir.is_dir() {
        return;
    }
    let walker = ignore::WalkBuilder::new(&memory_dir)
        .hidden(false)
        .standard_filters(false) // memory is curated; copy it whole.
        .follow_links(false)
        .build();
    for entry in walker {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        if entry.depth() == 0 {
            continue;
        }
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let path = entry.path();
        // rel is relative to the memory dir → re-rooted under `memory/`
        // in the bundle.
        if let Ok(rel) = path.strip_prefix(&memory_dir) {
            entries.push(InventoryEntry {
                abs_path: path.to_path_buf(),
                rel_path: rel.to_string_lossy().to_string(),
                class: DestinationClass::Memory,
            });
        }
    }
}

/// Collect session `.jsonl`(s). Default: the newest-mtime live session in
/// the slug dir. With `include_all_history`: every `*.jsonl` in the slug
/// dir AND in each `<slug>-<branch>/` worktree variant dir.
///
/// `rel_path` for a session is relative to `projects_dir` (i.e. it
/// includes the `<slug>/` or `<slug>-<branch>/` prefix) so the remote can
/// place it under its recomputed slug while preserving the branch suffix.
fn collect_sessions(
    projects_dir: &Path,
    slug: &str,
    opts: &CloneOptions,
    entries: &mut Vec<InventoryEntry>,
) -> Result<(), String> {
    if !projects_dir.is_dir() {
        return Ok(());
    }

    // Candidate dirs: the slug dir + any `<slug>-<branch>/` variant.
    let mut session_dirs: Vec<PathBuf> = Vec::new();
    let slug_dir = projects_dir.join(slug);
    if slug_dir.is_dir() {
        session_dirs.push(slug_dir);
    }
    if opts.include_all_history {
        if let Ok(rd) = std::fs::read_dir(projects_dir) {
            for ent in rd.flatten() {
                let name = ent.file_name().to_string_lossy().to_string();
                if name.starts_with(&format!("{slug}-")) && ent.path().is_dir() {
                    session_dirs.push(ent.path());
                }
            }
        }
    }

    // (abs_path, rel_to_projects_dir, mtime)
    let mut found: Vec<(PathBuf, String, std::time::SystemTime)> = Vec::new();
    for dir in &session_dirs {
        let rd = match std::fs::read_dir(dir) {
            Ok(rd) => rd,
            Err(_) => continue,
        };
        for ent in rd.flatten() {
            let path = ent.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            if !path.is_file() {
                continue;
            }
            let mtime = ent
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            let rel = match path.strip_prefix(projects_dir) {
                Ok(r) => r.to_string_lossy().to_string(),
                Err(_) => continue,
            };
            found.push((path, rel, mtime));
        }
    }

    if found.is_empty() {
        return Ok(());
    }

    if opts.include_all_history {
        for (abs, rel, _) in found {
            entries.push(InventoryEntry {
                abs_path: abs,
                rel_path: rel,
                class: DestinationClass::Session,
            });
        }
    } else {
        // Live = newest mtime across the slug dir.
        found.sort_by(|a, b| b.2.cmp(&a.2));
        let (abs, rel, _) = found.into_iter().next().unwrap();
        entries.push(InventoryEntry {
            abs_path: abs,
            rel_path: rel,
            class: DestinationClass::Session,
        });
    }

    Ok(())
}

//! Workspace SKILL scaffolding + one-shot migration helpers.
//!
//! Phase 2 Unit 7b — relocation from `src-tauri/src/commands/k2so_agents.rs`
//! into `k2so-core` so the daemon can run these on its own first boot
//! without Tauri being present. The Tauri-side `#[tauri::command]`
//! wrappers in `commands/k2so_agents.rs` remain as thin forwards into
//! this module (deleted entirely by Unit 7c).
//!
//! Two clusters of functionality live here:
//!
//! 1. **One-shot migration helpers** (post-0.32.x heartbeat + filename
//!    layout fixups). Each is idempotent and gated by an on-disk
//!    sentinel or row-existence check so re-running on every boot is
//!    cheap and safe.
//!
//! 2. **SKILL scaffolding** (the regen orchestrator that writes
//!    `.k2so/skills/k2so/SKILL.md`, fans it out to every harness
//!    discovery path, adopts SOURCE-region drift back into PROJECT.md /
//!    AGENT.md, archives any pre-existing user-authored files, and
//!    stamps content-hash drift baselines).
//!
//! Neither cluster touches Tauri state, AppHandle, or Emitter — the
//! whole thing is host-agnostic by construction, so the Unit 1
//! `WorkspaceRegenProvider` trait pattern wasn't needed.

use std::fs;
use std::path::{Path, PathBuf};

use crate::agents::skill_writer::force_symlink;
use crate::agents::wake::strip_frontmatter;
use crate::fs_atomic::{atomic_write_str, log_if_err};
// Phase 2.5d: archive utilities live in `crate::workspace::migrations`
// (pub(crate)) after the split.
use crate::workspace::migrations::{
    archive_claude_md_file, inject_first_migration_banner,
};
// Phase 2.5d: SKILL.md regen cluster moved to
// `crate::workspace::skill_writer`. The private helper
// `import_claude_md_into_user_notes` is pub(crate) there so this module
// (still hosting harness scaffolding through Tier A) can call it.
use crate::workspace::skill_writer::import_claude_md_into_user_notes;
// Phase 2.5d: back-compat re-exports. Migration helpers moved to
// `crate::workspace::migrations`; existing call sites
// (`k2so_daemon::main`, `projects_ops`, etc.) still spell the path
// `crate::agents::workspace::<fn>`. Retire these aliases together with
// `agents/` in Tier C.
pub use crate::workspace::migrations::{
    archive_orphan_top_tier_agents, detect_interrupted_regen, ensure_workspace_wakeups,
    harvest_per_agent_claude_md_files, migrate_filenames_to_uppercase,
    migrate_or_scaffold_lead_heartbeat, promote_legacy_heartbeat, repair_mismigrated_heartbeats,
};
// Phase 2.5d: SKILL.md regen cluster re-exports. Same back-compat rule
// as above. `disable_workspace_claude_md` stays in this file for now
// (until Tier A.3 moves it into the harness cluster).
pub use crate::workspace::skill_writer::{
    append_workspace_source_regions, ensure_all_skills_up_to_date, regenerate_workspace_skill,
    strip_workspace_skill_tail, write_workspace_skill_file,
    write_workspace_skill_file_with_body, SKILL_USER_NOTES_SENTINEL, USER_NOTES_PLACEHOLDER,
};


// ══════════════════════════════════════════════════════════════════════
// Constants
// ══════════════════════════════════════════════════════════════════════

/// The six workspace-root files K2SO can take over via symlink / scaffold.
/// On teardown we walk this list and either freeze the current SKILL.md
/// body into each as a real file (keep_current mode), or restore the
/// archive from `.k2so/migration/` (restore_original mode).
pub const HARNESS_WORKSPACE_FILES: &[&str] = &[
    "CLAUDE.md",
    "GEMINI.md",
    "AGENT.md",
    ".goosehints",
    "SKILL.md",
    ".cursor/rules/k2so.mdc",
    // NOT .aider.conf.yml — that's a config file with merged entries,
    // handled separately below.
];

// ══════════════════════════════════════════════════════════════════════
// Public types
// ══════════════════════════════════════════════════════════════════════

/// Summary returned to the UI after a teardown. One entry per file
/// touched, with a human-readable note so the dialog can show what
/// happened.
#[derive(serde::Serialize, Debug)]
pub struct TeardownResult {
    pub action: String,
    pub path: String,
    pub note: String,
}

/// User's choice when they remove/disconnect a workspace.
///
/// - `keep_current`: freeze the current canonical SKILL.md body into each
///   symlinked file as a real file. Every CLI LLM the user had enabled
///   keeps working, reading the last-known merged context. Best when
///   the user is stepping away but wants their tools to still have context.
///
/// - `restore_original`: replace each symlinked file with whatever was
///   there before K2SO took over (from `.k2so/migration/`). Files K2SO
///   created fresh (no archive) are deleted. The workspace looks like
///   it did pre-K2SO except for the `.k2so/` folder, which stays
///   intact as the restore source (and the reconnect-later safety net).
///
/// In both modes `.k2so/` itself is preserved. Nothing is destroyed.
#[derive(Clone, Copy, Debug)]
pub enum TeardownMode {
    KeepCurrent,
    RestoreOriginal,
}

/// One entry in the Add-Workspace preview. Mirrors what the CLI's
/// `k2so workspace preview` reports, but structured for the UI.
#[derive(serde::Serialize, Debug)]
pub struct WorkspacePreviewEntry {
    pub path: String,
    pub action: String, // "archive_and_import" | "refresh" | "create" | "marker_injected"
    pub size_bytes: Option<u64>,
    pub note: String,
}

// ══════════════════════════════════════════════════════════════════════
// Extended harness file-discovery coverage
// ══════════════════════════════════════════════════════════════════════

/// Create a symlink for a workspace-root harness file with the contract:
///   1. Archive the original to `.k2so/migration/` (never destroy).
///   2. Import its body into SKILL.md's USER_NOTES so the new symlinked
///      SKILL.md still surfaces the user's accumulated context.
///   3. Replace the target with the symlink.
fn safe_symlink_harness_file(
    canonical: &Path,
    target: &Path,
    project_path: &str,
    harness_display: &str,
) {
    match fs::symlink_metadata(target) {
        Ok(meta) if meta.file_type().is_symlink() => {
            force_symlink(canonical, target);
        }
        Ok(meta) if meta.file_type().is_file() => {
            let content = fs::read_to_string(target).unwrap_or_default();
            let filename = target
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(harness_display)
                .to_string();
            let archived = archive_claude_md_file(project_path, target, &filename);
            if !content.trim().is_empty() {
                let archive_display = archived
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "(archive unavailable)".to_string());
                import_claude_md_into_user_notes(
                    project_path,
                    &content,
                    &format!("pre-existing {}", harness_display),
                    &archive_display,
                );
            }
            force_symlink(canonical, target);
            if let Some(p) = archived {
                inject_first_migration_banner(project_path, &[p]);
            }
        }
        _ => {
            force_symlink(canonical, target);
        }
    }
}

/// Workspace-level harness file-discovery targets.
///
/// Phase 2.5d: `pub(crate)` so the sibling
/// [`crate::workspace::skill_writer`] can invoke it from
/// `write_workspace_skill_file_with_body` while harness cluster still
/// lives in this file (moves out in Tier A.3).
pub(crate) fn write_workspace_harness_discovery_targets(project_path: &str, canonical: &Path) {
    let root = PathBuf::from(project_path);

    safe_symlink_harness_file(canonical, &root.join("GEMINI.md"), project_path, "GEMINI.md");
    safe_symlink_harness_file(canonical, &root.join("AGENT.md"), project_path, "AGENT.md");
    safe_symlink_harness_file(canonical, &root.join(".goosehints"), project_path, ".goosehints");

    write_cursor_rules_mdc(project_path, canonical);
    scaffold_aider_conf(project_path);
}

/// Generate `./.cursor/rules/k2so.mdc` with MDC frontmatter + the
/// canonical SKILL.md body.
fn write_cursor_rules_mdc(project_path: &str, canonical: &Path) {
    let Ok(raw) = fs::read_to_string(canonical) else { return };
    let body = strip_frontmatter(&raw).trim().to_string();
    if body.is_empty() {
        return;
    }

    let dir = PathBuf::from(project_path).join(".cursor").join("rules");
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    let target = dir.join("k2so.mdc");

    const K2SO_MDC_SIGNATURE: &str = "k2so_generated: true";

    if target.exists() {
        if let Ok(existing) = fs::read_to_string(&target) {
            let is_our_output = existing.contains(K2SO_MDC_SIGNATURE);
            if !is_our_output {
                let existing_body = strip_frontmatter(&existing).trim().to_string();
                if !existing_body.is_empty() {
                    let archived = archive_claude_md_file(
                        project_path,
                        &target,
                        "cursor/rules/k2so.mdc",
                    );
                    let archive_display = archived
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "(archive unavailable)".to_string());
                    import_claude_md_into_user_notes(
                        project_path,
                        &existing_body,
                        "pre-existing .cursor/rules/k2so.mdc",
                        &archive_display,
                    );
                }
            }
        }
    }

    let mdc = format!(
        "---\n{signature}\ndescription: K2SO workspace context — CLI reference + project context + primary agent persona\nalwaysApply: true\n---\n\n{body}\n",
        signature = K2SO_MDC_SIGNATURE,
        body = body,
    );
    log_if_err(
        "write_cursor_rules_mdc",
        &target,
        atomic_write_str(&target, &mdc),
    );
}

/// Scaffold `./.aider.conf.yml` with `read: [SKILL.md]` so Aider pulls
/// the workspace context on every session.
fn scaffold_aider_conf(project_path: &str) {
    let path = PathBuf::from(project_path).join(".aider.conf.yml");
    if !path.exists() {
        log_if_err(
            "scaffold_aider_conf create",
            &path,
            atomic_write_str(
                &path,
                "# K2SO: ship workspace context to Aider on every session.\nread:\n  - SKILL.md\n",
            ),
        );
        return;
    }
    let Ok(existing) = fs::read_to_string(&path) else { return };
    if existing.contains("SKILL.md") {
        return;
    }

    let _ = archive_claude_md_file(project_path, &path, ".aider.conf.yml");

    let lines: Vec<&str> = existing.lines().collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len() + 4);
    let mut injected = false;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim_start();
        if !injected && (trimmed == "read:" || trimmed.starts_with("read:")) {
            out.push(line.to_string());
            let indent: String = line.chars().take_while(|c| c.is_whitespace()).collect();
            out.push(format!("{}  - SKILL.md", indent));
            out.push(format!("{}  # ^ added by K2SO — workspace context", indent));
            injected = true;
            i += 1;
            continue;
        }
        out.push(line.to_string());
        i += 1;
    }
    if !injected {
        if !out.last().map(|l| l.trim().is_empty()).unwrap_or(true) {
            out.push(String::new());
        }
        out.push("# K2SO: ship workspace context on every session.".to_string());
        out.push("read:".to_string());
        out.push("  - SKILL.md".to_string());
    }
    let mut final_out = out.join("\n");
    if !final_out.ends_with('\n') {
        final_out.push('\n');
    }
    log_if_err(
        "scaffold_aider_conf merge",
        &path,
        atomic_write_str(&path, &final_out),
    );
}

// ══════════════════════════════════════════════════════════════════════
// Workspace teardown (disconnect)
// ══════════════════════════════════════════════════════════════════════

/// Freeze or restore every workspace-root symlink, returning a per-file
/// summary the UI can display.
pub fn teardown_workspace_harness_files(
    project_path: &str,
    mode: TeardownMode,
) -> Vec<TeardownResult> {
    let root = PathBuf::from(project_path);
    let canonical = root.join(".k2so/skills/k2so/SKILL.md");
    let current_body = fs::read_to_string(&canonical).unwrap_or_default();
    let mut results: Vec<TeardownResult> = Vec::new();

    for rel in HARNESS_WORKSPACE_FILES {
        let path = root.join(rel);
        let Ok(meta) = fs::symlink_metadata(&path) else { continue };
        if !meta.file_type().is_symlink() {
            continue;
        }

        match mode {
            TeardownMode::KeepCurrent => match atomic_write_str(&path, &current_body) {
                Ok(()) => results.push(TeardownResult {
                    action: "froze".to_string(),
                    path: rel.to_string(),
                    note: "Replaced symlink with a frozen snapshot of the current SKILL.md. Tool will keep reading this context.".to_string(),
                }),
                Err(e) => results.push(TeardownResult {
                    action: "failed".to_string(),
                    path: rel.to_string(),
                    note: format!(
                        "Could not write frozen snapshot ({}); original symlink left intact.",
                        e
                    ),
                }),
            },
            TeardownMode::RestoreOriginal => match find_latest_archive(project_path, rel) {
                Some(archive_path) => match fs::read_to_string(&archive_path) {
                    Ok(body) => match atomic_write_str(&path, &body) {
                        Ok(()) => results.push(TeardownResult {
                            action: "restored".to_string(),
                            path: rel.to_string(),
                            note: format!("Restored from archive: {}", archive_path.display()),
                        }),
                        Err(e) => results.push(TeardownResult {
                            action: "failed".to_string(),
                            path: rel.to_string(),
                            note: format!(
                                "Found archive {} but write failed: {}; symlink left intact.",
                                archive_path.display(),
                                e
                            ),
                        }),
                    },
                    Err(e) => results.push(TeardownResult {
                        action: "failed".to_string(),
                        path: rel.to_string(),
                        note: format!(
                            "Archive unreadable ({}): {}; symlink left intact.",
                            archive_path.display(),
                            e
                        ),
                    }),
                },
                None => {
                    log_if_err(
                        "restore_original trash symlink",
                        &path,
                        crate::safe_delete::trash_or_remove(&path),
                    );
                    results.push(TeardownResult {
                        action: "removed".to_string(),
                        path: rel.to_string(),
                        note: "No prior archive — K2SO created this file fresh; sent to Trash.".to_string(),
                    });
                }
            },
        }
    }

    if matches!(mode, TeardownMode::RestoreOriginal) {
        let aider_path = root.join(".aider.conf.yml");
        if let Some(archive) = find_latest_archive(project_path, ".aider.conf.yml") {
            match fs::read_to_string(&archive) {
                Ok(body) => match atomic_write_str(&aider_path, &body) {
                    Ok(()) => results.push(TeardownResult {
                        action: "restored".to_string(),
                        path: ".aider.conf.yml".to_string(),
                        note: format!("Restored from archive: {}", archive.display()),
                    }),
                    Err(e) => results.push(TeardownResult {
                        action: "failed".to_string(),
                        path: ".aider.conf.yml".to_string(),
                        note: format!(
                            "Archive {} read ok but restore write failed: {}",
                            archive.display(),
                            e
                        ),
                    }),
                },
                Err(e) => results.push(TeardownResult {
                    action: "failed".to_string(),
                    path: ".aider.conf.yml".to_string(),
                    note: format!("Archive unreadable: {}", e),
                }),
            }
        } else if aider_path.exists() {
            log_if_err(
                "teardown trash aider.conf.yml",
                &aider_path,
                crate::safe_delete::trash_or_remove(&aider_path),
            );
            results.push(TeardownResult {
                action: "removed".to_string(),
                path: ".aider.conf.yml".to_string(),
                note: "No prior archive — K2SO scaffolded this file fresh; sent to Trash.".to_string(),
            });
        }
    }

    results
}

/// Walk `.k2so/migration/` looking for the most-recent archive that
/// matches the relative harness path.
fn find_latest_archive(project_path: &str, rel: &str) -> Option<PathBuf> {
    let migration_root = PathBuf::from(project_path).join(".k2so").join("migration");
    if !migration_root.is_dir() {
        return None;
    }

    let (subdir, leaf) = match rel.rsplit_once('/') {
        Some((parent, leaf)) => (Some(parent.to_string()), leaf.to_string()),
        None => (None, rel.to_string()),
    };
    let search_dir = match &subdir {
        Some(s) => migration_root.join(s),
        None => migration_root.clone(),
    };
    if !search_dir.is_dir() {
        return None;
    }

    let (leaf_stem, leaf_ext) = match leaf.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem.to_string(), format!(".{}", ext)),
        _ => (leaf.clone(), String::new()),
    };

    let mut best: Option<(u128, PathBuf)> = None;
    if let Ok(entries) = fs::read_dir(&search_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n,
                None => continue,
            };
            let prefix = format!("{}-", leaf_stem);
            if !name.starts_with(&prefix) {
                continue;
            }
            if !leaf_ext.is_empty() && !name.ends_with(&leaf_ext) {
                continue;
            }
            let rest = &name[prefix.len()..];
            let rest = if leaf_ext.is_empty() {
                rest
            } else {
                rest.trim_end_matches(&leaf_ext[..])
            };
            let ts_str: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            let Ok(ts) = ts_str.parse::<u128>() else { continue };
            match &best {
                Some((existing_ts, _)) if ts <= *existing_ts => {}
                _ => best = Some((ts, path.clone())),
            }
        }
    }
    best.map(|(_, p)| p)
}

/// Tauri-style entrypoint: parse mode string + run teardown. Returns
/// the per-file result list ready for JSON-encoding back to the UI.
pub fn k2so_agents_teardown_workspace(
    project_path: String,
    mode: String,
) -> Result<Vec<TeardownResult>, String> {
    let m = match mode.as_str() {
        "keep_current" => TeardownMode::KeepCurrent,
        "restore_original" => TeardownMode::RestoreOriginal,
        other => return Err(format!("unknown teardown mode: {}", other)),
    };
    Ok(teardown_workspace_harness_files(&project_path, m))
}

/// Inspect a workspace path WITHOUT mutating anything. Returns a list
/// of entries describing what K2SO will do on add.
pub fn k2so_agents_preview_workspace_ingest(
    project_path: String,
) -> Result<Vec<WorkspacePreviewEntry>, String> {
    let root = PathBuf::from(&project_path);
    let mut entries: Vec<WorkspacePreviewEntry> = Vec::new();

    let collision_targets: &[(&str, &str)] = &[
        ("CLAUDE.md", "Claude Code memory"),
        ("GEMINI.md", "Gemini CLI instructions"),
        ("AGENT.md", "agent.md spec file"),
        (".goosehints", "Goose hints"),
        (".cursor/rules/k2so.mdc", "Cursor rule"),
    ];
    for (rel, label) in collision_targets {
        let path = root.join(rel);
        match fs::symlink_metadata(&path) {
            Ok(meta) if meta.file_type().is_symlink() => {
                entries.push(WorkspacePreviewEntry {
                    path: rel.to_string(),
                    action: "refresh".to_string(),
                    size_bytes: None,
                    note: format!("{} — already symlinked to K2SO canonical (will refresh)", label),
                });
            }
            Ok(meta) if meta.file_type().is_file() => {
                let is_ours = fs::read_to_string(&path)
                    .map(|s| s.contains("k2so_generated: true"))
                    .unwrap_or(false);
                if is_ours {
                    entries.push(WorkspacePreviewEntry {
                        path: rel.to_string(),
                        action: "refresh".to_string(),
                        size_bytes: Some(meta.len()),
                        note: format!("{} — K2SO-generated, will refresh in place", label),
                    });
                } else {
                    entries.push(WorkspacePreviewEntry {
                        path: rel.to_string(),
                        action: "archive_and_import".to_string(),
                        size_bytes: Some(meta.len()),
                        note: format!("{} — archive → import body into SKILL.md USER_NOTES → symlink", label),
                    });
                }
            }
            _ => {
                entries.push(WorkspacePreviewEntry {
                    path: rel.to_string(),
                    action: "create".to_string(),
                    size_bytes: None,
                    note: format!("{} — no prior file, will create symlink", label),
                });
            }
        }
    }

    let aider_path = root.join(".aider.conf.yml");
    if aider_path.is_file() {
        let already = fs::read_to_string(&aider_path)
            .map(|s| s.contains("SKILL.md"))
            .unwrap_or(false);
        let size = fs::metadata(&aider_path).ok().map(|m| m.len());
        if already {
            entries.push(WorkspacePreviewEntry {
                path: ".aider.conf.yml".to_string(),
                action: "refresh".to_string(),
                size_bytes: size,
                note: "Aider config — already references SKILL.md, no change".to_string(),
            });
        } else {
            entries.push(WorkspacePreviewEntry {
                path: ".aider.conf.yml".to_string(),
                action: "archive_and_import".to_string(),
                size_bytes: size,
                note: "Aider config — archive → merge SKILL.md into read: list (preserves other keys)".to_string(),
            });
        }
    } else {
        entries.push(WorkspacePreviewEntry {
            path: ".aider.conf.yml".to_string(),
            action: "create".to_string(),
            size_bytes: None,
            note: "Aider config — scaffold fresh with read: [SKILL.md]".to_string(),
        });
    }

    let marker_targets: &[(&str, &str)] = &[
        ("AGENTS.md", "Codex / OpenCode / Pi"),
        (".github/copilot-instructions.md", "GitHub Copilot"),
    ];
    for (rel, label) in marker_targets {
        let path = root.join(rel);
        let size = fs::metadata(&path).ok().map(|m| m.len());
        let action = if path.exists() { "marker_injected" } else { "create" };
        let note = if path.exists() {
            format!(
                "{} — K2SO block inserted between markers, your content preserved",
                label
            )
        } else {
            format!("{} — will create with K2SO block only", label)
        };
        entries.push(WorkspacePreviewEntry {
            path: rel.to_string(),
            action: action.to_string(),
            size_bytes: size,
            note,
        });
    }

    Ok(entries)
}

/// Trigger the workspace skill write for a single project on demand.
pub fn k2so_agents_run_workspace_ingest(project_path: String) -> Result<(), String> {
    harvest_per_agent_claude_md_files(&project_path);
    write_workspace_skill_file(&project_path);
    Ok(())
}



/// Remove or disable the workspace SKILL.md + CLAUDE.md symlink
/// (when the Agent toggle is turned off).
///
/// Phase 2 Unit 7d: moved from
/// `src-tauri/src/commands/k2so_agents.rs::k2so_agents_disable_workspace_claude_md`.
pub fn disable_workspace_claude_md(project_path: String) -> Result<(), String> {
    let claude_md = PathBuf::from(&project_path).join("CLAUDE.md");
    let disabled = PathBuf::from(&project_path).join(".k2so").join("CLAUDE.md.disabled");

    if claude_md.exists() {
        // Move to .k2so/ rather than delete — preserves any user edits
        fs::rename(&claude_md, &disabled)
            .map_err(|e| format!("Failed to disable CLAUDE.md: {}", e))?;
    }
    Ok(())
}

// ══════════════════════════════════════════════════════════════════════
// Migration-safety tests (Phase 7c/7d invariants)
// ══════════════════════════════════════════════════════════════════════
//
// These tests pin down the "never lose user data" contract. Every
// migration path we ship must:
//   1. Archive user-authored content before mutating or deleting it.
//   2. Be idempotent (running twice never doubles or re-loses content).
//   3. Return preserved content from strip_workspace_skill_tail so
//      append_workspace_source_regions can re-emit it losslessly.
//   4. Never stack duplicate USER_NOTES sentinels / placeholder comments.

#[cfg(test)]
mod migration_safety_tests {
    use super::*;
    use crate::workspace::skill_writer::{content_hash_of, mtime_secs, read_regen_hashes};
    use crate::agents::skill::{SKILL_BEGIN_MARKER, SKILL_END_MARKER};
    use std::path::PathBuf;
    use uuid::Uuid;

    /// Make a scratch `.k2so/` scaffold for a migration test.
    fn scratch_project() -> PathBuf {
        let dir = std::env::temp_dir()
            .join("k2so-migration-test")
            .join(Uuid::new_v4().to_string());
        fs::create_dir_all(dir.join(".k2so/skills/k2so")).unwrap();
        fs::create_dir_all(dir.join(".k2so/agents")).unwrap();
        dir
    }

    #[test]
    fn archive_claude_md_never_deletes_source() {
        let proj = scratch_project();
        let root_claude = proj.join("CLAUDE.md");
        let body = "# My K2SO notes\n\nThis is my workspace context.\n";
        fs::write(&root_claude, body).unwrap();

        let archive = archive_claude_md_file(
            proj.to_str().unwrap(),
            &root_claude,
            "CLAUDE.md",
        )
        .expect("archive should succeed");

        assert!(root_claude.exists(), "archive must not delete the source");
        let archived_body = fs::read_to_string(&archive).unwrap();
        assert_eq!(archived_body, body, "archive must preserve content byte-for-byte");
        assert!(
            archive.starts_with(proj.join(".k2so").join("migration")),
            "archive path must land under .k2so/migration/, got {}",
            archive.display(),
        );
        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn harvest_per_agent_claude_md_archives_then_removes_source() {
        let proj = scratch_project();
        fs::create_dir_all(proj.join(".k2so/agents/backend-eng")).unwrap();
        let agent_claude = proj.join(".k2so/agents/backend-eng/CLAUDE.md");
        let body = "# backend-eng persona\n\nUser-authored memory.\n";
        fs::write(&agent_claude, body).unwrap();

        harvest_per_agent_claude_md_files(proj.to_str().unwrap());

        assert!(!agent_claude.exists(), "per-agent CLAUDE.md should be removed after harvest");
        let archive_root = proj.join(".k2so/migration/agents/backend-eng");
        let entries: Vec<_> = fs::read_dir(&archive_root).unwrap().flatten().collect();
        assert_eq!(entries.len(), 1, "expected exactly one archive, got {:?}", entries);
        let archived = fs::read_to_string(entries[0].path()).unwrap();
        assert_eq!(archived, body, "archive must preserve content byte-for-byte");
        assert!(
            proj.join(".k2so/.harvest-0.32.7-done").exists(),
            "harvest sentinel must be written"
        );
        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn harvest_is_idempotent_even_if_file_regenerated_later() {
        let proj = scratch_project();
        fs::create_dir_all(proj.join(".k2so/agents/backend-eng")).unwrap();
        let agent_claude = proj.join(".k2so/agents/backend-eng/CLAUDE.md");
        fs::write(&agent_claude, "first content").unwrap();

        harvest_per_agent_claude_md_files(proj.to_str().unwrap());

        fs::write(&agent_claude, "user-regenerated content").unwrap();

        harvest_per_agent_claude_md_files(proj.to_str().unwrap());

        assert!(agent_claude.exists(), "second run must not re-harvest");
        assert_eq!(fs::read_to_string(&agent_claude).unwrap(), "user-regenerated content");
        let archive_root = proj.join(".k2so/migration/agents/backend-eng");
        let entries: Vec<_> = fs::read_dir(&archive_root).unwrap().flatten().collect();
        assert_eq!(entries.len(), 1, "idempotent harvest must not double-archive");
        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn strip_tail_preserves_user_freeform_but_discards_placeholders() {
        let proj = scratch_project();
        let canonical = proj.join(".k2so/skills/k2so/SKILL.md");
        let corrupted = format!(
            "---\nk2so_skill: workspace\n---\n\n{begin}\nManaged body\n{end}\n\n{sentinel}\n{placeholder}\n\n{sentinel}\n{placeholder}\n\nMy real user note line 1.\nMy real user note line 2.\n",
            begin = SKILL_BEGIN_MARKER,
            end = SKILL_END_MARKER,
            sentinel = SKILL_USER_NOTES_SENTINEL,
            placeholder = USER_NOTES_PLACEHOLDER,
        );
        fs::write(&canonical, &corrupted).unwrap();

        let preserved = strip_workspace_skill_tail(proj.to_str().unwrap());

        let preserved = preserved.expect("user freeform should be preserved");
        assert!(
            preserved.contains("My real user note line 1"),
            "user line 1 should survive, got: {:?}",
            preserved
        );
        assert!(
            preserved.contains("My real user note line 2"),
            "user line 2 should survive, got: {:?}",
            preserved
        );
        assert!(
            !preserved.contains(USER_NOTES_PLACEHOLDER),
            "placeholder comments must be stripped from preserved content"
        );
        let post = fs::read_to_string(&canonical).unwrap();
        assert!(
            post.ends_with(&format!("{}\n", SKILL_END_MARKER)),
            "file must end at the managed END marker after strip"
        );
        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn strip_tail_returns_none_when_tail_is_empty_or_placeholder_only() {
        let proj = scratch_project();
        let canonical = proj.join(".k2so/skills/k2so/SKILL.md");
        let noise = format!(
            "{begin}\nManaged\n{end}\n\n{sentinel}\n{placeholder}\n",
            begin = SKILL_BEGIN_MARKER,
            end = SKILL_END_MARKER,
            sentinel = SKILL_USER_NOTES_SENTINEL,
            placeholder = USER_NOTES_PLACEHOLDER,
        );
        fs::write(&canonical, &noise).unwrap();

        let preserved = strip_workspace_skill_tail(proj.to_str().unwrap());
        assert!(
            preserved.is_none(),
            "pure K2SO noise must not be preserved as user content"
        );
        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn migration_banner_is_idempotent_and_appends_new_archives() {
        let proj = scratch_project();
        let project_path = proj.to_str().unwrap();
        let first_archive = proj.join(".k2so/migration/round-1.md");
        let second_archive = proj.join(".k2so/migration/round-2.md");
        fs::create_dir_all(first_archive.parent().unwrap()).unwrap();
        fs::write(&first_archive, "round 1").unwrap();
        fs::write(&second_archive, "round 2").unwrap();

        inject_first_migration_banner(project_path, &[first_archive.clone()]);

        let notice_path = proj.join(".k2so/MIGRATION-0.32.7.md");
        assert!(notice_path.exists(), "migration notice must be created");
        let after_first = fs::read_to_string(&notice_path).unwrap();
        assert!(after_first.contains("round-1"), "first archive must be referenced");
        let first_len = after_first.len();

        inject_first_migration_banner(project_path, &[second_archive.clone()]);
        let after_second = fs::read_to_string(&notice_path).unwrap();
        assert!(after_second.starts_with(&after_first), "append must preserve existing content");
        assert!(after_second.len() > first_len, "second invocation must grow the file");
        assert!(after_second.contains("round-2"), "second archive must be appended");

        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn safe_symlink_archives_existing_regular_file() {
        let proj = scratch_project();
        let canonical = proj.join(".k2so/skills/k2so/SKILL.md");
        let canonical_body = format!(
            "---\nk2so_skill: workspace\n---\n\n{begin}\nManaged body\n{end}\n\n{sentinel}\n{placeholder}\n",
            begin = SKILL_BEGIN_MARKER,
            end = SKILL_END_MARKER,
            sentinel = SKILL_USER_NOTES_SENTINEL,
            placeholder = USER_NOTES_PLACEHOLDER,
        );
        fs::write(&canonical, &canonical_body).unwrap();
        let target = proj.join("GEMINI.md");
        fs::write(&target, "user authored Gemini instructions").unwrap();

        safe_symlink_harness_file(
            &canonical,
            &target,
            proj.to_str().unwrap(),
            "GEMINI.md",
        );

        let meta = fs::symlink_metadata(&target).unwrap();
        assert!(meta.file_type().is_symlink(), "target must be a symlink after safe-link");
        let linked_body = fs::read_to_string(&target).unwrap();
        assert!(linked_body.contains("Managed body"), "managed region must survive import");
        assert!(
            linked_body.contains("user authored Gemini instructions"),
            "user's pre-existing body must be imported into canonical so the symlink still surfaces it"
        );
        let migration_dir = proj.join(".k2so/migration");
        let entries: Vec<_> = std::fs::read_dir(&migration_dir).unwrap().flatten().collect();
        let has_archive = entries.iter().any(|e| {
            let p = e.path();
            let body = fs::read_to_string(&p).unwrap_or_default();
            body == "user authored Gemini instructions"
        });
        assert!(
            has_archive,
            "pre-existing user file must be archived before symlink replaces it"
        );
        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn import_claude_md_lands_in_user_notes_and_is_idempotent() {
        let proj = scratch_project();
        let canonical = proj.join(".k2so/skills/k2so/SKILL.md");
        let seeded = format!(
            "---\nk2so_skill: workspace\n---\n\n{begin}\nManaged body\n{end}\n\n{sentinel}\n{placeholder}\n",
            begin = SKILL_BEGIN_MARKER,
            end = SKILL_END_MARKER,
            sentinel = SKILL_USER_NOTES_SENTINEL,
            placeholder = USER_NOTES_PLACEHOLDER,
        );
        fs::write(&canonical, &seeded).unwrap();

        let user_body = "# My Claude memory\n\nA useful note about my codebase.";
        import_claude_md_into_user_notes(
            proj.to_str().unwrap(),
            user_body,
            "pre-existing user-authored CLAUDE.md",
            "/tmp/fake/archive.md",
        );

        let after_first = fs::read_to_string(&canonical).unwrap();
        assert!(
            after_first.contains("A useful note about my codebase."),
            "imported body must land in SKILL.md"
        );
        assert!(
            after_first.contains("<!-- K2SO:IMPORT:CLAUDE_MD archive=/tmp/fake/archive.md -->"),
            "import sentinel must be written"
        );
        let user_notes_pos = after_first.find(SKILL_USER_NOTES_SENTINEL).unwrap();
        let import_pos = after_first.find("A useful note").unwrap();
        assert!(import_pos > user_notes_pos, "import must be below USER_NOTES sentinel");

        import_claude_md_into_user_notes(
            proj.to_str().unwrap(),
            user_body,
            "pre-existing user-authored CLAUDE.md",
            "/tmp/fake/archive.md",
        );
        let after_second = fs::read_to_string(&canonical).unwrap();
        assert_eq!(after_first, after_second, "re-import with same archive must be idempotent");

        import_claude_md_into_user_notes(
            proj.to_str().unwrap(),
            "another body",
            "upgrade-era CLAUDE.md",
            "/tmp/fake/archive-2.md",
        );
        let after_third = fs::read_to_string(&canonical).unwrap();
        assert!(after_third.contains("another body"), "second archive must be imported");
        assert!(
            after_third.contains("<!-- K2SO:IMPORT:CLAUDE_MD archive=/tmp/fake/archive-2.md -->"),
            "second import sentinel must be present"
        );
        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn workspace_remove_then_readd_leaves_data_intact() {
        let proj = scratch_project();
        let project_path = proj.to_str().unwrap();
        fs::create_dir_all(proj.join(".k2so/agents/backend-eng")).unwrap();
        let agent_claude = proj.join(".k2so/agents/backend-eng/CLAUDE.md");
        fs::write(&agent_claude, "backend agent notes").unwrap();

        harvest_per_agent_claude_md_files(project_path);

        let archive_dir = proj.join(".k2so/migration/agents/backend-eng");
        let archive_files: Vec<_> = fs::read_dir(&archive_dir).unwrap().flatten().collect();
        assert_eq!(archive_files.len(), 1, "first launch should archive once");
        let archived_body = fs::read_to_string(archive_files[0].path()).unwrap();
        assert_eq!(archived_body, "backend agent notes");

        harvest_per_agent_claude_md_files(project_path);

        let archive_files_after: Vec<_> = fs::read_dir(&archive_dir).unwrap().flatten().collect();
        assert_eq!(
            archive_files_after.len(),
            1,
            "re-add must not duplicate archives (sentinel gates re-harvest)"
        );
        let archived_after = fs::read_to_string(archive_files_after[0].path()).unwrap();
        assert_eq!(archived_after, "backend agent notes", "archive content must survive remove+re-add");
        assert!(
            proj.join(".k2so/.harvest-0.32.7-done").exists(),
            "sentinel persists across remove+re-add (it's filesystem, not DB)"
        );
        fs::remove_dir_all(&proj).ok();
    }

    /// Build a mock workspace that looks like the user was using every
    /// supported CLI LLM already.
    fn mock_multi_harness_workspace() -> PathBuf {
        let proj = scratch_project();
        fs::write(proj.join("CLAUDE.md"), "# Claude memory\nMy codebase notes from # memory writes.\n").unwrap();
        fs::write(proj.join("GEMINI.md"), "# Gemini instructions\nCustom Gemini behavior for this repo.\n").unwrap();
        fs::write(proj.join("AGENT.md"), "# AGENT.md\nAgent persona customizations.\n").unwrap();
        fs::write(proj.join(".goosehints"), "Goose hints — how to navigate this codebase.\n").unwrap();
        fs::write(
            proj.join(".aider.conf.yml"),
            "# Existing Aider config\nmodel: gpt-4o\nread:\n  - CONVENTIONS.md\n  - ARCHITECTURE.md\n",
        )
        .unwrap();
        fs::create_dir_all(proj.join(".opencode/agent")).unwrap();
        fs::write(
            proj.join(".opencode/agent/my-refactor-helper.md"),
            "# My custom OpenCode agent\nSpecialized refactoring persona.\n",
        )
        .unwrap();
        fs::create_dir_all(proj.join(".cursor/rules")).unwrap();
        fs::write(
            proj.join(".cursor/rules/my-codebase.mdc"),
            "---\nalwaysApply: true\n---\nMy project-specific Cursor rule.\n",
        )
        .unwrap();
        fs::write(
            proj.join(".k2so/PROJECT.md"),
            "# K2SO\n\nTauri workspace manager. Rust backend + React 19 frontend.\n",
        )
        .unwrap();
        proj
    }

    #[test]
    fn add_workspace_ingests_all_harness_files_into_skill_and_archives() {
        let proj = mock_multi_harness_workspace();
        let project_path = proj.to_str().unwrap();

        write_workspace_skill_file_with_body(project_path, None);

        let canonical = proj.join(".k2so/skills/k2so/SKILL.md");
        assert!(canonical.exists(), "canonical SKILL.md must be written");
        let skill_body = fs::read_to_string(&canonical).unwrap();

        for name in ["CLAUDE.md", "GEMINI.md", "AGENT.md", ".goosehints", "SKILL.md"] {
            let path = proj.join(name);
            let meta = fs::symlink_metadata(&path).unwrap();
            assert!(
                meta.file_type().is_symlink(),
                "{} should be a symlink after ingest, got {:?}",
                name,
                meta.file_type(),
            );
        }

        let migration_root = proj.join(".k2so/migration");
        let mut found_archives = 0;
        if let Ok(entries) = fs::read_dir(&migration_root) {
            for e in entries.flatten() {
                let path = e.path();
                if path.is_file() {
                    found_archives += 1;
                }
            }
        }
        assert!(
            found_archives >= 4,
            "expected archives for CLAUDE.md/GEMINI.md/AGENT.md/.goosehints at least, got {}",
            found_archives,
        );

        assert!(
            skill_body.contains("My codebase notes from # memory writes"),
            "CLAUDE.md body not imported into SKILL.md USER_NOTES"
        );
        assert!(
            skill_body.contains("Custom Gemini behavior for this repo"),
            "GEMINI.md body not imported into SKILL.md USER_NOTES"
        );
        assert!(
            skill_body.contains("Agent persona customizations"),
            "root AGENT.md body not imported into SKILL.md USER_NOTES"
        );
        assert!(
            skill_body.contains("Goose hints"),
            ".goosehints body not imported into SKILL.md USER_NOTES"
        );

        assert!(
            proj.join(".opencode/agent/my-refactor-helper.md").exists(),
            "user's OpenCode agent files must be preserved untouched"
        );

        assert!(
            proj.join(".cursor/rules/my-codebase.mdc").exists(),
            "user's Cursor rule files must be preserved"
        );
        assert!(
            proj.join(".cursor/rules/k2so.mdc").exists(),
            "K2SO's Cursor MDC must be added"
        );

        let aider = fs::read_to_string(proj.join(".aider.conf.yml")).unwrap();
        assert!(aider.contains("SKILL.md"), "SKILL.md must be injected into Aider read: list");
        assert!(aider.contains("CONVENTIONS.md"), "existing Aider reads must be preserved");
        assert!(aider.contains("ARCHITECTURE.md"), "existing Aider reads must be preserved");
        assert!(aider.contains("model: gpt-4o"), "non-read keys must be preserved");

        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn add_workspace_is_idempotent_second_launch_imports_nothing_new() {
        let proj = mock_multi_harness_workspace();
        let project_path = proj.to_str().unwrap();

        write_workspace_skill_file_with_body(project_path, None);
        let first_body = fs::read_to_string(proj.join(".k2so/skills/k2so/SKILL.md")).unwrap();

        write_workspace_skill_file_with_body(project_path, None);
        let second_body = fs::read_to_string(proj.join(".k2so/skills/k2so/SKILL.md")).unwrap();

        let first_imports = first_body.matches("<!-- K2SO:IMPORT:CLAUDE_MD archive=").count();
        let second_imports = second_body.matches("<!-- K2SO:IMPORT:CLAUDE_MD archive=").count();
        assert_eq!(
            first_imports, second_imports,
            "second launch must not re-import (sentinel should block duplicate adds)"
        );

        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn teardown_keep_current_freezes_symlinks_into_real_files() {
        let proj = mock_multi_harness_workspace();
        let project_path = proj.to_str().unwrap();
        write_workspace_skill_file_with_body(project_path, None);
        let canonical_body = fs::read_to_string(proj.join(".k2so/skills/k2so/SKILL.md")).unwrap();

        let results = teardown_workspace_harness_files(project_path, TeardownMode::KeepCurrent);
        assert!(!results.is_empty(), "teardown should report at least one action");
        assert!(
            results.iter().all(|r| r.action == "froze"),
            "keep_current should produce only 'froze' actions: {:?}",
            results
        );

        for name in ["CLAUDE.md", "GEMINI.md", "AGENT.md", ".goosehints", "SKILL.md"] {
            let path = proj.join(name);
            let meta = fs::symlink_metadata(&path).expect(name);
            assert!(
                !meta.file_type().is_symlink(),
                "{} must no longer be a symlink after teardown(keep_current)",
                name,
            );
            assert!(meta.file_type().is_file(), "{} must be a regular file", name);
            let body = fs::read_to_string(&path).unwrap();
            assert_eq!(body, canonical_body, "{} must contain the frozen SKILL.md body", name);
        }

        assert!(proj.join(".k2so/skills/k2so/SKILL.md").exists());
        assert!(proj.join(".k2so/migration").is_dir());
        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn teardown_restore_original_brings_back_every_archive() {
        let proj = mock_multi_harness_workspace();
        let project_path = proj.to_str().unwrap();
        let pre_claude = fs::read_to_string(proj.join("CLAUDE.md")).unwrap();
        let pre_gemini = fs::read_to_string(proj.join("GEMINI.md")).unwrap();
        let pre_agent = fs::read_to_string(proj.join("AGENT.md")).unwrap();
        let pre_goose = fs::read_to_string(proj.join(".goosehints")).unwrap();
        let pre_aider = fs::read_to_string(proj.join(".aider.conf.yml")).unwrap();

        write_workspace_skill_file_with_body(project_path, None);
        let results = teardown_workspace_harness_files(project_path, TeardownMode::RestoreOriginal);
        assert!(!results.is_empty(), "teardown should report actions");

        assert_eq!(fs::read_to_string(proj.join("CLAUDE.md")).unwrap(), pre_claude);
        assert_eq!(fs::read_to_string(proj.join("GEMINI.md")).unwrap(), pre_gemini);
        assert_eq!(fs::read_to_string(proj.join("AGENT.md")).unwrap(), pre_agent);
        assert_eq!(fs::read_to_string(proj.join(".goosehints")).unwrap(), pre_goose);
        assert_eq!(fs::read_to_string(proj.join(".aider.conf.yml")).unwrap(), pre_aider);

        assert!(!proj.join("SKILL.md").exists(), "SKILL.md should be removed on restore (no prior original)");

        assert!(proj.join(".k2so/skills/k2so/SKILL.md").exists());
        assert!(proj.join(".k2so/migration").is_dir());
        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn reconnect_after_restore_original_reingests_cleanly() {
        let proj = mock_multi_harness_workspace();
        let project_path = proj.to_str().unwrap();

        write_workspace_skill_file_with_body(project_path, None);
        teardown_workspace_harness_files(project_path, TeardownMode::RestoreOriginal);
        write_workspace_skill_file_with_body(project_path, None);

        assert!(fs::symlink_metadata(proj.join("CLAUDE.md")).unwrap().file_type().is_symlink());
        assert!(fs::symlink_metadata(proj.join("GEMINI.md")).unwrap().file_type().is_symlink());

        let skill_body = fs::read_to_string(proj.join(".k2so/skills/k2so/SKILL.md")).unwrap();
        assert!(skill_body.contains("My codebase notes from # memory writes"));
        assert!(skill_body.contains("Custom Gemini behavior for this repo"));

        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn teardown_leaves_k2so_dir_fully_intact() {
        let proj = mock_multi_harness_workspace();
        let project_path = proj.to_str().unwrap();
        write_workspace_skill_file_with_body(project_path, None);
        let pre_project_md = fs::read_to_string(proj.join(".k2so/PROJECT.md")).unwrap();

        let pre_paths: Vec<PathBuf> = walk_dir(&proj.join(".k2so"));
        assert!(!pre_paths.is_empty(), "expected a populated .k2so/ before teardown");

        teardown_workspace_harness_files(project_path, TeardownMode::KeepCurrent);
        let post_paths: Vec<PathBuf> = walk_dir(&proj.join(".k2so"));

        for p in &pre_paths {
            assert!(
                post_paths.contains(p),
                "{} disappeared from .k2so/ during teardown — invariant violated",
                p.display(),
            );
        }
        assert_eq!(fs::read_to_string(proj.join(".k2so/PROJECT.md")).unwrap(), pre_project_md);

        fs::remove_dir_all(&proj).ok();
    }

    fn walk_dir(root: &Path) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = fs::read_dir(&dir) else { continue };
            for e in entries.flatten() {
                let p = e.path();
                out.push(p.clone());
                if p.is_dir() && !p.is_symlink() {
                    stack.push(p);
                }
            }
        }
        out.sort();
        out
    }

    #[test]
    fn aider_conf_merge_preserves_user_reads_and_archives_original() {
        let proj = scratch_project();
        let project_path = proj.to_str().unwrap();
        let aider_path = proj.join(".aider.conf.yml");
        let original = "# my aider config\nmodel: gpt-4o\nread:\n  - CONVENTIONS.md\n  - ARCHITECTURE.md\nauto-lint: true\n";
        fs::write(&aider_path, original).unwrap();

        scaffold_aider_conf(project_path);

        let merged = fs::read_to_string(&aider_path).unwrap();
        assert!(merged.contains("SKILL.md"), "SKILL.md must be injected");
        assert!(merged.contains("CONVENTIONS.md"), "original read entries preserved");
        assert!(merged.contains("ARCHITECTURE.md"), "original read entries preserved");
        assert!(merged.contains("model: gpt-4o"), "non-read top-level keys preserved");
        assert!(merged.contains("auto-lint: true"), "non-read top-level keys preserved");

        let migration_root = proj.join(".k2so/migration");
        let mut found = false;
        if let Ok(entries) = fs::read_dir(&migration_root) {
            for e in entries.flatten() {
                if let Ok(body) = fs::read_to_string(e.path()) {
                    if body == original {
                        found = true;
                    }
                }
            }
        }
        assert!(found, "original .aider.conf.yml must be archived before mutation");

        scaffold_aider_conf(project_path);
        let second = fs::read_to_string(&aider_path).unwrap();
        assert_eq!(merged, second, "idempotent — second call must not re-inject");

        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn safe_symlink_is_idempotent_when_target_is_already_symlink() {
        let proj = scratch_project();
        let canonical = proj.join(".k2so/skills/k2so/SKILL.md");
        fs::write(&canonical, "canonical").unwrap();
        let target = proj.join(".goosehints");

        safe_symlink_harness_file(&canonical, &target, proj.to_str().unwrap(), ".goosehints");
        safe_symlink_harness_file(&canonical, &target, proj.to_str().unwrap(), ".goosehints");

        let migration_dir = proj.join(".k2so/migration");
        let entries_count = std::fs::read_dir(&migration_dir)
            .map(|r| r.flatten().count())
            .unwrap_or(0);
        assert_eq!(
            entries_count, 0,
            "symlink-to-symlink re-run must not produce spurious archive entries"
        );
        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn completed_regen_clears_in_flight_marker() {
        let proj = mock_multi_harness_workspace();
        let project_path = proj.to_str().unwrap();
        write_workspace_skill_file_with_body(project_path, None);
        let marker = proj.join(".k2so/.regen-in-flight");
        assert!(!marker.exists(), "regen marker must be cleared on successful completion");
        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn detect_interrupted_regen_flags_stale_marker_once() {
        let proj = scratch_project();
        let project_path = proj.to_str().unwrap();
        let k2so_dir = proj.join(".k2so");
        fs::create_dir_all(&k2so_dir).unwrap();
        let marker = k2so_dir.join(".regen-in-flight");
        fs::write(&marker, b"").unwrap();
        assert!(detect_interrupted_regen(project_path), "must flag the stale marker");
        assert!(!marker.exists(), "must clear the marker after surfacing the warning");
        assert!(!detect_interrupted_regen(project_path), "must not re-fire after the marker is cleared");
        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn detect_interrupted_regen_is_silent_when_no_marker() {
        let proj = scratch_project();
        fs::create_dir_all(proj.join(".k2so")).unwrap();
        assert!(!detect_interrupted_regen(proj.to_str().unwrap()));
        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn archive_names_never_collide_under_rapid_fire() {
        let proj = scratch_project();
        let project_path = proj.to_str().unwrap();
        let agents = proj.join(".k2so/agents");
        fs::create_dir_all(&agents).unwrap();
        for i in 0..10 {
            let agent_dir = agents.join(format!("agent-{}", i));
            fs::create_dir_all(&agent_dir).unwrap();
            fs::write(agent_dir.join("CLAUDE.md"), format!("body for agent-{}", i)).unwrap();
        }
        harvest_per_agent_claude_md_files(project_path);

        let mut archive_bodies = std::collections::HashSet::new();
        let migration_root = proj.join(".k2so/migration/agents");
        for i in 0..10 {
            let sub = migration_root.join(format!("agent-{}", i));
            let mut count = 0;
            if let Ok(entries) = fs::read_dir(&sub) {
                for e in entries.flatten() {
                    if let Ok(body) = fs::read_to_string(e.path()) {
                        assert!(archive_bodies.insert(body), "duplicate archive body found");
                        count += 1;
                    }
                }
            }
            assert_eq!(count, 1, "agent-{}: expected 1 archive, got {}", i, count);
        }
        assert_eq!(archive_bodies.len(), 10, "all 10 agents must have distinct archives");

        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn teardown_keep_current_leaves_file_usable_even_on_tight_retries() {
        let proj = mock_multi_harness_workspace();
        let project_path = proj.to_str().unwrap();
        write_workspace_skill_file_with_body(project_path, None);

        let _ = teardown_workspace_harness_files(project_path, TeardownMode::KeepCurrent);
        let claude = proj.join("CLAUDE.md");
        assert!(claude.exists(), "CLAUDE.md must exist after first keep_current");
        let first_body = fs::read_to_string(&claude).unwrap();
        assert!(!first_body.is_empty());

        for _ in 0..5 {
            let _ = teardown_workspace_harness_files(project_path, TeardownMode::KeepCurrent);
        }
        let final_body = fs::read_to_string(&claude).unwrap();
        assert_eq!(first_body, final_body, "repeated no-op teardowns must not mutate the frozen body");

        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn regen_stamps_content_hashes_for_drift_detection() {
        let proj = mock_multi_harness_workspace();
        let project_path = proj.to_str().unwrap();
        write_workspace_skill_file_with_body(project_path, None);

        let stamp_path = proj.join(".k2so/.last-skill-regen");
        let body = fs::read_to_string(&stamp_path).expect("stamp must exist");
        assert!(!body.trim().is_empty(), "stamp must no longer be empty (hash JSON required)");
        let parsed: std::collections::HashMap<String, String> =
            serde_json::from_str(&body).expect("stamp must parse as JSON hash map");
        assert!(parsed.contains_key("project_md"), "PROJECT.md hash must be recorded: {:?}", parsed);

        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn drift_adoption_prefers_content_hash_over_mtime() {
        let proj = mock_multi_harness_workspace();
        let project_path = proj.to_str().unwrap();
        write_workspace_skill_file_with_body(project_path, None);

        let project_md = proj.join(".k2so/PROJECT.md");
        let original = fs::read_to_string(&project_md).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        fs::write(&project_md, &original).unwrap();
        assert!(
            mtime_secs(&project_md) > mtime_secs(&proj.join(".k2so/.last-skill-regen")),
            "test setup: source mtime must be newer than regen stamp"
        );

        let hashes = read_regen_hashes(project_path);
        let stored = hashes.get("project_md").cloned().unwrap_or_default();
        let current = content_hash_of(&project_md);
        assert_eq!(stored, current, "hash-based drift detection must ignore identical content");

        fs::remove_dir_all(&proj).ok();
    }

    #[test]
    fn drift_adoption_detects_real_content_change() {
        let proj = mock_multi_harness_workspace();
        let project_path = proj.to_str().unwrap();
        write_workspace_skill_file_with_body(project_path, None);

        let project_md = proj.join(".k2so/PROJECT.md");
        fs::write(&project_md, "completely different body\n").unwrap();

        let hashes = read_regen_hashes(project_path);
        let stored = hashes.get("project_md").cloned().unwrap_or_default();
        let current = content_hash_of(&project_md);
        assert_ne!(stored, current, "hash-based drift detection must flag modified content");

        fs::remove_dir_all(&proj).ok();
    }
}



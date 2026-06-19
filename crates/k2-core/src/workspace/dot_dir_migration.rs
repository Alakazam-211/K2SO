//! 0.40.4 one-go cutover: rename each workspace's legacy `.k2so/` dot-dir
//! to `.k2/` and re-point the harness fan-out symlinks that targeted it.
//!
//! Why a rename (not a per-file move): `fs::rename` on the directory is an
//! O(1), atomic re-link of the directory entry — it doesn't matter how much
//! lives inside `.k2so/`. It also works whether or not the workspace is a
//! git repo and whether the contents are tracked or gitignored. We do NOT
//! `git mv`: that would stage changes in the user's index (intrusive) and
//! fail on untracked paths; git detects the rename by content at commit /
//! `log --follow` time anyway, so history still follows a plain rename.
//!
//! Why re-point symlinks: the harness fan-out writes symlinks at the
//! workspace root (`CLAUDE.md`, `SKILL.md`, `GEMINI.md`, `AGENT.md`,
//! `.goosehints`) and under `.claude/` `.opencode/` `.pi/` that target the
//! ABSOLUTE canonical path `<root>/.k2so/skills/<name>/SKILL.md`. The
//! instant we rename the dir those symlinks dangle, so we rewrite every one
//! whose target points into the old `.k2so/` at the new `.k2/`.
//!
//! Idempotent + safe to run every boot: a workspace that already has `.k2/`
//! is skipped untouched, so only pure-legacy `.k2so/` workspaces convert,
//! and they convert exactly once.

use std::fs;
use std::path::Path;

/// The harness subdirectories the fan-out writes symlinks into. Scanned
/// (shallow-recursive) for dangling `.k2so/` targets after a rename. We do
/// NOT walk the whole workspace tree — that could wander into node_modules
/// / vendored trees — only these K2SO-owned dirs plus the root (depth 1).
const HARNESS_SYMLINK_SUBDIRS: &[&str] = &[".claude", ".opencode", ".pi", ".cursor"];

/// Outcome of migrating one workspace — for boot logging + tests.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct DotDirMigration {
    /// `.k2so/` was renamed to `.k2/` this call.
    pub renamed: bool,
    /// How many dangling fan-out symlinks were re-pointed to `.k2/`.
    pub symlinks_repointed: usize,
    /// `.gitignore` had `.k2so/` ignore rules rewritten to `.k2/` so git
    /// treats the renamed tree the same way (previously-ignored subdirs
    /// like inbox/sessions/logs stay ignored instead of flooding `git
    /// status`, and the rename of tracked files reads as a clean diff).
    pub gitignore_rewritten: bool,
    /// Set when nothing was done; explains why (already migrated, conflict).
    pub skipped: Option<&'static str>,
}

/// Migrate one workspace root from `.k2so/` → `.k2/` (idempotent).
///
/// - No `.k2so/` dir → nothing to do (already converted, or never had one).
/// - Both `.k2/` and `.k2so/` exist → conflict; we never clobber `.k2/`,
///   so we leave both in place and report the skip (an operator can merge).
/// - Otherwise → rename, then re-point dangling fan-out symlinks.
pub fn migrate_workspace_dot_dir(root: &Path) -> DotDirMigration {
    let old = root.join(".k2so");
    let new = root.join(".k2");

    if !old.is_dir() {
        return DotDirMigration {
            skipped: Some("no legacy .k2so dir"),
            ..Default::default()
        };
    }
    if new.exists() {
        // Already-migrated workspaces don't reach here (old would be gone).
        // Both present = a genuine conflict — don't clobber the new dir.
        return DotDirMigration {
            skipped: Some("both .k2 and .k2so present — left for manual merge"),
            ..Default::default()
        };
    }

    if let Err(e) = fs::rename(&old, &new) {
        crate::log_debug!(
            "[dot-dir-migration] rename {} -> {} failed: {e}",
            old.display(),
            new.display()
        );
        return DotDirMigration {
            skipped: Some("rename failed"),
            ..Default::default()
        };
    }

    let symlinks_repointed = repoint_fanout_symlinks(root, &old, &new);
    let gitignore_rewritten = rewrite_gitignore_dot_dir(root);
    DotDirMigration {
        renamed: true,
        symlinks_repointed,
        gitignore_rewritten,
        skipped: None,
    }
}

/// Rewrite the repo-root `.gitignore` so `.k2so/` ignore rules follow the
/// rename to `.k2/`. SURGICAL: only entries whose path is the workspace
/// dot-dir (`.k2so`, `.k2so/`, `.k2so/<sub>`, optionally `/`-anchored) are
/// touched. Skill-name references that merely contain `k2so` —
/// `.cursor/rules/k2so.mdc`, `.opencode/agent/k2so*.md`, `.pi/skills/k2so/`,
/// `crates/k2so-core/...` — are LEFT ALONE (the skill is still named k2so;
/// only the dot-dir moved). Returns true if anything changed.
fn rewrite_gitignore_dot_dir(root: &Path) -> bool {
    let gi = root.join(".gitignore");
    let Ok(content) = fs::read_to_string(&gi) else {
        return false;
    };
    let mut changed = false;
    let mut out: Vec<String> = Vec::with_capacity(content.lines().count());
    for line in content.lines() {
        if line_is_dot_dir_rule(line) {
            changed = true;
            out.push(line.replacen(".k2so", ".k2", 1));
        } else {
            out.push(line.to_string());
        }
    }
    if !changed {
        return false;
    }
    let mut joined = out.join("\n");
    if content.ends_with('\n') {
        joined.push('\n');
    }
    if let Err(e) = crate::fs_atomic::atomic_write_str(&gi, &joined) {
        crate::log_debug!("[dot-dir-migration] rewrite .gitignore failed: {e}");
        return false;
    }
    true
}

/// True iff a `.gitignore` line's pattern is the `.k2so` workspace dot-dir
/// (exact, or a path under it), so it should retarget to `.k2`. Tolerates a
/// leading `/` anchor and a trailing `/`. Does NOT match `.k2so`-containing
/// substrings that are really skill/crate names (`.../k2so.mdc`,
/// `crates/k2so-core/...`).
fn line_is_dot_dir_rule(line: &str) -> bool {
    let pat = line.trim();
    if pat.is_empty() || pat.starts_with('#') {
        return false;
    }
    let pat = pat.strip_prefix('/').unwrap_or(pat);
    pat == ".k2so" || pat == ".k2so/" || pat.starts_with(".k2so/")
}

/// Re-point every K2SO fan-out symlink whose target points into `old_dir`
/// at the matching path under `new_dir`. Targeted scan: the workspace root
/// (depth 1) plus the known harness subdirs — the only places the fan-out
/// ever writes symlinks.
fn repoint_fanout_symlinks(root: &Path, old_dir: &Path, new_dir: &Path) -> usize {
    let mut fixed = 0;
    repoint_in_dir_shallow(root, old_dir, new_dir, &mut fixed);
    for sub in HARNESS_SYMLINK_SUBDIRS {
        repoint_in_dir_recursive(&root.join(sub), old_dir, new_dir, &mut fixed);
    }
    fixed
}

/// Re-point symlinks among the immediate entries of `dir` (no recursion).
fn repoint_in_dir_shallow(dir: &Path, old_dir: &Path, new_dir: &Path, fixed: &mut usize) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_symlink() {
            if repoint_one(&path, old_dir, new_dir) {
                *fixed += 1;
            }
        }
    }
}

/// Re-point symlinks under `dir`, recursing into real subdirectories
/// (bounded to the K2SO-owned harness trees — they're tiny).
fn repoint_in_dir_recursive(dir: &Path, old_dir: &Path, new_dir: &Path, fixed: &mut usize) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_symlink() {
            if repoint_one(&path, old_dir, new_dir) {
                *fixed += 1;
            }
        } else if path.is_dir() {
            repoint_in_dir_recursive(&path, old_dir, new_dir, fixed);
        }
    }
}

/// If `link`'s target starts with `old_dir`, rewrite it to point under
/// `new_dir`. Returns true when the symlink was re-pointed.
fn repoint_one(link: &Path, old_dir: &Path, new_dir: &Path) -> bool {
    let Ok(target) = fs::read_link(link) else {
        return false;
    };
    let Ok(rel) = target.strip_prefix(old_dir) else {
        return false;
    };
    let new_target = new_dir.join(rel);
    // Replace atomically-ish: remove the dangling link, recreate it.
    if fs::remove_file(link).is_err() {
        return false;
    }
    match std::os::unix::fs::symlink(&new_target, link) {
        Ok(()) => true,
        Err(e) => {
            crate::log_debug!(
                "[dot-dir-migration] re-point {} -> {} failed: {e}",
                link.display(),
                new_target.display()
            );
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "k2-dotmig-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&p).unwrap();
        p
    }

    fn symlink(target: &Path, link: &Path) {
        if let Some(parent) = link.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        std::os::unix::fs::symlink(target, link).unwrap();
    }

    #[test]
    fn renames_k2so_to_k2_and_is_idempotent() {
        let root = tmp("rename");
        fs::create_dir_all(root.join(".k2so/inbox")).unwrap();
        fs::write(root.join(".k2so/PROJECT.md"), b"hello").unwrap();

        let r1 = migrate_workspace_dot_dir(&root);
        assert!(r1.renamed, "first run renames");
        assert!(root.join(".k2").is_dir(), ".k2/ exists after migrate");
        assert!(!root.join(".k2so").exists(), ".k2so/ is gone");
        assert_eq!(
            fs::read_to_string(root.join(".k2/PROJECT.md")).unwrap(),
            "hello",
            "contents moved intact"
        );

        // Second run is a no-op.
        let r2 = migrate_workspace_dot_dir(&root);
        assert!(!r2.renamed, "second run does nothing");
        assert_eq!(r2.skipped, Some("no legacy .k2so dir"));

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn no_k2so_is_a_noop() {
        let root = tmp("nolegacy");
        let r = migrate_workspace_dot_dir(&root);
        assert!(!r.renamed);
        assert_eq!(r.skipped, Some("no legacy .k2so dir"));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn does_not_clobber_existing_k2() {
        let root = tmp("conflict");
        fs::create_dir_all(root.join(".k2so")).unwrap();
        fs::create_dir_all(root.join(".k2")).unwrap();
        fs::write(root.join(".k2/keep"), b"precious").unwrap();

        let r = migrate_workspace_dot_dir(&root);
        assert!(!r.renamed, "must not rename over an existing .k2/");
        assert!(root.join(".k2so").exists(), ".k2so/ left in place");
        assert_eq!(
            fs::read_to_string(root.join(".k2/keep")).unwrap(),
            "precious",
            ".k2/ untouched"
        );
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn rewrites_only_dot_dir_gitignore_rules() {
        let root = tmp("gitignore");
        fs::create_dir_all(root.join(".k2so")).unwrap();
        // A realistic .gitignore: dot-dir rules (must retarget) mixed with
        // skill-name / crate refs (must be untouched).
        let gi = "\
# K2 workspace
.k2so/inbox/
.k2so/sessions/
/.k2so/logs/
.k2so/.archive/
.k2so
.cursor/rules/k2so.mdc
.opencode/agent/k2so*.md
.pi/skills/k2so/
crates/k2so-core/drizzle_sql/meta/
node_modules/
";
        fs::write(root.join(".gitignore"), gi).unwrap();

        let r = migrate_workspace_dot_dir(&root);
        assert!(r.renamed);
        assert!(r.gitignore_rewritten, "gitignore should be rewritten");

        let after = fs::read_to_string(root.join(".gitignore")).unwrap();
        // Dot-dir rules retargeted.
        assert!(after.contains(".k2/inbox/"), "inbox rule retargeted");
        assert!(after.contains(".k2/sessions/"));
        assert!(after.contains("/.k2/logs/"), "anchored rule retargeted");
        assert!(after.contains(".k2/.archive/"));
        // The bare `.k2so` line becomes `.k2` (exact, on its own line).
        assert!(after.lines().any(|l| l == ".k2"), "bare .k2so -> .k2");
        // Skill-name / crate refs LEFT ALONE.
        assert!(after.contains(".cursor/rules/k2so.mdc"), "skill mdc untouched");
        assert!(after.contains(".opencode/agent/k2so*.md"), "opencode untouched");
        assert!(after.contains(".pi/skills/k2so/"), "pi skill untouched");
        assert!(
            after.contains("crates/k2so-core/drizzle_sql/meta/"),
            "crate path untouched"
        );
        // No stray `.k2so/` dot-dir rules remain.
        assert!(
            !after.lines().any(|l| line_is_dot_dir_rule(l)),
            "no .k2so dot-dir rules should remain"
        );
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn repoints_root_and_subdir_symlinks() {
        let root = tmp("symlinks");
        let canonical = root.join(".k2so/skills/k2so/SKILL.md");
        fs::create_dir_all(canonical.parent().unwrap()).unwrap();
        fs::write(&canonical, b"# skill").unwrap();

        // Root-level fan-out symlinks → absolute canonical (as force_symlink writes).
        symlink(&canonical, &root.join("CLAUDE.md"));
        symlink(&canonical, &root.join("SKILL.md"));
        // Subdir fan-out symlinks.
        symlink(&canonical, &root.join(".claude/skills/k2so/SKILL.md"));
        symlink(&canonical, &root.join(".opencode/agent/k2so.md"));
        symlink(
            &root.join(".k2so/skills/k2so/SKILL.md"),
            &root.join(".pi/skills/k2so/SKILL.md"),
        );
        // A non-K2SO symlink that must be LEFT ALONE.
        fs::write(root.join("README.md"), b"readme").unwrap();
        symlink(&root.join("README.md"), &root.join("README.link"));

        let r = migrate_workspace_dot_dir(&root);
        assert!(r.renamed);
        assert_eq!(r.symlinks_repointed, 5, "all 5 fan-out symlinks re-pointed");

        // Every fan-out symlink now resolves under .k2/ and is NOT dangling.
        for link in [
            "CLAUDE.md",
            "SKILL.md",
            ".claude/skills/k2so/SKILL.md",
            ".opencode/agent/k2so.md",
            ".pi/skills/k2so/SKILL.md",
        ] {
            let p = root.join(link);
            let target = fs::read_link(&p).unwrap();
            assert!(
                target.starts_with(root.join(".k2")),
                "{link} should target .k2/, got {}",
                target.display()
            );
            assert!(p.exists(), "{link} must resolve (not dangling)");
        }

        // The unrelated symlink is untouched.
        assert_eq!(
            fs::read_link(root.join("README.link")).unwrap(),
            root.join("README.md")
        );

        fs::remove_dir_all(&root).ok();
    }
}

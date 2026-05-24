//! Phase 2.5b follow-up: CRUD helpers over `.k2so/skills/<name>/`.
//!
//! The Tauri command surface (`src-tauri/src/commands/skills.rs`)
//! and the CLI verbs (`k2so skills list|create|profile|remove`) each
//! wrap these functions one-for-one. Underlying file shape post-2.5b:
//!
//! ```text
//! .k2so/skills/
//! ├── <name>/SKILL.md
//! └── <name>/SKILL.md
//! ```
//!
//! `SkillSummary` is the row shape both surfaces render. It's
//! intentionally narrow — name + title + mtime — so callers can list
//! cheaply without parsing the full body. Heavier fields (role,
//! inbox/active/done counts) live on `K2soAgentInfo` and remain on
//! the legacy `k2so_core::agents::commands::list` surface for the
//! existing K2SO Agents panel.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use std::time::UNIX_EPOCH;

use crate::agents::{skill_dir, skills_dir};
use crate::safe_delete;

/// Compact row for the workspace-settings Skills list. Returned by
/// [`list`] and [`create`].
///
/// `title` is the first H1 (`# foo`) in `SKILL.md` if present, else
/// `None` — the UI falls back to `name` when title is absent so empty
/// or freshly-scaffolded skills still render usefully.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkillSummary {
    /// Folder basename — also the skill id everywhere (CLI args, URLs,
    /// frontmatter `name:` field). Lowercase, alphanumeric + `-` / `_`.
    pub name: String,
    /// First `# heading` line in `SKILL.md`, with the `# ` prefix stripped
    /// and whitespace trimmed. `None` when the file has no H1.
    pub title: Option<String>,
    /// `SKILL.md` mtime as a Unix timestamp (seconds since epoch). 0 if
    /// the file is missing or the mtime is unreadable — both pathological;
    /// the UI should treat 0 as "unknown" rather than the literal 1970.
    pub last_modified: i64,
}

/// Enumerate every skill directory under `.k2so/skills/`. Alphabetical
/// by name. Hidden / dotfile dirs are skipped — `.archive` and friends
/// are housekeeping artifacts, not user-visible skills.
///
/// Missing `.k2so/skills/` returns `Ok(vec![])` rather than an error
/// because that's the steady state for pre-2.5b workspaces caught
/// before the daemon's boot-time consolidation has run. The CLI /
/// renderer can render an empty list with a "Create Skill" affordance
/// just fine.
pub fn list(project_path: &str) -> Result<Vec<SkillSummary>, String> {
    let dir = skills_dir(project_path);
    if !dir.exists() {
        return Ok(vec![]);
    }

    let entries = fs::read_dir(&dir).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for entry in entries.flatten() {
        if !entry.file_type().map_or(false, |ft| ft.is_dir()) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let skill_md = entry.path().join("SKILL.md");
        // Be tolerant of transitional shapes: a workspace mid-migration
        // may still have AGENT.md. Treat either as the profile file
        // so the UI lists every dir that has *something* in it.
        let profile_md = if skill_md.exists() {
            skill_md
        } else {
            let alt = entry.path().join("AGENT.md");
            if alt.exists() {
                alt
            } else {
                // Empty dir; surface it anyway so the user can see
                // (and clean up) zombie skills, but without a title.
                out.push(SkillSummary {
                    name,
                    title: None,
                    last_modified: 0,
                });
                continue;
            }
        };

        let title = first_h1(&profile_md);
        let last_modified = mtime_secs(&profile_md);
        out.push(SkillSummary {
            name,
            title,
            last_modified,
        });
    }

    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// Read the raw `SKILL.md` body for a single skill. Returns
/// `Err` if the skill dir doesn't exist or the file is missing —
/// callers should treat both as "the skill isn't here" and not try
/// to differentiate. Transitional `AGENT.md` is read transparently
/// for skills caught mid-migration.
pub fn profile(project_path: &str, name: &str) -> Result<String, String> {
    let dir = skill_dir(project_path, name);
    if !dir.exists() {
        return Err(format!("Skill '{}' does not exist", name));
    }
    let skill_md = dir.join("SKILL.md");
    let agent_md = dir.join("AGENT.md");
    let path = if skill_md.exists() {
        skill_md
    } else if agent_md.exists() {
        agent_md
    } else {
        return Err(format!("Skill '{}' has no SKILL.md or AGENT.md", name));
    };
    fs::read_to_string(&path).map_err(|e| e.to_string())
}

/// Create a new skill dir at `.k2so/skills/<name>/` and write a
/// `SKILL.md` seed.
///
/// `from_skill` (optional) is the name of an existing skill in this
/// workspace to copy as the starting point. Per A19's "any skill can
/// seed any other" semantic this is just `cp source/SKILL.md
/// dest/SKILL.md`, with the new skill's `name:` frontmatter rewritten
/// to match the new folder name (so the seed doesn't leak the old
/// identity into the new skill). If `from_skill` is `None`, the new
/// skill gets a minimal scaffold:
///
/// ```text
/// ---
/// name: <name>
/// role:
/// type: skill
/// ---
///
/// # <name>
///
/// _Documentation profile for <name>. Edit me._
/// ```
///
/// Name validation: alphanumeric + `-` / `_` only. Mirrors the
/// validation in `k2so_core::agents::commands::create`.
pub fn create(
    project_path: &str,
    name: &str,
    from_skill: Option<&str>,
) -> Result<SkillSummary, String> {
    if name.is_empty() {
        return Err("Skill name cannot be empty".to_string());
    }
    if !name.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
        return Err(
            "Skill name must be alphanumeric (hyphens and underscores allowed)".to_string(),
        );
    }
    let dir = skill_dir(project_path, name);
    if dir.exists() {
        return Err(format!("Skill '{}' already exists", name));
    }

    // Make sure `.k2so/skills/` itself exists — the parent of `dir`.
    if let Some(parent) = dir.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            format!("Failed to create skills root {}: {}", parent.display(), e)
        })?;
    }
    fs::create_dir_all(&dir).map_err(|e| {
        format!("Failed to create skill dir {}: {}", dir.display(), e)
    })?;

    let body = match from_skill {
        Some(seed_name) => {
            let seed = profile(project_path, seed_name).map_err(|e| {
                format!("Failed to read seed skill '{}': {}", seed_name, e)
            })?;
            rewrite_name_frontmatter(&seed, name)
        }
        None => default_scaffold(name),
    };

    let skill_md = dir.join("SKILL.md");
    fs::write(&skill_md, &body)
        .map_err(|e| format!("Failed to write {}: {}", skill_md.display(), e))?;

    Ok(SkillSummary {
        name: name.to_string(),
        title: first_h1(&skill_md),
        last_modified: mtime_secs(&skill_md),
    })
}

/// Trash a skill dir (`.k2so/skills/<name>/`) — recoverable via the
/// OS recycle bin for ~30 days. Refuses if the dir doesn't exist so
/// the UI can show a meaningful error instead of silently no-op'ing.
/// Per `feedback_recycle_bin_tests`: NEVER `fs::remove_dir_all` here.
pub fn remove(project_path: &str, name: &str) -> Result<(), String> {
    let dir = skill_dir(project_path, name);
    if !dir.exists() {
        return Err(format!("Skill '{}' does not exist", name));
    }
    safe_delete::trash(&dir).map_err(|e| format!("Failed to trash skill '{}': {}", name, e))
}

// ── Internals ────────────────────────────────────────────────────────

fn first_h1(path: &Path) -> Option<String> {
    let content = fs::read_to_string(path).ok()?;
    // Skip YAML frontmatter (the same parser shape `parse_frontmatter`
    // uses): if the file opens with `---`, walk past the next `---`.
    let body_start = if content.starts_with("---") {
        match content[3..].find("---") {
            Some(end) => 3 + end + 3,
            None => 0,
        }
    } else {
        0
    };
    for line in content[body_start..].lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("# ") {
            let title = rest.trim();
            if !title.is_empty() {
                return Some(title.to_string());
            }
        }
    }
    None
}

fn mtime_secs(path: &Path) -> i64 {
    let Ok(meta) = fs::metadata(path) else {
        return 0;
    };
    let Ok(mt) = meta.modified() else { return 0 };
    match mt.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(_) => 0,
    }
}

fn default_scaffold(name: &str) -> String {
    format!(
        "---\nname: {}\nrole:\ntype: skill\n---\n\n# {}\n\n_Documentation profile for {}. Edit me._\n",
        name, name, name
    )
}

/// When seeding a new skill from an existing one, rewrite the
/// frontmatter `name:` line to match the destination. Leaves every
/// other field (role, type, manager, etc.) alone — that's the whole
/// point of seeding.
fn rewrite_name_frontmatter(content: &str, new_name: &str) -> String {
    if !content.starts_with("---") {
        return content.to_string();
    }
    let Some(end_idx) = content[3..].find("---") else {
        return content.to_string();
    };
    let frontmatter = &content[3..3 + end_idx];
    let body = &content[3 + end_idx + 3..];

    let mut saw_name = false;
    let updated_fm: String = frontmatter
        .lines()
        .map(|line| {
            if let Some((key, _)) = line.split_once(':') {
                if key.trim() == "name" {
                    saw_name = true;
                    return format!("name: {}", new_name);
                }
            }
            line.to_string()
        })
        .collect::<Vec<_>>()
        .join("\n");

    let final_fm = if saw_name {
        updated_fm
    } else {
        format!("name: {}\n{}", new_name, updated_fm.trim_start_matches('\n'))
    };
    format!("---\n{}\n---{}", final_fm.trim(), body)
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_workspace() -> std::path::PathBuf {
        let base = env::temp_dir().join(format!(
            "k2so-skills-crud-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&base).unwrap();
        base
    }

    fn cleanup(ws: &Path) {
        let _ = std::fs::remove_dir_all(ws);
    }

    #[test]
    fn list_empty_workspace_returns_empty() {
        let ws = temp_workspace();
        let result = list(ws.to_str().unwrap()).unwrap();
        assert!(result.is_empty());
        cleanup(&ws);
    }

    #[test]
    fn create_then_list_round_trips() {
        let ws = temp_workspace();
        let summary = create(ws.to_str().unwrap(), "frontend-eng", None).unwrap();
        assert_eq!(summary.name, "frontend-eng");
        assert_eq!(summary.title.as_deref(), Some("frontend-eng"));

        let listed = list(ws.to_str().unwrap()).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "frontend-eng");
        cleanup(&ws);
    }

    #[test]
    fn create_rejects_bad_names() {
        let ws = temp_workspace();
        assert!(create(ws.to_str().unwrap(), "", None).is_err());
        assert!(create(ws.to_str().unwrap(), "has space", None).is_err());
        assert!(create(ws.to_str().unwrap(), "has/slash", None).is_err());
        cleanup(&ws);
    }

    #[test]
    fn create_rejects_duplicate() {
        let ws = temp_workspace();
        create(ws.to_str().unwrap(), "dup", None).unwrap();
        let err = create(ws.to_str().unwrap(), "dup", None).unwrap_err();
        assert!(err.contains("already exists"), "got: {}", err);
        cleanup(&ws);
    }

    #[test]
    fn create_from_seed_copies_body_and_rewrites_name() {
        let ws = temp_workspace();
        // Seed source with custom body.
        let src = ws.join(".k2so").join("skills").join("source");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(
            src.join("SKILL.md"),
            "---\nname: source\nrole: source role\n---\n\n# source\n\nseed body\n",
        )
        .unwrap();

        let summary = create(ws.to_str().unwrap(), "copy", Some("source")).unwrap();
        assert_eq!(summary.name, "copy");

        let body = profile(ws.to_str().unwrap(), "copy").unwrap();
        assert!(body.contains("name: copy"), "frontmatter not rewritten: {}", body);
        assert!(body.contains("role: source role"), "role lost: {}", body);
        assert!(body.contains("seed body"), "body not copied: {}", body);
        cleanup(&ws);
    }

    #[test]
    fn profile_reads_skill_md() {
        let ws = temp_workspace();
        create(ws.to_str().unwrap(), "alpha", None).unwrap();
        let body = profile(ws.to_str().unwrap(), "alpha").unwrap();
        assert!(body.contains("name: alpha"));
        assert!(body.contains("# alpha"));
        cleanup(&ws);
    }

    #[test]
    fn profile_falls_back_to_agent_md_during_migration() {
        let ws = temp_workspace();
        let dir = ws.join(".k2so").join("skills").join("legacy");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("AGENT.md"),
            "---\nname: legacy\n---\n\nlegacy body\n",
        )
        .unwrap();
        let body = profile(ws.to_str().unwrap(), "legacy").unwrap();
        assert!(body.contains("legacy body"));
        cleanup(&ws);
    }

    #[test]
    fn profile_missing_skill_errors() {
        let ws = temp_workspace();
        let err = profile(ws.to_str().unwrap(), "nope").unwrap_err();
        assert!(err.contains("does not exist"), "got: {}", err);
        cleanup(&ws);
    }

    #[test]
    fn list_skips_dotfile_dirs() {
        let ws = temp_workspace();
        // Real skill.
        create(ws.to_str().unwrap(), "real", None).unwrap();
        // Synthetic .archive sibling.
        std::fs::create_dir_all(ws.join(".k2so").join("skills").join(".archive")).unwrap();
        let listed = list(ws.to_str().unwrap()).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "real");
        cleanup(&ws);
    }

    #[test]
    fn first_h1_skips_frontmatter() {
        let ws = temp_workspace();
        let dir = ws.join(".k2so").join("skills").join("hh");
        std::fs::create_dir_all(&dir).unwrap();
        // Frontmatter contains a `# foo`-shaped comment that must
        // NOT win over the real H1 in the body.
        std::fs::write(
            dir.join("SKILL.md"),
            "---\nname: hh\n# not a heading\n---\n\n# Real Title\n\nbody\n",
        )
        .unwrap();
        let listed = list(ws.to_str().unwrap()).unwrap();
        assert_eq!(listed[0].title.as_deref(), Some("Real Title"));
        cleanup(&ws);
    }

    #[test]
    fn remove_errors_when_skill_missing() {
        let ws = temp_workspace();
        let err = remove(ws.to_str().unwrap(), "nope").unwrap_err();
        assert!(err.contains("does not exist"), "got: {}", err);
        cleanup(&ws);
    }

    // NOTE: a `remove_round_trips_to_trash` test would shell into the
    // macOS Trash via the `trash` crate, which fires a Finder Touch ID
    // prompt during `cargo test` on macOS. Per memory/feedback_recycle_bin_tests.md
    // we deliberately omit this test from the unit suite; the CLI / Tauri
    // path is exercised end-to-end via the parity shell test instead.
}

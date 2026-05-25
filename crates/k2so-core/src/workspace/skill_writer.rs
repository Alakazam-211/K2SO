//! Workspace SKILL.md regen + scaffolding cluster.
//!
//! Phase 2.5d: extracted from the monolithic `agents/workspace.rs`. This
//! module owns the canonical SKILL.md write flow — composing the
//! lead-agent body (manager brief or AI planner brief), writing it to
//! `.k2so/skills/k2so/SKILL.md`, adopting SOURCE-region drift back into
//! PROJECT.md / AGENT.md, archiving pre-existing user-authored files,
//! and stamping content-hash drift baselines.
//!
//! Sibling [`crate::workspace::harness`] owns the harness file-discovery
//! fan-out (symlink scaffolding, cursor MDC, aider conf merge), and
//! [`crate::workspace::teardown`] owns the disconnect/restore flow.
//! [`crate::workspace::migrations`] hosts the three archive-utility
//! helpers (`archive_claude_md_file`, `inject_first_migration_banner`,
//! `log_adoption_event`) all three modules share.


use std::fs;
use std::path::{Path, PathBuf};

use crate::agents::onboarding::is_harness_management_skipped;
use crate::agents::skill::{
    skill_checksum_hex, skill_source_agent_md_begin, skill_source_agent_md_end,
    SKILL_END_MARKER, SKILL_SOURCE_PROJECT_MD_BEGIN, SKILL_SOURCE_PROJECT_MD_END,
    SKILL_VERSION_WORKSPACE,
};
use crate::agents::skill_writer::{
    force_symlink, skill_update_footer, upsert_k2so_section, write_skill_to_all_harnesses,
};
use crate::agents::wake::strip_frontmatter;
use crate::agents::{
    agent_dir, agent_type_for, agents_dir, find_primary_agent, parse_frontmatter,
};
use crate::fs_atomic::{self, atomic_symlink, atomic_write_str, log_if_err};
use crate::log_debug;
use crate::workspace::migrations::{
    archive_claude_md_file, inject_first_migration_banner, log_adoption_event,
};

// ══════════════════════════════════════════════════════════════════════
// Constants
// ══════════════════════════════════════════════════════════════════════

/// Sentinel marker users can place in the below-END tail to claim freeform
/// content that should survive regeneration. Anything BETWEEN this marker
/// and EOF is preserved verbatim. Useful for notes the user wants to keep
/// but doesn't want routed into PROJECT.md / AGENT.md.
pub const SKILL_USER_NOTES_SENTINEL: &str = "<!-- K2SO:USER_NOTES -->";

/// The placeholder comment emitted alongside the USER_NOTES sentinel on
/// every regen. Tracked as a constant so `strip_workspace_skill_tail` can
/// discard any existing copies from the preserved freeform — otherwise
/// each regen would stack another placeholder copy onto the tail.
pub const USER_NOTES_PLACEHOLDER: &str =
    "<!-- Content below the K2SO:USER_NOTES sentinel is yours — K2SO preserves it verbatim across regenerations. -->";

// ══════════════════════════════════════════════════════════════════════
// SKILL scaffolding helpers (private to this module)
// ══════════════════════════════════════════════════════════════════════

/// Generate the workspace-level skill for users working directly with an LLM.
/// Lightweight — just the commands a human user would need when working alongside K2SO agents.
fn generate_workspace_skill_content(project_name: &str) -> String {
    format!(
r#"# K2SO Skill

This workspace ({project_name}) is managed by K2SO. You can use these commands to interact with the agent system.

## Send Work to a Workspace

Send a task to a workspace's manager for triage and execution:
```
k2so msg <workspace-name>:inbox "description of work needed"
k2so msg --wake <workspace-name>:inbox "urgent — wake the agent"
```

## View Activity Feed

See recent agent activity in this workspace:
```
k2so feed
```

## View Connections

See which workspaces are connected:
```
k2so connections list
```

## Create a Work Item

Add work to this workspace's inbox for the manager to triage:
```
k2so work create --title "Fix login bug" --body "Users can't log in after password reset" --source issue
```

## Heartbeats

The agent in this workspace can have one or more scheduled wakeups. Manage them with:
```
k2so heartbeat list                   # see configured schedules
k2so heartbeat show <name>            # full details for one
k2so heartbeat add --name <n> --daily --time HH:MM
k2so heartbeat wakeup <name> --edit   # edit the prompt that fires
k2so heartbeat wake                   # trigger a tick now
```

Run `k2so heartbeat --help` for the full surface.
"#,
        project_name = project_name,
    )
}

/// Extract the body between a BEGIN/END marker pair, if both are present.
fn extract_source_region(content: &str, begin: &str, end: &str) -> Option<String> {
    let b_idx = content.find(begin)?;
    let after_begin = b_idx + begin.len();
    let e_rel = content[after_begin..].find(end)?;
    let e_idx = after_begin + e_rel;
    Some(content[after_begin..e_idx].trim().to_string())
}

/// Strip an optional leading heading (`## Something\n\n`) from a SOURCE
/// region body so the comparison / commit targets the raw file content.
fn strip_leading_heading(body: &str) -> String {
    let trimmed = body.trim_start();
    if trimmed.starts_with("## ") {
        if let Some(nl) = trimmed.find('\n') {
            return trimmed[nl + 1..].trim_start().to_string();
        }
    }
    trimmed.to_string()
}

/// Return the mtime of a file as seconds since epoch, or 0 if unknown.
///
/// Phase 2.5d: `pub(crate)` so the migration-safety tests in
/// `agents/workspace.rs` (which still reference this helper for
/// drift-detection assertions) keep compiling through Tier A.
pub(crate) fn mtime_secs(path: &Path) -> u64 {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Content hash of a file path, suitable for drift detection. Returns an
/// empty string on read failure (callers treat empty == "no stored hash"
/// and fall back to mtime comparison).
///
/// Phase 2.5d: `pub(crate)` so the migration-safety tests can call it.
pub(crate) fn content_hash_of(path: &Path) -> String {
    match fs::read(path) {
        Ok(bytes) => skill_checksum_hex(&bytes),
        Err(_) => String::new(),
    }
}

/// Read the `.last-skill-regen` JSON payload, which stores the content
/// hashes of every source file at the time of the last regen. Used by
/// drift adoption to tell "source was edited since last regen" apart
/// from "SKILL.md was edited since last regen" — compared to the old
/// mtime-based heuristic this is immune to clock skew, NTP jumps, and
/// cross-machine rsync mtime quirks.
///
/// Phase 2.5d: `pub(crate)` so the migration-safety tests can read the
/// stamp file and assert drift-detection behavior.
pub(crate) fn read_regen_hashes(project_path: &str) -> std::collections::HashMap<String, String> {
    let stamp_path = PathBuf::from(project_path).join(".k2so").join(".last-skill-regen");
    let Ok(raw) = fs::read_to_string(&stamp_path) else {
        return std::collections::HashMap::new();
    };
    if raw.trim().is_empty() {
        return std::collections::HashMap::new();
    }
    serde_json::from_str::<std::collections::HashMap<String, String>>(&raw).unwrap_or_default()
}

/// Persist the content hashes of every source file that participates in
/// drift detection. Called at the end of a successful regen so the next
/// regen has a baseline for comparison.
fn write_regen_hashes(
    project_path: &str,
    hashes: &std::collections::HashMap<String, String>,
) {
    let stamp_path = PathBuf::from(project_path).join(".k2so").join(".last-skill-regen");
    let payload = serde_json::to_string(hashes).unwrap_or_else(|_| "{}".to_string());
    log_if_err(
        "write_regen_hashes",
        &stamp_path,
        atomic_write_str(&stamp_path, &payload),
    );
}

// ══════════════════════════════════════════════════════════════════════
// SKILL scaffolding — public entry points
// ══════════════════════════════════════════════════════════════════════

// ══════════════════════════════════════════════════════════════════════
// SKILL scaffolding — public entry points
// ══════════════════════════════════════════════════════════════════════

/// Write the workspace-level K2SO skill to all harness locations.
pub fn write_workspace_skill_file(project_path: &str) {
    write_workspace_skill_file_with_body(project_path, None);
}

/// Variant that lets callers pass a pre-composed body so that content
/// lands in the canonical SKILL.md rather than being lost when CLAUDE.md
/// collapsed to a symlink.
pub fn write_workspace_skill_file_with_body(project_path: &str, base_body: Option<&str>) {
    let project_name = std::path::Path::new(project_path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "workspace".to_string());

    let regen_marker = PathBuf::from(project_path)
        .join(".k2so")
        .join(".regen-in-flight");
    log_if_err(
        "regen-in-flight stamp",
        &regen_marker,
        fs_atomic::atomic_write(&regen_marker, b""),
    );

    // Step 1: Adoption sweep
    adopt_workspace_skill_drift(project_path);

    // Step 2: Clear stale tail
    let preserved_freeform = strip_workspace_skill_tail(project_path);

    // Step 3: Compose K2SO-managed body
    let mut managed_body = match base_body {
        Some(body) => body.to_string(),
        None => generate_workspace_skill_content(&project_name),
    };

    let primary_agent = find_primary_agent(project_path);
    if !managed_body.ends_with('\n') {
        managed_body.push('\n');
    }
    managed_body.push('\n');
    managed_body.push_str(&skill_update_footer(project_path, primary_agent.as_deref()));

    // Step 4: Write managed body
    write_skill_to_all_harnesses(
        project_path,
        "k2so",
        "workspace",
        SKILL_VERSION_WORKSPACE,
        "K2SO workspace context — CLI reference + project context + primary agent persona",
        &managed_body,
        false,
    );

    // Step 5: Append fresh SOURCE regions
    append_workspace_source_regions(project_path, preserved_freeform.as_deref());

    // Steps 6 + 7: fan out
    let canonical = PathBuf::from(project_path).join(".k2so/skills/k2so/SKILL.md");
    if !is_harness_management_skipped(project_path) {
        if let Ok(full) = fs::read_to_string(&canonical) {
            let injection_body = strip_frontmatter(&full).trim().to_string();
            let root = PathBuf::from(project_path);
            upsert_k2so_section(&root.join("AGENTS.md"), &injection_body);
            let github_dir = root.join(".github");
            let _ = fs::create_dir_all(&github_dir);
            upsert_k2so_section(&github_dir.join("copilot-instructions.md"), &injection_body);
        }

        let root_skill = PathBuf::from(project_path).join("SKILL.md");
        force_symlink(&canonical, &root_skill);
        let root_claude = PathBuf::from(project_path).join("CLAUDE.md");
        migrate_and_symlink_root_claude_md(&canonical, &root_claude, project_path);
        crate::workspace::harness::write_workspace_harness_discovery_targets(project_path, &canonical);
    }

    // Step 8: Stamp last-regen hashes
    let mut hashes: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    let project_md_path = PathBuf::from(project_path).join(".k2so").join("PROJECT.md");
    let project_hash = content_hash_of(&project_md_path);
    if !project_hash.is_empty() {
        hashes.insert("project_md".to_string(), project_hash);
    }
    if let Some(primary) = find_primary_agent(project_path) {
        let agent_md_path = agent_dir(project_path, &primary).join("AGENT.md");
        let agent_hash = content_hash_of(&agent_md_path);
        if !agent_hash.is_empty() {
            hashes.insert(format!("agent_md::{}", primary), agent_hash);
        }
    }
    write_regen_hashes(project_path, &hashes);

    log_if_err(
        "regen-in-flight clear",
        &regen_marker,
        fs::remove_file(&regen_marker),
    );
}

/// Walk the existing canonical SKILL.md and adopt any SOURCE-region drift
/// back into its canonical source file (PROJECT.md or the primary agent's
/// AGENT.md).
fn adopt_workspace_skill_drift(project_path: &str) {
    let canonical = PathBuf::from(project_path).join(".k2so/skills/k2so/SKILL.md");
    let Ok(skill_content) = fs::read_to_string(&canonical) else {
        return;
    };
    let stamp_path = PathBuf::from(project_path).join(".k2so").join(".last-skill-regen");
    let last_regen = mtime_secs(&stamp_path);
    let stored_hashes = read_regen_hashes(project_path);

    let source_touched_since_regen = |source_path: &Path, key: &str| -> bool {
        if let Some(stored) = stored_hashes.get(key) {
            let current = content_hash_of(source_path);
            !current.is_empty() && current.as_str() != stored.as_str()
        } else {
            mtime_secs(source_path) > last_regen
        }
    };

    // PROJECT.md adoption
    if let Some(region_body) = extract_source_region(
        &skill_content,
        SKILL_SOURCE_PROJECT_MD_BEGIN,
        SKILL_SOURCE_PROJECT_MD_END,
    ) {
        let project_md = PathBuf::from(project_path).join(".k2so").join("PROJECT.md");
        let region_stripped = strip_leading_heading(&region_body);
        let file_body = fs::read_to_string(&project_md)
            .map(|raw| strip_frontmatter(&raw).trim().to_string())
            .unwrap_or_default();
        if region_stripped.trim() != file_body.trim() {
            if source_touched_since_regen(&project_md, "project_md") {
                log_adoption_event(
                    project_path,
                    "PROJECT.md: user edit detected — downstream SKILL.md + harness files will pick up the new content on this regen",
                );
            } else if !region_stripped.trim().is_empty() {
                let frontmatter = if let Ok(raw) = fs::read_to_string(&project_md) {
                    if raw.starts_with("---") {
                        if let Some(end) = raw[3..].find("---") {
                            Some(raw[..3 + end + 3].to_string())
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                } else {
                    None
                };
                let new_contents = match frontmatter {
                    Some(fm) => format!("{}\n\n{}\n", fm.trim_end(), region_stripped.trim()),
                    None => format!("{}\n", region_stripped.trim()),
                };
                match atomic_write_str(&project_md, &new_contents) {
                    Ok(()) => log_adoption_event(
                        project_path,
                        "ADOPTED PROJECT.md: SKILL.md SOURCE region committed back to .k2so/PROJECT.md",
                    ),
                    Err(e) => log_if_err::<(), _>("adopt PROJECT.md", &project_md, Err::<(), _>(e)),
                }
            }
        }
    }

    // Primary agent's AGENT.md adoption
    if let Some(primary_agent) = find_primary_agent(project_path) {
        let agent_type = agent_type_for(project_path, &primary_agent);
        let include_primary = matches!(
            agent_type.as_str(),
            "custom" | "k2so" | "manager" | "coordinator" | "pod-leader"
        );
        if include_primary {
            let begin = skill_source_agent_md_begin(&primary_agent);
            let end = skill_source_agent_md_end(&primary_agent);
            if let Some(region_body) = extract_source_region(&skill_content, &begin, &end) {
                let agent_md = agent_dir(project_path, &primary_agent).join("AGENT.md");
                let region_stripped = strip_leading_heading(&region_body);
                let file_body = fs::read_to_string(&agent_md)
                    .map(|raw| strip_frontmatter(&raw).trim().to_string())
                    .unwrap_or_default();
                if region_stripped.trim() != file_body.trim() {
                    let key = format!("agent_md::{}", primary_agent);
                    if source_touched_since_regen(&agent_md, &key) {
                        log_adoption_event(
                            project_path,
                            &format!(
                                "AGENT.md ({}): user edit detected — downstream SKILL.md + harness files will pick up the new content on this regen",
                                primary_agent
                            ),
                        );
                    } else if !region_stripped.trim().is_empty() {
                        let frontmatter = if let Ok(raw) = fs::read_to_string(&agent_md) {
                            if raw.starts_with("---") {
                                if let Some(end) = raw[3..].find("---") {
                                    Some(raw[..3 + end + 3].to_string())
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        } else {
                            None
                        };
                        let new_contents = match frontmatter {
                            Some(fm) => format!("{}\n\n{}\n", fm.trim_end(), region_stripped.trim()),
                            None => format!("{}\n", region_stripped.trim()),
                        };
                        match atomic_write_str(&agent_md, &new_contents) {
                            Ok(()) => log_adoption_event(
                                project_path,
                                &format!(
                                    "ADOPTED AGENT.md ({}): SKILL.md SOURCE region committed back to agent file",
                                    primary_agent
                                ),
                            ),
                            Err(e) => log_if_err::<(), _>(
                                "adopt AGENT.md",
                                &agent_md,
                                Err::<(), _>(e),
                            ),
                        }
                    }
                }
            }
        }
    }
}

/// Remove everything after the MANAGED:END marker in the canonical SKILL.md.
/// Returns any truly user-authored content found after the LAST
/// `<!-- K2SO:USER_NOTES -->` sentinel so it can be re-appended after
/// regeneration.
pub fn strip_workspace_skill_tail(project_path: &str) -> Option<String> {
    let canonical = PathBuf::from(project_path).join(".k2so/skills/k2so/SKILL.md");
    let Ok(content) = fs::read_to_string(&canonical) else { return None };
    let end_idx = content.find(SKILL_END_MARKER)?;
    let after_end_start = end_idx + SKILL_END_MARKER.len();
    let tail = &content[after_end_start..];

    let preserved = tail.rfind(SKILL_USER_NOTES_SENTINEL).map(|idx| {
        let after = idx + SKILL_USER_NOTES_SENTINEL.len();
        tail[after..].to_string()
    });

    let truncated = format!("{}\n", &content[..after_end_start]);
    log_if_err(
        "strip_workspace_skill_tail write",
        &canonical,
        atomic_write_str(&canonical, &truncated),
    );

    let preserved = preserved.map(|raw| {
        raw.lines()
            .filter(|l| l.trim() != USER_NOTES_PLACEHOLDER.trim())
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string()
    });
    preserved.filter(|s| !s.is_empty())
}

/// After the managed region has been re-written, append fresh SOURCE
/// sub-regions below the END marker in the canonical file.
pub fn append_workspace_source_regions(project_path: &str, preserved_freeform: Option<&str>) {
    let canonical = PathBuf::from(project_path).join(".k2so/skills/k2so/SKILL.md");
    let Ok(mut content) = fs::read_to_string(&canonical) else { return };
    if !content.ends_with('\n') {
        content.push('\n');
    }

    let project_md = PathBuf::from(project_path).join(".k2so").join("PROJECT.md");
    if let Ok(raw) = fs::read_to_string(&project_md) {
        let stripped = strip_frontmatter(&raw);
        let has_content = stripped.lines().any(|line| {
            let t = line.trim();
            !t.is_empty() && !t.starts_with('#') && !t.starts_with("<!--")
        });
        if has_content {
            content.push_str(&format!(
                "\n{begin}\n## Project Context\n\n{body}\n{end}\n",
                begin = SKILL_SOURCE_PROJECT_MD_BEGIN,
                body = stripped.trim(),
                end = SKILL_SOURCE_PROJECT_MD_END,
            ));
        }
    }

    if let Some(primary_agent) = find_primary_agent(project_path) {
        let agent_type = agent_type_for(project_path, &primary_agent);
        let include_primary = matches!(
            agent_type.as_str(),
            "custom" | "k2so" | "manager" | "coordinator" | "pod-leader"
        );
        if include_primary {
            let agent_md = agent_dir(project_path, &primary_agent).join("AGENT.md");
            if let Ok(raw) = fs::read_to_string(&agent_md) {
                let stripped = strip_frontmatter(&raw).trim().to_string();
                if !stripped.is_empty() {
                    content.push_str(&format!(
                        "\n{begin}\n## Primary Agent: {name}\n\n{body}\n{end}\n",
                        begin = skill_source_agent_md_begin(&primary_agent),
                        name = primary_agent,
                        body = stripped,
                        end = skill_source_agent_md_end(&primary_agent),
                    ));
                }
            }
        }
    }

    content.push_str(&format!(
        "\n{sentinel}\n{placeholder}\n",
        sentinel = SKILL_USER_NOTES_SENTINEL,
        placeholder = USER_NOTES_PLACEHOLDER,
    ));
    if let Some(freeform) = preserved_freeform {
        let cleaned = freeform.trim();
        if !cleaned.is_empty() {
            content.push('\n');
            content.push_str(cleaned);
            content.push('\n');
        }
    }

    log_if_err(
        "append_workspace_source_regions",
        &canonical,
        atomic_write_str(&canonical, &content),
    );
}

/// CLAUDE.md migration helper for the 0.32.7 transition.
///
/// Phase 2.5d: `pub(crate)` so the sibling [`crate::workspace::harness`]
/// module (and the agents/workspace.rs shim during Tier A) can call it.
pub(crate) fn migrate_and_symlink_root_claude_md(canonical: &Path, root_claude: &Path, project_path: &str) {
    match fs::symlink_metadata(root_claude) {
        Ok(meta) if meta.file_type().is_symlink() => {
            force_symlink(canonical, root_claude);
        }
        Ok(meta) if meta.file_type().is_file() => {
            let content = fs::read_to_string(root_claude).unwrap_or_default();
            let is_k2so_generated = content.starts_with("# K2SO ");
            let archived = archive_claude_md_file(project_path, root_claude, "CLAUDE.md");
            let source_label = if is_k2so_generated {
                "pre-0.32.7 K2SO-generated CLAUDE.md"
            } else {
                "pre-existing user-authored CLAUDE.md"
            };
            let archive_display = archived
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "(archive unavailable)".to_string());
            if !content.trim().is_empty() {
                import_claude_md_into_user_notes(
                    project_path,
                    &content,
                    source_label,
                    &archive_display,
                );
            }
            log_if_err(
                "migrate_and_symlink_root_claude_md",
                root_claude,
                atomic_symlink(canonical, root_claude),
            );
            if let Some(archive_path) = archived {
                inject_first_migration_banner(project_path, &[archive_path]);
            }
            log_debug!(
                "[workspace-skill] Migrated {}./CLAUDE.md → archived + imported body into SKILL.md USER_NOTES; root CLAUDE.md now symlinks to canonical SKILL.md",
                if is_k2so_generated {
                    "K2SO-generated "
                } else {
                    "user-authored "
                },
            );
        }
        _ => {
            force_symlink(canonical, root_claude);
        }
    }
}

/// Append the body of a pre-existing CLAUDE.md into the canonical
/// SKILL.md's USER_NOTES region so the migrated content stays visible
/// to Claude Code (via the symlink) without requiring the user to hand-
/// merge from `.k2so/migration/`.
///
/// Phase 2.5d: `pub(crate)` so the sibling [`crate::workspace::harness`]
/// module can call it when it scaffolds harness discovery targets.
pub(crate) fn import_claude_md_into_user_notes(
    project_path: &str,
    body: &str,
    source_label: &str,
    archive_display: &str,
) {
    let canonical = PathBuf::from(project_path).join(".k2so/skills/k2so/SKILL.md");
    if !canonical.exists() {
        return;
    }
    let Ok(existing) = fs::read_to_string(&canonical) else { return };

    let import_sentinel = format!(
        "<!-- K2SO:IMPORT:CLAUDE_MD archive={} -->",
        archive_display
    );
    if existing.contains(&import_sentinel) {
        return;
    }

    let Some(sentinel_idx) = existing.find(SKILL_USER_NOTES_SENTINEL) else {
        let import_block = format!(
            "\n\n{sentinel}\n## Imported: {label}\n\n> Archived at `{archive}`. You can prune this section once reviewed; K2SO preserves anything below the `K2SO:USER_NOTES` sentinel verbatim.\n\n{body}\n",
            sentinel = import_sentinel,
            label = source_label,
            archive = archive_display,
            body = body.trim(),
        );
        let mut out = existing;
        out.push_str(&import_block);
        log_if_err(
            "import_claude_md fallback append",
            &canonical,
            atomic_write_str(&canonical, &out),
        );
        return;
    };
    let insertion_anchor = existing[sentinel_idx..]
        .find(USER_NOTES_PLACEHOLDER)
        .map(|rel| sentinel_idx + rel + USER_NOTES_PLACEHOLDER.len())
        .unwrap_or(sentinel_idx + SKILL_USER_NOTES_SENTINEL.len());
    let import_block = format!(
        "\n\n{sentinel}\n## Imported: {label}\n\n> Archived at `{archive}`. You can prune this section once reviewed; K2SO preserves anything below the `K2SO:USER_NOTES` sentinel verbatim across regenerations.\n\n{body}\n",
        sentinel = import_sentinel,
        label = source_label,
        archive = archive_display,
        body = body.trim(),
    );
    let mut out = String::with_capacity(existing.len() + import_block.len());
    out.push_str(&existing[..insertion_anchor]);
    out.push_str(&import_block);
    out.push_str(&existing[insertion_anchor..]);
    log_if_err(
        "import_claude_md_into_user_notes",
        &canonical,
        atomic_write_str(&canonical, &out),
    );
    log_adoption_event(
        project_path,
        &format!(
            "IMPORTED {} body into SKILL.md USER_NOTES (archive: {})",
            source_label, archive_display
        ),
    );
}

/// Universal skill refresh. Walks every agent folder + the workspace
/// skill and re-invokes the regular write_* functions. Because those now
/// route through ensure_skill_up_to_date, this is idempotent.
pub fn ensure_all_skills_up_to_date(project_path: &str) {
    write_workspace_skill_file(project_path);

    let agents_root = agents_dir(project_path);
    if !agents_root.exists() {
        return;
    }
    let Ok(entries) = fs::read_dir(&agents_root) else { return };
    for entry in entries.flatten() {
        let agent_path = entry.path();
        if !agent_path.is_dir() {
            continue;
        }
        let name_osstr = entry.file_name();
        let agent_name = name_osstr.to_string_lossy();
        if agent_name.starts_with('.') {
            continue;
        }

        let agent_md = agent_path.join("AGENT.md");
        if !agent_md.exists() {
            continue;
        }
        let agent_content = fs::read_to_string(&agent_md).unwrap_or_default();
        let fm = parse_frontmatter(&agent_content);
        let agent_type = fm.get("type").cloned().unwrap_or_else(|| "agent-template".to_string());
        let normalized_type = match agent_type.as_str() {
            "pod-leader" | "coordinator" => "manager".to_string(),
            other => other.to_string(),
        };
        crate::agents::skill_writer::write_agent_skill_file(
            project_path,
            &agent_name,
            &normalized_type,
        );
    }
}

// ══════════════════════════════════════════════════════════════════════
// Workspace SKILL.md regen — public entry point
// ══════════════════════════════════════════════════════════════════════

// is unchanged.

/// User-facing CLI command documentation injected into the manager and
/// AI planner CLAUDE.md briefs. Kept as a single `const` so the two
/// templates below stay in sync.
const CLI_TOOLS_DOCS: &str = r#"## K2SO CLI Tools

You are operating inside K2SO. The `k2so` command is available in your terminal.
K2SO does the heavy lifting — each command is a single atomic operation.

### Assign Work to an Agent (one step)
```
k2so delegate <agent> <work-file>
```
This single command does everything:
- Creates a git worktree (branch: `agent/<name>/<task>`)
- Writes a CLAUDE.md into the worktree with the agent's identity + task context
- Moves the work item from inbox → active with worktree metadata
- Opens a Claude terminal session in the worktree for the agent to start working

### Create Work Items
```
k2so work create --title "..." --body "..." --agent <name> --priority high --type task
k2so work create --title "..." --body "..."   # Goes to workspace inbox (no agent)
```

### Check Status
```
k2so agents list                     # All agents with inbox/active/done counts
k2so agents work <name>              # Agent's work items
k2so work inbox                      # Workspace-level inbox
k2so reviews                         # Pending reviews (completed work)
```

### Reviews (one step each)
```
k2so review approve <agent> <branch>   # Merges branch + removes worktree + cleans up
k2so review reject <agent>             # Removes worktree + moves work back to inbox
k2so review reject <agent> --reason "..." # Same + creates feedback file
k2so review feedback <agent> -m "..."  # Send feedback without rejecting
```

### Git
```
k2so commit                          # AI-assisted commit review
k2so commit-merge                    # AI commit then merge into main
```

### Waking the Workspace Manager (USE THIS — not `k2so heartbeat`)
```
k2so heartbeat wake                     # THE RIGHT WAY: resumes manager session, sends triage message
```
**IMPORTANT:** Always use `k2so heartbeat wake` to wake the workspace manager, NOT `k2so heartbeat`.
- `heartbeat wake` → resumes the manager's previous session, detects inbox work, sends delegation instructions
- `heartbeat` (without "wake") → raw triage that launches the workspace's primary agent, does NOT resume sessions or send messages

### Workspace Setup
```
k2so mode                               # Show current settings
k2so mode <off|agent|manager>            # Set workspace agent mode
k2so heartbeat <on|off>                 # Enable/disable automatic heartbeat
k2so settings                           # Show all workspace settings
```

### Agent Management
```
k2so agent create <name> --role "..."   # Create a new agent
k2so agent update --name <n> --field <f> --value "..."  # Update agent profile
k2so agent list                         # List all agents with work counts
k2so agent profile <name>              # Read agent's identity (agent.md)
k2so agents work <name>                 # Show agent's work items
k2so agents launch <name>              # Launch agent's Claude session
```

### Cross-Workspace (use K2SO_PROJECT_PATH, not cd)
```
K2SO_PROJECT_PATH=/path/to/workspace k2so work send --title "..." --body "..."
K2SO_PROJECT_PATH=/path/to/workspace k2so heartbeat wake
k2so work move --agent <name> --file <f> --from inbox --to active
```
**IMPORTANT:** When targeting a different workspace, use `K2SO_PROJECT_PATH=/path k2so ...`
Do NOT use `cd /path && k2so ...` — the cd resets your shell and may cause path resolution issues.

### Running Agents & Terminal I/O
```
k2so agents running                 # List all active CLI LLM sessions
k2so terminal write <id> "message"  # Send text to a running terminal
k2so terminal read <id> --lines 50  # Read last N lines from terminal buffer
```

### Completion
```
k2so agent complete --agent <n> --file <f>  # Complete work (auto-merge or submit for review)
```

"#;

/// Manager-specific workflow guidance injected into the manager CLAUDE.md.
const WORKFLOW_DOCS: &str = r#"## Workflow

### If you are the Lead Agent (orchestrator):
1. Check for work: `k2so inbox` (workspace-implicit; pass `--workspace <path>` to target another)
2. Read each request and decide which agent should handle it
3. Assign work with a single command — K2SO handles everything else:
   ```
   k2so delegate backend-eng .k2so/inbox/add-oauth-support.md
   ```
   This creates a worktree, writes a CLAUDE.md, and launches the agent automatically.
4. To break a large request into sub-tasks first:
   ```
   k2so inbox compose --title "Build API endpoints" --body "..."
   k2so inbox compose --title "Build login UI" --body "..."
   ```
   Then delegate each: `k2so delegate backend-eng .k2so/inbox/build-api-endpoints.md`
5. If a request is blocked or needs user input, leave it in the workspace inbox
6. You orchestrate — you do NOT implement code yourself

### If you are a Sub-Agent (executor):
You are launched into a dedicated worktree with your task already set up.
1. Read your task file (path is in your launch prompt)
2. Implement the changes — all work happens in your worktree
3. Commit to your branch as you go
4. When done: `k2so work move --agent <your-name> --file <task>.md --from active --to done`
5. Your work appears in the review queue — the user will approve, reject, or request changes

### Review lifecycle (handled by user or lead agent):
- **Approve**: `k2so review approve <agent> <branch>` — merges to main, cleans up worktree
- **Reject**: `k2so review reject <agent> --reason "..."` — cleans up worktree, puts task back in inbox with feedback, agent retries with a fresh worktree on next launch
- **Feedback**: `k2so review feedback <agent> -m "..."` — sends feedback without rejecting

## Important Rules
- Each agent works in its own worktree — never edit main directly
- K2SO creates worktrees, branches, and CLAUDE.md files for you automatically
- Commit often with clear messages referencing your task
- If blocked, move your task back to inbox and document the blocker
"#;

/// Regenerate the workspace-root SKILL.md — the lead agent's complete
/// operating manual. Written to `<project-root>/SKILL.md` with a
/// matching `<project-root>/CLAUDE.md` symlink so Claude Code auto-
/// discovers it. The SKILL.md is the canonical source of truth;
/// CLAUDE.md is a harness-specific entry point.
///
/// Also auto-scaffolds the `.k2so/` layout on first call (manager +
/// k2so-agent dirs, inbox/active/done folders, prds/, PROJECT.md).
///
/// Phase 2 Unit 7d: moved from
/// `src-tauri/src/commands/k2so_agents.rs::k2so_agents_regenerate_workspace_skill`
/// so the daemon can run this directly. The Tauri-side wrapper now
/// forwards here.
///
/// Returns the composed CLAUDE.md body that was injected into the
/// canonical SKILL.md — the renderer's regen UIs use this for preview.
pub fn regenerate_workspace_skill(project_path: String) -> Result<String, String> {
    use crate::agents::skill_writer::{
        generate_default_agent_body, write_agent_skill_file,
    };
    use crate::agents::work_item::read_work_item;

    let project_name = std::path::Path::new(&project_path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "workspace".to_string());

    // Scaffold .k2so/ structure if it doesn't exist.
    // Post-Phase-2.1: the workspace inbox lives at `.k2so/inbox/` (the
    // unified `k2so_core::inbox::*` primitive). The legacy
    // `.k2so/work/inbox/` was retired and the first-boot daemon hook
    // trashes any straggler.
    let k2so_dir = PathBuf::from(&project_path).join(".k2so");
    let _ = fs::create_dir_all(k2so_dir.join("inbox"));
    let _ = fs::create_dir_all(k2so_dir.join("prds"));

    // 0.37.0 unification check: if the workspace has been migrated to
    // the single-agent layout (`.k2so/agent/AGENT.md` exists OR the
    // unification sentinel is stamped), the legacy auto-scaffold of
    // `.k2so/agents/manager/` and `.k2so/agents/k2so-agent/` is a
    // regression — it'd repopulate the directory tree the migration
    // just retired and re-create files at paths the runtime no longer
    // reads. Skip the legacy scaffold entirely when migrated.
    let unification_sentinel = k2so_dir.join(".unification-0.37.0-done");
    let unified_agent_dir = k2so_dir.join("agent");
    let post_unification = unification_sentinel.exists() || unified_agent_dir.exists();
    if post_unification {
        // Don't recreate `.k2so/agents/` either — the post-migration
        // layout uses `.k2so/agent/` (singular) and
        // `.k2so/agent-templates/<n>/`. Skip straight to PROJECT.md +
        // workspace SKILL writes below.
    } else {
        let _ = fs::create_dir_all(k2so_dir.join("agents"));
    }

    // Auto-create manager agent if it doesn't exist (pre-unification only).
    // Check for old "pod-leader" and "coordinator" directory names as fallback.
    let manager_dir = k2so_dir.join("agents").join("manager");
    let legacy_coordinator_dir = k2so_dir.join("agents").join("coordinator");
    let legacy_pod_leader_dir = k2so_dir.join("agents").join("pod-leader");
    if !post_unification
        && !manager_dir.exists()
        && !legacy_coordinator_dir.exists()
        && !legacy_pod_leader_dir.exists()
    {
        let _ = fs::create_dir_all(manager_dir.join("work").join("inbox"));
        let _ = fs::create_dir_all(manager_dir.join("work").join("active"));
        let _ = fs::create_dir_all(manager_dir.join("work").join("done"));
        let manager_role = "Workspace Manager — delegates work to agents, reviews completed branches, drives milestones";
        let manager_body =
            generate_default_agent_body("manager", "manager", manager_role, &project_path);
        let manager_md = format!(
            "---\nname: manager\nrole: {}\ntype: manager\nmanager: true\n---\n\n{}\n",
            manager_role, manager_body
        );
        let manager_md_path = manager_dir.join("AGENT.md");
        log_if_err(
            "auto-scaffold manager AGENT.md",
            &manager_md_path,
            atomic_write_str(&manager_md_path, &manager_md),
        );
        write_agent_skill_file(&project_path, "manager", "manager");
    }

    // Auto-create K2SO agent if it doesn't exist (pre-unification only).
    // Post-0.37.0 the workspace agent lives at .k2so/agent/, not
    // .k2so/agents/k2so-agent/.
    let k2so_agent_dir = k2so_dir.join("agents").join("k2so-agent");
    if !post_unification && !k2so_agent_dir.exists() {
        let _ = fs::create_dir_all(k2so_agent_dir.join("work").join("inbox"));
        let _ = fs::create_dir_all(k2so_agent_dir.join("work").join("active"));
        let _ = fs::create_dir_all(k2so_agent_dir.join("work").join("done"));
        let k2so_role = "K2SO planner — builds PRDs, milestones, and technical plans";
        let k2so_body =
            generate_default_agent_body("k2so", "k2so-agent", k2so_role, &project_path);
        let k2so_md = format!(
            "---\nname: k2so-agent\nrole: {}\ntype: k2so\n---\n\n{}\n",
            k2so_role, k2so_body
        );
        let k2so_md_path = k2so_agent_dir.join("AGENT.md");
        log_if_err(
            "auto-scaffold k2so-agent AGENT.md",
            &k2so_md_path,
            atomic_write_str(&k2so_md_path, &k2so_md),
        );
        write_agent_skill_file(&project_path, "k2so-agent", "k2so");
    }

    // List existing agents
    let mut agent_list = String::new();
    let agents_root = agents_dir(&project_path);
    if agents_root.exists() {
        if let Ok(entries) = fs::read_dir(&agents_root) {
            for entry in entries.flatten() {
                if entry.file_type().map_or(false, |ft| ft.is_dir()) {
                    let name = entry.file_name().to_string_lossy().to_string();
                    let agent_md = entry.path().join("AGENT.md");
                    let role = if agent_md.exists() {
                        let content = fs::read_to_string(&agent_md).unwrap_or_default();
                        let fm = parse_frontmatter(&content);
                        fm.get("role").cloned().unwrap_or_default()
                    } else {
                        String::new()
                    };
                    agent_list.push_str(&format!("- **{}** — {}\n", name, role));
                }
            }
        }
    }

    // List workspace inbox items.
    //
    // Post-Phase-2.1: workspace inbox lives at `.k2so/inbox/` (the
    // unified `k2so_core::inbox::*` primitive). Root-level items
    // (`folder == ""`) are the untriaged arrivals; sub-foldered items
    // have already been organized and aren't surfaced in the manager's
    // skill summary.
    let mut inbox_summary = String::new();
    let ws_inbox_items =
        crate::inbox::list_folder(std::path::Path::new(&project_path), "");
    for item in &ws_inbox_items {
        // `type` dropped in the WorkItem → InboxItem migration; `source`
        // (the WorkItem `source` field) is preserved on InboxItem and
        // does double duty here.
        inbox_summary.push_str(&format!(
            "- **{}** (priority: {}, source: {})\n",
            item.title, item.priority, item.source
        ));
    }

    // Detect mode — read from DB, fall back to filesystem
    let is_manager_mode = {
        // Try reading from DB first — shared process-wide connection.
        let db_mode: Option<String> = {
            let db = crate::db::shared();
            let conn = db.lock();
            conn.query_row(
                "SELECT agent_mode FROM projects WHERE path = ?1",
                rusqlite::params![project_path],
                |row| row.get::<_, String>(0),
            )
            .ok()
        };

        match db_mode.as_deref() {
            Some("manager") | Some("coordinator") | Some("pod") => true,
            Some("agent") => false,
            _ => {
                // Fallback: if agents dir has sub-agents, assume manager mode
                let agents_root = agents_dir(&project_path);
                agents_root.exists()
                    && fs::read_dir(&agents_root)
                        .map(|e| {
                            e.flatten()
                                .any(|e| e.file_type().map_or(false, |ft| ft.is_dir()))
                        })
                        .unwrap_or(false)
            }
        }
    };

    // Scaffold PROJECT.md for manager mode — shared context across all agents
    if is_manager_mode {
        let project_md_path = k2so_dir.join("PROJECT.md");
        if !project_md_path.exists() {
            let project_md_content = format!(
r#"# {project_name}

<!--
  PROJECT.md is the "what" half of agent context — the codebase facts
  every agent needs regardless of role. K2SO ships this file as part of
  the agent's system prompt on every launch, via --append-system-prompt
  (injected alongside SKILL.md as a "Project Context (shared)" section).
  You don't need to reference it from wakeup.md — it's always there.

  Pair it with Agent Skills (SKILL.md layers) which cover the "how":
    PROJECT.md = what this project IS (tech stack, conventions)
    SKILL.md   = what the agent DOES (standing orders, procedures)

  Edit this file directly or via Settings → Projects → "Manage Project
  Context". Applies to Workspace Manager and Agent Template agents.
  Custom Agents don't receive PROJECT.md by design — they may not be
  codebase-scoped.

  Delete these comments once you've filled the sections in.
-->

## About This Project

<!-- What does this codebase do? What problem does it solve? -->

## Tech Stack

<!-- Languages, frameworks, databases, infrastructure. Include versions
     where they matter (e.g. "Tauri v2, React 19, TailwindCSS v4"). -->

## Key Directories

<!-- Important paths and what lives in them. Call out where tests live,
     where generated files go, where NOT to edit. -->

## Conventions

<!-- Code style, commit message format, PR process, branch naming.
     Anything an engineer would otherwise have to discover by osmosis. -->

## External Systems

<!-- Links to issue trackers, CI dashboards, staging environments, docs.
     If the project depends on an external service the agent may need to
     know about or call, document it here. -->
"#,
                project_name = project_name,
            );
            let _ = crate::agents::work_item::atomic_write(&project_md_path, &project_md_content);
        }
    }

    let md = if is_manager_mode {
        // ── Workspace Manager CLAUDE.md ──────────────────────────────────────
        format!(
            r#"# K2SO Workspace Manager: {project_name}

You are the **workspace manager** for the {project_name} workspace, operating inside K2SO.

## Your Role

You manage a team of AI agents that build this project. You:
- **Read PRDs and milestones** in `.k2so/prds/` and `.k2so/milestones/` to understand the plan
- **Delegate work** to sub-agents — K2SO automatically creates a worktree, writes a CLAUDE.md, and launches the agent
- **Manage your team** — create new agents when you need new skills, assign multiple tasks to the same agent type across parallel worktrees
- **Review completed work** — when agents finish, review their diffs and either approve (merge to main) or reject with feedback
- **Drive milestones forward** — after merging one batch, assign the next batch of tasks

**Important:** An agent is a role template, not a person. `backend-eng` can run in 5 worktrees simultaneously — each gets its own branch, its own CLAUDE.md, and its own Claude session. Don't wait for one task to finish before assigning the next.

## Workspace Inbox

{inbox_section}

## Your Agents

{agent_section}

## Delegation (one command does everything)

```bash
# Create a task and assign it
k2so work create --agent backend-eng --title "Build OAuth endpoints" \
  --body "Implement /auth/login and /auth/callback. See PRD: .k2so/prds/auth.md" \
  --priority high --type task

# Delegate — creates worktree, writes CLAUDE.md, launches the agent:
k2so delegate backend-eng .k2so/agents/backend-eng/work/inbox/build-oauth-endpoints.md
```

You can delegate multiple tasks to the same agent simultaneously:
```bash
k2so delegate backend-eng .k2so/agents/backend-eng/work/inbox/task-1.md
k2so delegate backend-eng .k2so/agents/backend-eng/work/inbox/task-2.md
k2so delegate backend-eng .k2so/agents/backend-eng/work/inbox/task-3.md
```
Each gets its own worktree and runs in parallel.

## Reviewing and Merging

When agents move their work to done/, it appears in the review queue:
```bash
k2so reviews                                    # See all pending reviews with diffs
k2so review approve backend-eng <branch>        # Merge to main + cleanup worktree
k2so review reject backend-eng --reason "..."   # Discard worktree + send back to inbox
k2so review feedback backend-eng -m "..."       # Send feedback without rejecting
```

**Your review responsibility:** You are the first reviewer. Check the diff, verify it meets the task's acceptance criteria, and approve or reject. Only escalate to the user when a milestone is complete or if you're unsure about a design decision.

## Creating New Agents

When you need a skill your team doesn't have:
```bash
k2so agents create devops-eng --role "DevOps — CI/CD, Docker, deployment, infrastructure"
k2so agents create docs-writer --role "Documentation — README, API docs, user guides"
```

## Communicating with Running Agents

You can see and message any running agent session:
```bash
k2so agents running                            # List all active sessions with terminal IDs
k2so terminal read <terminal-id> --lines 30    # See what an agent is doing
k2so terminal write <terminal-id> "message"    # Send instructions to a running agent
```

**Auto-merge (Build state):** When all capabilities are "auto", tell the sub-agent to self-merge:
```bash
k2so terminal write <id> "Your work is approved. Run: k2so agent complete --agent <name> --file <filename>"
```

**Gated (Managed Service state):** The agent moves work to done and you review:
```bash
k2so reviews                                   # Check pending reviews
k2so review approve <agent> <branch>           # Merge after reviewing
```

## Planning

Store plans as markdown files:
- `.k2so/prds/` — Product requirement documents
- `.k2so/milestones/` — Milestone breakdowns with task lists
- `.k2so/specs/` — Technical specifications

{cli_section}

{workflow_section}
"#,
            project_name = project_name,
            inbox_section = if inbox_summary.is_empty() {
                "*Workspace inbox is empty. Waiting for tasks from the AI Planner or user.*".to_string()
            } else {
                format!("### Current Inbox\n{}", inbox_summary)
            },
            agent_section = if agent_list.is_empty() {
                "*No agents yet. Create agents based on the skills this project needs.*".to_string()
            } else {
                format!("{}\n\nRead each agent's profile at `.k2so/agents/<name>/agent.md` to understand their strengths before delegating. You can also update their profiles with `k2so agent update --name <name> --field role --value \"...\"`.", agent_list)
            },
            cli_section = CLI_TOOLS_DOCS,
            workflow_section = WORKFLOW_DOCS,
        )
    } else {
        // ── Agent 1: AI Planner CLAUDE.md ──────────────────────────────
        format!(
            r#"# K2SO AI Planner: {project_name}

You are the **AI Planner** for the {project_name} workspace, operating inside K2SO.

## Your Role

You collaborate with the user to plan and orchestrate software projects. You:
- **Talk with the user** to understand what they want to build
- **Create PRDs** (product requirement documents), milestones, and technical specifications
- **Set up workspaces** for each project — enable worktrees, manager mode, create agent teams
- **Coordinate across workspaces** — send work to different projects, check on progress
- **You do NOT write code** — you plan, then hand off execution to workspace managers and their agent teams

## Setting Up a Project Workspace

When the user has a project they want to build or maintain with agents:

```bash
# 1. Enable the workspace for autonomous work
k2so mode manager                    # Enable multi-agent orchestration
k2so heartbeat on                   # Agents wake up automatically on schedule

# 2. Create the agent team based on the project's tech stack
k2so agents create backend-eng --role "Backend engineer — APIs, databases, server logic"
k2so agents create frontend-eng --role "Frontend engineer — React, UI, styling, UX"
k2so agents create qa-tester --role "QA — testing, test automation, quality assurance"

# 3. Verify setup
k2so settings                       # Shows mode, worktrees, heartbeat status
k2so agents list                    # Shows agents with work counts
```

## Planning Workflow

1. **Discuss with the user** what they want built — goals, constraints, timeline
2. **Create a PRD** that captures the full scope:
   ```
   mkdir -p .k2so/prds
   # Write the PRD as a markdown file
   ```
3. **Break the PRD into milestones** — each milestone should be shippable
4. **Break milestones into tasks** with clear acceptance criteria
5. **Send tasks to the project workspace** for the workspace manager to execute:
   ```bash
   k2so work send --workspace /path/to/project \
     --title "Milestone 1: User Authentication" \
     --body "See PRD at .k2so/prds/auth.md. Tasks: ..."
   ```
   The workspace manager picks it up and delegates to its agents.

## Cross-Workspace Coordination

You can see and manage multiple workspaces:
```bash
# Send work to any workspace
k2so work send --workspace /path/to/frontend-app --title "..." --body "..."
k2so work send --workspace /path/to/api-server --title "..." --body "..."

# Set up a new workspace from scratch
K2SO_PROJECT_PATH="/path/to/new-project" k2so mode manager
K2SO_PROJECT_PATH="/path/to/new-project" k2so heartbeat on
K2SO_PROJECT_PATH="/path/to/new-project" k2so agents create backend-eng --role "..."

# Register a new workspace via CLI
k2so workspace create /path/to/new-project   # Create folder + register
k2so workspace open /path/to/existing        # Register existing folder
```

## Testing Workspace Manager Workflows

To wake the workspace manager and have it process inbox work:
```bash
# Add work to the workspace inbox
k2so work create --title "..." --body "..." --priority high --type task --source feature

# Wake the workspace manager (resumes previous session, sends triage message)
k2so heartbeat wake
```

The workspace manager will check inbox, delegate to agents, and track progress.

## Monitoring Running Agents

```bash
# See all active CLI LLM sessions across workspaces
k2so agents running

# Read what an agent is doing
k2so terminal read <terminal-id> --lines 30

# Send a message to a running agent
k2so terminal write <terminal-id> "message"

# Check agent work status
k2so agents list
k2so reviews                    # See pending reviews
```

## Workspace States

Workspaces operate under states that control agent autonomy:
- **Build** — agents auto-merge everything
- **Managed Service** — features are gated (need human approval), bugs/security auto-merge
- **Maintenance** — everything gated
- **Locked** — no agent activity

The workspace manager and sub-agents adapt their completion behavior based on the state.
Sub-agents use `k2so agent complete` which auto-merges or submits for review accordingly.

## Current Context

{inbox_section}

{cli_section}
"#,
            project_name = project_name,
            inbox_section = if inbox_summary.is_empty() {
                "No items in the workspace inbox.".to_string()
            } else {
                format!("### Workspace Inbox\n{}", inbox_summary)
            },
            cli_section = CLI_TOOLS_DOCS,
        )
    };

    // As of 0.32.7: the rich workspace-level content (manager brief or AI
    // planner brief + agent list + inbox summary + CLI tools docs) now
    // flows into the canonical SKILL.md instead of a separate ./CLAUDE.md
    // file. `write_workspace_skill_file_with_body` takes the composed `md`
    // as the base body, appends `.k2so/PROJECT.md` body + primary agent's
    // `agent.md` body, writes the canonical at `.k2so/skills/k2so/SKILL.md`,
    // and fans it out via symlinks to every harness discovery path
    // (`./CLAUDE.md`, `./SKILL.md`, `./GEMINI.md`, `./AGENT.md`,
    // `./.goosehints`, `./.claude/skills/k2so/SKILL.md`, etc.).
    //
    // Existing `./CLAUDE.md` files: migrated to `.k2so/CLAUDE.md.migrated` if
    // K2SO-generated, preserved as-is if user-authored (see
    // migrate_and_symlink_root_claude_md).
    write_workspace_skill_file_with_body(&project_path, Some(&md));

    // Clean up the stale `.k2so/CLAUDE.md.disabled` artifact from the
    // pre-symlink era — the disable flow is now "symlink goes away when the
    // workspace is off", not a file rename.
    let disabled_path = PathBuf::from(&project_path).join(".k2so").join("CLAUDE.md.disabled");
    if disabled_path.exists() {
        let _ = fs::remove_file(&disabled_path);
    }

    Ok(md)
}


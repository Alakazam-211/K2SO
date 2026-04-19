//! Harness-agnostic skill writer.
//!
//! The K2SO agent system writes a single canonical SKILL.md per tier
//! + agent pair, then symlinks or marker-injects it into every
//! LLM-harness discovery path the workspace might encounter —
//! Claude Code, OpenCode, Pi, Cursor, Copilot, etc. That way the
//! user writes K2SO's protocol once and every CLI-LLM tool reads
//! the same source of truth, even as harness file layouts churn.
//!
//! Discovery paths this module maintains:
//!
//! | Path | Form | Owner |
//! |---|---|---|
//! | `.k2so/skills/<n>/SKILL.md` | canonical file | K2SO (upgrade-tracked) |
//! | `.claude/skills/<n>/SKILL.md` | symlink | Claude Code |
//! | `.opencode/agent/<n>.md` | symlink | OpenCode |
//! | `.pi/skills/<n>/SKILL.md` | symlink | Pi |
//! | `AGENTS.md` | marker-injected block | multi-harness convention |
//! | `.github/copilot-instructions.md` | marker-injected block | GitHub Copilot |
//!
//! The canonical path is upgrade-tracked (the full SKILL upgrade
//! protocol in [`super::skill`]); symlinks always point at it, so a
//! version bump propagates to every harness on its next discovery
//! pass without writing to each location individually.

use std::fs;
use std::path::{Path, PathBuf};

use crate::agents::{agent_dir, agents_dir, parse_frontmatter};
use crate::agents::skill::{
    ensure_skill_up_to_date, SKILL_VERSION_CUSTOM_AGENT, SKILL_VERSION_K2SO_AGENT,
    SKILL_VERSION_MANAGER, SKILL_VERSION_TEMPLATE,
};
use crate::agents::skill_content::{
    generate_custom_agent_skill_content, generate_k2so_agent_skill_content,
    generate_manager_skill_content, generate_template_skill_content,
};
use crate::fs_atomic::{atomic_symlink, atomic_write_str, log_if_err};

/// Markers that delimit the K2SO-managed section inside
/// marker-injected files (AGENTS.md, copilot-instructions.md).
/// Content between the markers is considered K2SO's; user edits
/// outside the pair are preserved.
pub const K2SO_SECTION_BEGIN: &str = "<!-- K2SO:BEGIN -->";
pub const K2SO_SECTION_END: &str = "<!-- K2SO:END -->";

/// Update (or create) the K2SO-managed block inside a shared
/// harness file. If the markers don't exist yet, appends the block
/// at the bottom; otherwise replaces just the content between them,
/// preserving user content above and below. Atomic write — a crash
/// mid-update can't corrupt the file.
pub fn upsert_k2so_section(file_path: &Path, content: &str) {
    let section = format!("{}\n{}\n{}", K2SO_SECTION_BEGIN, content, K2SO_SECTION_END);

    let existing = fs::read_to_string(file_path).unwrap_or_default();
    let composed = if let (Some(start), Some(end)) =
        (existing.find(K2SO_SECTION_BEGIN), existing.find(K2SO_SECTION_END))
    {
        let before = &existing[..start];
        let after = &existing[end + K2SO_SECTION_END.len()..];
        format!("{}{}{}", before, section, after)
    } else if existing.is_empty() {
        section.clone()
    } else {
        format!("{}\n\n{}", existing.trim_end(), section)
    };
    log_if_err(
        "upsert_k2so_section",
        file_path,
        atomic_write_str(file_path, &composed),
    );
}

/// Create a symlink atomically. Writes the new symlink at a sibling
/// tempfile then renames into place, so concurrent readers never see
/// a missing file mid-swap.
pub fn force_symlink(source: &Path, target: &Path) {
    log_if_err("force_symlink", target, atomic_symlink(source, target));
}

/// Write the canonical SKILL.md and symlink/marker-inject from every
/// harness discovery path. `skill_name` is the directory name under
/// `.k2so/skills/`; `skill_type` + `skill_version` feed the upgrade
/// protocol; `description` goes into the canonical file's
/// frontmatter so harnesses can list it without reading the body.
///
/// `write_shared_markers` controls the marker-injection writes
/// (AGENTS.md + copilot-instructions.md). Only the workspace-level
/// skill should set this true — per-agent skills would otherwise
/// clobber each other in the single K2SO marker block.
pub fn write_skill_to_all_harnesses(
    project_path: &str,
    skill_name: &str,
    skill_type: &str,
    skill_version: u32,
    description: &str,
    content: &str,
    write_shared_markers: bool,
) {
    let root = PathBuf::from(project_path);

    // Canonical skill with both harness-format (name/description) AND
    // upgrade-tracking frontmatter (k2so_skill/skill_version/checksum +
    // managed markers). Written via ensure_skill_up_to_date so user
    // edits below the managed region or the closing marker survive
    // future regenerations, and version bumps auto-upgrade unmodified
    // files.
    let canonical_dir = root.join(".k2so/skills").join(skill_name);
    let canonical_path = canonical_dir.join("SKILL.md");
    let extras = format!("name: {}\ndescription: {}", skill_name, description);
    ensure_skill_up_to_date(
        &canonical_path,
        skill_type,
        skill_version,
        content,
        Some(&extras),
    );

    // Claude Code: .claude/skills/{name}/SKILL.md → symlink
    let claude_dir = root.join(".claude/skills").join(skill_name);
    let _ = fs::create_dir_all(&claude_dir);
    force_symlink(&canonical_path, &claude_dir.join("SKILL.md"));

    // OpenCode: .opencode/agent/{name}.md → symlink
    let opencode_dir = root.join(".opencode/agent");
    let _ = fs::create_dir_all(&opencode_dir);
    force_symlink(
        &canonical_path,
        &opencode_dir.join(format!("{}.md", skill_name)),
    );

    // Pi: .pi/skills/{name}/SKILL.md → symlink
    let pi_dir = root.join(".pi/skills").join(skill_name);
    let _ = fs::create_dir_all(&pi_dir);
    force_symlink(&canonical_path, &pi_dir.join("SKILL.md"));

    // Marker-injected shared files. Only the workspace skill writes
    // here — otherwise each per-agent run clobbers the block written
    // by the previous one.
    if write_shared_markers {
        upsert_k2so_section(&root.join("AGENTS.md"), content);
        let github_dir = root.join(".github");
        let _ = fs::create_dir_all(&github_dir);
        upsert_k2so_section(&github_dir.join("copilot-instructions.md"), content);
    }
}

/// Regenerate a single agent's SKILL.md suite — dispatches to the
/// tier-specific content generator, writes the canonical agent-dir
/// file (upgrade-tracked), then fans out to every harness via
/// [`write_skill_to_all_harnesses`]. Called during launch + from the
/// workspace regen orchestrator.
pub fn write_agent_skill_file(project_path: &str, agent_name: &str, agent_type: &str) {
    let project_name = Path::new(project_path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "workspace".to_string());

    let (skill_content, skill_type_tag, skill_version) = match agent_type {
        "manager" | "coordinator" | "pod-leader" => (
            generate_manager_skill_content(project_path, &project_name),
            "manager",
            SKILL_VERSION_MANAGER,
        ),
        "k2so" => (
            generate_k2so_agent_skill_content(&project_name, agent_name),
            "k2so-agent",
            SKILL_VERSION_K2SO_AGENT,
        ),
        "custom" => (
            generate_custom_agent_skill_content(&project_name, agent_name),
            "custom-agent",
            SKILL_VERSION_CUSTOM_AGENT,
        ),
        _ => (
            generate_template_skill_content(&project_name, agent_name),
            "agent-template",
            SKILL_VERSION_TEMPLATE,
        ),
    };

    // Agent-dir SKILL.md (for harnesses that launch in the agent's
    // cwd). Same upgrade protocol so user edits survive.
    let agent_skill_path = agent_dir(project_path, agent_name).join("SKILL.md");
    ensure_skill_up_to_date(
        &agent_skill_path,
        skill_type_tag,
        skill_version,
        &skill_content,
        None,
    );

    // Harness-specific symlinks + marker-injected files share the
    // same canonical source, also upgrade-tracked.
    let description = match agent_type {
        "manager" | "coordinator" | "pod-leader" => format!(
            "K2SO Workspace Manager commands for {} — checkin, delegate, message, reserve files",
            agent_name
        ),
        "k2so" => format!(
            "K2SO Agent commands for {} — full surface (checkin, heartbeats, work, messaging, reserves)",
            agent_name
        ),
        "custom" => format!(
            "K2SO agent commands for {} — checkin, message connected workspaces, reserve files",
            agent_name
        ),
        _ => format!(
            "K2SO agent template commands for {} — checkin, status, done, reserve files",
            agent_name
        ),
    };
    write_skill_to_all_harnesses(
        project_path,
        &format!("k2so-{}", agent_name),
        skill_type_tag,
        skill_version,
        &description,
        &skill_content,
        false,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_creates_file_with_markers_when_missing() {
        let tmp = std::env::temp_dir().join(format!(
            "k2so-skill-writer-test-{}.md",
            uuid::Uuid::new_v4()
        ));
        upsert_k2so_section(&tmp, "hello content");
        let s = std::fs::read_to_string(&tmp).unwrap();
        assert!(s.contains(K2SO_SECTION_BEGIN));
        assert!(s.contains("hello content"));
        assert!(s.contains(K2SO_SECTION_END));
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn upsert_replaces_existing_section_preserving_user_content() {
        let tmp = std::env::temp_dir().join(format!(
            "k2so-skill-writer-preserve-{}.md",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(
            &tmp,
            format!(
                "# User Heading\n\n{}\nold content\n{}\n\n## User Footer\n",
                K2SO_SECTION_BEGIN, K2SO_SECTION_END
            ),
        )
        .unwrap();
        upsert_k2so_section(&tmp, "new content");
        let s = std::fs::read_to_string(&tmp).unwrap();
        assert!(s.contains("# User Heading"), "user heading gone: {s}");
        assert!(s.contains("## User Footer"), "user footer gone: {s}");
        assert!(s.contains("new content"));
        assert!(!s.contains("old content"));
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn upsert_appends_when_no_markers_present() {
        let tmp = std::env::temp_dir().join(format!(
            "k2so-skill-writer-append-{}.md",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&tmp, "# Pure user content\n").unwrap();
        upsert_k2so_section(&tmp, "injected");
        let s = std::fs::read_to_string(&tmp).unwrap();
        assert!(s.contains("# Pure user content"));
        assert!(s.contains(K2SO_SECTION_BEGIN));
        assert!(s.contains("injected"));
        std::fs::remove_file(&tmp).ok();
    }
}

// ── Default agent.md bodies ───────────────────────────────────────────
//
// Starting-template bodies emitted when K2SO auto-scaffolds a new
// agent directory. Users (or an AI via AIFileEditor) refine them
// from here. Kept alongside the skill writer because both live
// behind the workspace-regen flow.

/// Generate a default agent.md body based on agent type.
/// This gives each agent a rich starting template that users (or AI) can refine via AIFileEditor.
pub fn generate_default_agent_body(agent_type: &str, name: &str, role: &str, project_path: &str) -> String {
    let project_name = std::path::Path::new(project_path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "workspace".to_string());

    match agent_type {
        "manager" | "coordinator" | "pod-leader" => {
            // List existing agent templates for the "Your Team" section
            let mut team_lines = String::new();
            let agents_root = agents_dir(project_path);
            if agents_root.exists() {
                if let Ok(entries) = fs::read_dir(&agents_root) {
                    for entry in entries.flatten() {
                        if entry.file_type().map_or(false, |ft| ft.is_dir()) {
                            let member_name = entry.file_name().to_string_lossy().to_string();
                            if member_name == name { continue; }
                            let md_path = entry.path().join("AGENT.md");
                            let member_role = if md_path.exists() {
                                let content = fs::read_to_string(&md_path).unwrap_or_default();
                                let fm = parse_frontmatter(&content);
                                fm.get("role").cloned().unwrap_or_default()
                            } else {
                                String::new()
                            };
                            team_lines.push_str(&format!(
                                "- **{}**: `.k2so/agents/{}/agent.md` — {}\n",
                                member_name, member_name, member_role
                            ));
                        }
                    }
                }
            }
            let team_section = if team_lines.is_empty() {
                "No agent templates yet. Create agents based on the skills this project needs.".to_string()
            } else {
                format!("Read their agent.md profiles when delegating to match tasks to the right specialist.\n\n{}", team_lines)
            };

            format!(
r#"You are the Workspace Manager for the {project_name} workspace.

## Work Sources

Primary (always checked by local LLM triage — near-zero cost):
- Workspace inbox: `.k2so/work/inbox/` (unassigned work items)
- Your inbox: `.k2so/agents/{name}/work/inbox/` (delegated to you)

External (scan these proactively when woken — customize for your project):
- GitHub Issues: `gh issue list --repo OWNER/REPO --label bug,feature --state open`
- Open PRs needing review: `gh pr list --repo OWNER/REPO --review-requested`
- Local PRDs: `.k2so/prds/*.md`

## Your Team

{team_section}

## Tools Available

- `k2so agent create --name "new-agent" --role "Specialization description"` — create a new agent template
- `k2so agent update --name "agent-name" --field role --value "Updated role"` — update a member's profile
- `k2so delegate <agent> <work-file>` — assign work (creates worktree + launches agent)
- `k2so work create --agent <name> --title "..." --body "..."` — create a task for an agent
- `k2so reviews` — see completed work ready for review
- `k2so review approve <agent> <branch>` — merge completed work
- `k2so terminal spawn --title "..." --command "..."` — run parallel tasks

## Standing Orders

<!-- Persistent directives checked every time this agent wakes up. -->
<!-- Unlike work items (which are one-off tasks), standing orders are ongoing. -->
<!-- Examples: -->
<!-- - Check CI status on main branch every wake and report failures -->
<!-- - Review open PRs older than 24 hours -->
<!-- - Monitor .k2so/work/inbox/ for unassigned items and delegate immediately -->

## Operational Notes

- An agent is a role template, not a person — the same agent can run in multiple worktrees simultaneously
- You orchestrate and review — you do NOT implement code yourself
- When you need a new skill, create a new agent with `k2so agent create`
- Read agent templates' agent.md files to understand their strengths before delegating
"#,
                project_name = project_name,
                name = name,
                team_section = team_section,
            )
        }
        "agent-template" | "pod-member" => {
            format!(
r#"## Specialization

{role}

## Capabilities

- Implement changes in isolated git worktrees (one branch per task)
- Commit frequently with clear messages referencing the task
- Follow existing code patterns and conventions in the project
- Run tests before marking work as done

## How You Work

1. You are launched into a dedicated worktree with your task in the CLAUDE.md
2. Read the task file for full requirements and acceptance criteria
3. Implement the changes — all work happens in your worktree
4. Commit to your branch as you go
5. When done: `k2so work move --agent {name} --file <task>.md --from active --to done`
6. Your work appears in the review queue for the Workspace Manager to approve or reject

## Standing Orders

<!-- Persistent directives checked every time this agent wakes up. -->
<!-- Examples: -->
<!-- - Run tests before marking any task as done -->
<!-- - Follow the project's commit message convention -->
<!-- - Never modify files outside your assigned scope -->

## If Blocked

- If you need clarification, move the task back to inbox with a note
- If you need another agent's work first, document the dependency in the task file
- Never edit files outside your worktree
"#,
                role = role,
                name = name,
            )
        }
        "custom" => {
            format!(
r#"## Role

{role}

## Heartbeat Control

You run on an adaptive heartbeat. Adjust your check-in frequency based on what you're doing:

- `k2so heartbeat set --interval 60 --phase "active"` — check every minute (busy periods)
- `k2so heartbeat set --interval 300 --phase "monitoring"` — every 5 minutes (watching)
- `k2so heartbeat set --interval 3600 --phase "idle"` — every hour (dormant)

## Tools Available

- `k2so terminal spawn --title "..." --command "..."` — run parallel tasks
- `k2so heartbeat set --interval N --phase "..."` — adjust your check-in frequency
- Standard CLI tools available in your terminal: `gh`, `git`, `curl`, etc.

## Standing Orders

<!-- Persistent directives checked every time this agent wakes on the heartbeat. -->
<!-- Unlike one-off tasks, standing orders are ongoing responsibilities. -->
<!-- Examples: -->
<!-- - Check GitHub issues every wake: `gh issue list --repo OWNER/REPO --state open` -->
<!-- - Monitor a Slack channel for requests -->
<!-- - Run a health check script and report failures -->

## Operational Notes

- Your agent.md is your complete identity — everything about who you are and what you do lives here
- Customize the sections above to match your specific use case
- Use the AIFileEditor in K2SO Settings to refine your profile with AI assistance
"#,
                role = role,
            )
        }
        "k2so" => {
            format!(
r#"You are the K2SO Agent for the {project_name} workspace — the top-level planner and orchestrator.

## Work Sources

Primary (checked automatically by the heartbeat system at near-zero cost):
- Workspace inbox: `.k2so/work/inbox/` (unassigned work items)
- Your inbox: `.k2so/agents/{name}/work/inbox/` (items delegated to you)

External (add your project-specific sources below — CLI tools only, no MCP):
- GitHub Issues: `gh issue list --repo OWNER/REPO --label bug,feature --state open`
- Open PRs: `gh pr list --repo OWNER/REPO --review-requested`
<!-- Add more work sources here: Linear, Jira, custom APIs, intake directories, etc. -->

## Project Context

<!-- Describe what this project does, key directories, conventions, tech stack -->

## Integration Commands

<!-- CLI tools this agent should use to check for work, report status, or interact with external systems -->
- `gh` — GitHub CLI for issues, PRs, releases
- `git` — Version control operations
- `curl` / `jq` — API calls and JSON processing

## Constraints

<!-- Hours of operation, cost limits, repos off-limits, branches to protect -->

## Standing Orders

<!-- Persistent directives checked every time this agent wakes up. -->
<!-- Unlike work items in the inbox (one-off tasks), standing orders are ongoing. -->
<!-- Examples: -->
<!-- - Scan GitHub issues for new bugs every wake -->
<!-- - Check CI pipeline status on main and report failures -->
<!-- - Review PRs older than 48 hours -->
<!-- - Monitor .k2so/work/inbox/ and delegate unassigned items immediately -->

## Operational Notes

- Editing the sections above is how you customize the K2SO agent for your project
- The default K2SO knowledge (CLI tools, workflow, work queues) is auto-injected at launch
- Modifying the auto-injected defaults in CLAUDE.md is at your own risk
- Use the Manage Persona button in Settings to refine this profile with AI assistance
"#,
                project_name = project_name,
                name = name,
            )
        }
        _ => {
            // Unknown type — empty body
            String::new()
        }
    }
}

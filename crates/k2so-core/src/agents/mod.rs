//! K2SO Agent system — the heartbeat scheduler, primary-agent
//! resolution, and project/filesystem bookkeeping that the Tauri app
//! and the k2so-daemon both need.
//!
//! Home for the slice of `src-tauri/src/commands/k2so_agents.rs` that
//! has to run inside the daemon so agents keep firing while the
//! laptop lid is closed. Each submodule carries a narrow, testable
//! responsibility:
//!
//! - [`heartbeat`] — multi-heartbeat CRUD + tick evaluation + audit
//!   stamping. The piece that turns a launchd wake into actual fired
//!   `heartbeat_fires` rows.
//!
//! Helpers at this level are small, pure-ish utilities that multiple
//! submodules (and the in-progress route migration) depend on. They
//! stay public so src-tauri's existing call sites can re-export them
//! via `pub use k2so_core::agents::*` without churning 170+ lines of
//! renames.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

pub mod build_launch;
pub mod channel;
pub mod checkin;
pub mod commands;
pub mod connections;
pub mod delegate;
pub mod events;
pub mod heartbeat;
pub mod heartbeat_install;
pub mod launch_profile;
pub mod onboarding;
pub mod reviews;
pub mod scheduler;
pub mod session;
pub mod settings;
pub mod skill;
pub mod skill_content;
pub mod skill_writer;
pub mod terminal_id;
pub mod triage_summary;
pub mod unification;
pub mod wake;
pub mod work_item;
pub mod workspace_regen;
pub mod workspaces;

/// Resolve a project's primary-key id from its filesystem path. `None`
/// when the project hasn't been registered via `projects` yet.
pub fn resolve_project_id(conn: &rusqlite::Connection, path: &str) -> Option<String> {
    conn.query_row(
        "SELECT id FROM projects WHERE path = ?1",
        rusqlite::params![path],
        |r| r.get(0),
    )
    .ok()
}

/// Root of the legacy multi-agent tree for a given workspace:
/// `<project>/.k2so/agents/`. Post-0.37.0 unification this directory
/// is removed by the migration sweep and only sticks around for
/// fresh workspaces that haven't been onboarded yet (it gets created
/// briefly by older code paths that still scaffold it). Prefer
/// [`workspace_agent_path`] / [`agent_template_dir`] for new code.
pub fn agents_dir(project_path: &str) -> PathBuf {
    PathBuf::from(project_path).join(".k2so").join("agents")
}

/// Post-0.37.0: `<project>/.k2so/agent/` — the single workspace
/// agent's directory. Created by the unification migration; every
/// call site that historically did `agent_dir(project, primary_name)`
/// converges on this path after the migration runs.
pub fn workspace_agent_path(project_path: &str) -> PathBuf {
    PathBuf::from(project_path).join(".k2so").join("agent")
}

/// `<project>/.k2so/agent/AGENT.md` — the workspace agent's persona
/// file post-0.37.0. Convenience over `workspace_agent_path().join("AGENT.md")`.
#[allow(dead_code)]
pub fn workspace_agent_md_path(project_path: &str) -> PathBuf {
    workspace_agent_path(project_path).join("AGENT.md")
}

/// Post-0.37.0: `<project>/.k2so/agent-templates/<template_name>/` —
/// role personas for delegation/worktrees.
pub fn agent_template_dir(project_path: &str, template_name: &str) -> PathBuf {
    PathBuf::from(project_path)
        .join(".k2so")
        .join("agent-templates")
        .join(template_name)
}

/// Resolve the on-disk directory for an agent name within a workspace.
///
/// **Layout-aware.** Probes in this order:
/// 1. `<project>/.k2so/agent/AGENT.md` exists — post-0.37.0 primary.
///    `agent_name` is ignored here; the primary is keyed on
///    workspace, not name. Callers that pass a name (e.g.
///    `agent_dir(project, "pod-leader")`) get the unified path
///    transparently — historic call sites keep working without
///    changes during the deprecation window.
///
///    Probing for AGENT.md (not just the dir) matters because the
///    unification migration `mkdir -p`s `.k2so/agent/` BEFORE it
///    moves any files. During that window the dir exists but has
///    no AGENT.md yet, and the migration's own primary-detection
///    walk needs `agent_type_for` to read from the LEGACY
///    `.k2so/agents/<name>/AGENT.md` to determine the primary.
///    Gating the probe on the populated state avoids a
///    chicken-and-egg failure during migration.
/// 2. `<project>/.k2so/agent-templates/<agent_name>/AGENT.md`
///    exists — a template (for delegate/worktree spawn).
/// 3. Legacy `<project>/.k2so/agents/<agent_name>/` — pre-0.37.0
///    workspaces that haven't been migrated yet (rare; the daemon's
///    boot sweep migrates every registered workspace).
pub fn agent_dir(project_path: &str, agent_name: &str) -> PathBuf {
    let primary = workspace_agent_path(project_path);
    if primary.join("AGENT.md").exists() {
        return primary;
    }
    let template = agent_template_dir(project_path, agent_name);
    if template.join("AGENT.md").exists() {
        return template;
    }
    agents_dir(project_path).join(agent_name)
}

/// Extract YAML-ish `key: value` frontmatter from a markdown blob.
/// Tolerant: empty keys/values skipped, missing closing fence returns
/// an empty map. Used by [`agent_type_for`] and [`super::scheduler`]
/// consumers to read agent.md metadata without pulling a full YAML dep.
pub fn parse_frontmatter(content: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    if !content.starts_with("---") {
        return map;
    }
    if let Some(end) = content[3..].find("---") {
        let frontmatter = &content[3..3 + end];
        for line in frontmatter.lines() {
            if let Some((key, value)) = line.split_once(':') {
                let key = key.trim().to_string();
                let value = value.trim().to_string();
                if !key.is_empty() && !value.is_empty() {
                    map.insert(key, value);
                }
            }
        }
    }
    map
}

/// Determine an agent's type from its `agent.md` frontmatter. Returns
/// `"agent-template"` if no frontmatter or no `type:` field is found
/// (same default the scheduler uses elsewhere).
pub fn agent_type_for(project_path: &str, agent_name: &str) -> String {
    let md = agent_dir(project_path, agent_name).join("AGENT.md");
    if let Ok(content) = fs::read_to_string(&md) {
        let fm = parse_frontmatter(&content);
        if let Some(t) = fm.get("type") {
            return t.clone();
        }
    }
    "agent-template".to_string()
}

/// Find the workspace's primary scheduleable agent.
///
/// A workspace is one-of Custom / K2SO Agent / Workspace Manager
/// (mutually exclusive by design), but agent-mode swaps can leave
/// orphan directories from prior modes on disk. This fn uses
/// `projects.agent_mode` as the source of truth and only returns an
/// agent dir whose type matches the workspace's declared mode.
/// Agent-templates are never scheduleable.
pub fn find_primary_agent(project_path: &str) -> Option<String> {
    let agents_root = agents_dir(project_path);
    if !agents_root.exists() {
        return None;
    }

    // Resolve the declared workspace mode from the DB. Prevents
    // alphabetical scan order from picking a stale orphan (e.g.
    // returning pod-leader before sarah when the workspace is actually
    // a Custom agent workspace for sarah).
    let declared_mode: Option<String> = {
        let db = crate::db::shared();
        let conn = db.lock();
        conn.query_row(
            "SELECT agent_mode FROM projects WHERE path = ?1",
            rusqlite::params![project_path],
            |row| row.get::<_, Option<String>>(0),
        )
        .ok()
        .flatten()
    };

    let type_for_mode = |mode: &str| match mode {
        "custom" => "custom",
        "manager" => "manager",
        "k2so" | "agent" => "k2so",
        _ => "",
    };

    // Pass 1: prefer the agent whose type matches the declared mode.
    if let Some(ref mode) = declared_mode {
        let wanted = type_for_mode(mode);
        if !wanted.is_empty() {
            if let Ok(entries) = fs::read_dir(&agents_root) {
                for entry in entries.flatten() {
                    if !entry.file_type().map_or(false, |ft| ft.is_dir()) {
                        continue;
                    }
                    let name = entry.file_name().to_string_lossy().to_string();
                    if agent_type_for(project_path, &name) == wanted {
                        return Some(name);
                    }
                }
            }
            // Manager mode doesn't require a filesystem dir — __lead__
            // lives at the project root. Return the sentinel.
            if wanted == "manager" {
                return Some("__lead__".to_string());
            }
        }
    }

    // Pass 2 (fallback, no declared mode): first scheduleable dir wins.
    let Ok(entries) = fs::read_dir(&agents_root) else {
        return None;
    };
    for entry in entries.flatten() {
        if !entry.file_type().map_or(false, |ft| ft.is_dir()) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        let agent_type = agent_type_for(project_path, &name);
        if matches!(agent_type.as_str(), "custom" | "manager" | "k2so") {
            return Some(name);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_frontmatter_reads_simple_kv() {
        let md = "---\nname: test\ntype: custom\n---\n# body\n";
        let fm = parse_frontmatter(md);
        assert_eq!(fm.get("name"), Some(&"test".to_string()));
        assert_eq!(fm.get("type"), Some(&"custom".to_string()));
    }

    #[test]
    fn parse_frontmatter_empty_when_no_fence() {
        let fm = parse_frontmatter("# heading only\n");
        assert!(fm.is_empty());
    }

    #[test]
    fn parse_frontmatter_skips_empty_keys_and_values() {
        // `: lonely` has empty key; `key:` has empty value. Both skipped.
        let fm = parse_frontmatter("---\n: lonely\nkey:\nrole: eng\n---\n");
        assert_eq!(fm.len(), 1);
        assert_eq!(fm.get("role"), Some(&"eng".to_string()));
    }

    #[test]
    fn agents_dir_and_agent_dir_are_consistent() {
        let root = agents_dir("/tmp/proj");
        assert_eq!(root, PathBuf::from("/tmp/proj/.k2so/agents"));
        let agent = agent_dir("/tmp/proj", "foo");
        assert_eq!(agent, root.join("foo"));
    }
}

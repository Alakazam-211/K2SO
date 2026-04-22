//! Roster — "who's in the office?"
//!
//! E3 of Phase 3. Agents discover each other by querying the roster:
//! they get back the list of known agents with their current
//! liveness state and a short skill summary. This is the primitive
//! that turns a loose collection of agents into
//! colleagues-who-know-each-other-exist.
//!
//! Data sources:
//!
//! - **Known agents** → scanning `.k2so/agents/<name>/` directories.
//!   Any directory with an `agent.md` file counts as a registered
//!   agent in that workspace. Pure filesystem read, no DB.
//! - **Liveness** → `session::registry` — for each agent name, check
//!   if any registered session has that agent_name. An agent with
//!   no live session is `Offline`.
//! - **Skill summary** → first 200 chars of `.k2so/agents/<name>/SKILL.md`
//!   if present; empty string otherwise.
//!
//! No provider trait here (unlike `companion::settings_bridge`) —
//! the roster's data sources are all either filesystem or
//! already-singleton (`session::registry`). Callers pass the
//! workspace root in explicitly so tests can point at a temp dir.

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::awareness::WorkspaceId;
use crate::session::registry;

/// Filter for `query`. Three shapes covering the common questions:
/// "who can I talk to here?", "who's live anywhere?", "who exists
/// at all?"
#[derive(Debug, Clone)]
pub enum RosterFilter<'a> {
    /// Live agents in a specific workspace root.
    LiveInWorkspace(&'a Path),
    /// Live agents anywhere the session::registry knows about.
    LiveEverywhere,
    /// All known agents in a workspace root, live or offline.
    AllKnown(&'a Path),
}

/// Per-agent roster entry. Carries enough for a caller to render
/// a list, choose a target, and decide whether to interrupt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentInfo {
    pub name: String,
    pub workspace: Option<WorkspaceId>,
    pub state: RosterState,
    /// First 200 chars of the agent's `SKILL.md`. Empty if no
    /// skill file is present.
    pub skill_summary: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(rename_all = "lowercase")]
pub enum RosterState {
    Live,
    Offline,
}

const SKILL_SUMMARY_MAX_CHARS: usize = 200;

/// Run a roster query. Never errors — filesystem read failures
/// return empty / skip-entry semantics rather than propagating.
/// The roster is a best-effort view; callers should treat its
/// output as "the agents I currently know about" not "all agents
/// that will ever exist."
pub fn query(filter: RosterFilter<'_>) -> Vec<AgentInfo> {
    match filter {
        RosterFilter::LiveEverywhere => live_agents_anywhere(),
        RosterFilter::LiveInWorkspace(root) => {
            let mut out = scan_workspace_root(root);
            out.retain(|info| info.state == RosterState::Live);
            out
        }
        RosterFilter::AllKnown(root) => scan_workspace_root(root),
    }
}

/// Convenience: look up one specific agent in a workspace root.
/// Returns `None` if the agent has no `.k2so/agents/<name>/`
/// directory.
pub fn lookup(workspace_root: &Path, agent: &str) -> Option<AgentInfo> {
    let agent_dir = workspace_root.join(".k2so/agents").join(agent);
    if !agent_dir.join("agent.md").exists() {
        return None;
    }
    Some(build_info(agent, workspace_root, &live_agent_names()))
}

// ─────────────────────────────────────────────────────────────────────
// Internals
// ─────────────────────────────────────────────────────────────────────

fn live_agents_anywhere() -> Vec<AgentInfo> {
    let live = live_agent_names();
    live.into_iter()
        .map(|name| AgentInfo {
            name,
            workspace: None,
            state: RosterState::Live,
            skill_summary: String::new(),
        })
        .collect()
}

fn scan_workspace_root(workspace_root: &Path) -> Vec<AgentInfo> {
    let agents_dir = workspace_root.join(".k2so/agents");
    let entries = match fs::read_dir(&agents_dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };
    let live = live_agent_names();
    let mut out = Vec::new();
    for entry in entries.filter_map(|r| r.ok()) {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        if name.starts_with('.') {
            // Skip hidden / dotfile-style dirs like `.archive`.
            continue;
        }
        if !path.join("agent.md").exists() {
            continue;
        }
        out.push(build_info(&name, workspace_root, &live));
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn build_info(name: &str, workspace_root: &Path, live: &[String]) -> AgentInfo {
    let state = if live.iter().any(|n| n == name) {
        RosterState::Live
    } else {
        RosterState::Offline
    };
    let skill_summary = read_skill_summary(workspace_root, name);
    AgentInfo {
        name: name.to_string(),
        workspace: Some(WorkspaceId(
            workspace_root.to_string_lossy().into_owned(),
        )),
        state,
        skill_summary,
    }
}

fn read_skill_summary(workspace_root: &Path, agent: &str) -> String {
    let skill_path = workspace_root
        .join(".k2so/agents")
        .join(agent)
        .join("SKILL.md");
    let contents = match fs::read_to_string(&skill_path) {
        Ok(s) => s,
        Err(_) => return String::new(),
    };
    // Strip frontmatter if present (between leading `---` fences)
    // so the summary shows the actual description rather than
    // YAML metadata.
    let body = strip_frontmatter(&contents);
    body.chars().take(SKILL_SUMMARY_MAX_CHARS).collect()
}

fn strip_frontmatter(s: &str) -> &str {
    if !s.starts_with("---") {
        return s.trim_start();
    }
    // Find the closing `---` on its own line (or leading whitespace +
    // `---`).
    let mut lines = s.lines();
    let first = lines.next();
    if first.map(|l| l.trim()) != Some("---") {
        return s.trim_start();
    }
    let mut byte_offset = first.map(|l| l.len()).unwrap_or(0) + 1;
    for line in lines {
        byte_offset += line.len() + 1;
        if line.trim() == "---" {
            return s[byte_offset..].trim_start();
        }
    }
    // No closing fence — treat whole thing as body.
    s.trim_start()
}

/// Enumerate agent names that currently have a live session in
/// `session::registry`. A session without an `agent_name` binding
/// (anonymous / test fixtures) is skipped.
fn live_agent_names() -> Vec<String> {
    registry::list_ids()
        .into_iter()
        .filter_map(|id| registry::lookup(&id).and_then(|e| e.agent_name()))
        .collect()
}

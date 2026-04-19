//! Wake-prompt composers + per-agent wakeup.md resolution.
//!
//! When the heartbeat scheduler decides an agent should wake, the
//! launch path needs to turn that decision into the `--append-system-
//! prompt <string>` argument passed to `claude`. This module owns that
//! composition:
//!
//! - Per-type wakeup templates compiled into the binary
//!   ([`WAKEUP_TEMPLATE_WORKSPACE`] etc.).
//! - Filesystem resolution for the on-disk `WAKEUP.md` ([`read_agent_wakeup`]).
//! - Heartbeat-row-aware resolution ([`default_heartbeat_wakeup_abs`]).
//! - Pure composers ([`compose_manager_wake_from_body`], …) that take
//!   a raw body string and emit the full wake message. Split out so
//!   the branch coverage (body present / empty / missing) is unit-
//!   testable without scaffolding a filesystem.
//!
//! All entry points are Tauri-free so the daemon can call them
//! identically to the Tauri app.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use crate::agents::{agent_dir, agent_type_for, resolve_project_id};
use crate::db::schema::AgentHeartbeat;
use crate::terminal::event_sink::TerminalEventSink;
use crate::terminal::grid_types::GridUpdate;

// Shipped templates. Built-in markdown copied from
// `crates/k2so-core/wakeup_templates/*.md` at compile time. The
// matching files ship alongside this module; src-tauri's historical
// copies under `src-tauri/wakeup_templates/` are retained only for any
// reference callers that still want the UPPERCASE file scaffold — the
// authoritative sources now live here.

pub const WAKEUP_TEMPLATE_WORKSPACE: &str =
    include_str!("../../wakeup_templates/workspace.md");
pub const WAKEUP_TEMPLATE_MANAGER: &str =
    include_str!("../../wakeup_templates/manager.md");
pub const WAKEUP_TEMPLATE_CUSTOM: &str =
    include_str!("../../wakeup_templates/custom.md");
pub const WAKEUP_TEMPLATE_K2SO: &str = include_str!("../../wakeup_templates/k2so.md");

/// Resolve the shipped template for a given agent type. Returns `None`
/// for agent types that don't use wake-up at all (currently just
/// `agent-template`, which is always dispatched with explicit orders
/// by a manager).
pub fn wakeup_template_for(agent_type: &str) -> Option<&'static str> {
    match agent_type {
        "manager" | "coordinator" | "pod-leader" => Some(WAKEUP_TEMPLATE_MANAGER),
        "custom" => Some(WAKEUP_TEMPLATE_CUSTOM),
        "k2so" => Some(WAKEUP_TEMPLATE_K2SO),
        _ => None,
    }
}

/// Canonical location of an agent's WAKEUP.md on disk (UPPERCASE as of
/// 0.32.7).
pub fn agent_wakeup_path(project_path: &str, agent_name: &str) -> PathBuf {
    agent_dir(project_path, agent_name).join("WAKEUP.md")
}

/// Canonical location of the workspace-level WAKEUP.md (used by the
/// `__lead__` Workspace Manager).
pub fn workspace_wakeup_path(project_path: &str) -> PathBuf {
    PathBuf::from(project_path).join(".k2so").join("WAKEUP.md")
}

/// Read an agent's `WAKEUP.md`, falling back to the shipped template
/// if the file is missing or empty. Returns `None` for agent types
/// that don't use wake-up at all.
pub fn read_agent_wakeup(
    project_path: &str,
    agent_name: &str,
    agent_type: &str,
) -> Option<String> {
    let template = wakeup_template_for(agent_type)?;
    let path = agent_wakeup_path(project_path, agent_name);
    match fs::read_to_string(&path) {
        Ok(s) if !s.trim().is_empty() => Some(s),
        _ => Some(template.to_string()),
    }
}

/// Strip YAML frontmatter (`---`-delimited) from markdown content,
/// returning just the body. Body-less inputs (no closing fence) are
/// returned trimmed.
pub fn strip_frontmatter(content: &str) -> String {
    if content.starts_with("---") {
        if let Some(end) = content[3..].find("---") {
            return content[3 + end + 3..].trim().to_string();
        }
    }
    content.trim().to_string()
}

/// Absolute path of the primary heartbeat's `WAKEUP.md` for an agent.
/// Prefers a row named `"triage"` (the one
/// `migrate_or_scaffold_lead_heartbeat` creates for manager mode);
/// falls back to the first enabled row. `None` when the agent has no
/// heartbeats configured — callers should fall back to the shipped
/// template in that case.
pub fn default_heartbeat_wakeup_abs(
    project_path: &str,
    _agent_name: &str,
) -> Option<String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let project_id = resolve_project_id(&conn, project_path)?;
    let rows = AgentHeartbeat::list_enabled(&conn, &project_id).ok()?;
    let hb = rows.iter().find(|h| h.name == "triage").or_else(|| rows.first())?;
    let abs = std::path::Path::new(project_path).join(&hb.wakeup_path);
    Some(abs.to_string_lossy().to_string())
}

/// Pure composer for the Workspace Manager wake prompt. Given an
/// optional raw wakeup body (as read from disk), produces the full
/// wake message — frontmatter stripped, fallback to
/// [`WAKEUP_TEMPLATE_WORKSPACE`] if the body is empty or missing.
pub fn compose_manager_wake_from_body(raw_body: Option<&str>) -> String {
    let wakeup_body = raw_body
        .map(|s| strip_frontmatter(s).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| WAKEUP_TEMPLATE_WORKSPACE.trim().to_string());
    format!(
        "# K2SO Heartbeat Wake — Workspace Manager\n\n\
         The heartbeat scheduler woke you because new work has arrived in the \
         workspace inbox. Your wake-up instructions are below; follow them \
         and exit when done.\n\n\
         ----\n\n{}",
        wakeup_body
    )
}

/// Compose the `--append-system-prompt` text for `__lead__` at wake
/// time. Reads the wakeup body from the default heartbeat row and
/// falls back to the shipped template if no row exists yet.
pub fn compose_wake_prompt_for_lead(project_path: &str) -> String {
    let raw = default_heartbeat_wakeup_abs(project_path, "__lead__")
        .and_then(|p| fs::read_to_string(&p).ok());
    compose_manager_wake_from_body(raw.as_deref())
}

/// Pure composer for a regular-agent wake prompt. Given the raw
/// wakeup body string (already read from disk), produces the full
/// wake message. Returns `None` when the input is `None`, matching
/// the "no wake for agent-template" semantic.
pub fn compose_agent_wake_from_body(raw_body: Option<&str>) -> Option<String> {
    let body = raw_body?;
    Some(format!(
        "# K2SO Heartbeat Wake\n\n\
         The heartbeat scheduler woke you. Your wake-up instructions are below; \
         follow them and exit when done.\n\n\
         ----\n\n{}",
        body.trim()
    ))
}

/// Compose the `--append-system-prompt` text for a regular agent
/// woken by the heartbeat scheduler. Returns `None` for agent types
/// that don't have wake-up semantics (agent-template).
pub fn compose_wake_prompt_for_agent(
    project_path: &str,
    agent_name: &str,
) -> Option<String> {
    let agent_type = agent_type_for(project_path, agent_name);
    let wakeup = read_agent_wakeup(project_path, agent_name, &agent_type)?;
    compose_agent_wake_from_body(Some(&wakeup))
}

/// Compose the wake prompt from an explicit wakeup file path. Used by
/// the multi-heartbeat scheduler — each heartbeat row stores the path
/// it should read rather than relying on a naming convention.
pub fn compose_wake_prompt_from_path(
    wakeup_path: &std::path::Path,
) -> Option<String> {
    let content = std::fs::read_to_string(wakeup_path).ok()?;
    compose_agent_wake_from_body(Some(&content))
}

// ── Headless wake spawn ─────────────────────────────────────────────────
//
// Daemon-side entry point. The full `spawn_wake_pty` in
// src-tauri/agent_hooks.rs composes per-agent CLAUDE.md + inspects
// active worktrees + auto-saves the Claude session-ID 5 seconds later
// by scanning ~/.claude/projects. That machinery is Tauri-app territory.
//
// The daemon doesn't need any of it for lid-closed wakes. The agent
// just needs to launch `claude --dangerously-skip-permissions
// --append-system-prompt <body>` in the project directory, with a PTY
// so the TUI initializes. This helper is that minimal path.
//
// Design choices:
// - NoOp `TerminalEventSink`. Lid-closed wakes have no UI consumer;
//   when the Tauri app reopens, it learns about the running PTY via
//   `HookEvent::CliTerminalSpawnBackground` (which propagates through
//   the daemon's /events WS).
// - `k2so_agents_lock` marks the session `running` so the scheduler's
//   `is_agent_locked` check skips the agent on the next tick.
// - Sensible default terminal dims (120×38) match the existing nudge
//   size from src-tauri.
//
// This is intentionally simpler than `k2so_agents_build_launch` —
// no worktree resume, no inbox delegate. Those paths remain Tauri-
// only for v1; they involve user-facing decisions (new branches,
// work-item moves) that belong in supervised sessions.

/// `TerminalEventSink` impl that ignores every event. Lives alongside
/// the wake path because that's currently its only consumer; if more
/// host-less terminal use cases appear we can promote it to
/// `k2so_core::terminal`.
pub struct NoOpTerminalEventSink;

impl TerminalEventSink for NoOpTerminalEventSink {
    fn on_title(&self, _terminal_id: &str, _title: &str) {}
    fn on_bell(&self, _terminal_id: &str) {}
    fn on_exit(&self, _terminal_id: &str, _exit_code: i32) {}
    fn on_grid_update(&self, _terminal_id: &str, _update: &GridUpdate) {}
}

/// Spawn a wake PTY headlessly from the daemon. Returns the generated
/// terminal ID on success.
///
/// Side effects in order:
/// 1. `TerminalManager::create` in `crate::terminal::shared()` — the
///    PTY is owned by the daemon process, survives the Tauri app
///    reopening/closing.
/// 2. [`crate::agents::session::k2so_agents_lock`] — writes the
///    `agent_sessions` row + legacy `.lock` file so subsequent
///    scheduler ticks skip this agent.
/// 3. `AgentHookEventSink` fires `HookEvent::CliTerminalSpawnBackground`
///    — lets any connected Tauri UI create a tab for the new PTY via
///    the daemon's /events WebSocket.
pub fn spawn_wake_headless(
    agent_name: &str,
    project_path: &str,
    wake_prompt: &str,
) -> Result<String, String> {
    let terminal_id = format!("wake-{}-{}", agent_name, uuid::Uuid::new_v4());

    let args = vec![
        "--dangerously-skip-permissions".to_string(),
        "--append-system-prompt".to_string(),
        wake_prompt.to_string(),
    ];

    let sink: Arc<dyn TerminalEventSink> = Arc::new(NoOpTerminalEventSink);
    let manager = crate::terminal::shared();
    let mut manager = manager.lock();
    manager
        .create(
            terminal_id.clone(),
            project_path.to_string(),
            Some("claude".to_string()),
            Some(args),
            Some(120),
            Some(38),
            sink,
        )
        .map_err(|e| format!("spawn wake PTY: {e}"))?;
    drop(manager);

    crate::log_debug!(
        "[daemon/wake] spawned PTY for {} in {} (id={})",
        agent_name,
        project_path,
        terminal_id
    );

    // Mark the session running so the next scheduler tick skips it.
    // Best-effort: don't fail the spawn if the DB write trips (PTY is
    // already live and will run regardless).
    let _ = crate::agents::session::k2so_agents_lock(
        project_path.to_string(),
        agent_name.to_string(),
        Some(terminal_id.clone()),
        Some("system".to_string()),
    );

    // Tell any connected UI. Wire format matches what the existing
    // src-tauri spawn_wake_pty emits so the React frontend's listener
    // doesn't need to branch on origin.
    crate::agent_hooks::emit(
        crate::agent_hooks::HookEvent::CliTerminalSpawnBackground,
        serde_json::json!({
            "terminalId": &terminal_id,
            "command": "claude",
            "cwd": project_path,
            "projectPath": project_path,
            "agentName": agent_name,
        }),
    );

    Ok(terminal_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_frontmatter_drops_fm_and_keeps_body() {
        let md = "---\nname: x\n---\n\n# body\ncontent\n";
        let body = strip_frontmatter(md);
        assert_eq!(body, "# body\ncontent");
    }

    #[test]
    fn strip_frontmatter_leaves_body_without_fm_alone() {
        assert_eq!(strip_frontmatter("# heading\n"), "# heading");
    }

    #[test]
    fn compose_manager_wake_falls_back_to_shipped_template_on_none() {
        let composed = compose_manager_wake_from_body(None);
        assert!(composed.contains("K2SO Heartbeat Wake — Workspace Manager"));
        // Shipped template text should appear in the composed prompt.
        assert!(composed.len() > 200, "composed prompt too short: {}", composed.len());
    }

    #[test]
    fn compose_manager_wake_strips_frontmatter() {
        let raw = "---\ndescription: blurb\n---\n\n# body\nkey text\n";
        let composed = compose_manager_wake_from_body(Some(raw));
        assert!(composed.contains("# body"));
        assert!(composed.contains("key text"));
        assert!(!composed.contains("description:"));
    }

    #[test]
    fn compose_agent_wake_returns_none_on_none_input() {
        assert!(compose_agent_wake_from_body(None).is_none());
    }

    #[test]
    fn compose_agent_wake_wraps_body() {
        let composed = compose_agent_wake_from_body(Some("do the thing")).unwrap();
        assert!(composed.contains("K2SO Heartbeat Wake"));
        assert!(composed.contains("do the thing"));
    }

    #[test]
    fn wakeup_template_for_known_types() {
        assert!(wakeup_template_for("manager").is_some());
        assert!(wakeup_template_for("k2so").is_some());
        assert!(wakeup_template_for("custom").is_some());
        assert!(wakeup_template_for("agent-template").is_none());
        assert!(wakeup_template_for("bogus").is_none());
    }
}

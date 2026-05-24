//! Read-only triage summary. Returns the plain-text view the user
//! sees when they run `k2so agents triage`: which agents have
//! pending work, their inbox items with priority/type/source
//! labels, which are locked, and the workspace-level inbox waiting
//! on the manager.
//!
//! History: pre-Phase-4 this was `k2so_agents_triage_summary` in
//! `src-tauri/src/commands/k2so_agents.rs` and was served by
//! Tauri's `agent_hooks` HTTP listener at `/cli/agents/triage`.
//! Phase 4 H7 retired that listener and pointed the CLI at the
//! daemon, which had its own `/cli/agents/triage` that ran a
//! destructive `scheduler_tick` + `heartbeat_tick` + spawn.
//!
//! Shipping H7 without migrating this function silently changed
//! the semantics of `k2so agents triage` from "show me what's
//! pending" to "launch everything that's pending." This module
//! restores the read-only behavior. The destructive path moves
//! to `/cli/scheduler-tick` where `~/.k2so/heartbeat.sh` already
//! expects to find it (matching 0.33.0 Tauri route layout).
//!
//! Behavior is byte-for-byte identical to the pre-H7 Tauri
//! version — tier1 behavior tests (which assert on specific
//! substrings like `LOCKED`, agent names, and priority labels)
//! exercise this shape.

use std::fs;

use crate::agents::scheduler::{
    agent_work_dir, get_workspace_state, is_agent_locked, workspace_inbox_dir,
};
use crate::agents::work_item::{read_work_item, WorkItem};
use crate::agents::{agents_dir, parse_frontmatter};

/// Plain-text triage summary. Walks `.k2so/agents/*/work/inbox` +
/// `.k2so/work/inbox` on disk (no DB access) and renders a human-
/// readable report.
///
/// Workspace-state capability gating:
/// - `off`  → items in that source category are silently omitted.
/// - `gated` → items appear with a `[NEEDS APPROVAL]` tag.
/// - `auto` → plain listing.
/// Missing workspace state = "auto" everywhere (permissive).
pub fn triage_summary(project_path: &str) -> Result<String, String> {
    let dir = agents_dir(project_path);
    if !dir.exists() {
        return Ok("No agents configured.".to_string());
    }

    let ws_state = get_workspace_state(project_path);
    let state_name = ws_state
        .as_ref()
        .map(|t| t.name.as_str())
        .unwrap_or("(no state set)");

    let mut summary = String::new();
    summary.push_str(&format!("Workspace state: {}\n\n", state_name));
    let entries = fs::read_dir(&dir).map_err(|e| e.to_string())?;

    for entry in entries.flatten() {
        if !entry.file_type().map_or(false, |ft| ft.is_dir()) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();

        let inbox = agent_work_dir(project_path, &name, "inbox");
        let active = agent_work_dir(project_path, &name, "active");

        let inbox_items: Vec<WorkItem> = if inbox.exists() {
            fs::read_dir(&inbox)
                .ok()
                .map(|entries| {
                    entries
                        .flatten()
                        .filter(|e| {
                            e.path().extension().map_or(false, |ext| ext == "md")
                        })
                        .filter_map(|e| read_work_item(&e.path(), "inbox"))
                        .collect()
                })
                .unwrap_or_default()
        } else {
            vec![]
        };

        let active_count = if active.exists() {
            fs::read_dir(&active)
                .map(|e| {
                    e.flatten()
                        .filter(|e| {
                            e.path().extension().map_or(false, |ext| ext == "md")
                        })
                        .count()
                })
                .unwrap_or(0)
        } else {
            0
        };

        let is_locked = is_agent_locked(project_path, &name);

        if inbox_items.is_empty() && active_count == 0 {
            continue;
        }

        let agent_md_path = entry.path().join("AGENT.md");
        let (agent_type, agent_role) = if agent_md_path.exists() {
            let content = fs::read_to_string(&agent_md_path).unwrap_or_default();
            let fm = parse_frontmatter(&content);
            (
                fm.get("type")
                    .cloned()
                    .unwrap_or_else(|| "agent-template".to_string()),
                fm.get("role").cloned().unwrap_or_default(),
            )
        } else {
            ("agent-template".to_string(), String::new())
        };

        summary.push_str(&format!(
            "Agent: {} (type: {}, role: {})\n",
            name, agent_type, agent_role
        ));
        if is_locked {
            summary.push_str("  Status: LOCKED (active session running)\n");
        }
        if active_count > 0 {
            summary.push_str(&format!(
                "  Active: {} items in progress\n",
                active_count
            ));
        }
        for item in &inbox_items {
            let cap_status = ws_state
                .as_ref()
                .map(|t| t.capability_for_source(&item.source).to_string())
                .unwrap_or_else(|| "auto".to_string());
            if cap_status == "off" {
                continue;
            }
            let gate_label = if cap_status == "gated" {
                " [NEEDS APPROVAL]"
            } else {
                ""
            };
            summary.push_str(&format!(
                "  Inbox: \"{}\" (priority: {}, type: {}, source: {}{})\n",
                item.title, item.priority, item.item_type, item.source, gate_label
            ));
        }
        summary.push('\n');
    }

    let ws_inbox = workspace_inbox_dir(project_path);
    if ws_inbox.exists() {
        let ws_items: Vec<WorkItem> = fs::read_dir(&ws_inbox)
            .ok()
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|e| e.path().extension().map_or(false, |ext| ext == "md"))
                    .filter_map(|e| read_work_item(&e.path(), "inbox"))
                    .collect()
            })
            .unwrap_or_default();

        if !ws_items.is_empty() {
            let lead_locked = is_agent_locked(project_path, "__lead__");
            summary.push_str("Workspace Inbox (unassigned — needs Coordinator):\n");
            if lead_locked {
                summary.push_str("  Coordinator: LOCKED (active session running)\n");
            }
            for item in &ws_items {
                let cap_status = ws_state
                    .as_ref()
                    .map(|t| t.capability_for_source(&item.source).to_string())
                    .unwrap_or_else(|| "auto".to_string());
                if cap_status == "off" {
                    continue;
                }
                let gate_label = if cap_status == "gated" {
                    " [NEEDS APPROVAL]"
                } else {
                    ""
                };
                summary.push_str(&format!(
                    "  \"{}\" (priority: {}, type: {}, source: {}{})\n",
                    item.title, item.priority, item.item_type, item.source, gate_label
                ));
            }
            summary.push('\n');
        }
    }

    if summary.is_empty() {
        Ok("No agents have pending work.".to_string())
    } else {
        Ok(summary)
    }
}

/// Determine what should be launched based on triage.
///
/// Agents are templates — the same agent (e.g., "backend-eng") can run
/// in multiple worktrees simultaneously. Each inbox item gets its own
/// worktree when delegated.
///
/// Triage order:
/// 1. Workspace inbox has items → wake lead agent ("__lead__")
/// 2. Sub-agent inboxes have items → wake those agents (one launch per inbox item)
///
/// **DEPRECATED — `legacy-per-agent-heartbeat` chokepoint.**
/// Pre-0.30s K2SO used inbox contents to decide whether to autonomously
/// wake an agent. The new model lives in the `agent_heartbeats` table
/// (workspace-scoped, explicit schedules). This function is being kept
/// alive only for the launch-failure-retry path in active-agents.ts;
/// gated on `projects.heartbeat_mode != 'off'` so opted-out workspaces
/// don't get auto-launched even if a stray caller invokes us. Planned
/// for removal in 0.37.x.
///
/// Phase 2 Unit 7d: moved from `src-tauri/src/commands/k2so_agents.rs`
/// so the daemon and any other thin client can call the same code
/// without depending on the Tauri command crate.
#[deprecated(
    note = "Inbox-driven triage — superseded by agent_heartbeats. \
            Planned for removal in 0.37.x. See `legacy-per-agent-heartbeat` tag."
)]
pub fn triage_decide(project_path: &str) -> Result<Vec<String>, String> {
    // Gate 0: project must have heartbeats enabled. Without this, an
    // inbox with items unconditionally fires wakes — which is what was
    // happening to the K2SO workspace in 0.36.3 even with all DB
    // heartbeat rows disabled.
    let project_mode = {
        let db = crate::db::shared();
        let conn = db.lock();
        conn.query_row(
            "SELECT heartbeat_mode FROM projects WHERE path = ?1",
            rusqlite::params![project_path],
            |row| row.get::<_, String>(0),
        )
        .ok()
    };
    if project_mode.as_deref() == Some("off") {
        return Ok(Vec::new());
    }

    let mut launchable = Vec::new();

    // Step 1: Check workspace inbox
    let ws_inbox = workspace_inbox_dir(project_path);
    let has_workspace_inbox = ws_inbox.exists()
        && fs::read_dir(&ws_inbox)
            .map(|e| {
                e.flatten()
                    .any(|e| e.path().extension().map_or(false, |ext| ext == "md"))
            })
            .unwrap_or(false);

    if has_workspace_inbox {
        launchable.push("__lead__".to_string());
    }

    // Step 2: Check sub-agent inboxes
    // An agent is a template/role — it can have multiple items in its
    // inbox and each one gets its own worktree. We launch once per
    // agent that has inbox items. The delegate/build_launch function
    // handles picking the top-priority item.
    let dir = agents_dir(project_path);
    if dir.exists() {
        if let Ok(entries) = fs::read_dir(&dir) {
            for entry in entries.flatten() {
                if !entry.file_type().map_or(false, |ft| ft.is_dir()) {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().to_string();

                let inbox = agent_work_dir(project_path, &name, "inbox");
                let has_inbox = inbox.exists()
                    && fs::read_dir(&inbox)
                        .map(|e| {
                            e.flatten()
                                .any(|e| e.path().extension().map_or(false, |ext| ext == "md"))
                        })
                        .unwrap_or(false);

                if has_inbox {
                    launchable.push(name);
                }
            }
        }
    }

    Ok(launchable)
}

#[cfg(test)]
mod tests {
    //! Tests for `triage_decide` — exercising the inbox-driven wake
    //! decision logic. Tests scaffold a workspace under
    //! `std::env::temp_dir()` (uuid-suffixed) to match the existing
    //! core-test convention (k2so-core has no `tempfile` dev-dep).
    //! `heartbeat_mode` DB lookup returns `None` for projects not
    //! registered in the shared DB — fine for these tests because the
    //! function treats `None` as not-gated-off.
    use super::*;
    use std::fs;
    use uuid::Uuid;

    fn fixture() -> std::path::PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("k2so-triage-decide-{}", Uuid::new_v4()));
        fs::create_dir_all(
            dir.join(".k2so")
                .join("agents")
                .join("backend-eng")
                .join("work")
                .join("inbox"),
        )
        .unwrap();
        fs::create_dir_all(dir.join(".k2so").join("work").join("inbox")).unwrap();
        dir
    }

    fn cleanup(dir: &std::path::Path) {
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    #[allow(deprecated)]
    fn triage_decide_returns_empty_when_no_inbox_items() {
        let dir = fixture();
        let project = dir.to_string_lossy().to_string();
        let launchable = triage_decide(&project).unwrap();
        assert!(
            launchable.is_empty(),
            "no inbox items → empty: {:?}",
            launchable
        );
        cleanup(&dir);
    }

    #[test]
    #[allow(deprecated)]
    fn triage_decide_picks_up_workspace_inbox() {
        let dir = fixture();
        fs::write(
            dir.join(".k2so").join("work").join("inbox").join("task.md"),
            "---\ntitle: Test\n---\nBody",
        )
        .unwrap();
        let launchable = triage_decide(&dir.to_string_lossy()).unwrap();
        assert!(
            launchable.contains(&"__lead__".to_string()),
            "workspace inbox triggers __lead__: {:?}",
            launchable
        );
        cleanup(&dir);
    }

    #[test]
    #[allow(deprecated)]
    fn triage_decide_picks_up_agent_inbox() {
        let dir = fixture();
        fs::write(
            dir.join(".k2so")
                .join("agents")
                .join("backend-eng")
                .join("work")
                .join("inbox")
                .join("oauth.md"),
            "---\ntitle: OAuth\n---\nBody",
        )
        .unwrap();
        let launchable = triage_decide(&dir.to_string_lossy()).unwrap();
        assert!(
            launchable.contains(&"backend-eng".to_string()),
            "agent inbox triggers agent: {:?}",
            launchable
        );
        cleanup(&dir);
    }
}

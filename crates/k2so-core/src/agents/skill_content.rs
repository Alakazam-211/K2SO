//! Skill + CLAUDE.md content generators for each agent tier.
//!
//! Four generators (`generate_manager_skill_content`,
//! `generate_custom_agent_skill_content`, `generate_k2so_agent_skill_content`,
//! `generate_template_skill_content`) each produce the tier's canonical
//! SKILL.md body. [`compose_agent_wake_context`] (previously known as
//! `generate_agent_claude_md_content` — renamed during the 0.33.0 move
//! to reflect what it actually returns: the full `--append-system-prompt`
//! text an agent sees on wake) composes identity + project context +
//! standing orders + tier skill body into the final system prompt
//! string, and as a side effect writes the tier skill body to the
//! agent's SKILL.md.
//!
//! The SKILL wrapping / versioning / checksum protocol itself lives in
//! [`super::skill`]; this module is the content side.
//!
//! All four `generate_*_skill_content` entry points also pull custom
//! layers from `~/.k2so/templates/<tier>/*.md` via [`load_custom_layers`]
//! — that's how the Agent Skills Settings UI's user-editable tab
//! injects project-global conventions into every agent of a given tier.
//!
//! [`super::skill`]: crate::agents::skill

use std::fs;
use std::path::PathBuf;

use crate::agents::work_item::{safe_read_to_string, WorkItem};
use crate::agents::{
    agent_dir, agent_type_for, agents_dir, parse_frontmatter, resolve_project_id,
};
use crate::agents::scheduler::{agent_work_dir, get_workspace_state};
use crate::agents::skill::{ensure_skill_up_to_date, SKILL_VERSION_TEMPLATE};
use crate::agents::wake::strip_frontmatter;
use crate::fs_atomic::{atomic_write_str, log_if_err};

// Embedded documentation snippet that's appended to Custom-mode
// agents' CLAUDE.md when the user hasn't overridden it in AGENT.md.
// Moved from src-tauri/src/commands/k2so_agents.rs alongside the
// generators that use it.
pub const CUSTOM_AGENT_HEARTBEAT_DOCS: &str = r#"## Heartbeats

Your wake schedule is controlled by **named heartbeats** the user
configures in Settings → Heartbeats (or via the CLI). Each
heartbeat has its own WAKEUP.md file and its own cron-style
schedule — daily at a clock time, weekly on specific days,
hourly at a fixed interval, etc. You don't adjust your own
cadence; the user owns the schedule and you focus on responding
to whatever the heartbeat woke you to do.

When a heartbeat fires, you receive its WAKEUP.md content as
your first user message. Do what it asks, then exit.

## Inspecting + managing heartbeats from your terminal

```
k2so heartbeat list                                  # Heartbeats configured for this workspace
k2so heartbeat show <name>                           # Schedule + last fire details
k2so heartbeat add --name <name> --hourly --interval-seconds 300
k2so heartbeat add --name <name> --daily --time 09:00
k2so heartbeat edit <name> --hourly --interval-seconds 600
k2so heartbeat remove <name>
```

The user can also drive the same actions from Settings →
Heartbeats. Both paths converge on the same `agent_heartbeats`
table; whichever you prefer is fine.

## Available Tools

Standard CLI tools are available in your terminal (`gh`, `git`,
`curl`, etc.). K2SO tools:

```
k2so terminal spawn --title "..." --command "..."   # Run parallel tasks
k2so checkin --agent <your-name>                    # Read your inbox + peer status + activity
k2so status "<message>" --agent <your-name>         # Update your visible status
k2so done                                           # Signal task completion
```
"#;

/// Format a capability state for display in CLAUDE.md.
pub fn format_cap(cap: &str) -> &str {
    match cap {
        "auto" => "auto (build + merge)",
        "gated" => "gated (build PR, wait for approval)",
        "off" => "off (do not act)",
        _ => cap,
    }
}

/// Extract a named section from markdown content (## Heading through next ## or end).
/// Returns the body text (without the heading itself), or None if the section is empty/absent.
pub fn extract_section(content: &str, heading: &str) -> Option<String> {
    let marker = format!("## {}", heading);
    let start = content.find(&marker)?;
    let after_heading = start + marker.len();
    // Skip to the line after the heading (or use remaining content if heading is at EOF)
    let body_start = match content[after_heading..].find('\n') {
        Some(i) => after_heading + i + 1,
        None => return None, // heading at EOF with no body
    };
    // Find the next ## heading or end of content
    let body_end = content[body_start..]
        .find("\n## ")
        .map(|i| body_start + i)
        .unwrap_or(content.len());
    let body = content[body_start..body_end].trim();
    // Check if there's meaningful content (not just pure HTML comments)
    // A line is a "pure comment" only if it starts with <!-- and ends with -->
    // Lines with mixed content (e.g., "real text<!-- note -->") are kept
    let meaningful: Vec<&str> = body.lines()
        .filter(|l| {
            let t = l.trim();
            if t.is_empty() { return false; }
            // Pure comment line: starts with <!-- and ends with -->
            if t.starts_with("<!--") && t.ends_with("-->") { return false; }
            true
        })
        .collect();
    if meaningful.is_empty() {
        None
    } else {
        Some(body.to_string())
    }
}

/// Generate the universal skill protocol for the Workspace Manager.
/// Includes delegation, cross-workspace messaging, and full orchestration commands.
/// Load user-created custom layers from ~/.k2so/templates/{tier}/*.md.
/// Returns concatenated markdown sections with titles derived from filenames.
pub fn load_custom_layers(tier: &str) -> String {
    let dir = match dirs::home_dir() {
        Some(h) => h.join(".k2so/templates").join(tier),
        None => return String::new(),
    };
    if !dir.exists() { return String::new(); }
    let mut layers = Vec::new();
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map_or(false, |ext| ext == "md") {
                if let Ok(content) = fs::read_to_string(&path) {
                    if content.trim().is_empty() { continue; }
                    let name = path.file_stem().unwrap_or_default().to_string_lossy().replace('-', " ");
                    let title: String = name.split_whitespace()
                        .map(|w| {
                            let mut c = w.chars();
                            match c.next() {
                                Some(f) => f.to_uppercase().to_string() + c.as_str(),
                                None => String::new(),
                            }
                        })
                        .collect::<Vec<_>>().join(" ");
                    layers.push(format!("## {}\n\n{}", title, content.trim()));
                }
            }
        }
    }
    layers.sort(); // Alphabetical for consistency
    if layers.is_empty() { return String::new(); }
    layers.join("\n\n") + "\n\n"
}

pub fn generate_manager_skill_content(project_path: &str, project_name: &str) -> String {
    let mut skill = String::new();

    // ── 1. Identity + Workspace Context ──
    skill.push_str(&format!("# K2SO Workspace Manager Skill\n\nYou are the Workspace Manager for **{}**.\n\n", project_name));

    // Read workspace state from DB
    {
        let db = crate::db::shared();
        let conn = db.lock();
        if let Some(project_id) = resolve_project_id(&conn, project_path) {
            // Get workspace state
            let state_info: Option<(String, String)> = conn.query_row(
                "SELECT ws.name, ws.description FROM workspace_states ws \
                 JOIN projects p ON p.tier_id = ws.id WHERE p.id = ?1",
                rusqlite::params![project_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            ).ok();

            if let Some((state_name, state_desc)) = state_info {
                skill.push_str(&format!("**Mode: {}** — {}\n\n", state_name, state_desc));
            }

            // Get connected workspaces
            let mut connections = Vec::new();
            if let Ok(rels) = crate::db::schema::WorkspaceRelation::list_for_source(&conn, &project_id) {
                for r in &rels {
                    if let Ok(name) = conn.query_row(
                        "SELECT name FROM projects WHERE id = ?1",
                        rusqlite::params![r.target_project_id],
                        |row| row.get::<_, String>(0),
                    ) {
                        connections.push(format!("- **{}** (oversees)", name));
                    }
                }
            }
            if let Ok(rels) = crate::db::schema::WorkspaceRelation::list_for_target(&conn, &project_id) {
                for r in &rels {
                    if let Ok(name) = conn.query_row(
                        "SELECT name FROM projects WHERE id = ?1",
                        rusqlite::params![r.source_project_id],
                        |row| row.get::<_, String>(0),
                    ) {
                        connections.push(format!("- **{}** (connected agent)", name));
                    }
                }
            }
            if !connections.is_empty() {
                skill.push_str("## Connected Workspaces\n\n");
                for c in &connections {
                    skill.push_str(c);
                    skill.push('\n');
                }
                skill.push('\n');
            }
        }
    }

    // ── 2. Team Roster (from agents directory) ──
    let agents_root = agents_dir(project_path);
    if agents_root.exists() {
        let mut team = Vec::new();
        if let Ok(entries) = fs::read_dir(&agents_root) {
            for entry in entries.flatten() {
                if !entry.file_type().map_or(false, |ft| ft.is_dir()) { continue; }
                let name = entry.file_name().to_string_lossy().to_string();
                let agent_md = entry.path().join("AGENT.md");
                if agent_md.exists() {
                    let content = fs::read_to_string(&agent_md).unwrap_or_default();
                    let fm = parse_frontmatter(&content);
                    let role = fm.get("role").cloned().unwrap_or_default();
                    let agent_type = fm.get("type").cloned().unwrap_or_default();
                    // Skip the manager itself and k2so-agent
                    if agent_type == "manager" || agent_type == "coordinator" || agent_type == "pod-leader" || agent_type == "k2so" { continue; }
                    team.push(format!("- **{}** — {}", name, role));
                }
            }
        }
        if !team.is_empty() {
            skill.push_str("## Your Team\n\nThese agent templates can be delegated work. Each runs in its own worktree branch.\n\n");
            for t in &team {
                skill.push_str(t);
                skill.push('\n');
            }
            skill.push('\n');
        }
    }

    // ── User Custom Layers (from ~/.k2so/templates/manager/) ──
    let custom_layers = load_custom_layers("manager");
    if !custom_layers.is_empty() {
        skill.push_str(&custom_layers);
    }

    // ── 3. Standing Orders ──
    skill.push_str(r#"## Standing Orders (Every Wake Cycle)

On each wake, run through this in order:

1. `k2so checkin` — read your messages, work items, peer status, and activity feed
2. **Triage messages** — respond to any messages from connected agents or the user
3. **Triage work items** — sort by priority (critical > high > normal > low)
4. **Simple tasks**: work directly in the main branch. No delegation needed.
5. **Complex tasks**: delegate to the best-matched agent template (see Delegation below)
6. **Check active agents** — are any blocked or waiting for review?
7. **Review completed work** — approve (merge) or reject with feedback
8. `k2so status "triaging 3 inbox items"` — keep your status updated
9. When everything is handled: `k2so done` or `k2so done --blocked "reason"`

"#);

    // ── 4. Decision Framework by Mode ──
    skill.push_str(r#"## Decision Framework

### By Task Complexity
- **Simple** (typo, config, single-file fix): Work directly. No worktree needed.
- **Complex** (multi-file feature, refactor, new system): Delegate to agent template.

### By Workspace Mode
- **Build**: Full autonomy. Triage, delegate, merge, ship. No human sign-off needed.
- **Managed**: Features and audits need human approval before merge. Crashes and security auto-ship.
- **Maintenance**: No new features. Fix bugs and security only. Issues and audits need approval.
- **Locked**: No agent activity. Do not act.

"#);

    // ── 5. Delegation Protocol ──
    skill.push_str(r#"## Delegation

When a task needs a specialist:

1. Choose the best agent template based on the task domain
2. If the work item doesn't exist as a .md file yet, create one:
   ```
   k2so work create --title "Fix auth module" --body "Detailed spec..." --agent backend-eng --priority high --source feature
   ```
3. Delegate the work item:
   ```
   k2so delegate <agent-name> <work-item-file>
   ```
   This creates a worktree branch, moves the work to active, generates the agent's CLAUDE.md with task context, and launches the agent.
4. The agent works autonomously in its worktree
5. When done, review their work (see Review below)

"#);

    // ── 6. Review Protocol ──
    skill.push_str(r#"## Reviewing Agent Work

When an agent completes work in a worktree:

```
k2so review approve <agent-name>
```
Merges the agent's branch to main, cleans up the worktree.

```
k2so review reject <agent-name> --reason "Tests not passing"
```
Sends feedback to the agent, moves work back to inbox for retry.

```
k2so review feedback <agent-name> --message "Add error handling for edge cases"
```
Request specific changes without rejecting.

"#);

    // ── 7. Communication ──
    skill.push_str(r#"## Communication

### Check In
```
k2so checkin
```

### Report Status
```
k2so status "working on auth refactor"
```

### Complete Task
```
k2so done
k2so done --blocked "waiting for API spec"
```

### Send Message (cross-workspace)
```
k2so msg <workspace>:inbox "description of work needed"
k2so msg --wake <workspace>:inbox "urgent — wake the agent"
```

### Claim Files
```
k2so reserve src/auth/ src/middleware/jwt.ts
k2so release
```

"#);

    skill
}

/// Generate the skill protocol for custom agents.
/// Has checkin, status, done, msg (to connected workspaces), reserve/release.
/// No delegation — custom agents send work to workspace inboxes.
pub fn generate_custom_agent_skill_content(project_name: &str, agent_name: &str) -> String {
    let mut skill = format!(
r#"# K2SO Agent Skill

You are {agent_name}, a custom agent for {project_name}.

"#,
        agent_name = agent_name,
        project_name = project_name,
    );

    // User custom layers
    let custom_layers = load_custom_layers("custom-agent");
    if !custom_layers.is_empty() {
        skill.push_str(&custom_layers);
    }

    skill.push_str(r#"## Check In (do this first on every wake)

```
k2so checkin
```

Returns your current task, inbox messages, peer status, file reservations, and recent activity.

## Report Status

```
k2so status "reviewing security audit"
```

## Complete Task

```
k2so done
k2so done --blocked "waiting for API access"
```

## Send Work to a Connected Workspace

```
k2so msg <workspace-name>:inbox "description of work needed"
k2so msg --wake <workspace-name>:inbox "urgent — wake the agent"
```

Only works for workspaces connected via `k2so connections`.

## Claim Files

```
k2so reserve src/auth/ src/config.ts
k2so release
```
"#);
    skill
}

/// Generate the comprehensive K2SO Agent skill. Broader than the custom-agent
/// template: includes the full multi-heartbeat CRUD, connections messaging,
/// work creation, and audit commands — because a K2SO agent is the top-tier
/// autonomous role in its workspace and needs the full surface area.
///
/// Detected by the migration in ensure_k2so_skills_up_to_date() via the
/// first-line signature "# K2SO Agent Skill (Comprehensive)" which the
/// older shared `generate_custom_agent_skill_content` doesn't emit.
pub fn generate_k2so_agent_skill_content(project_name: &str, agent_name: &str) -> String {
    let mut skill = format!(
r#"# K2SO Agent Skill (Comprehensive)

You are **{agent_name}**, the top-level K2SO Agent for **{project_name}**. This skill lists the full CLI surface — check in, manage your own schedules, create and route work, and coordinate with other workspaces.

"#,
        agent_name = agent_name,
        project_name = project_name,
    );

    // Let user layers inject project-specific policy on top
    let custom_layers = load_custom_layers("k2so-agent");
    if !custom_layers.is_empty() {
        skill.push_str(&custom_layers);
    }

    skill.push_str(r#"## Every wake (do this first)

```
k2so checkin
```

Returns your current task, inbox messages, peer status, file reservations, and the recent activity feed for the workspace.

## Report + complete

```
k2so status "triaging inbox"
k2so done
k2so done --blocked "waiting for design review"
```

## Your own heartbeats

A K2SO agent can have multiple scheduled heartbeats — each has its own `wakeup.md` file that fires on its schedule. You can manage them from the CLI:

```
k2so heartbeat list                          # see what you have
k2so heartbeat show <name> [--json]          # full details of one
k2so heartbeat add --name daily-brief --daily --time 08:00
k2so heartbeat add --name end-of-day --daily --time 17:30
k2so heartbeat add --name weekly-review --weekly --days fri --time 16:00
k2so heartbeat edit <name> --weekly --days mon,wed --time 14:00
k2so heartbeat rename <old> <new>
k2so heartbeat enable <name>
k2so heartbeat disable <name>
k2so heartbeat remove <name>
k2so heartbeat status <name>                 # recent fire history for one
k2so heartbeat log                           # workspace-wide fire log
```

### Editing your wakeup prompts

Each heartbeat has a `wakeup.md` that is injected as the user message on fire.

```
k2so heartbeat wakeup <name>                 # print the current contents
k2so heartbeat wakeup <name> --path-only     # print just the absolute path
k2so heartbeat wakeup <name> --edit          # open it in $EDITOR
```

### Forcing a wake

Any heartbeat can be fired on demand (bypassing its schedule):

```
k2so heartbeat wake                          # triage + wake the right agent(s)
```

## Your role: planning, not implementation

You don't implement. Your job is to turn raw requests into well-scoped plans — PRDs, milestones, technical specs — that can be handed off to workspaces with engineering templates. When the right way to ship something is "hand it to another workspace", do that via cross-workspace messaging below; don't try to execute the work yourself.

### PRDs (product requirement documents)

Long-form docs that capture the *why* and *what* of a piece of work. Keep them under `.k2so/prds/` on disk, then register each one as a work item so it shows up in triage:

```
k2so work create --type prd --title "Auth V2: session rotation" --body-file .k2so/prds/auth-v2.md --priority high
```

### Milestones

Break a PRD into milestones — each is a ship-sized slice with its own acceptance criteria:

```
k2so work create --type milestone --title "M1: Rotate on login" --body "Rotate session token on every successful login. Keep the old token valid for 60s for in-flight requests." --priority high
k2so work create --type milestone --title "M2: Force rotation on password reset" --body "..." --priority normal
```

### Tasks for triage

Everyday work items for this workspace's own inbox:

```
k2so work create --title "Ship auth fix" --body "..." --priority high --source feature
k2so work inbox                              # this workspace's inbox
```

## Cross-workspace messaging

```
k2so connections list                        # who's wired up to me
k2so msg <workspace>:inbox "work needed over there"
k2so msg --wake <workspace>:inbox "urgent — wake their agent"
```

Only workspaces linked via Connected Workspaces in Settings (or `k2so connections`) are reachable.

## Claim files

Before editing shared paths, coordinate with any other active agents:

```
k2so reserve src/auth/ src/middleware/jwt.ts
k2so release
```

## Settings + diagnostic

```
k2so settings                                # current mode, state, heartbeat, connections
k2so feed                                    # recent activity feed
k2so hooks status                            # verify CLI-LLM hook wiring is live
```
"#);
    skill
}

/// Generate the universal skill protocol for agent templates (delegates).
/// Focused protocol — NO delegate, NO cross-workspace messaging.
pub fn generate_template_skill_content(project_name: &str, agent_name: &str) -> String {
    let mut skill = format!(
r#"# K2SO Agent Skill

You are {agent_name}, a specialist agent working in a dedicated worktree for {project_name}.

"#,
        agent_name = agent_name,
        project_name = project_name,
    );

    // User custom layers
    let custom_layers = load_custom_layers("agent-template");
    if !custom_layers.is_empty() {
        skill.push_str(&custom_layers);
    }

    skill.push_str(r#"## Check In (do this first)

```
k2so checkin
```

This returns your assigned task and any file reservations from other active agents.

## Report Status

```
k2so status "implementing JWT validation"
```

## Complete Task

When you have finished your assigned work:
```
k2so done
```

If you are blocked and cannot proceed:
```
k2so done --blocked "need clarification on auth flow"
```

## Claim Files (coordinate with other active agents)

Before editing shared paths, check reservations and claim what you need:
```
k2so reserve src/auth/ src/middleware/jwt.ts
k2so release
```
"#);
    skill
}

/// Generate the CLAUDE.md content for an agent, optionally focused on a specific task.
pub fn compose_agent_wake_context(
    project_path: &str,
    agent_name: &str,
    current_task: Option<&WorkItem>,
) -> Result<String, String> {
    let dir = agent_dir(project_path, agent_name);
    if !dir.exists() {
        return Err(format!("Agent '{}' does not exist", agent_name));
    }

    // Read agent identity
    let agent_md_path = dir.join("AGENT.md");
    let agent_md = fs::read_to_string(&agent_md_path).unwrap_or_default();
    let fm = parse_frontmatter(&agent_md);
    let role = fm.get("role").cloned().unwrap_or("AI Agent".to_string());
    let agent_type = fm.get("type").cloned().map(|t| {
        match t.as_str() {
            "pod-leader" | "coordinator" => "manager".to_string(),
            "pod-member" => "agent-template".to_string(),
            other => other.to_string(),
        }
    }).unwrap_or("agent-template".to_string());
    let is_custom = agent_type == "custom";

    let agent_body = strip_frontmatter(&agent_md);

    // Read shared project context (.k2so/PROJECT.md) — manager mode agents
    let is_manager_type = agent_type == "manager" || agent_type == "agent-template";
    let project_md_path = PathBuf::from(project_path).join(".k2so").join("PROJECT.md");
    let project_context = if is_manager_type && project_md_path.exists() {
        let raw = safe_read_to_string(&project_md_path).unwrap_or_default();
        let stripped = strip_frontmatter(&raw);
        // Only include if it has real content (not just comments/empty sections)
        let has_content = stripped.lines().any(|line| {
            let trimmed = line.trim();
            !trimmed.is_empty() && !trimmed.starts_with('#') && !trimmed.starts_with("<!--")
        });
        if has_content { Some(stripped) } else { None }
    } else {
        None
    };

    // Extract Standing Orders section from agent body (if user filled it in)
    let standing_orders = extract_section(&agent_body, "Standing Orders");

    let mut md = String::new();

    if is_custom {
        // ── Custom Agent: agent.md body + heartbeat control + tools ──
        md.push_str(&format!("# {}\n\n", agent_name));
        md.push_str(&format!("**Role:** {}\n\n", role));

        if !agent_body.is_empty() {
            md.push_str(&format!("{}\n\n", agent_body));
        }

        // Add heartbeat control docs if not already in agent body
        if !agent_body.contains("Heartbeat Control") {
            md.push_str(CUSTOM_AGENT_HEARTBEAT_DOCS);
        }

        return Ok(md);
    }

    // ── K2SO / Coordinator agents: full infrastructure CLAUDE.md ───────

    // List other agents for delegation awareness
    let mut other_agents = Vec::new();
    let agents_root = agents_dir(project_path);
    if agents_root.exists() {
        if let Ok(entries) = fs::read_dir(&agents_root) {
            for entry in entries.flatten() {
                if entry.file_type().map_or(false, |ft| ft.is_dir()) {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name != agent_name {
                        let their_md = entry.path().join("AGENT.md");
                        let their_role = if their_md.exists() {
                            let content = fs::read_to_string(&their_md).unwrap_or_default();
                            let fm = parse_frontmatter(&content);
                            fm.get("role").cloned().unwrap_or_default()
                        } else {
                            String::new()
                        };
                        other_agents.push((name, their_role));
                    }
                }
            }
        }
    }

    md.push_str(&format!("# K2SO Agent: {}\n\n", agent_name));
    md.push_str(&format!("## Identity\n**Role:** {}\n\n", role));
    // Reference the agent's full profile (absolute path so it resolves from worktrees)
    md.push_str(&format!(
        "**Full profile:** `{}`\n\n",
        agent_md_path.to_string_lossy()
    ));
    if !agent_body.is_empty() {
        md.push_str(&format!("{}\n\n", agent_body));
    }

    // Inject shared project context
    if let Some(ref ctx) = project_context {
        md.push_str("## Project Context (shared)\n\n");
        md.push_str(ctx);
        md.push_str("\n\n");
    }

    // Inject standing orders (persistent directives from agent.md)
    if let Some(ref orders) = standing_orders {
        md.push_str("## Standing Orders\n\n");
        md.push_str(orders);
        md.push_str("\n\n");
    }

    // Current task (if launching with specific work)
    if let Some(task) = current_task {
        // Use absolute path so it resolves from worktrees (where relative .k2so/ doesn't exist)
        let task_file_abs = agent_work_dir(project_path, agent_name, "active").join(&task.filename);
        md.push_str("## Current Task\n\n");
        md.push_str(&format!("**{}** (priority: {}, type: {})\n\n", task.title, task.priority, task.item_type));
        md.push_str(&format!("Task file: `{}`\n\n", task_file_abs.to_string_lossy()));
        md.push_str("Read the full task file for complete details, acceptance criteria, and context.\n\n");
    }

    // Work queue info (absolute paths for worktree compatibility)
    let work_dir_abs = PathBuf::from(project_path).join(".k2so").join("agents").join(agent_name).join("work");
    md.push_str("## Work Queue\n\n");
    md.push_str(&format!(
        "Your work items are at: `{}/`\n",
        work_dir_abs.to_string_lossy()
    ));
    md.push_str(&format!("- `{}/inbox/` — assigned to you, pick the highest priority\n", work_dir_abs.to_string_lossy()));
    md.push_str(&format!("- `{}/active/` — items you're currently working on\n", work_dir_abs.to_string_lossy()));
    md.push_str(&format!("- `{}/done/` — move items here when complete\n\n", work_dir_abs.to_string_lossy()));

    // Other agents — for managers, include profile paths so they can read agent.md files
    let is_manager_lead = agent_type == "manager" || agent_type == "k2so";
    if !other_agents.is_empty() {
        if is_manager_lead {
            md.push_str("## Your Team\n\n");
            md.push_str("These are your agent templates. Read their `agent.md` profiles to understand their strengths before delegating:\n\n");
            for (name, their_role) in &other_agents {
                md.push_str(&format!(
                    "- **{}** — {} (profile: `.k2so/agents/{}/agent.md`)\n",
                    name, their_role, name
                ));
            }
            md.push_str("\nYou can create new agents (`k2so agents create <name> --role \"...\"`) or update existing ones (`k2so agent update --name <name> --field role --value \"...\"`).\n\n");
        } else {
            md.push_str("## Other Agents\n");
            md.push_str("You can delegate work to these agents:\n\n");
            for (name, their_role) in &other_agents {
                md.push_str(&format!("- **{}** — {}\n", name, their_role));
            }
            md.push_str("\n");
        }
    }

    // Add workspace state constraints
    if let Some(ws_state) = get_workspace_state(project_path) {
        md.push_str("## Workspace State Constraints\n\n");
        md.push_str(&format!("This workspace operates under the **{}** state.\n\n", ws_state.name));
        if let Some(ref desc) = ws_state.description {
            md.push_str(&format!("{}\n\n", desc));
        }
        md.push_str("| Source Type | Permission |\n|---|---|\n");
        md.push_str(&format!("| Features | {} |\n", format_cap(&ws_state.cap_features)));
        md.push_str(&format!("| Issues | {} |\n", format_cap(&ws_state.cap_issues)));
        md.push_str(&format!("| Crashes | {} |\n", format_cap(&ws_state.cap_crashes)));
        md.push_str(&format!("| Security | {} |\n", format_cap(&ws_state.cap_security)));
        md.push_str(&format!("| Audits | {} |\n", format_cap(&ws_state.cap_audits)));
        md.push_str("\n**auto** = build and merge automatically. **gated** = build PR but wait for human approval. **off** = do not act.\n\n");
    }

    // Write the SKILL.md file alongside the CLAUDE.md.
    // SKILL.md is harness-agnostic — works with Claude Code, Pi, Aider, etc.
    // CLAUDE.md contains identity + task context only. SKILL.md has the CLI protocol.
    let project_name = std::path::Path::new(project_path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "workspace".to_string());

    let skill_content = if is_manager_lead {
        generate_manager_skill_content(project_path, &project_name)
    } else if agent_type == "custom" {
        generate_custom_agent_skill_content(&project_name, agent_name)
    } else {
        generate_template_skill_content(&project_name, agent_name)
    };

    // Write SKILL.md to agent directory
    let skill_path = agent_dir(project_path, agent_name).join("SKILL.md");
    log_if_err(
        "agent skill write",
        &skill_path,
        atomic_write_str(&skill_path, &skill_content),
    );

    // Inject skill content directly into the system prompt so it's always available
    // (no extra tool call needed to read SKILL.md)
    md.push_str("\n");
    md.push_str(&skill_content);

    Ok(md)
}

/// Legacy name retained to keep the src-tauri `pub use` re-export
/// short. New code should use [`compose_agent_wake_context`]; the
/// symbol still ends up writing SKILL.md + composing the wake system
/// prompt, but the new name matches what it actually does.
pub fn generate_agent_claude_md_content(
    project_path: &str,
    agent_name: &str,
    current_task: Option<&WorkItem>,
) -> Result<String, String> {
    compose_agent_wake_context(project_path, agent_name, current_task)
}

## 0.32.6 — Heartbeats that actually run the hamburger

This release closes the gap between the Agent Skills hamburger you configure in Settings and the prompt claude actually receives when a heartbeat fires. Previously heartbeats shipped a bare argv message with no skills, no resume, no session continuity, and silently dropped the prompt in claude 2.1.114's minimal-argv mode — so fires landed in the audit log but the agent sat at an empty `❯`. Every fire now flows through the unified launch-args builder, carries the full `SKILL.md` + `PROJECT.md` + per-row `wakeup.md`, and survives the Claude Code v2.1.90 stale-session dialog without human intervention.

### Fixed

- **Heartbeats now actually run.** All fire paths (forced fire, scheduled tick, `__lead__` triage, work-send wake) route through `k2so_agents_build_launch`. Every wake gets `--dangerously-skip-permissions`, `--append-system-prompt <skill-hamburger + project context>`, `--resume <session-id>`, and the per-row `wakeup.md` as the user message.
- **Session continuity across wakes.** Heartbeats pass `skip_fork_session=true` to opt out of `--fork-session`, so each agent keeps one chat thread instead of a new chat per fire. A new post-spawn watcher detects Claude Code's stale-session confirmation dialog and auto-selects option 3 ("never ask again") — so dropping the fork doesn't trap the agent at a dialog.
- **Stale session IDs no longer trap the wake.** Before `--resume`, K2SO validates the claude session file still exists on disk. If it's been pruned (workspace remove/readd, claude-side cleanup), the DB row clears and the wake falls through to the history-scan fallback instead of dying with "No conversation found".
- **Headless wake works without the tab open.** `spawn_wake_pty` now fires a deferred `SIGWINCH` (via the same `terminal_resize` path the frontend uses on mount). Claude's TUI was holding off on meaningful work until it saw that first resize event — now it gets one regardless of whether any window has the tab.

### Retired

- **Workspace Wake-up (`.k2so/wakeup.md`) is gone.** Its content is now one piece of the per-row heartbeat hamburger. On first 0.32.6 boot, every manager-mode workspace gets a `triage` heartbeat row automatically. If `.k2so/wakeup.md` had customized content, it's copied into the new row's `wakeup.md` and the original is archived as `.k2so/wakeup.md.migrated`. Settings → Projects no longer shows the "Manage Wake-up" card; the Heartbeats panel is the single surface.

### Added

- **Context Layers preview on the workspace page.** Right-column stack shows every layer that composes into the agent's system prompt on each wake — auto-generated (`Identity + Workspace State`, `Connected Workspaces`, `Team Roster`, `Standing Orders`, `Decision Framework`, `Delegation + Review`, `Communication Commands`), user-added global layers, Project Context, and each heartbeat row's `wakeup.md` as the bottom piece. Click a layer to expand its content in place. "Edit layers ↗" deep-links to Settings → Agent Skills for editing.
- **Heartbeats section relocated.** Heartbeats + Context Layers + History now live together in the right aside on the workspace page. The left column stays focused on workspace identity / mode / worktree setup.
- **Heartbeat header shows the agent mode label.** "Scheduled wakeups for Workspace Manager" instead of the internal dir name (`__lead__`, `pod-leader`, etc.). The technical agent id still drives the wire protocol; only the UI text changed.
- **K2SO Agent skill now includes planning guidance.** Removed the misplaced manager-tier `--agent <template>` delegation line; added PRD and milestone creation commands that match the K2SO Agent's actual role ("planner — builds PRDs, milestones, and technical plans").

### Tests

- 27 new tier3 source-level assertions covering the heartbeat context pipeline: migration wiring, `skip_fork_session` / `wakeup_override` parameters, SIGWINCH nudge, stale-session dialog auto-dismissal, session-file validation, manager-skill section inventory, custom-layer loading per tier, K2SO Agent skill correctness, heading consistency.

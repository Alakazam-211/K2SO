---
type: bug
priority: normal
---

# Agent-mode swap leaves orphan artifacts

## Problem

When the user switches a workspace between agent modes (Custom ↔ K2SO Agent ↔ Workspace Manager), K2SO doesn't clean up the previous mode's artifacts. The workspace ends up looking like it has multiple top-tier agents when in fact only one is active. This violates the "one top-tier agent per workspace" invariant at the filesystem/DB level, even though the invariant holds at the product level.

## Example (observed 2026-04-17)

Cortana's workspace (`/Users/z3thon/DevProjects/Cortana`) is configured as a Custom Agent workspace, but has directories and DB rows for `pod-leader` (Manager) and `k2so-agent` (K2SO Agent) from prior mode selections. From `k2so agents running`:

```
shell  wake-qa-reviewer-...  /Users/z3thon/DevProjects/Cortana/.worktrees/...
shell  wake-agent-swarm-testing-...  /Users/z3thon/DevProjects/Cortana/.worktrees/...
shell  aeacb6e3-...  /Users/z3thon/DevProjects/Cortana
```

Templates like `qa-reviewer` and `agent-swarm-testing` are fine (they're Manager-adjacent and the user may have explicitly delegated to them). The problem is that `pod-leader` and `k2so-agent` directories exist alongside `cortana/` in `.k2so/agents/` even though the workspace is in Custom mode.

This led Cortana to incorrectly conclude during the multi-schedule heartbeat design conversation that workspaces can have multiple top-tier agents simultaneously. They can't — the workspace was just in a polluted state from prior mode swaps.

## Root cause

`k2so mode <custom|agent|manager>` (or whatever the equivalent code path is for swapping modes) writes the new agent's files but never removes the previous mode's:
- `.k2so/agents/<prev-agent>/` directory (persona, wakeup.md, CLAUDE.md, work/, etc.)
- `agent_sessions` rows for the previous agent
- Injected workspace-level `CLAUDE.md` content specific to the previous mode
- Possibly skill symlinks, heartbeat config rows, etc.

## Acceptance criteria

- Switching agent modes removes the prior mode's `.k2so/agents/<name>/` directory (or moves it to a `.k2so/agents/.archive/<name>-<timestamp>/` holding area for recovery).
- Prior agent's `heartbeats/` folder goes along with the archive (it lives inside the agent dir) AND corresponding `agent_heartbeats` rows for the project/prior-agent are removed or archived. Otherwise the scheduler keeps firing heartbeats pointing at dead wakeup paths.
- `agent_sessions` rows for the removed agent are deleted (or status-flagged as archived).
- `k2so agents list` after a mode swap shows only the new mode's agent + any templates.
- `k2so heartbeat list` after a mode swap shows only the new agent's heartbeats (or none if none are configured yet).
- Workspace-level `CLAUDE.md` regenerates for the new mode instead of accumulating sections.
- User is warned before the swap: "This will remove the existing <prev-agent> configuration AND <N> scheduled heartbeat(s). Proceed? [y/N]"

## Scope

- Does NOT touch agent-templates (they're Manager-adjacent and the user may have intentionally delegated to them even after leaving Manager mode, though this is an edge case)
- Does NOT touch Claude session files in `~/.claude/projects/` (Claude owns those; we shouldn't mess with them)
- Should prune `heartbeat_fires` rows for the removed agent? Probably not — audit history is useful even for removed agents

## Related

- `.k2so/prds/multi-schedule-heartbeat.md` — references this bug as the reason for an apparent-but-not-real multi-agent workspace

# Agent Orchestration System

## Overview

K2SO's agent orchestration enables autonomous AI agents to delegate work, manage worktrees, communicate across terminals, and complete tasks with minimal human intervention. The system is built on the `k2so` CLI tool, filesystem-based work queues, and Tauri's PTY infrastructure.

## Architecture

```
User / Heartbeat Scheduler
        │
        ▼
  k2so heartbeat wake
        │
        ├─ Checks workspace inbox + agent inboxes
        ├─ Finds or launches coordinator terminal
        └─ Sends triage message
                │
                ▼
        Coordinator (persistent session)
                │
                ├─ k2so agents list → sees inbox work
                ├─ k2so delegate <agent> <file>
                │       │
                │       ├─ Creates git worktree (branch: agent/<name>/<task>)
                │       ├─ Generates CLAUDE.md with task + completion protocol
                │       ├─ Moves work from inbox → active
                │       └─ Launches background PTY in worktree
                │
                └─ Monitors for completed work (review queue)
                        │
                        ├─ k2so review approve → merge + cleanup
                        └─ k2so review reject → discard + feedback
```

## Chat Tab Terminal Lifecycle

The Chat tab for both Coordinators and Worktrees follows this flow:

1. **Attach to live terminal** — if a terminal with the deterministic ID already exists, connect to it with real-time grid updates
2. **Resume previous session** — if no terminal exists, check Claude's `history.jsonl` for the most recent session in that directory and launch with `--resume`
3. **Start fresh** — if no terminal and no previous session, launch a new Claude session

### Terminal ID Conventions

| Context | Terminal ID | CWD |
|---------|-------------|-----|
| Coordinator | `agent-chat-coordinator` | `.k2so/agents/coordinator/` |
| Worktree | `agent-chat-wt-{workspaceId}` | `.worktrees/{branch}/` |

The delegate launch and Chat tab use the **same terminal ID** so the Chat tab can attach to the delegate's live session.

## Completion Protocol

When `k2so delegate` assigns work, the CLAUDE.md includes completion instructions based on the workspace state:

### Auto Mode (Build state)
```
1. Commit changes
2. Run: k2so agent complete --agent <name> --file <filename>
   → Merges branch into main, cleans up worktree
3. Notify coordinator: k2so terminal write <coord-id> "Completed: <task>. Branch merged."
```

### Gated Mode (Managed Service state)
```
1. Commit changes
2. Run: k2so agent complete --agent <name> --file <filename>
   → Moves work to done, flags for human review
3. Notify coordinator: k2so terminal write <coord-id> "Ready for review: <task>."
```

### Off Mode
Work items with "off" capability sources are excluded from triage entirely.

## Heartbeat Wake Flow

`k2so heartbeat wake` automates the full coordinator wake cycle:

1. Check workspace inbox + agent inboxes for work
2. If coordinator terminal is running → send triage message directly
3. If coordinator is asleep → launch with `--resume` (resumes previous session) + `--dangerously-skip-permissions`
4. Wait for terminal to be ready (polls every 5s, up to 60s)
5. Send triage message: "New work detected. Run k2so agents list and delegate any inbox items."

## Virtual Terminal I/O

Agents can communicate across terminals:

```bash
k2so agents running                        # List all active sessions
k2so terminal read <id> --lines 50         # Read terminal output
k2so terminal write <id> "message"         # Send text (PTY keystrokes + Enter)
```

This enables:
- Coordinator messaging sub-agents to merge or request changes
- Sub-agents notifying the coordinator of completion
- External scripts monitoring agent progress
- Future: remote access via port forwarding or Vercel Functions

## Session Detection

Claude Code stores sessions in `~/.claude/projects/{hash}/`. The hash converts paths by replacing `/` with `-` and stripping dots from hidden directories (`.k2so` → `--k2so`, `.worktrees` → `--worktrees`).

For subpaths (worktrees, agent dirs), session detection uses **exact path matching** to avoid resuming sessions from the wrong worktree.

## CLI Commands Reference

### Workspace Management
```
k2so workspace create <path>         Create folder + register
k2so workspace open <path>           Register existing folder
k2so workspace remove <path>         Deregister (keeps files)
k2so workspace cleanup               Remove stale worktree DB records
```

### Agent Operations
```
k2so agents list                     All agents with work counts
k2so agents running                  Active CLI LLM sessions
k2so agent create <name> --role "."  Create agent template
k2so agent complete --agent <n> --file <f>  Complete work (auto/gated)
k2so delegate <agent> <file>         Create worktree + launch agent
```

### Heartbeat
```
k2so heartbeat wake                  Auto-wake coordinator
k2so heartbeat <on|off>              Toggle heartbeat
k2so heartbeat set --agent <n>       Configure adaptive timing
```

### Reviews
```
k2so reviews                         List pending reviews
k2so review approve <agent> <branch> Merge + cleanup
k2so review reject <agent>           Discard + feedback
k2so review feedback <agent> -m ".." Request changes
```

### Terminal I/O
```
k2so terminal write <id> "message"   Send to terminal
k2so terminal read <id> --lines N    Read terminal buffer
k2so terminal spawn --command "."    Spawn sub-terminal
```

## Review Cleanup

When `review approve` or `review reject` runs:
1. Git worktree is removed (recycled to Trash)
2. Git branch is deleted (merged or discarded)
3. **Workspace DB record is deleted** — worktree disappears from UI
4. Done items are archived (approve) or moved back to inbox (reject)

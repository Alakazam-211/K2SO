---
title: Heartbeat sessions + sidebar audit panel
status: draft
created: 2026-04-25
authors: [pod-leader, rosson]
---

# Heartbeat sessions + sidebar audit panel

## Problem

After A8/A9 (daemon-headless v2 migration, shipped 0.35.4–0.35.6), the daemon
can spawn and run agent sessions completely autonomously — heartbeats can
fire, agents can pick up inbox items, and work can complete with Tauri
quit. **But the user has no surface to audit any of it.** The Settings
page has the heartbeat config + a fire-decision history list, but there's
no way to *open* the chat session a heartbeat used so you can see what
the agent actually did.

This PRD addresses three coupled concerns:

1. **Auditability** — users need a way to read each heartbeat's running
   conversation, so they can verify that scheduled work executed correctly
   and improve `WAKEUP.md` based on observed agent behavior.
2. **Pinned-tab cleanup** — the current pinned agent tab is a single tab
   with 4 sub-tabs (Work / Chat / CLAUDE.md / Profile). Two of those
   sub-tabs are redundant (already editable from Workspace Settings), and
   the Work + Chat ones are better as separate top-level pinned tabs.
3. **Pane-ID collision bug** — `agent-chat-<agent_name>` (no project
   namespace) means six workspaces using the default `manager` agent all
   render the same Claude session. Whichever spawned first wins; the other
   five render empty/stale.

These three are coupled because the auditability work introduces a third
session shape (per-heartbeat) that needs the same project-namespaced
terminal-id scheme to avoid the same collision class.

## Invariants we're preserving

- **Headless-first design.** The daemon owns sessions; Tauri is one viewer.
  Heartbeats run autonomously even with Tauri quit. Surfacing a chat tab
  is opt-in, not required.
- **Settings owns configuration.** All heartbeat CRUD, schedule editing,
  WAKEUP.md AI-edit, and fire history already exist in
  `Settings → Workspace → Heartbeats`. We don't duplicate any of that
  surface in the new sidebar — the sidebar is purely live-state + jump.
- **One top-tier agent per workspace.** Heartbeats bind implicitly to
  whichever the workspace's agent is. No `--agent` flag.
- **Heartbeats are user-defined.** Triage is no longer a special-case
  surface — it's just a default heartbeat that uses the `k2so triage`
  CLI tool. Users can edit, rename, or delete it like any other.

## Architecture overview

After this PRD ships:

```
Tauri window
├── Pinned tabs (per workspace, when agent enabled)
│   ├── Inbox             ← was the "Work" sub-tab
│   └── Chat              ← was the "Chat" sub-tab
│                            (CLAUDE.md / AGENT.md sub-tabs deleted —
│                             already editable from Settings)
└── Sidebar (drawer-swappable)
    └── Heartbeats panel  ← NEW, workspace-scoped
        ├── Live          (PTY running) — green dot or braille spinner
        ├── Resumable     (has session_id, PTY stopped) — neutral
        ├── Scheduled     (configured, never fired) — grey
        └── Archived      (collapsed, retired heartbeats with preserved
                           sessions for read-back)

Daemon
├── v2_session_map  (the source of truth)
│   ├── agent-chat:<project_id>:<agent>           ← Chat tab session
│   └── agent-chat:<project_id>:<agent>:hb:<hb>   ← per-heartbeat session
└── DaemonPtySession with --resume <last_session_id>

DB (rusqlite)
├── agent_sessions(project_id, agent_name, session_id, terminal_id)
│     ← keyed by (project_id, agent_name); driver of Chat tab resume
└── agent_heartbeats(project_id, name, ..., last_session_id, archived_at)
                                            ↑ NEW          ↑ NEW
      ← driver of per-heartbeat resume + archive bucket
```

## Phase 1 — Pinned tab restructure + pane-id namespacing fix

**Goal:** Two top-level pinned tabs (Inbox + Chat). Pane IDs scoped to
project so multi-project users never collide. CLAUDE.md / AGENT.md
sub-tabs deleted.

### Pane-ID collision bug (fixed inside this phase)

**Sites that construct collision-prone IDs today:**

| File | Line | Current | Issue |
|---|---|---|---|
| `src/renderer/components/AgentPane/AgentPane.tsx` | 136 | `\`agent-chat-${agentName}\`` | No project namespace |
| `src/renderer/stores/active-agents.ts` | 647 | `\`agent-chat-${agentName}\`` | No project namespace |
| `src-tauri/src/agent_hooks.rs` | 1546 | `format!("agent-chat-{}", wake_agent)` | No project namespace |

**Sites that consume the prefix and need lockstep update:**

| File | Line | What |
|---|---|---|
| `src/renderer/components/TabBar/TabBar.tsx` | 229 | regex `/^agent-chat-(?:wt-)?/` |
| `src/renderer/components/RunningAgentsPanel/RunningAgentsPanel.tsx` | 408, 413 | same regex |
| `cli/k2so` | 1172, 1203 | `tid.startswith('agent-chat-manager')` / `'-coordinator'` |

**New scheme (uses `:` so hyphenated names parse cleanly):**

| use | format | example |
|---|---|---|
| agent chat (no worktree) | `agent-chat:<project_id>:<agent>` | `agent-chat:p_abc123:manager` |
| agent chat (worktree) | `agent-chat:wt:<workspace_id>` | `agent-chat:wt:ws_def456` *(unchanged)* |
| **per-heartbeat session** | `agent-chat:<project_id>:<agent>:hb:<hb>` | `agent-chat:p_abc123:manager:hb:triage` |

**Migration (one-time, on app upgrade):**

```sql
-- Pseudocode for the migration logic
FOR each agent_sessions row WHERE terminal_id LIKE 'agent-chat-%' AND terminal_id NOT LIKE 'agent-chat:%':
    new_id := format('agent-chat:%s:%s', project_id, agent_name)
    -- Preserve session for whichever row currently has a live PTY in v2_session_map
    -- Fall back to most-recently-touched (max(updated_at)) if no live PTY
    IF this row's terminal_id matches a live v2_session_map entry:
        UPDATE agent_sessions SET terminal_id = new_id WHERE rowid = this.rowid
    ELIF this row has the max updated_at among collisions:
        UPDATE agent_sessions SET terminal_id = new_id WHERE rowid = this.rowid
    ELSE:
        UPDATE agent_sessions SET terminal_id = new_id, session_id = NULL WHERE rowid = this.rowid
        -- session_id cleared so the orphaned rows respawn fresh next time
```

### Pinned tab restructure

**Files to change:**

- `src/renderer/components/AgentPane/AgentPane.tsx`
  - Drop the sub-tab strip (lines ~280-310)
  - Drop the CLAUDE.md and Profile sub-tab branches (lines ~347-371)
  - Component splits into two new files: `AgentInboxPane.tsx` and
    `AgentChatPane.tsx` (former contents of the Work and Chat
    sub-tabs respectively)
- `src/renderer/stores/tabs.ts`
  - `ensureSystemAgentTab` becomes `ensureSystemAgentTabs` — creates
    two pinned tabs in canonical order: Inbox first, Chat second
  - `removeSystemAgentTab` removes both
- `src/renderer/components/Settings/sections/ProjectsSection.tsx`
  - Confirm the existing "Edit AGENT.md" / "Edit CLAUDE.md" / persona
    UI is sufficient — out-of-scope to add new edit buttons; just
    verify these exist post-tab-deletion

### Icons

Both pinned tab icons authored as inline SVG (no library available):

- **Inbox** — outlined tray with a downward arrow
- **Chat** — speech-bubble outline (no emoji)

The Heartbeat icon (heart with EKG line) is added in Phase 3.

## Phase 2 — DB schema + per-heartbeat sessions + workspace settings

**Goal:** Each heartbeat has its own dedicated Claude session that
resumes on every fire. Archived heartbeats preserve session for
read-back. Workspace setting controls whether heartbeats auto-surface
as tabs when they spawn.

### DB migration (additive, no breaking changes)

```sql
ALTER TABLE agent_heartbeats ADD COLUMN last_session_id TEXT;
ALTER TABLE agent_heartbeats ADD COLUMN archived_at TEXT;  -- ISO timestamp; NULL = active

ALTER TABLE projects ADD COLUMN show_heartbeat_sessions INTEGER DEFAULT 0;
-- 0 = silent autonomous (default; matches v2-headless vision)
-- 1 = each scheduled fire surfaces as a tab in the Tauri window
```

### Per-heartbeat session save flow

After every spawn that resulted from a heartbeat fire:

- `crates/k2so-core/src/agents/wake.rs::spawn_wake_headless` already
  detects the new Claude session ID after PTY spawn settles. Today it
  calls `agent_sessions::save_session_id(project_id, agent_name, sid)`.
- Add a parallel call when the spawn was driven by a named heartbeat:
  `agent_heartbeats::save_session_id(project_id, heartbeat_name, sid)`.
- Next fire: `compose_wake_prompt_for_*` looks up
  `agent_heartbeats.last_session_id` first; falls back to
  `agent_sessions.session_id` for legacy / non-heartbeat spawns.

### Default triage heartbeat seeding

- New manager-mode workspace: daemon hook (or Tauri command on the
  mode-flip path) auto-creates a `triage` row in `agent_heartbeats`
  with `frequency=hourly`, `every_seconds=3600`, plus a stock
  `WAKEUP.md` at `.k2so/agents/<agent>/heartbeats/triage/WAKEUP.md`.
- Stock template invokes `k2so triage` (the existing CLI tool that
  checks inbox + acts only when work is present).
- One-time migration script on app upgrade: for any existing
  manager-mode workspace lacking a `triage` heartbeat row, seed one.
- Users can rename, edit schedule, edit WAKEUP.md, or remove like any
  other heartbeat.

### Lazy WS subscription mode (B8)

When `show_heartbeat_sessions = 0` (default), the renderer should NOT
open a full v2 grid WS for spawned heartbeat sessions — wasted
bandwidth. Two-step:

1. **Existence channel** — daemon already has session-state telemetry
   for the Active Bar. Sidebar reuses the same source: a lightweight
   poll or session-state WS that emits `{session_id, agent, heartbeat,
   alive: bool}` events. No grid bytes.
2. **Full subscription on click** — when user clicks a Live entry in
   the sidebar, Tauri opens a new tab and that tab's `TerminalPane`
   opens its own full-grid WS using the existing v2 protocol.
   Identical code path to today's Chat tab.

When `show_heartbeat_sessions = 1`, every heartbeat fire that produces
a new PTY *also* triggers Tauri to open a tab in the background — same
as the existing `BackgroundTerminalSpawner` path for agent panel
sessions, but pinned to a workspace's heartbeat IDs. Tab persists until
the user closes it; does NOT steal focus.

### Settings UI changes

`src/renderer/components/Settings/sections/HeartbeatsSection.tsx`:

- "Remove" button text changes to "Archive". Wire it to set
  `archived_at = now()` instead of deleting the row + folder.
  Archived rows hide from the Settings list. (A `?show_archived=1`
  query flag could be added later for power-user surfacing; not in
  this PRD.)

`src/renderer/components/Settings/sections/ProjectsSection.tsx`
or its workspace-state subsection:

- New checkbox: **"Show heartbeat sessions in tabs"** — default unchecked.
  Subtitle: "When checked, each scheduled heartbeat fire opens a new
  tab in the background. When unchecked, heartbeat sessions run
  silently in the daemon (recommended)."

## Phase 3 — Sidebar Heartbeats panel

**Goal:** Drawer-swappable workspace-scoped panel that surfaces all
heartbeat sessions and lets the user jump to any of them.

### Layout

```
┌─ Heartbeats ─────────────── ⚙ ▢ ┐   ⚙ → opens Settings → Heartbeats
│                                  │   ▢ → swap drawer
│ ⠹  triage                  10:22 │   ← Live (braille spinner)
│        hourly · last fire 2m ago │
│                                  │
│ ●  daily-brief                   │   ← Resumable (neutral dot)
│        daily 7am · last fire 9h  │
│                                  │
│ ○  weekly-financial              │   ← Scheduled (grey, never fired)
│        weekly sun 7am · not fired│
│                                  │
│ ▾ Archived (2)                   │   ← Collapsed by default
│   monthly-payroll  archived 4/20 │
│   end-of-day       archived 4/18 │
└──────────────────────────────────┘
```

### State machine

Each entry derived from `agent_heartbeats` row + live state from session
existence channel:

| state | precondition | indicator | click |
|---|---|---|---|
| **Live** | row alive + PTY in v2_session_map | braille spinner glyph (animated) | focus existing tab if surfaced; else open new tab and subscribe to existing PTY (no `--resume`, it's already running) |
| **Resumable** | row alive + has `last_session_id` + no PTY | filled dot | open new tab; spawn PTY with `--resume <last_session_id>` |
| **Scheduled** | row alive + no `last_session_id` | hollow dot | open new tab; spawn PTY fresh (no `--resume`) |
| **Archived** | `archived_at IS NOT NULL` | collapsed list | open new tab; spawn PTY with `--resume <last_session_id>` (read-back; if user keeps interacting, that's their call) |

### Files

- New: `src/renderer/components/HeartbeatsPanel/HeartbeatsPanel.tsx` —
  the panel itself
- New: `src/renderer/components/HeartbeatsPanel/HeartbeatEntry.tsx` —
  one row
- New: `src/renderer/stores/heartbeat-sessions.ts` — Zustand store that
  joins `agent_heartbeats` + live session existence + drives clicks
- New: `src/renderer/lib/icons.tsx` (or extend existing) — `IconHeartEKG`
  SVG for the panel header
- Update: `src/renderer/components/Sidebar/*` — register the panel as a
  drawer-swappable target, default right drawer
- Update: `src/renderer/stores/tabs.ts` — `openHeartbeatTab(heartbeatId)`
  helper that opens a workspace tab bound to the heartbeat's terminal ID

### Interaction details

- **Workspace switch** — panel re-renders for the new workspace's
  heartbeats. Live indicators come from the existence channel filtered
  by `project_id`.
- **Empty state (workspace has agent but no heartbeats)** — "No
  heartbeats yet. Open Settings → Heartbeats to add one."
- **Empty state (workspace agent disabled)** — panel hidden entirely.
- **Archived collapsed default** — collapsed state persisted per-user-
  workspace via localStorage key
  `heartbeats.archive-collapsed.<project_id>`.

## Phase 4 — Smoke test heartbeats end-to-end

**Goal:** Once Phases 1-3 ship, exercise the full flow with the actual
installed daemon (launchd-fired, Tauri-quit).

### Test plan

1. **Daemon-only fire (Tauri quit).** Quit Tauri. Wait for next launchd
   tick. Verify in `~/.k2so/heartbeat.log` that `/cli/scheduler-tick`
   was hit and decided `fired`. Open Tauri. Confirm the heartbeat shows
   **Live** or **Resumable** in the sidebar. Click → see the resumed
   chat history.
2. **Multi-heartbeat workspace.** Workspace with `triage` (hourly) +
   `daily-brief` (daily 7am). Verify each fires on its own schedule
   and each gets its own session_id. Confirm sidebar distinguishes
   them.
3. **`show_heartbeat_sessions = on`.** Toggle the workspace setting on.
   Wait for next fire. Confirm a tab opens in the background without
   stealing focus. Close it. Wait for next fire. Confirm a NEW tab
   opens (or, if same `last_session_id`, the same tab re-resumes —
   spec'd behavior is "new tab per fire" since user closing was
   intentional dismissal).
4. **`k2so msg --wake <agent>` routes to Chat tab, not heartbeat.**
   Run from another terminal with workspace's manager agent name.
   Confirm message appears in the Chat pinned tab session, not in
   any heartbeat session.
5. **Pane-id migration didn't break existing chats.** Open six manager
   workspaces. Confirm each Chat tab now shows its own session
   (whichever was the live one keeps its `session_id`; the others
   spawn fresh on first open).
6. **Archive flow.** In Settings, archive `daily-brief`. Confirm it
   disappears from Settings list, appears in sidebar's Archived
   collapsed section. Click → opens with `--resume`, history
   readable.

### Deliverables

- Bug fixes for anything broken on Phases 1-3 (commits as needed)
- Release notes for the next version covering the full flow
- No new code if all six tests pass

## Out of scope (explicit non-goals)

- **Manual "Fire now" button in the sidebar.** Use Settings or just
  let the schedule run.
- **Cross-workspace "Show all heartbeats" toggle** in the sidebar.
  Workspace-scoped only for v1.
- **Permanent-delete from Archived.** Archived rows live forever
  unless we add a power-user UI later.
- **`k2so heartbeat wake` CLI command.** Deprecated by this PRD.
  Heartbeats are managed via Settings + launchd cadence.
  `k2so msg --wake <agent>` continues to work for the Chat tab.
- **Workspace manager awareness of its own heartbeat roster.** A
  separate gap (the manager doesn't know which heartbeats it has);
  out of scope for this PRD but flagged for future.
- **Adding new "Edit AGENT.md" or "Edit CLAUDE.md" buttons.**
  Existing Workspace Settings already provides these surfaces; the
  pinned tabs are removed without replacement here.
- **Triage as a special-case heartbeat.** Triage is now a CLI tool
  (`k2so triage`) referenced by a default heartbeat's WAKEUP.md.
  Architectural simplification.

## Open follow-ups (not blocking)

- **Right-click sidebar entry → "Reset session"** — clears
  `last_session_id` for users who want a fresh chat on the next fire
  (e.g. after a major Claude version bump that breaks `--resume`).
- **Permanent-delete from Archived.**
- **Workspace manager roster + heartbeat awareness.**
- **`k2so heartbeat fire <name>` CLI command** for debugging.

## Definition of done

After all four phases ship:

1. The pinned-tab strip on agent-enabled workspaces shows exactly two
   tabs: Inbox and Chat.
2. Six workspaces sharing the `manager` agent name each have their
   own distinct Chat session; no collision.
3. Each heartbeat's WAKEUP.md fires on its schedule; each gets its
   own resumable Claude session.
4. The sidebar Heartbeats panel shows Live / Resumable / Scheduled /
   Archived states for the active workspace's heartbeats.
5. Click → focus or open a tab with the heartbeat's session resumed.
6. The "Show heartbeat sessions in tabs" workspace setting works:
   off = silent autonomous; on = background tab per fire.
7. Heartbeats fire correctly with Tauri quit (verified by Phase 4
   smoke test).

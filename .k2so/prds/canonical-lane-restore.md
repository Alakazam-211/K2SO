# Canonical-lane restore — pinned chat + heartbeats survive close/crash

**Status:** Drafted 2026-05-14 for 0.37.12.
**Builds on:** A9 (daemon-headless v2 unification, complete). See
`.k2so/prds/a9-daemon-headless-session-unification.md`.

## Why this exists

K2SO became daemon-first in the **data layer** (PTYs persist across
Tauri quit via `v2_session_map`), but the **lifecycle decision** — "which
sessions should exist right now" — still lives in the renderer's
`loadLayoutForWorkspace` and the heartbeat scheduler. The auto-stamp
hook reacts to whatever spawns first, which produces races and stale
state across app close, crash, and kernel panic.

User-observable symptoms:

1. **Pinned chat tab forgets its Claude session.** Send a message,
   close K2SO, reopen → the chat tab opens fresh, no history. Worse:
   `k2so msg <workspace>` now targets the wrong (or duplicated)
   session because `workspace_sessions.session_id` got overwritten.
2. **Heartbeats reconnect to the wrong session.** Watch a heartbeat
   fire, leave the surfaced tab open, close K2SO, reopen → the
   heartbeat row's `active_terminal_id` is stale, `smart_launch`
   spawns a new `claude --resume`, the original tab adopts a
   different PTY, and the user loses the fire's working context.

Both symptoms have the same architectural cause: **state that's
modelled per-lane in SQLite, but the lanes don't coordinate on
"renderer is restoring this — daemon, don't claim it yet."**

## The design — sleeping sessions + lazy revive

Three principles.

### Principle 1: Each lane has a stable, never-rotating `session_id`

Already true in the schema, not yet enforced end-to-end:

| Lane | Canonical row | `session_id` field | Invariant |
|---|---|---|---|
| Pinned chat tab (per workspace) | `workspace_sessions` keyed by `project_id` | `session_id` | Allocated at first chat-tab open. **Never overwritten** for the lifetime of the workspace. |
| Heartbeat (per workspace+name) | `agent_heartbeats` keyed by `(project_id, name)` | `last_session_id` | Allocated at first fire. **Never overwritten** by subsequent fires. |
| User-spawned Cmd+T tab | `workspace_layouts` JSON | NEW: `sessionId` on `SerializedItem` for `agent` items | Persisted in the layout blob, restored on workspace open. |

`active_terminal_id` (daemon-side PTY UUID) remains a best-effort
*cache*. The Claude UUID is the actual identity; when `active_terminal_id`
disagrees with what's live, the daemon reconciles by walking
`v2_session_map` and matching `--session-id <X>` in the session's args
(same mechanism `closeTerminalForRenderer` already uses for heartbeat
cross-reference at `tabs.ts:87-95`).

### Principle 2: Sleeping sessions are first-class citizens

A "sleeping session" is an entry in a daemon-side map representing
**intent without consumption**:

```rust
pub struct SleepingSession {
    pub agent_name: String,        // canonical key (e.g. project_id)
    pub session_id: SessionId,     // pre-assigned daemon SessionId
    pub claude_session_id: String, // Claude UUID for --session-id <X>
    pub command: Option<String>,   // 'claude', 'codex', etc.
    pub args: Vec<String>,         // including --session-id <X> and --resume flags
    pub cwd: PathBuf,
    pub label: String,
    pub source_lane: SourceLane,   // ChatTab | Heartbeat { name } | UserTab
}
```

Stored in a new `crates/k2so-daemon/src/sleeping_session_map.rs`
module that mirrors `v2_session_map`'s shape.

**Population**:
- Daemon startup: one-time pass reads `workspace_sessions` rows with
  `session_id IS NOT NULL` + `agent_heartbeats` rows with
  `last_session_id IS NOT NULL`, and populates the sleeping map.
- Renderer close (graceful): on the way out, the renderer
  POSTs `/cli/sessions/v2/sleep?id=<sessionId>` per live session.
  Daemon unregisters from `v2_session_map`, drops the PTY child via
  SIGHUP, and adds the entry to `sleeping_session_map`.
- Daemon graceful shutdown: every entry in `v2_session_map` moves
  to `sleeping_session_map` before tear-down.

**Lookup unification**: `session_lookup::lookup_any` (already exists)
now also checks `sleeping_session_map` and returns a `LiveSession::Sleeping(...)`
variant. Consumers that need an *actually-running* PTY (`/cli/terminal/write`,
the awareness bus inject path) get a clear signal vs. an "I see the
intent but the PTY isn't running" signal.

### Principle 3: Lazy revive on attach

When the renderer mounts a `TerminalPane` and POSTs
`/cli/sessions/v2/spawn` with `agent_name = <canonical_key>`:

1. **Live in `v2_session_map`?** → return `reused: true` with existing
   session_id. (Current behavior — unchanged.)
2. **Sleeping in `sleeping_session_map`?** → **revive**: spawn the
   recorded `command + args` (which include `--session-id <X>` for
   Claude), register in `v2_session_map`, remove from sleeping map.
   Return `reused: true` with the **same** session_id (renderer
   doesn't need to know it was asleep).
3. **Neither?** → fresh spawn (current behavior).

For `k2so msg <workspace>` / `deliver_live`:
- Branch 1 unchanged.
- Branch 2 (saved session, dead `active_terminal_id`) **becomes**:
  consult `sleeping_session_map` first. If found, revive it via the
  same path. If not, fall through to spawn `claude --resume <session_id>`
  (existing behavior).

For heartbeat `smart_launch`:
- Same cascade. The "active" branch consults sleeping map first.

## What ships in 0.37.12

The minimum change to fix both user-reported symptoms, without yet
building the full sleeping-session infrastructure. We can ship Phase 1
+ Phase 2 in 0.37.12 and defer Phase 3 (sleeping-session-map) to 0.38.

### Phase 1 — Renderer-side: serialize the pinned chat tab's session_id

**Goal**: on close/reopen, the renderer's `AgentChatPane` immediately
asks for `claude --session-id <X>` where `X` is the same UUID that was
running before, without depending on a daemon DB roundtrip that can
race.

Changes:

- **`src/renderer/stores/tabs.ts:298-321`** — add `sessionId?: string`
  to `SerializedItem` for `type='agent'` items (currently only
  `terminal` items carry session_id).
- **`src/renderer/stores/tabs.ts:562-570`** (`serializeTab` agent
  branch) — capture the current Claude session id from the agent
  pane's launch config and serialize it.
- **`src/renderer/stores/tabs.ts:2261-2266`** (`restoreLayout` agent
  branch) — restore `sessionId` into the agent item's data.
- **`src/renderer/components/AgentPane/AgentChatPane.tsx`** — read
  the restored `sessionId` as a hint. If present, skip the
  `k2so_agents_resume_chat_args` lookup roundtrip and directly use
  `claude --session-id <X>` (the renderer is now self-sufficient).
  Daemon's auto-stamp hook still updates `workspace_sessions.active_terminal_id`
  reactively.

Net effect: even if `workspace_sessions.session_id` somehow gets
overwritten or `active_terminal_id` is stale, the **renderer holds the
canonical session id in its serialized layout** and drives the
restoration deterministically.

### Phase 2 — Heartbeat tab persistence + idempotency

**Goal**: a heartbeat-surfaced tab survives close/reopen and resumes
the same Claude session.

Changes:

- **`src/renderer/stores/tabs.ts`** — when a heartbeat tab is in the
  active layout, its `surfacedAgentName` + `heartbeatName` are
  already stamped on `TerminalItemData`. Make sure these survive
  `serializeTab`/`restoreLayout` (currently they don't — verify and fix).
- **`crates/k2so-daemon/src/heartbeat_launch.rs::smart_launch`** —
  Branch 2 (saved session, dead `active_terminal_id`) gets an
  **idempotency guard**: before spawning a fresh `claude --resume <X>`,
  walk `v2_session_map.snapshot()` and check if any live session has
  `--session-id X` or `--resume X` in its args. If yes, that's our
  PTY (the renderer just restored it); stamp `active_terminal_id` to
  match and inject without spawning. Reuses the same arg-matching
  logic the renderer's `closeTerminalForRenderer` uses at
  `tabs.ts:87-95`.

Net effect: even if the heartbeat fires after the renderer has
already restored the tab, the daemon adopts the existing PTY instead
of spawning a duplicate.

### Phase 3 — Daemon sleeping-session-map (deferred to 0.38)

The full design above. Adds `sleeping_session_map.rs`, the startup
restore pass, the `/cli/sessions/v2/sleep` endpoint, and the lazy
revive path in `/cli/sessions/v2/spawn`. Bigger architectural shift;
not needed for the immediate user pain.

When this lands, K2SO becomes fully headless-capable: daemon-only
deployments survive forever, mobile companion plugs in as a fresh
viewer, the v2 WS multi-subscribe protocol makes mobile + desktop +
tablet coexistence trivial.

## E2E test plan

Two scenarios from the user, executed against a development build:

### Test 1 — Pinned chat tab survives close

1. Launch K2SO on a workspace that has a pinned Chat tab.
2. Send a message via the Chat tab (e.g. "remember the number 42").
3. Wait for Claude's response.
4. Note `workspace_sessions.session_id` for the workspace via
   `sqlite3 ~/.k2so/k2so.db "SELECT session_id, active_terminal_id FROM workspace_sessions WHERE project_id='<id>';"`.
5. Quit K2SO.
6. Relaunch K2SO.
7. **Assert**: the Chat tab opens showing the conversation including
   "remember the number 42" and Claude's response. New message ("what
   number do you remember?") gets a Claude reply referencing 42.
8. **Assert**: `session_id` in DB is unchanged. `active_terminal_id`
   is non-null and matches the live PTY.

### Test 2 — Heartbeat tab survives close

1. Launch K2SO on a workspace with an active heartbeat configured.
2. Wait for (or force-trigger) a heartbeat fire. Open the surfaced
   tab. Wait for the fire's action to complete (Claude finishes
   processing the WAKEUP.md).
3. Leave the tab open. Note `agent_heartbeats.last_session_id` and
   `active_terminal_id` for the heartbeat.
4. Quit K2SO.
5. Relaunch K2SO.
6. **Assert**: the heartbeat tab opens, shows the same conversation
   the fire ran in. Sending a message gets a Claude reply that
   references the WAKEUP.md context (proves session was resumed,
   not re-spawned fresh).
7. **Assert**: `agent_heartbeats.last_session_id` unchanged.
   `active_terminal_id` is non-null and matches the live PTY.

### Regression: Cmd+T tab session survives

Quick check that existing user-spawned tab behavior didn't regress:
open a Cmd+T tab running `claude`, send a message, quit, reopen,
confirm session resumes. (Already works in 0.37.11 via the
`SerializedItem.sessionId` field — this is the precedent we're
extending to agent items.)

## Out of scope (explicit)

- Mobile companion integration. Phase 3 work; not in 0.37.12.
- Kernel panic recovery beyond what graceful quit already provides.
  Periodic auto-save for background workspaces is a separate
  follow-up.
- "Sleep this session" UI / explicit sleep verbs. Deferred to 0.38
  with Phase 3.
- Cross-window workspace-switch propagation. 0.38 / focus-window
  follow-up.

## Files touched (0.37.12 only — Phases 1 + 2)

| Layer | File | Change |
|---|---|---|
| renderer | `src/renderer/stores/tabs.ts` | `SerializedItem.sessionId` for agent items, serialize/restore the field |
| renderer | `src/renderer/components/AgentPane/AgentChatPane.tsx` | use restored sessionId hint to skip resume-args roundtrip |
| daemon | `crates/k2so-daemon/src/heartbeat_launch.rs` | idempotency check in Branch 2 — walk v2_session_map for matching `--session-id` arg before re-spawning |
| daemon | `crates/k2so-daemon/src/workspace_msg.rs` | same idempotency check in `deliver_live` Branch 2 |
| docs | `.k2so/prds/canonical-lane-restore.md` | this file |

Net: ~80 lines renderer + ~40 lines daemon + tests.

## Definition of done

Test 1 and Test 2 both pass. Cmd+T regression check passes. No
regression to `k2so msg <workspace>` from before-close target session.

# PRD: Heartbeat Active-Session Tracking + Surfaced Sessions

## Problem

Clicking "Launch" on a heartbeat fires the wakeup into one PTY (daemon-internal),
but clicking the heartbeat row to open it spawns a *second* PTY also running
`claude --resume <id>`. The two PTYs share Claude's session-history JSONL but
otherwise run independently — the wakeup the user just fired is invisible in
the tab they just opened, because the tab is a fresh resume of the chat
history, not a connection to the live PTY.

Root cause is that the daemon and the renderer find live sessions through
two different mechanisms that can disagree:

- **Daemon's `smart_launch`** scans `session_lookup::snapshot_all()` and
  matches by string-comparing `--session-id <id>` / `--resume <id>` in the
  PTY's args. It correctly finds the daemon-spawned PTY and injects there.
- **Renderer's `openHeartbeatTab`** scans only `state.tabs` (renderer-local),
  which never sees daemon-spawned PTYs. It falls through to spawning a fresh
  resume PTY of its own.

Args-matching is also brittle by construction — every change to the spawn
flags risks silently breaking the matcher (this is likely why "it was
working a bit ago"). This is why heartbeat audit shows
`reason = smart_launch: injected into live session 285d3896-...` but the user
is staring at a tab whose Claude session id is the same `13ab0f50-...` that's
on the heartbeat row.

## Goal

Replace the args-matching heuristic with explicit data: each heartbeat row
stores a foreign-key-style pointer to its live PTY's terminal id. The
renderer reads that column to decide whether to attach an existing PTY into
a tab or spawn a fresh one. No string-matching against args anywhere.

Pair this with a per-session "surfaced" flag that replaces the workspace-wide
`show_heartbeat_sessions` gate as the source of truth for whether a session
appears as a tab. Surfacing becomes a runtime toggle (user clicks to summon a
heartbeat session into a tab; user closes the tab to hide it) instead of a
workspace-wide preference baked in at spawn time.

Together these two changes:

1. Eliminate the daemon-vs-renderer PTY visibility gap that causes today's
   "fired-but-not-here" bug.
2. Make heartbeat sessions first-class citizens — they always run in the
   background, the user *summons* them when they want to watch.
3. Eliminate the "two PTYs, one session" zombie that today's flow can
   produce.

## Out of scope

- Cross-workspace heartbeat sessions (heartbeats are workspace-scoped today,
  staying that way).
- Per-heartbeat session multiplicity (one active PTY per heartbeat — by
  design enforced by the new column).
- Migration of legacy `session_map` (Kessel) sessions to use this same
  pattern. Heartbeats are v2-only. If a workspace's renderer setting is
  legacy, heartbeat tabs override and use v2 anyway (see "Force v2"
  section).

## Design

### New DB columns

```sql
-- agent_heartbeats: foreign-key-style pointer to live PTY's terminal id
ALTER TABLE agent_heartbeats ADD COLUMN active_terminal_id TEXT;

-- agent_sessions: surfaced flag — whether this session appears as a tab
ALTER TABLE agent_sessions ADD COLUMN surfaced INTEGER NOT NULL DEFAULT 0;
```

`active_terminal_id` is NULL when no live PTY is associated with the
heartbeat (cold heartbeat, post-exit, post-daemon-restart).

`surfaced` defaults to 0 (hidden); user-initiated sessions (chat tab,
persona editor, AIFileEditor terminals) flip it to 1 immediately on spawn.
Heartbeat-spawned sessions stay 0 until the user summons them.

### Stamping `active_terminal_id`

Where: inside `crates/k2so-daemon/src/heartbeat_launch.rs`, in the same
critical section that already calls `AgentHeartbeat::save_session_id` for
the Claude session UUID. The daemon's terminal manager hands back a
`terminal_id` at spawn time — we already know it, just need to write it to
the new column.

```rust
let _ = AgentHeartbeat::save_session_id(&conn, &project_id, hb_name, &pinned_session_id);
let _ = AgentHeartbeat::save_active_terminal_id(&conn, &project_id, hb_name, &terminal_id);
```

The two writes happen under the same DB lock so they can't diverge.

### Clearing `active_terminal_id`

Three paths:

1. **Child-exit observer (NEW)** — `DaemonPtySession::spawn` already runs an
   IO read-loop. When that loop sees EOF (child exited), unregister from
   `v2_session_map` and emit a `HookEvent::PtyExited` event. Daemon-side
   listener wires the event to:
   - `AgentHeartbeat::clear_active_terminal_id` for any row whose
     `active_terminal_id` matches the exited terminal_id.
   - `AgentSession::set_status('sleeping')` for the agent_sessions row.

2. **Lazy cleanup on read** — when `openHeartbeatTab` reads a non-NULL
   `active_terminal_id`, it asks the daemon `terminal_exists?`. If false,
   null the column inline and fall through to fresh spawn. Defends
   against missed exit events (process crashes, stale rows after daemon
   restart, etc.).

3. **Boot-time sweep** — daemon startup walks `agent_heartbeats` for
   non-NULL `active_terminal_id` columns and nulls any whose terminal id
   isn't in `v2_session_map` after rehydration. Keeps the column consistent
   with the in-memory map every restart.

### Surfacing a session into a tab

User-driven. `openHeartbeatTab` rewrites to:

```ts
// 1. Read heartbeat row + active_terminal_id
const row = await invoke('k2so_heartbeat_get', { projectPath, name })

// 2. If active_terminal_id is non-null and the terminal is alive,
//    flip the session's `surfaced` flag → daemon emits SessionSurfaced
//    → renderer creates a tab attached to that terminal_id (existing
//    reattach path in TerminalPane already supports this).
if (row.activeTerminalId && await invoke('terminal_exists', { id: row.activeTerminalId })) {
  await invoke('k2so_session_set_surfaced', {
    projectPath,
    agentName: row.agentName,
    surfaced: true,
  })
  return  // tab will be created by the SessionSurfaced event listener
}

// 3. No live PTY: clear the column (lazy cleanup) and spawn fresh.
//    Falls through to today's claude --resume code path.
if (row.activeTerminalId) {
  await invoke('k2so_heartbeat_clear_active_terminal_id', { projectPath, name })
}
return spawnFreshHeartbeatTab(projectPath, row)
```

### `surfaced` flag semantics

- `agent_sessions.surfaced = 1` means "this session has a tab somewhere in
  the renderer." Only one tab per session — surfacing again is a no-op.
- `agent_sessions.surfaced = 0` means "this session is running headless;
  user can summon it via heartbeat row click."
- Heartbeat-spawned sessions: default `0`.
- User-initiated sessions (chat tab, persona editor): default `1` —
  rendered immediately.
- Closing a heartbeat tab flips `surfaced` to `0` but does NOT kill the
  PTY. Closing a non-heartbeat tab still kills the PTY (existing behavior).

The workspace-wide `show_heartbeat_sessions` setting becomes vestigial-ish:
its only remaining role is "default for newly-fired heartbeats." In a
future cleanup pass we can probably retire it entirely; for now leave it
in place since other surfaces read it.

### New events

`HookEvent::SessionSurfaced` — emitted by daemon when `agent_sessions.surfaced`
flips 0 → 1. Payload:

```json
{
  "agentName": "cortana",
  "projectPath": "/Users/...",
  "terminalId": "wake-cortana-...",
  "heartbeatName": "daily-email-brief",
  "command": "claude",
  "args": ["--dangerously-skip-permissions", "--resume", "13ab0f50-..."]
}
```

Renderer's existing event listener for `CliTerminalSpawnBackground` already
creates tabs on event. Add a parallel branch for `SessionSurfaced`. The two
events differ in semantic — Spawn means "I just spawned a new PTY, here's
its info"; Surfaced means "this PTY is already running, mount a tab on
it." The tab-creation code paths are otherwise identical (both end at
`addTabToGroup` with the right command/args).

### Force v2 for heartbeat tabs

Inside `openHeartbeatTab`, override the workspace's `renderer_choice`
setting to `'alacritty-v2'` for the spawned tab regardless of the user's
preference. Heartbeat-spawned PTYs already only live in `v2_session_map`;
the override just makes sure a follow-up tab uses the same shape.

### "Minimize" button on heartbeat tabs

Tab close button on heartbeat tabs renders as `–` instead of `×`. Click
fires `k2so_session_set_surfaced(..., false)` — flips `surfaced` to 0 in
the DB AND removes the tab from the renderer's tab list. PTY keeps
running. User can re-summon via the heartbeat row.

For non-heartbeat tabs (chat tab, regular terminals): unchanged — `×`
still kills the PTY.

The visual swap is keyed on `tab.kind === 'heartbeat'`.

## Implementation phases

### Phase 1 — DB migrations + core helpers

- Migration: add `active_terminal_id` to `agent_heartbeats`, `surfaced` to
  `agent_sessions`. Defaults handled in the `ALTER TABLE` so existing rows
  upgrade cleanly.
- Helper functions: `AgentHeartbeat::save_active_terminal_id`,
  `AgentHeartbeat::clear_active_terminal_id`,
  `AgentSession::set_surfaced`, `AgentSession::is_surfaced`.
- Boot-time sweep in daemon main.

### Phase 2 — Stamp + clear

- Stamp `active_terminal_id` in `smart_launch` alongside `last_session_id`.
- Add child-exit observer to `DaemonPtySession`'s read loop. On EOF, emit
  `HookEvent::PtyExited` and unregister from `v2_session_map`.
- Daemon-side listener for `PtyExited` calls
  `clear_active_terminal_id` and flips `surfaced=0` on matching rows.

### Phase 3 — Surfaced flag + new event

- Tauri command `k2so_session_set_surfaced(projectPath, agentName, surfaced)`.
  Flips DB column. If transitioning 0 → 1, emits `HookEvent::SessionSurfaced`.
- Daemon HTTP route `/cli/sessions/set-surfaced`.
- Renderer event listener for `SessionSurfaced` — same shape as the
  existing `CliTerminalSpawnBackground` listener but ATTACHES to existing
  terminal_id instead of triggering a fresh spawn.
- Default `surfaced=1` for non-heartbeat spawns (chat tab, persona editor,
  etc.). Default `surfaced=0` for heartbeat spawns.

### Phase 4 — `openHeartbeatTab` rewrite

- Read `active_terminal_id` via new Tauri command
  `k2so_heartbeat_get(projectPath, name)`.
- If non-null + `terminal_exists`: flip surfaced flag, return early.
- Else: clear column (lazy cleanup), fall through to existing fresh-spawn
  path with `claude --resume <last_session_id>`.

### Phase 5 — Force v2 + minimize button

- `openHeartbeatTab` always uses v2 path.
- Tab system: `tab.kind === 'heartbeat'` → close button is `–`, on click
  flips `surfaced=false` instead of killing PTY.

### Phase 6 — Tests + manual verification

- Unit test: stamp/clear round-trip on `active_terminal_id`.
- Unit test: `surfaced` flag round-trip.
- Integration test: child-exit clears columns.
- Manual: fire heartbeat → click row → confirm tab attaches to existing
  PTY (verify by checking daemon log says ATTACH not SPAWN).
- Manual: close heartbeat tab → confirm PTY still running (`k2so heartbeat
  status`).

## Critical files

| Phase | File | Change |
|---|---|---|
| 1 | `crates/k2so-core/src/db/schema.rs` | new columns + helpers |
| 1 | `crates/k2so-core/src/db/migrations/*.rs` | new ALTER TABLE migration |
| 1 | `crates/k2so-daemon/src/main.rs` | boot-time sweep |
| 2 | `crates/k2so-daemon/src/heartbeat_launch.rs` | stamp on spawn |
| 2 | `crates/k2so-core/src/terminal/daemon_pty.rs` | child-exit observer |
| 2 | `crates/k2so-daemon/src/agent_hooks.rs` (or similar) | PtyExited listener |
| 3 | `src-tauri/src/commands/k2so_agents.rs` | set_surfaced command |
| 3 | `crates/k2so-daemon/src/cli.rs` | /cli/sessions/set-surfaced route |
| 3 | `src-tauri/src/lib.rs` | register new commands |
| 3 | `src/renderer/App.tsx` (or event registrar) | SessionSurfaced listener |
| 4 | `src/renderer/stores/tabs.ts` | openHeartbeatTab rewrite |
| 5 | `src/renderer/components/TabBar/*.tsx` | minimize button |

## Rollback

Each phase is an additive commit. To revert:

- Phase 1 alone: harmless (new columns unused).
- Phase 2 alone: heartbeats stop tracking active session, but no behavior
  change because no reader yet.
- Phase 3 alone: new event/flag exist but no UI hooks them.
- Phase 4 onwards: revert restores the args-matching path. Old behavior
  comes back.

## Definition of done

1. Click row on a fired heartbeat → tab opens attached to the existing
   daemon PTY (not a fresh `claude --resume`). Verified via daemon log.
2. Click Launch → wakeup arrives in the same PTY the user is watching
   (when a tab is open) or stamps the column for the next click.
3. Heartbeat tab close button is `–`, clicking it leaves PTY alive.
4. PTY exit (claude --print finishing, daemon kill, etc.) clears the
   column within seconds. No zombies in `v2_session_map`.
5. After daemon restart, `active_terminal_id` columns are reconciled
   with the actual map within the boot sweep.

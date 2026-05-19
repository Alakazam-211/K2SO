# 0.38.5 — Cmd+T tabs survive daemon restart (app updates, kickstart, crash)

Closes the gap left by the 0.38.0 daemon-authoritative-tabs work:
**Cmd+T terminal tabs no longer become generic shells after an app update.**

## The bug we're closing

Pinned chat tabs and heartbeat tabs already survived daemon restarts
because their session ids lived in dedicated SQL columns
(`workspace_sessions.session_id`,
`workspace_heartbeats.last_session_id`). Cmd+T terminal tabs didn't —
schema v2 (0.38.0) stripped `command`/`args`/`sessionId` from terminal
items in `workspace_layouts.layout_json` because the daemon's in-memory
`v2_session_map` was supposed to be the single source of truth.

That assumption held for **Tauri close + reopen** (daemon process
untouched) but broke on **daemon restart**: `v2_session_map` evaporates
with the process, the renderer's spawn request lands at the new daemon
with no command, daemon defaults to shell, the user's `claude` tab
becomes a generic shell.

## What changed

**New `workspace_tab_sessions` SQL table, daemon-managed:**

```sql
project_id, pane_group_id, agent_name, session_id, command, args_json, cwd, last_seen_at
PK (project_id, pane_group_id)
```

- **`v2_session_map::register`** upserts the row on every PTY
  registration. The persisted args reflect what the renderer
  originally spawned with — `claude --dangerously-skip-permissions`,
  `bash`, etc.
- **`v2_session_map::unregister`** leaves the row alone. Absence from
  `v2_session_map` already signals "not running"; the persisted row
  is for next-restart recovery.
- **`v2_spawn::handle_v2_spawn`** consults the table when the
  renderer's spawn request arrives with no command (the schema-v2
  default for terminal items). If a row exists, the saved
  command + args drive the spawn — `claude` instead of `bash`. If the
  row also has a `session_id` stamped (claude reported one on a prior
  run), `--resume <id>` gets spliced in automatically.

**Stays 100% daemon-side.** Tauri (and any future headless deployment
of the daemon) gets this behavior identically — the renderer is even
thinner now: send the canonical key + (optional) initial command; the
daemon owns continuity.

## Cleanup that fell out of this

Migration 0045 also **drops `terminal_tabs` and `terminal_panes`** —
two tables created in migration 0000 (2024) for a renderer-normalized
layout design that never shipped. Both sat at **0 rows for the entire
lifetime of K2SO**. Removing them was easier than dismantling their
FK chain (`terminal_panes → terminal_tabs → workspaces`, NOT NULL,
ON DELETE CASCADE) to repurpose. Net: -2 unused tables, +1 used
table.

## Smoke-tested end-to-end

1. Spawn `claude` as a v2 tab → row persisted with `command=claude`.
2. Restart daemon via `launchctl kickstart -k` (simulates app update).
3. v2_session_map empty, but the SQL row survives.
4. Replay the renderer's restart-time spawn with **empty command** —
   what the schema-v2 layout will send.
5. Daemon log: `[v2-spawn] restart-recovery: agent=tab-… replayed
   command=claude args=[…]`
6. New session running `claude`, not `shell`. ✅

`cargo test --workspace`: 802 pass / 0 fail (no regression).

## Known follow-up

The persisted `session_id` is only filled in if the original spawn
included `--resume <id>` in its args (which happens for restored
pinned-chat tabs but not for fresh Cmd+T tabs). To get **conversation
continuity** (not just "claude vs shell"), we'd add a renderer-side
session-id-detection step that calls a new
`k2so_tab_session_stamp_session_id` daemon route after claude emits
its session id on first turn. Scoped for a follow-up release.

## Files touched

| File | Role |
|---|---|
| `crates/k2so-core/drizzle_sql/0045_workspace_tab_sessions.sql` | NEW: schema + drops legacy `terminal_tabs`/`terminal_panes` |
| `crates/k2so-core/src/db/mod.rs` | Register migration 0045 |
| `crates/k2so-core/src/db/schema.rs` | NEW `WorkspaceTabSession` struct + `upsert` / `get` / `get_by_agent_name` / `stamp_session_id`; deleted stale `TerminalTab`/`TerminalPane` stubs |
| `crates/k2so-daemon/src/v2_session_map.rs` | Upsert on register |
| `crates/k2so-daemon/src/v2_spawn.rs` | Restart-recovery: lookup row before spawn when command is empty |

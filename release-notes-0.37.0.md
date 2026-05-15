# K2SO 0.37.0 — Workspace–Agent Unification

The biggest architectural change since 0.34.0. Every workspace now
hosts **one** primary agent — no more per-workspace agent rosters,
no more `--agent` flag on every CLI verb. The filesystem layout, DB
schema, and CLI surface all simplified to match.

## What changes for you

- **Filesystem.** Your workspace's primary agent moves from
  `.k2so/agents/<name>/` to `.k2so/agent/`. Role personas you used
  for delegation move to `.k2so/agent-templates/<role>/`.
  Heartbeats moved to workspace-level `.k2so/heartbeats/<sched>/`.
  Pre-0.37.0 directories are archived in
  `.k2so/.archive/0.37.0-unification/` — safe to delete after a
  sanity check.
- **CLI.** Send a message to a workspace in one verb:
  ```bash
  k2so msg TestingK2SO "look at issue #42"           # → workspace inbox
  k2so msg TestingK2SO "ship it" --wake              # → live PTY (smart-launch cascade)
  ```
  `<workspace>` accepts a name, an absolute path, or a UUID. The
  legacy `k2so msg <agent>` form still works with a deprecation
  warning to stderr.
- **Settings.** Alacritty (Legacy) is removed from the Terminal
  Renderer dropdown. Anyone whose persisted setting was on it
  auto-migrates to Alacritty (v2) on first launch.
- **Migration.** Runs on first launch, atomic per workspace,
  idempotent, reversible (originals are archived, not deleted).

## The headline change: same agent across the workspace

Pre-0.37.0 a workspace could host multiple distinct agents
(`coordinator`, `pod-leader`, `rust-eng`, etc.) and every
heartbeat / message / task carried an `--agent` flag.

In practice, every workspace had **one** primary that actually did
the work. The other "agents" were either delegate-target templates
(used to scaffold worktrees) or vestigial state from earlier
experiments. The two-axis addressing — *which workspace? which
agent in that workspace?* — was a tax with no payoff.

0.37.0 collapses the axes:

- **One agent per workspace.** The primary's persona lives at
  `.k2so/agent/AGENT.md`.
- **Templates for delegation.** Role personas you scaffold worktrees
  from live at `.k2so/agent-templates/<role>/`, separate from the
  workspace's running agent.
- **CLI is workspace-keyed.** `k2so msg <workspace>`,
  `k2so heartbeat`, `k2so checkin`, `k2so done` — none of them
  need an agent flag anymore.

## Heartbeat / pinned-tab parity

Both the pinned chat tab and each heartbeat now follow the same
flow, keyed on `active_terminal_id` in their respective SQL rows
(`workspace_sessions` for the pinned tab, `workspace_heartbeats`
for each heartbeat):

```
Terminal ID present + alive?
  Yes → connect to it
  No  → Resume Session present + JSONL on disk?
          Yes → resume (claude --resume) + connect
          No  → create a fresh session, save BOTH ids
On terminal session end → null the terminal ID, leave the session
```

The fresh-fire path now spawns interactive (no more `--print`),
so you can open the heartbeat tab from the sidebar and watch the
session live — and subsequent fires inject into the same PTY
naturally. The smart-launch cascade self-heals when a saved
session id points at a JSONL that no longer exists on disk
(daemon-restart-during-spawn race) — clears the stamp and falls
through to fresh-fire instead of looping on `claude --resume <ghost>`.

## Daemon-driven, app-quit-survivable

The 0.37.0 work continues the daemon-first push from 0.34–0.36:
heartbeat fresh-fires now spawn through `spawn_agent_session_v2_blocking`
under the canonical `<project_id>:<agent>` key, so a heartbeat-spawned
PTY survives Tauri quit and `k2so msg --wake` reaches it on the
next reopen without a duplicate spawn. The legacy `terminal::shared()`
in-process path is no longer the default for any system-driven
spawn; it remains compiled for the long-tail of legacy tabs but is
slated for full removal in a future release.

## Schema migrations

Five SQL migrations land:

- **0038** — `workspace_sessions` (the tab-layout table) renamed
  to `workspace_layouts` to free the name for the renamed
  `agent_sessions`.
- **0039** — `agent_sessions` → `workspace_sessions` (one row per
  workspace, keyed on `project_id UNIQUE`, no `agent_name` column).
  Multi-agent rows collapse to one with original copies preserved
  in `workspace_sessions_legacy_archive`.
- **0040** — `agent_heartbeats` → `workspace_heartbeats`,
  `agent_name` column dropped (heartbeats are workspace-scoped).
- **0041** — `activity_feed` columns `agent_name` / `from_agent` /
  `to_agent` renamed to `actor` / `from_workspace` / `to_workspace`.

All migrations are idempotent + atomic per workspace.

## Notable bug fixes

- `find_primary_agent` now resolves correctly for migrated
  workspaces (pre-fix: returned None for every workspace whose
  primary moved to `.k2so/agent/`, breaking heartbeats silently).
- Active bar dismiss is immediate even on the active workspace.
- Active bar memory honors a 24-hour TTL — auto-expires
  long-dismissed entries.
- Heartbeat fresh-fires correctly pin `--session-id` synchronously
  so deferred-save races can't leave `last_session_id` unset.
- Pinned chat tab no longer gets coupled to a heartbeat's session
  by chat-history polling (renderer detection loop removed; daemon
  stamps session_id authoritatively at spawn time).
- AIFileEditor for heartbeats opens the canonical post-0.37.0 path
  (`.k2so/heartbeats/<sched>/WAKEUP.md`), not the legacy nested
  path. The path-rewrite migration catches both pre-0.37.0
  (`.k2so/agents/<X>/heartbeats/`) and 0.36.x→0.37.0 half-state
  (`.k2so/agent/heartbeats/`) variants.
- Heartbeat schedule active-hours window is correctly enforced
  (and the UI accurately shows the active window).

## Test coverage

- 302 Rust unit tests in `k2so-core`
- 76 Rust unit tests in the Tauri lib
- ~80 Rust integration tests across `k2so-daemon` (incl. new
  `workspace_msg_integration.rs` and `heartbeat_fire_v2_integration.rs`)
- 133 Vitest tests in the renderer
- 514 shell behavioral assertions across tier 1 / tier 3 / CLI
  integration

A new `tests/README.md` maps every test surface in the codebase
so future agents (and humans) know where to add new coverage.

## Phase 4.1 — coming in 0.37.1

The CLI redesign shipped its highest-impact piece (`k2so msg`)
in 0.37.0. The remaining verb surface is staged for 0.37.1:

- `k2so workspaces list` — yellow-pages of every registered
  workspace + agent + status + last activity
- `k2so workspaces running` — replaces `k2so agents running`
- `k2so workspace launch` — spawn-or-attach
- `k2so workspace profile / update`
- `k2so signal --workspace <path>` — workspace-keyed signal addressing
- `k2so template {list, create, delete}` — replaces `k2so agent template *`
- Removal of `k2so agents create / delete`
- `k2so help-deprecated` aggregator

Backwards-compat: every legacy verb keeps working in 0.37.0 with
a deprecation warning to stderr.

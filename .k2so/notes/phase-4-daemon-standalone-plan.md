# Phase 4 — Daemon Standalone (free from Tauri)

**Branch:** `feat/session-stream` (continues from Phase 3.2)
**Status:** PLANNED, not yet started
**Start:** 2026-04-20
**Strategic goal:** daemon no longer needs Tauri to serve any
`/cli/*` route. Tauri becomes a pure WS + command client. Thin
viewers (mobile, web, future native) become interchangeable.
**Engineering stance:** complete, not quick. No corner-cuts. Every
commit leaves the tree shippable.

---

## Why this matters

Phase 1-3.2 gave us daemon-owned SESSIONS. Phase 4 turns that into
daemon-owned CONTROL: every HTTP endpoint that today requires the
Tauri process running can now be served by a headless daemon.

Concrete payoffs once Phase 4 lands:

- **Lid-closed fully daemon-hosted.** Zero Tauri dependency on
  the hot path. Laptop reboots → launchd restarts daemon → every
  agent + every endpoint works without ever launching K2SO.app.
- **Thin clients are first-class.** Any WS subscriber can view any
  session. Future iOS native app, web viewer, CLI-attach from a
  remote machine — all become N-line clients.
- **Open-core split mechanical.** Daemon stays MIT; premium UI
  viewers can live in a separate crate without compromising the
  open primitive.
- **Remote connection story unblocked.** Daemon on workstation,
  viewer on phone over tailscale; same protocol as local.
- **Tauri's `agent_hooks.rs` HTTP server retires.** 3,454 lines of
  Tauri-specific routing go away. Desktop app shrinks to "UI +
  WS client + Tauri-command wrappers."

---

## What already exists (the gift of Phase 3.2)

The primitives Phase 4 needs are already in place:

| Primitive | Module | Purpose |
|---|---|---|
| `SessionEntry` + broadcast + replay ring | `k2so_core::session::entry` | Daemon-owned session state |
| `session::registry` | `k2so_core::session::registry` | Global session_id → SessionEntry map |
| `SessionStreamSession` + `spawn_session_stream` | `k2so_core::terminal::session_stream_pty` | Daemon-side PTY spawn |
| `session_map` | `k2so_daemon::session_map` | agent_name → Arc<Session> for inject reach |
| `spawn::spawn_agent_session` | `k2so_daemon::spawn` | Canonical spawn flow |
| `awareness::egress::deliver` | `k2so_core::awareness::egress` | Signal delivery (inject / wake / inbox / bus / audit) |
| `watchdog` | `k2so_core::session::watchdog` + daemon-side loop | Idle detection |
| `archive` | `k2so_core::session::archive` | NDJSON + rotation |
| `LaunchProfile` | `k2so_core::agents::launch_profile` | Auto-launch config |
| Scheduler tick | `k2so_core::agents::scheduler` | Decides which agents wake |
| Wake prompt composer | `k2so_core::agents::wake` | Builds wake context |
| `spawn_wake_headless` | `k2so_core::agents::wake` | Daemon spawns claude |
| Agent delegate | `k2so_core::agents::delegate` | Worktree + task CLAUDE.md |
| Heartbeat CRUD | `k2so_core::agents::heartbeat` | 6 daemon routes already shipped |

Every core helper Phase 4 needs is already in k2so-core. What's
left is the HTTP routing layer in the daemon + teaching routes to
call `spawn_session_stream` instead of legacy `spawn_wake_pty`.

---

## The 11 PRD-stated routes (mapped to new paths)

| Route | Today | Phase 4 target |
|---|---|---|
| `/cli/terminal/spawn` | Tauri emits HookEvent | Daemon `spawn_session_stream` |
| `/cli/terminal/spawn-background` | Tauri `terminal_manager.create()` | Daemon `spawn_session_stream` |
| `/cli/terminal/read` | Tauri `mgr.read_lines_with_scrollback()` | Daemon reads SessionEntry replay ring |
| `/cli/terminal/write` | Tauri `mgr.write()` | Daemon `session_map.lookup().write()` |
| `/cli/agents/running` | Tauri `mgr.list_terminal_ids()` | Daemon `session_map.snapshot()` |
| `/cli/agents/launch` | Tauri `spawn_wake_pty` | Daemon `spawn_wake_headless` (already core) |
| `/cli/agents/delegate` | Tauri `build_launch` + `spawn_wake_pty` | Daemon `agents::delegate` + `spawn_session_stream` |
| `/cli/heartbeat` (triage) | Tauri loop calls `spawn_wake_pty` per wake | Daemon already has `/cli/agents/triage` (lid-closed path); wire to Session Stream |
| `/cli/companion/sessions` | Tauri terminal_manager list | Daemon session_map + DB grouping |
| `/cli/companion/projects-summary` | Tauri terminal count | Daemon session_map grouped by project |
| `/cli/msg --wake` | Tauri PTY inject | Already done! `awareness::egress` via CLI's `signal` (Phase 3) |

---

## The seven commits (H1-H7)

### H1 — `/cli/terminal/read` + `/cli/terminal/write` (daemon-side)

**Scope:** session-stream-aware read + write, wired in the daemon.
- `/cli/terminal/read?session=<id>&lines=<n>[&scrollback]`
  - Reads `SessionEntry::replay_snapshot()`, decodes `Frame::Text`
    frames back to bytes, returns the last N lines.
- `/cli/terminal/write?session=<id>&text=<urlencoded>`
  - `session_map::lookup` (by session_id via a new
    `session_map::lookup_by_session_id` helper) or registry walk,
    call `session.write(bytes)`.

**New:** daemon route allowlist + handlers + helper to extract
text frames from replay into display bytes.

**Tests:** integration test that spawns a session, writes to it
via the new endpoint, reads back, asserts round-trip.

**Commit:** `H1 (phase 4): /cli/terminal/read + /cli/terminal/write daemon-side`

---

### H2 — `/cli/agents/running` (enumerate session_map)

**Scope:** JSON list of every active session's (agent_name,
session_id, cols, rows, started_at, idle_ms).

**New:** reuse `session_map::snapshot()` from G1; wrap in a route
handler that joins with `session::registry` for timestamps.

**Tests:** spawn two sessions, hit the endpoint, assert both
appear with correct metadata.

**Commit:** `H2 (phase 4): /cli/agents/running enumerates daemon session_map`

---

### H3 — `/cli/terminal/spawn` + `/cli/terminal/spawn-background`

**Scope:** thin wrappers over the existing
`spawn::spawn_agent_session` (G4). The difference between the two
routes is whether a "default consumer" (Tauri pane) gets
auto-subscribed — with daemon-owned sessions, that's a no-op at
spawn time (viewers subscribe on their own schedule).

**New:** route handlers delegating to `spawn::spawn_agent_session`
with defaults per the route semantics (spawn: has agent tag,
spawn-background: anonymous).

**Tests:** both routes spawn successfully and register in
session_map / registry.

**Commit:** `H3 (phase 4): /cli/terminal/spawn{,-background} use spawn_session_stream`

---

### H4 — `/cli/companion/sessions` + `/cli/companion/projects-summary`

**Scope:** cross-workspace session enumeration.
- `sessions` returns every live session joined with project info
  (path, name) from the DB.
- `projects-summary` groups count-of-sessions by project.

**New:** daemon-side SQL queries joining session_map's agent
names to `projects` + `workspaces`.

**Tests:** multi-workspace fixture, hit both endpoints, assert
correct grouping.

**Commit:** `H4 (phase 4): /cli/companion/{sessions,projects-summary} daemon-side`

---

### H5 — `/cli/agents/launch` + `/cli/agents/delegate`

**Scope:** agent-aware spawns that compose a wake prompt +
optionally create a worktree before the PTY opens.

**New:**
- Launch route: given `agent_name + project_path`, call
  `agents::wake::compose_wake_context` (already in core) →
  `spawn_wake_headless` (already in core). Register the resulting
  session in daemon's session_map.
- Delegate route: given `agent + work_file`, call
  `agents::delegate::delegate_work` (already in core) to
  create worktree + task CLAUDE.md, then `spawn_session_stream` in
  the worktree cwd.

**Dependency check:** `agents::delegate` today depends on
`crate::git::create_worktree` which is in src-tauri. Phase 4
blocker: need `git::create_worktree` moved to core, OR daemon
calls out to `git worktree add` via shell. Will decide during H5.

**Tests:** integration tests for both — launch an agent with an
`AGENT.md` profile, observe session spawn; delegate a work item,
observe worktree created + session spawned in worktree.

**Commit:** `H5 (phase 4): /cli/agents/launch + /cli/agents/delegate daemon-side`

---

### H6 — `/cli/heartbeat` triage side (daemon spawns via Session Stream)

**Scope:** the daemon's `/cli/agents/triage` (already there for
lid-closed) now spawns via `spawn_session_stream` when the
project has `use_session_stream='on'`. Keeps legacy
`spawn_wake_headless` for projects with the flag off.

**New:** triage's per-agent spawn branch reads
`get_use_session_stream(project_path)` and dispatches.

**Tests:** two-project fixture with flag on/off; triage fires for
both; session_map has the flag-on agent, legacy path handles the
flag-off agent.

**Commit:** `H6 (phase 4): triage spawns via Session Stream when project opt-in`

---

### H7 — Retire Tauri's agent_hooks HTTP server

**Scope:** daemon claims `heartbeat.port` unconditionally; Tauri's
agent_hooks HTTP server stops binding. Tauri becomes a pure
HTTP/WS client against the daemon for every `/cli/*` + `/events`
route.

**New:**
- Remove `src-tauri/src/agent_hooks.rs`'s HTTP listener startup
  code; keep the file only as a thin client wrapper for
  Tauri commands that need to POST to the daemon.
- Delete or deprecate `daemon.port` / `daemon.token` separate
  files; daemon writes only `heartbeat.port` + `heartbeat.token`
  (corner #2 of corners-cut-0.33.0.md closes here).
- Update `cli/k2so` CLI script to use `heartbeat.port` as the
  single source of truth.
- Test suite regeneration: any integration test that spun up
  Tauri's HTTP server for an /cli/* endpoint now spins up the
  daemon instead (no parallel test infra needed; daemon
  integration tests already exist).

**Tests:** the `cli/k2so sessions list` / `sessions spawn` etc.
all continue to work against the daemon; no regressions in the
existing 48-test daemon suite.

**Commit:** `H7 (phase 4): retire Tauri HTTP server; daemon owns every /cli/* route`

---

## Invariants preserved across Phase 4

Every Phase 1-3.2 invariant continues to hold:

1. **Subscribers never import alacritty types** — the new routes
   read from SessionEntry + session_map, not Term grid.
2. **LineMux sees raw PTY bytes** — unchanged; we're adding
   endpoints that observe the stream, not reshaping it.
3. **Feature flag `session_stream` gates consumer side only** — H6
   specifically honors the flag; H1-H5 are always on in the
   daemon (the daemon already requires `session_stream`).
4. **Sender's `Delivery` is load-bearing** — unchanged.
5. **Audit always fires** — unchanged; every route that mutates
   state writes to activity_feed as today.
6. **CLI uses `AGENT.md` as canonical filename** — unchanged.

---

## What's NOT in Phase 4

To keep scope honest, explicitly NOT in this phase:

- **Removing `alacritty_terminal` dep.** That's Phase 5, after
  Phase 4 proves the legacy Tauri PTY path is no longer needed.
- **Metal punch-through rendering.** That's Phase 8, independent
  of daemon migration.
- **Moving the full 5,683-line `commands::k2so_agents.rs`.**
  Phase 4 only requires the agents that are on the Session Stream
  critical path (`wake`, `delegate`, `triage` — all already in
  core). The rest can migrate incrementally in Phase 4-followup if
  needed, or stay src-tauri forever if we're fine with Tauri being
  thicker than a pure WS viewer.
- **Git worktree creation in core.** H5 will decide: either
  migrate `git::create_worktree`, or daemon shells out to
  `git worktree add`. Both are viable; will pick the simpler one
  during H5 implementation.

---

## Rollback

Every commit is additive + feature-flagged where applicable. H6
honors `use_session_stream='off'` explicitly. H7 is the only
commit with structural removal risk; before landing, we verify
the daemon serves every route the CLI + Tauri commands need.

- Emergency rollback for H1-H6: revert individual commits; each
  is self-contained.
- Emergency rollback for H7: revert, and Tauri's HTTP server
  resumes binding on next launch.
- Nuclear: `git reset --hard v0.33.0` + rebuild. Flag-off build
  is still bit-for-bit v0.33.0.

---

## Ordering and parallelism

H1 → H2 → H3 → H4 can ship in any order (each is independent).
H5 depends on the git worktree decision. H6 depends on H1-H5 for
the full daemon-side path. H7 depends on H1-H6 (daemon must be
complete before Tauri stops binding).

Recommended order: H1, H2, H3, H4, H5, H6, H7 — linear, each
commit independently shippable, each leaves the tree green.

---

## Before starting

H1 starts now. Scope-check every commit against this plan before
landing; if something grows past "thin route wrapper," stop and
write down the deviation here.

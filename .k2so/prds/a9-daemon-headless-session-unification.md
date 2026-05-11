# A9 — Daemon-headless v2 session unification

**Status:** ✅ **Phases 1-3 complete (landed pre-0.37.11). Phase 4 in flight for 0.37.11.**
**Audited:** 2026-05-11

## Why this exists

K2SO's vision: **the daemon is the source of truth, Tauri is one of N
possible viewers, and the server can run completely headless.** v2
(daemon-hosted Alacritty `Term` over the WS grid protocol) is the
session shape that supports this. Legacy `session_map` (with its
Kessel-T0 `SessionStreamSession`) is the path that *doesn't*.

A8 routed every system-driven terminal mount (`AgentPane`,
`BackgroundTerminalSpawner`, `AIFileEditor`) to v2. That fixed
"agent panel sessions die when Tauri quits" for the Tauri-open case
but exposed an inheritance gap: legacy `session_map` was hardcoded as
the single source of truth for "is agent X live?" / "give me agent X's
session handle" across ~15 daemon call sites.

A9 unified the lookup story so v2 is first-class end-to-end across the
awareness bus, heartbeat wake, durable signal queue drain, companion
roster, watchdog escalator, and the `/cli/terminal/{write,resize}`
HTTP routes. As a follow-on, A9 also migrated the daemon-side spawn
helper so heartbeat-headless wakes, `/cli/agents/launch`, and
`/cli/agents/delegate` all produce v2 sessions.

## End-state target

| concern | target | status |
|---|---|---|
| Cmd+T tab respects user's Settings → Renderer choice | ✅ unchanged | ✅ |
| AgentPane / Background / AIFileEditor → v2 | ✅ via A8 | ✅ |
| Heartbeat wake (Tauri-open via BackgroundTerminalSpawner) → v2 | ✅ via A8 | ✅ |
| Heartbeat wake (Tauri-closed via launchd) → v2 | ✅ A9 phase 3 | ✅ |
| `/cli/agents/launch` agent boot → v2 | ✅ A9 phase 3 | ✅ |
| `/cli/agents/delegate` task agent boot → v2 | ✅ A9 phase 3 | ✅ |
| `awareness::inject` finds v2 sessions | ✅ A9 phase 1 | ✅ |
| `pending_live` drained on v2 spawn | ✅ A9 phase 1 | ✅ |
| `/cli/terminal/{write,resize}` routes to v2 | ✅ A9 phase 2 | ✅ |
| Companion roster + watchdog see v2 sessions | ✅ A9 phase 2 | ✅ |
| Legacy `session_map` only used for explicit user-selected Kessel | ✅ everywhere else routes v2 | ✅ |

## What landed (phase by phase)

### Phase 1 — Unified lookup abstraction

- **`crates/k2so-daemon/src/session_lookup.rs`** — new module:
  - `LiveSession` enum: `Legacy(Arc<SessionStreamSession>)` |
    `V2(Arc<DaemonPtySession>)`
  - Helpers: `lookup_any`, `lookup_by_session_id`, `snapshot_all`,
    `list_agents`, `write`, `resize`, `cwd`, `command`, `args`,
    `is_v2`, `is_child_alive`
- **`crates/k2so-daemon/src/providers.rs`** — both providers now call
  `session_lookup::lookup_any`:
  - `DaemonInjectProvider::inject` + `::is_live` (line 31–59)
  - `DaemonWakeProvider::try_auto_launch` single-flight check (line
    181)
- **`crates/k2so-daemon/src/v2_spawn.rs`** — drain pending-live signals
  on register (lines 319–326): `pending_live::drain_for_agent` +
  `signal_format::inject_bytes` with two-phase write (body, settle,
  `\r`).

### Phase 2 — Observability surfaces see v2

- **`crates/k2so-daemon/src/terminal_routes.rs`** — `handle_write`
  (line 308) + `handle_resize` (line 280) route via
  `session_lookup::lookup_by_session_id`.
- **`crates/k2so-daemon/src/companion_routes.rs`** — `/cli/companion/sessions`
  (line 179) + `/cli/companion/projects-summary` (line 217) use
  `session_lookup::snapshot_all`.
- **`crates/k2so-daemon/src/watchdog.rs`** — `tick` (line 130) iterates
  `session_lookup::snapshot_all`, escalates idle sessions across both
  maps.

### Phase 3 — Daemon-spawned agents → v2

- **`crates/k2so-daemon/src/spawn.rs::spawn_agent_session_v2_blocking`**
  (lines 223–370) is the production spawn path:
  - Uses `DaemonPtySession::spawn(cfg)` (line 335)
  - Registers in `v2_session_map` (line 337)
  - Idempotency check via `v2_session_map::lookup_by_agent_name`
    (line 268)
  - Drains pending-live (lines 350–355)
- All daemon spawn callers route through it:
  - `/cli/agents/launch` (agents_routes.rs)
  - `/cli/agents/delegate` (delegation flow)
  - `DaemonWakeProvider::try_auto_launch` (providers.rs:212)
- Legacy `spawn_agent_session_blocking` (line 389–395) is
  `#[allow(dead_code)]` — reachable only by explicit user-selected
  Kessel via `/cli/sessions/spawn`.

## Phase 4 — Multi-viewer follow-on (0.37.11)

Phase 4 is the renderer-side adoption work that makes Phases 1-3
visible end-to-end. Without it, a second window viewing the same
workspace can't adopt the daemon's existing sessions; it spawns new
ones and the user sees duplicates.

| step | scope | status |
|---|---|---|
| **4a** — daemon `list_sessions_for_workspace(project_id)` query | new helper that filters `snapshot()` by project_id, exposed as Tauri command + HTTP route | in flight |
| **4b** — renderer adoption flow | `tabsStore.loadLayoutForWorkspace` consults daemon before spawning defaults; populates tabs with existing session_ids | in flight |
| **4c** — active-viewer resize gating | renderer-only: TerminalPane only sends WS resize when its parent window has OS focus | in flight |

After 4a-c land, opening a workspace in a second window (focus window,
"new window", or any future surface) adopts the daemon's live PTYs
rather than spawning duplicates, and only the focused window dictates
PTY size.

## Critical files (post-landing)

| concern | file |
|---|---|
| Unified lookup | `crates/k2so-daemon/src/session_lookup.rs` |
| Module registration | `crates/k2so-daemon/src/lib.rs:18` |
| Awareness + wake providers | `crates/k2so-daemon/src/providers.rs` |
| Pending-live drain on v2 spawn | `crates/k2so-daemon/src/v2_spawn.rs:319-326` |
| Terminal write/resize | `crates/k2so-daemon/src/terminal_routes.rs` |
| Companion roster | `crates/k2so-daemon/src/companion_routes.rs` |
| Watchdog | `crates/k2so-daemon/src/watchdog.rs` |
| Daemon-spawn helper (v2) | `crates/k2so-daemon/src/spawn.rs:223-370` |

## Out of scope (still)

- Retiring `SessionStreamSession` / Kessel-T0 entirely. Stays for
  users who explicitly select Kessel.
- Migrating the explicit `/cli/sessions/spawn` (Kessel) endpoint.
- Cmd+T user-spawn migration — covered separately by the renderer
  pathway that respects Settings → Renderer.

## Definition of done

All seven criteria below now hold:

1. ✅ `k2so msg --wake <agent>` works against v2 sessions.
2. ✅ Heartbeat-wake against an offline agent produces a v2 session,
   regardless of whether Tauri is open at wake time.
3. ✅ The pending-live signal queue drains on every spawn.
4. ✅ Mobile companion + sidebar `live count` reflect total live
   sessions across v2 + legacy.
5. ✅ Watchdog enforces idle-timeout escalation across both maps.
6. ✅ `/cli/terminal/{write,resize}` route to whichever map owns the
   requested session.
7. ✅ Legacy `session_map` only contains sessions for users who
   selected **Kessel** as their renderer for a Cmd+T tab.

A9 phases 1-3 are **done**. Phase 4 is the visible-to-the-user payoff
shipping in 0.37.11.

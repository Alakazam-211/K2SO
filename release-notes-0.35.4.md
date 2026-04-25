# K2SO 0.35.4 — A8 + A9: daemon goes v2-native, headless-ready

The architectural follow-up to 0.35.0–0.35.3. Now that
**Alacritty (v2)** is the default renderer everywhere a Tauri tab
is involved, this release rewires the daemon's CLI tools so they
**also** see v2 sessions — closing every audit gap the v2 cutover
exposed and unblocking the headless-server vision (Tauri can quit;
the daemon, agents, heartbeats, and signals keep running).

The headline number: heartbeats end-to-end now work against v2
agent sessions in every spawn path, including the
Tauri-closed/launchd one.

## What was broken before this

After 0.35.0 made v2 the default and 0.35.1/.2/.3 fixed the
PATH-from-launchd, login-shell `.zshrc` sourcing, and child
`TERM=dumb` color regressions, an audit found **15 daemon call
sites** that only consulted the legacy `session_map` and were
blind to v2 sessions. Concretely:

- `k2so msg --wake <agent>` returned *"no live session"* for
  every v2 tab, even though the session was right there.
- Heartbeat-driven Tauri-closed wakes spawned a *legacy*
  session, ignoring the user's renderer preference.
- The `pending_live` durable signal queue never drained on v2
  spawn — wake-queued signals were silently lost.
- `/cli/terminal/{write,resize}` 400'd on v2 session ids.
- Mobile companion + sidebar `live count` showed 0 for v2 tabs.
- Watchdog idle-escalation never fired against v2 sessions.

Unit tests stayed green throughout — these were daemon-runtime
integration paths none of the unit suites exercised.

## What's in 0.35.4

### A8 — frontend mounts already v2 (committed alongside)

Four uncommitted A8 edits ride along in this release: every
non-tab terminal mount now uses `<TerminalPane>` (v2) instead of
the legacy `<AlacrittyTerminalView>`. Affects:

- `src/renderer/components/AgentPane/AgentPane.tsx`
- `src/renderer/components/BackgroundTerminalSpawner.tsx`
- `src/renderer/components/AIFileEditor/AIFileEditor.tsx`
- `src/renderer/stores/tabs.ts` (renderer-type return fix)

These are the system-driven terminal mounts (agent panel,
heartbeat-wake background spawn, AI file editor preview). They
were the easy half of the v2 cutover; A9 closes the other half.

### A9 — daemon-side v2 plumbing

Three coordinated phases, all in this single release per
end-state target:

#### Phase 1 — Awareness-correct & no signal loss

- **New module** `crates/k2so-daemon/src/session_lookup.rs` —
  introduces `LiveSession::{Legacy, V2}` with polymorphic
  `write` / `resize` / `cwd` / `command` / `session_id`,
  plus `lookup_any` / `lookup_by_session_id` / `snapshot_all` /
  `list_agents` that walk both maps.
- **`providers.rs`** — `DaemonInjectProvider::inject` and
  `DaemonWakeProvider::try_auto_launch` switch to
  `session_lookup::lookup_any`. Closes "msg --wake doesn't see
  v2" + "wake duplicate-spawns against live v2".
- **`v2_spawn.rs`** — `handle_v2_spawn` now drains
  `pending_live::drain_for_agent` on register, mirroring the
  legacy spawn contract. No more silently-lost wake signals on
  v2 boot.

#### Phase 2 — Observability surfaces see v2

- **`terminal_routes.rs`** — `/cli/terminal/write`,
  `/cli/sessions/resize`, and `/cli/agents/running` all switch
  to `lookup_any` / `snapshot_all`. Mixed legacy + v2 sessions
  enumerate cleanly.
- **`companion_routes.rs`** — `/cli/companion/sessions` and
  `/cli/companion/projects-summary` use `snapshot_all`. Mobile
  companion + sidebar finally see every live session.
- **`watchdog.rs`** — tick walks both maps. Legacy still uses
  `session.kill()`; v2 unregisters from `v2_session_map` to
  trigger the IO-thread-exit-via-Arc-drop SIGHUP path. (V2
  registry-backed idle tracking lands in a follow-up; for now,
  v2 sessions skip the escalation ladder rather than misfiring.)

#### Phase 3 — Daemon-spawned agents go to v2 (the architectural step)

`crates/k2so-daemon/src/spawn.rs::spawn_agent_session_v2_blocking`
is the new helper. It takes the same `SpawnAgentSessionRequest`
and returns the same `SpawnAgentSessionOutcome` as the legacy
async `spawn_agent_session`, but the session it produces is a
`DaemonPtySession` registered in `v2_session_map`.

Migrated callers:

- `DaemonWakeProvider::try_auto_launch` (heartbeat headless
  wake, the launchd-fired Tauri-closed path).
- `terminal_routes::spawn_terminal_impl` (`/cli/terminal/spawn`,
  `/cli/terminal/spawn-background`).
- `agents_routes::*` (`/cli/agents/launch`,
  `/cli/agents/delegate`, `spawn_wake_via_session_stream`).

What stays on the legacy path: only `awareness_ws::handle_sessions_spawn`
(`POST /cli/sessions/spawn`), reached only when a user explicitly
selects **Kessel** in Settings → Renderer for a Cmd+T tab.

After 0.35.4, **both wake paths converge on v2 ownership**.
Heartbeat headless wake produces a v2 session whether Tauri was
open at wake time or not.

## Test coverage gaps closed

Two new regression tests in
`crates/k2so-daemon/tests/providers_inject_integration.rs`:

- `daemon_inject_provider_writes_bytes_to_v2_session` — register
  a v2 `DaemonPtySession` under an agent name, call the inject
  provider directly, assert `Ok`. Pre-A9 this would have
  returned `NotFound` because the provider only walked legacy
  `session_map`.
- `daemon_inject_provider_finds_legacy_first_then_v2` — register
  the same agent name in both maps, prove inject succeeds
  regardless of map registration order.

These are the tests that, had they existed, would have caught
the "msg --wake doesn't see v2" issue before it shipped. Future
hotfixes for daemon-runtime integration paths should land in
this suite.

Existing test files updated to use `session_lookup::lookup_any`
where they previously asserted `session_map::lookup`:

- `agents_routes_integration.rs`
- `scheduler_wake_integration.rs`
- `triage_integration.rs`
- `terminal_routes_integration.rs`

Also fixed two test files that had been calling
`awareness_ws::handle_sessions_spawn` (an async fn) without
`.await` and were relying on a compile error nobody had run
into yet: `spawn_to_signal_e2e.rs`,
`pending_live_durability.rs`. Those tests pass now under
`cargo test`.

Total: **all workspace tests pass; 4 inject integration tests
including 2 new v2 regressions; 0 type errors.**

## What's NOT in this release

- Watchdog escalation against v2 sessions (registry-backed idle
  tracking for v2 is a follow-up).
- Retiring `SessionStreamSession` / Kessel-T0 entirely. That
  endpoint stays alive for users who explicitly select Kessel.
- Frontend changes beyond A8. The renderer surface is unchanged.

## Definition of done — what now works end-to-end

1. `k2so msg --wake <agent>` reaches v2 sessions.
2. Heartbeat wake against an offline agent ends up creating a
   v2 session, regardless of whether Tauri was open at wake.
3. `pending_live` drains on every spawn, both maps.
4. Companion + sidebar live counts include v2.
5. `/cli/terminal/{write,resize}` route to whichever map owns
   the session.
6. Legacy `session_map` only grows on explicit Kessel spawns.

That last point is the qualitative test for the headless-server
vision: with v2 as the default, the daemon should function with
the legacy `session_map` permanently empty.

## Upgrade

The auto-updater will swap binaries and prompt to relaunch. The
daemon picks up the new binary on relaunch via the version-
mismatch auto-restart path landed in 0.35.0.

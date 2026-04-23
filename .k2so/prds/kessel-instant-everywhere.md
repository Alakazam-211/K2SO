# Instant Everywhere — Kessel Responsiveness PRD

**Status:** planning addendum to `canvas-plan.md`
**Date:** 2026-04-23
**Author:** Rosson (direction), Coordinator (draft)

## 1. What this PRD is

`canvas-plan.md` established the *architectural* end-state for
Kessel: every session is a byte stream; each client renders
locally via an alacritty Term; grow_boundary is an in-band APC.
That doc answered the question "what's the right data model?"

This document answers a different question: **"how do we make
that architecture feel exceptional in the hands of a user?"**

Specifically:
- Workspace switches should feel instant regardless of workspace
  size.
- Cold app launches should feel like resume-from-sleep.
- Visiting a workspace for the first time in days should never
  show an empty "loading" state.
- Deep scrollback, selection, reflow, find — all should perform
  like a mature native terminal.
- Agents keep running correctly through all of this. Heartbeats,
  wake-triggered spawns, headless operation preserved.

The Canvas Plan is the *chassis*. This PRD is the *ride quality*.

## 2. Motivating observations

### 2.1 Alacritty feels perfect. What's it doing right?

Three properties, measured from the code:

1. **UI paint is decoupled from terminal creation.**
   `AlacrittyTerminalView`'s mount effect fires
   `invoke('terminal_exists?')` and `invoke('terminal_create')` as
   async promises — React never awaits them. The tab layout and
   cursor appear; the PTY wires up in the background.
2. **Concurrent spawn is cheap.** `terminal_create` is a local
   PTY fork + exec (~10ms). N concurrent spawns complete in
   roughly 10ms wall time on the blocking pool.
3. **Retained view within a workspace.** Tab-within-workspace
   switches are CSS `display:none` toggles. No unmount, no
   remount, no re-init.

The result: 20 tabs spawning concurrently on first workspace
visit is imperceptible.

### 2.2 Kessel today (post-Phase-9) is close but not equal

- Within-workspace tab switches: good (retained view).
- Workspace switches where the daemon still has live sessions:
  good (Phase 9 idempotent spawn returns in µs).
- **Cold spawns** — first visit, post-app-launch, or after
  daemon restart: slow. Each `kessel_spawn` is a blocking HTTP
  POST that the daemon holds for ~400-800ms doing grow-settle.
  N concurrent = N × ~800ms (some parallelism, but serialized
  through Tauri's async command dispatch). 5 tabs → 4s
  beachball; 20 tabs → unusable.
- **No snapshot persistence.** Tab remount shows empty pane
  until kessel_spawn resolves AND the first grid snapshot
  arrives over the WS. A ~1s window of nothing.

## 3. Principles

Four invariants drive every decision in this plan:

1. **The UI never waits on anything that isn't a direct user
   action.** No invoke on the render path. No synchronous HTTP.
   Paint is decoupled from wiring; wiring catches up.
2. **The daemon is the source of truth for session state.**
   Spawn, grow, teardown, lifecycle — all happen daemon-side,
   independent of any client. Clients are stateless projections.
3. **Every piece of session content has a local cached
   representation.** Workspace switches, app restarts, new-
   workspace-first-visits — all surface cached content in one
   animation frame, then reconcile live bytes asynchronously.
4. **Scaling is linear in the number of visible panes, not
   total panes.** 200 mounted tabs are fine. Only on-screen
   tabs cost anything.

## 4. End-state experience

The experience we're shipping toward:

- User switches workspaces → tabs appear with yesterday's
  content in one animation frame. Live bytes layer in within
  100ms. No beachball, ever.
- User cold-launches K2SO → every tab appears populated before
  the main window fully fades in. Claude sessions resume in the
  background; the user sees conversation history immediately.
- User opens a workspace they haven't visited in a month →
  content is there from cache. Agents that were running continue
  uninterrupted (daemon kept them alive). Agents that exited
  restart with warm cache context.
- User drags the window narrower → scrollback reflows
  instantly, cursor tracks, selection survives.
- User does Cmd+F → searches 5000 rows of scrollback in ~10ms.
- Mobile companion on the couch → sees the same sessions at
  phone-width via Frame stream (T0.5/T1), perfect layout.

## 5. Heartbeat invariants

The daemon's agent-orchestration loop runs headlessly. These
invariants MUST hold through every phase of this plan:

- `/cli/scheduler-tick`, `/cli/heartbeat/*`, the LaunchAgent
  plist, `heartbeat.sh` — all unchanged.
- `session_map::register` stays synchronous inside
  `spawn_agent_session`. Any code path that expects to find a
  session in `session_map` by `agent_name` right after spawn
  continues to work.
- `session.write()` already locks its writer mutex.
  Concurrent writes (pending-live drain + egress inject)
  serialize per-call; signals never interleave byte-level.
- Pending-live drain ordering preserved: runs after
  grow-settle, before end of the background task.
- Grow-boundary APC stays in-band. Heartbeat-spawned sessions
  with no subscribers have it sitting in the ring until
  somebody eventually attaches. Correct.
- Agents run regardless of UI state. The Tauri app is an
  optional consumer.

Every phase below names its heartbeat touchpoints explicitly or
confirms it has none.

## 6. Phase plan

Ten phases, each independently shippable, each compounding with
the ones before it. Estimates are working days of focused
engineering.

### Phase A — Non-blocking spawn (2 days)

**What:** Split `spawn_session_stream_and_grow` so the PTY +
`session_map::register` return in ~20ms. Move grow-settle + APC
boundary injection + SIGWINCH + pending-live drain onto a
background tokio task.

**Why:** `kessel_spawn` HTTP POST resolves in ~20ms instead of
~800ms. N concurrent spawns complete in one animation frame on
the daemon's async pool. Cold workspace feels Alacritty-grade.

**Heartbeat:** careful. `session_map::register` stays
synchronous (invariant preserved). Pending-live drain moves
onto the same background task that runs grow-settle, so
ordering is: settle → APC → SIGWINCH → drain — identical to
today's blocking path, just off the hot path.

### Phase F — Kessel Term attach yields early (1 day)

**What:** `kessel_term_attach` serialized at 36 → 3399ms across
5 concurrent calls in Rosson's trace. That's Tauri's async
command dispatch thread. Fix by yielding at the first `.await`
and deferring Term allocation into the reader loop's first
`feed_bytes`.

**Why:** True parallelism for concurrent attach calls. 5 attaches
land in ~50ms instead of 3.4s serialized.

**Heartbeat:** none. Client-only concern.

### Phase B — Client-side snapshot cache on pane unmount (3 days)

**What:** On Kessel pane unmount (workspace stash, tab close),
persist the latest `TermGridSnapshot` to IndexedDB keyed by
`tab-<terminalId>`. On remount, rehydrate the snapshot into
initial component state BEFORE any invoke fires. Tab renders
populated in one animation frame; live reconcile happens in
background.

**Why:** Workspace switches paint cached content instantly.
Even if the underlying daemon reattach takes 500ms, the user
sees full content the entire time.

**Heartbeat:** none. Client-only.

**Size budget:** bounded per-tab cache (e.g. 500 scrollback
rows per cached snapshot). Aggregate eviction policy: drop
snapshots unused in 7 days; drop oldest when total cache > 100
MB.

### Phase C — Cross-launch persistence (4 days)

**What:** Two parallel tracks.

*Client:* Phase B's IndexedDB cache survives app restart. Cold
boot of K2SO → tabs rehydrate from cache as they mount, before
ANY network activity.

*Daemon:* persist session metadata to SQLite (`session_archive`
table with `agent_name`, `workspace_id`, `cwd`, `command`,
`args`, `created_at`, `last_seen_at`). On daemon restart, read
the table and rebuild session_map metadata — respawn via original
command. Claude resumes cleanly via `--resume`.

**Why:** Launching K2SO looks like resuming from sleep. Every
tab has content before the user does anything. Every daemon
restart (reboot, launchd reload) is transparent to sessions.

**Heartbeat:** Phase 9 idempotency ensures a heartbeat firing
during daemon warm-up doesn't double-spawn. If the warm-up is
mid-flight and a heartbeat tries to wake the same agent, it
sees session_map's pending entry and reattaches.

### Phase D — Daemon startup warm-up (3 days)

**What:** Daemon startup reads the Phase-C session metadata and
pre-spawns each agent's session in parallel. By the time K2SO
launches, session_map is populated; first kessel_spawn per tab
hits Phase 9 reattach and returns in µs.

**Why:** No workspace ever feels "cold" after any prior spawn
of any tab. Even agents that haven't been looked at in days are
pre-warmed.

**Heartbeat:** guarded by Phase 9. Heartbeats that fire during
warm-up hit the idempotency branch.

**Policy question:** do we pre-spawn ALL persisted sessions or
only those marked "keep warm"? Probably all by default; user
setting to opt out per-tab or per-workspace.

### Phase E — Lazy pane initialization within large workspaces (3 days)

**What:** Workspace restore mounts placeholders for every tab,
but only fires `kessel_spawn` for the active tab + N-1 nearest-
neighbor tabs (configurable, default N=3). Non-visible tabs
spawn on first visibility (IntersectionObserver or tab-click).

**Why:** A 50-tab workspace mounts in one animation frame.
Daemon isn't hit for tabs the user never opens in this session.

**Heartbeat:** none. Daemon-driven spawns (heartbeat, CLI,
scheduler) are unaffected — they always fire server-side, UI
lazy-init is purely about which client-initiated spawns defer.

### Phase G — Web Worker for snapshot deserialization (4 days)

**What:** Offload `JSON.parse` of big snapshot payloads +
`mergeDelta` reconciliation to a Web Worker. Main thread
receives pre-reconciled React state via postMessage and does
DOM diffing only.

**Why:** Main thread never hitches when a big Claude resume
paints into the ring and client has to catch up. 2MB JSON parse
doesn't block keyboard input anymore.

**Heartbeat:** none. Rendering-layer concern.

### Phase H — Virtualized row rendering (3 days)

**What:** IntersectionObserver-backed virtual rendering for the
row list. Only actually-on-screen rows reconcile; off-screen
rows are placeholders.

**Why:** 50k-row scrollback scrolls at 60fps because React only
diffs the rows in the viewport.

**Heartbeat:** none.

### Phase I — Content-space selection overlay (4 days)

**What:** Drop browser-native Selection. Custom overlay backed
by `selection.ts`. Selection is a pair of `(absRow, col)`
coordinates that survive resize, scroll, workspace switch.
Cmd+F uses the same coord system for find-in-scrollback.

**Why:** Selection feels like mature native terminals.
Copy-on-select works. Selection doesn't desync.

**Heartbeat:** none.

### Phase K — Claude stream-json (T1) adapter (~2 weeks, separate track)

**What:** Integrate `claude -p --output-format stream-json` as
an alternative ingestion path. Semantic events flow through
Frame stream; mobile/narrow clients render at native width with
reflow-free layout.

**Why:** Unlimited semantic scrollback for mobile. Unlocks the
PRD's original multi-device promise. Desktop Kessel continues
to use the byte stream for pixel-perfect rendering.

**Heartbeat:** none directly. T1 integration is per-harness;
doesn't alter the agent lifecycle.

## 7. Ordering + milestones

**Milestone 1 — "Instant everywhere" (Phases A + F + B + C)** —
2-3 weeks

After this milestone:
- Cold spawn: ~50ms perceived
- Hot reattach: ~5ms perceived
- Workspace switch: instant, cached content visible
- Cold app launch: cached content visible immediately
- Cross-restart: sessions respawn automatically

This is the shippable target for 0.35.0. 80% of the perceptual
win for 40% of the total work.

**Milestone 2 — "Foreverless latency" (Phases D + E + G + H)** —
2-3 weeks

After this milestone:
- 50-tab workspaces mount in one frame
- Daemon warm-up eliminates any cold-spawn cost entirely
- Big resume payloads don't hitch the main thread
- 50k scrollback scrolls at 60fps

Shippable target for 0.36.0.

**Milestone 3 — "Mature terminal parity" (Phase I)** — 1 week

Proper selection, find, polish. 0.36.1 or 0.37.0.

**Milestone 4 — "Mobile premium" (Phase K)** — separate track,
~2 weeks

T1 stream-json for Claude. Enables the PRD's original
multi-device reflow-free story. Runs on its own schedule.

## 8. Success criteria

Quantitative targets for milestone sign-off:

### Milestone 1
- Single-pane cold spawn, empty session: ≤ 100ms from
  `kessel_spawn` invoke fire to first frame painted.
- 10-pane concurrent cold spawn: ≤ 200ms total wall clock from
  workspace-switch-begin to all panes populated.
- Hot workspace switch (all sessions live on daemon): ≤ 50ms
  from workspace-switch-begin to visible.
- Cold app launch (daemon alive, client cold): every Kessel
  tab shows cached content within ≤ 50ms of pane mount.

### Milestone 2
- 50-pane workspace restore: ≤ 100ms total from workspace-
  switch-begin to all placeholders mounted.
- 2MB snapshot merge: main thread blocked for ≤ 16ms.
- 50k-row scrollback: ≥ 55fps sustained during fast scroll.

### Milestone 3
- Selection across scrollback → scroll → resize → copy: text
  copied is character-identical to what was selected.
- Cmd+F search on 50k-row buffer: first match visible ≤ 50ms.

## 9. Trade-offs + open questions

- **Phase A subtle semantic shift:** today's `kessel_spawn`
  response means "session is ready to accept input." After
  Phase A it means "session handle is allocated; setup
  continuing." Any downstream code that assumes the former
  needs review. Expected: only the existing spawn-site callers
  (awareness_ws, agents_routes, providers, scheduler-wake).
  All currently chain spawn → inject via egress which uses
  `session.write()` — works either way because the writer is
  ready the instant the PTY is opened.
- **Phase C size budget:** full byte archives on disk can get
  large for long sessions. Phase C persists METADATA only;
  byte archives already have their own eviction story. Just
  need to add a cleanup pass that drops sessions unused in N
  days.
- **Phase D policy:** "warm up ALL persisted sessions" could
  spawn 50+ processes on boot. Large RAM footprint. User
  setting? Workspace-level opt-in?
- **Phase E N=3 neighbor default:** what's "nearest neighbor"
  for a non-linear pane grouping? Probably just "tabs in the
  same paneGroup"; split-panes all mount eagerly.
- **Phase G worker overhead:** for small deltas (single-row
  update), Worker round-trip might be slower than inline.
  Hybrid policy: inline if payload < 50KB, Worker otherwise.

## 10. What this PRD does not do

- Does not replace `canvas-plan.md`. That still defines the
  architecture.
- Does not replace the original `session-stream-and-awareness-
  bus.md` PRD. Both still apply.
- Does not change the Frame stream model for thin clients.
  Mobile companion work (Phase K) is additive; Frame stream
  stays as the semantic channel.
- Does not change any heartbeat, scheduler, or wake-agent
  code path semantically. Timing may shift (Phase A), but
  contracts hold.

## 11. Rollback stance

Every phase is behind its own commit boundary. Each is
reversible without undoing the phases before it:
- Phase A revert: drop the background-task split; spawn becomes
  blocking again.
- Phase B revert: delete the IndexedDB cache read on mount;
  tabs render empty again until kessel_spawn resolves.
- Phase C revert: drop the daemon startup warm-up hook; phase
  B cache still works.
- Etc.

Any phase can be disabled independently if it misbehaves in
production.

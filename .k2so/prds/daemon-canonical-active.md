# PRD: Canonical Daemon-Owned "Active" + Daemon-Side Reaping

**Status:** Draft — 2026-06-07 (Rosson directive)
**Target release:** 0.39.38 (this is the *real* fix for GH#22; ships with the staged 0.39.38 batch)
**Task:** #672
**Supersedes (in this area):** the 0.39.38 renderer-side attach-guard band-aid (`sweepAgedOutWorkspaceChatsFromDaemon` subscriberCount gate). Keeps: real `subscriberCount` (promoted to a daemon reap gate) and the `v2/close` close-guard (belt-and-suspenders).
**Builds on:** #662/#663 session-lifecycle + Active-correctness (`.k2so/prds/0.39.x-session-lifecycle-active.md`).

---

## 1. Problem & Root Cause

GH#22: a remote K2 Connect client opens a dormant workspace's pinned Chat; ~15s later the host kills the session. Root cause, per Rosson's diagnosis (correct):

**"Active" is a per-client renderer-derived notion, and the reaping decision is driven off it — but a remote client's activation never becomes canonical, so the host doesn't see the workspace as Active and reaps it.**

Today:
- The **Active set** is computed *in the renderer* (`ActiveBar.tsx` / `projects.ts`) from two daemon DB fields — `projects.lastInteractionAt` and `projects.manuallyActive` — against a window (`active_window_hours`, default 24h).
- The **reap loop** is *renderer-owned*: `sweepAgedOutWorkspaceChats` + `sweepAgedOutWorkspaceChatsFromDaemon` (`tabs.ts`) schedule `scheduleWorkspaceChatReap` (15s grace, `DISMISS_REAP_GRACE_MS = 15_000`) → `closeV2Session` → `POST /cli/sessions/v2/close`.
- Each connected client derives Active **independently** and runs its **own** reap timers. On a shared server, two clients disagree about what's Active, and whichever host instance runs the sweep can reap a session another client is using.

**The fix Rosson directed:** Active is a **canonical, daemon-owned property of the server**, mirrored 1:1 to every connected client, and **the reap decision itself moves into the daemon**, keyed on that canonical Active set. The renderer stops owning both.

This also delivers the multi-user property: two users on one daemon each open a different workspace → **both workspaces are canonically Active → both users see both in their Active bar**, even the one they didn't open. Active is the **union** of all clients' activity, owned by the daemon.

---

## 2. Goals / Non-Goals

### Goals
1. **Daemon owns the canonical Active set.** One source of truth per daemon; computed server-side from `lastInteractionAt` + `manuallyActive` + `active_window_hours`.
2. **Daemon owns the reap loop.** A daemon-side task runs the grace-reap, keyed on canonical Active. Renderer no longer schedules or fires reaps.
3. **All clients mirror Active 1:1**, via a daemon push (broadcast) + snapshot-on-connect. No client-side derivation.
4. **Multi-user union semantics.** Any client/user activating a workspace makes it canonically Active for everyone connected to that daemon.
5. **Survive daemon restart.** Active is reconstructible from persisted fields on boot; in-flight reap timers reschedule.
6. **No `set_active` storms** (respect the #603/#604 guards — note this is a *different* "active" than grid-pane focus; see §4.0).
7. **Capability-gated** so older clients/daemons degrade gracefully.

### Non-Goals
- **Per-user Active filtering.** Rosson explicitly wants a *shared* mirror, not per-user views. The model is a global union, not `(user, project)` scoping. (Attribution — "who/what activated it" — is optional metadata for an indicator, not a filter.)
- Reworking grid-pane focus tracking (`decide_set_active` in `sessions_grid_ws.rs`) — unrelated.
- Changing the 15s grace duration or the Active window default.

---

## 3. Current Architecture (verified)

### Renderer (owns Active + reap today)
- `src/renderer/stores/projects.ts` — `activeProjectId`/`activeWorkspaceId` (~110-111); `setActiveProject` (~404) / `setActiveWorkspace` (~456) call `touchInteraction(projectId)` → bumps `lastInteractionAt`; `setManuallyActive` (~595) → `POST projects/update { manuallyActive }`.
- `src/renderer/stores/tabs.ts` — `DISMISS_REAP_GRACE_MS = 15_000` (45); `AgeOutSweepCandidate` (54-68); `scheduleWorkspaceChatReap` (~750-817); `sweepAgedOutWorkspaceChats` (~3826-3858); `sweepAgedOutWorkspaceChatsFromDaemon` (~3860-3950, the daemon-PTY-enumerating variant + the 0.39.38 subscriberCount gate). Reap gates: `isAged && !manuallyActive && !heartbeatEnabled && projectId !== foregroundId`.
- `src/renderer/stores/connect-host.ts` — `onActiveHostChange` (~170-181): on host switch, clears active IDs + re-fetches projects from the new host (the mirroring precedent, #625).

### Daemon (owns the underlying fields; does NOT compute Active or reap today)
- `projects` table: `lastInteractionAt`, `manuallyActive`, `heartbeat_enabled`, … (SQLite, k2so-core).
- `POST /cli/projects/update` (`db_routes.rs:687-729`) writes `manually_active`.
- `POST /cli/projects/touch-interaction` bumps `lastInteractionAt`.
- `crates/k2so-core/src/app_settings.rs:200-201` — **`active_window_hours: u32` already exists** (default 24 via `default_active_window_hours`, :77). Persisted in `app_settings.json`.
- **No** canonical Active route, **no** Active broadcast, **no** daemon reap loop today.

### Broadcast infra (exists, reusable)
- `crates/k2so-daemon/src/session_events.rs` — process-wide `OnceLock<broadcast::Sender<SessionEvent>>` (cap 256); `sender()`/`subscribe()`/`emit()`; `#[serde(tag="kind", rename_all="snake_case")]`; **"Adding new variants is [non-breaking]"** (:44-46). Today carries `SessionAdded`/`SessionRemoved`, emitted at `v2_session_map::register/unregister`.
- Renderer consumer: `src/renderer/stores/session-events.ts` — `subscribeToWorkspaceSessionEvents(projectPath, handlers)` with a `Hello`/snapshot-on-(re)connect trigger.

### Daemon-first precedent to mirror
- `workspace_sessions.surfaced` flag + `POST /cli/session/set-surfaced` (`agents_routes.rs:834-863`) → persists to SQLite → emits a hook event broadcast → all clients re-render. This is the exact shape we extend: **route mutates canonical field → daemon emits delta → clients mirror.**

---

## 4. Target Architecture

```
            ┌──────────────── DAEMON (canonical) ────────────────┐
client A ──▶│  POST /cli/projects/activate {projectId}           │
client B ──▶│     → bump lastInteractionAt / set manuallyActive  │
            │     → recompute Active set (union, window-based)    │
            │     → emit ActiveChanged delta                      │
            │                                                     │
            │  Active reaper task (owns the 15s grace timer):     │
            │     for each aged-out workspace chat PTID:          │
            │       gates: !inActiveSet && !heartbeat             │
            │              && subscriberCount==0  ── grace 15s ──▶ close v2 + persist sessionId for --resume
            │                                                     │
            │  /cli/projects/active  (snapshot GET)               │
            │  ActiveChanged broadcast  (push deltas)             │
            └──────────────────────┬──────────────────────────────┘
                                   │  mirror 1:1 (snapshot + deltas)
                    ┌──────────────┴───────────────┐
              client A Active bar           client B Active bar   ← both show the union
```

### 4.0 Two distinct "active" notions — do not conflate
- **Workspace-Active** (this PRD): membership in the Active bar; coarse; per-workspace; drives reaping.
- **Grid-pane-active** (`sessions_grid_ws.rs::decide_set_active`, #603 storm fix): which terminal pane a grid-WS subscriber has focused; fine-grained; unchanged. The storm guards (renderer send-dedup, empty-deps effect, symmetric unmount release) stay exactly as-is. Our new `activate` calls are **coarse and deduped** (§7), so they cannot reproduce the storm.

### 4.1 Canonical Active set (daemon)
- **Definition (the union):** a workspace is Active iff `manually_active = 1` **OR** `now - last_interaction_at < active_window_hours`. Computed server-side. Because `last_interaction_at`/`manually_active` are global-per-daemon, "any client activated it" ⇒ "Active for all" falls out for free — that *is* the union.
- **No new storage required for the set itself** — it's derived from existing persisted fields + `app_settings.active_window_hours`. (Survives restart by construction; recomputed on boot.)
- **Optional attribution** (for an "activated autonomously" indicator): reuse the existing heartbeat-surfaced signal; do **not** add per-user rows.

### 4.2 Activate route(s) (daemon)
Add a single coarse entrypoint the renderer calls instead of locally mutating Active:
- `POST /cli/projects/activate { projectId }` → `touch_interaction(projectId)` (bump window) → recompute → emit delta. (User opened/focused a workspace.)
- `POST /cli/projects/pin { projectId, pinned }` → set `manually_active` → recompute → emit delta. (Keep-in-Active-bar toggle; can stay folded into existing `projects/update`.)
- `POST /cli/projects/dismiss { projectId }` → clear `manually_active` if set, and mark the chat eligible for the grace-reap **now** (don't wait for window expiry) → emit delta + arm the daemon reap timer. (Explicit remove-from-Active.)
- `GET /cli/projects/active` → `{ projectIds: string[], activeWindowHours: u32 }` snapshot. Used on connect / host-switch.

All require a valid owner-or-connect-user session (existing `token_ok`/`validate_session`); all mutating routes POST-only (per the post-only route-guard rule).

### 4.3 Daemon-side reaper (the core change)

> **DESIGN DECISION (Rosson, 2026-06-07): Active-only reaping.** The reap decision is driven **solely** by the canonical Active set — no subscriber/attach gate. Active is the single authority. This rests on one load-bearing invariant (§4.3.1).

A single daemon task (tokio) owns the grace-reap. It replaces the renderer sweeps entirely.

- **Enumerate** live v2 chat PTYs from `v2_session_map` (the daemon already has these) joined with `projects` metadata.
- **Gates to reap** (Active-only, all daemon-sourced):
  1. `!in_active_set(projectId)` — the driver: aged out of the window **and** not `manually_active`.
  2. `!heartbeat_enabled(projectId)` — heartbeat keeps it warm.
  - **No subscriber gate.** Attachment does *not* keep a session alive; Active does. (See §4.3.1 for why this is safe.)
- **Grace:** when a workspace first becomes reap-eligible (window expiry tick **or** explicit `dismiss`), arm a 15s timer (`DISMISS_REAP_GRACE_MS`, moved daemon-side). If it re-enters the Active set within the grace, cancel.
- **On fire:** re-check the Active gates at fire time, persist the chat's `sessionId` (so `--resume` has a target — daemon already knows the PTY's session), then **force-close** the v2 session (`force: true`, bypassing the close-guard — the reaper is Active-authoritative).

#### 4.3.1 The load-bearing invariant: open/attach ⇒ activate
Active-only reaping is safe **only because opening or attaching to a workspace's chat is an activation.** Every path that surfaces a workspace's chat to a client — initial open, host-switch restore, K2 Connect remote-open, tab focus — **must** call `POST /cli/projects/activate`, which bumps `last_interaction_at` and puts the workspace in the canonical Active set. Therefore "a client is watching it" ⇒ "it is Active" ⇒ "the reaper won't touch it." This is what the old renderer reaper failed to do (it didn't treat a remote-open as activation), which *was* GH#22. **Implementation P3 must guarantee this wiring; it is the safety property the whole design leans on.**

Residual edge (accepted): a workspace held **open but entirely untouched** for longer than `active_window_hours` (default 24h) can be reaped while still on-screen. Re-focusing it re-activates it; the window is user-configurable. This is the single, narrow cost of dropping the subscriber gate, and it is acceptable per the design decision above.
- **Boot:** on daemon start, run one reconciliation pass and arm timers for already-aged survivors (this absorbs the #663 "boot eagerly restores / sweep can't reach hidden aged-out" fixes daemon-side).
- **Cadence:** a low-frequency tick (e.g. every 30–60s) recomputes eligibility; deltas (activate/dismiss/subscriber attach-detach) adjust timers immediately. No tight loop.

### 4.4 Broadcast: `ActiveChanged` delta
- **DECISION (Rosson, 2026-06-07): reuse the existing `session_events.rs` bus** (cleanest — no duplicated reconnect/snapshot plumbing). Add a non-breaking variant to `SessionEvent`:
  ```rust
  ActiveChanged {
      active_project_ids: Vec<String>,   // full set — simplest correct mirror, cheap at our scale
      active_window_hours: u32,
      // optional: changed: String, is_active: bool, activated_by: Option<String>
  }
  ```
  Emitting the **full set** (not just a diff) makes client convergence trivial and races impossible (last-write-wins on a monotonic snapshot). At our workspace counts this is negligible.
- **Emit sites:** after any recompute (activate / pin / dismiss / window-tick that changes membership / reap-close).
- **Client subscription:** a single **app-level** subscriber (not per-workspace) opened at boot, host-aware (re-subscribe on `onActiveHostChange`), with a `Hello` snapshot on connect. This differs from `subscribeToWorkspaceSessionEvents` (which is per-workspace); add an app-level `subscribeToActiveState(handlers)` alongside it on the same WS endpoint, filtering for `ActiveChanged`.

### 4.5 Renderer becomes a pure consumer
- **Delete** `sweepAgedOutWorkspaceChats`, `sweepAgedOutWorkspaceChatsFromDaemon`, `scheduleWorkspaceChatReap`, the 0.39.38 subscriberCount gate, and the local Active derivation (`isWithinActiveWindow`, hard-coded window in `ActiveBar.tsx`).
- **Active bar** renders from a new `useActiveStore` populated by the snapshot + `ActiveChanged` deltas.
- **Gestures** call the daemon: `setActiveProject(id)` → `POST projects/activate`; pin toggle → `projects/pin`; remove-from-Active → `projects/dismiss`. Local Active state updates **only** when the `ActiveChanged` delta returns (daemon is the truth). Optimistic local echo is allowed for snappiness but reconciles to the daemon snapshot.
- **Host switch:** unchanged pattern (#625) — on `onActiveHostChange`, clear + re-fetch `GET projects/active` from the new host and re-subscribe the active-state WS.

---

## 5. Multi-user semantics
- **Union, global per daemon.** No per-user Active map. If user A activates X, the daemon's canonical set includes X and the `ActiveChanged` delta reaches B, whose bar now shows X. This is exactly Rosson's "shared server, 1:1 mirror."
- **Reaping is server-global:** a workspace stays Active (and unreaped) as long as *anyone* has it within-window, pinned, heartbeat-warm, or attached.
- **Attribution (optional):** include `activated_by` for an "opened by <user> / autonomous" indicator; display-only, never a filter.

---

## 6. Persistence & restart
- Active set: **derived**, not stored — reconstructed on boot from `last_interaction_at` + `manually_active` + `active_window_hours`. ✅ survives restart.
- `active_window_hours`: already in `app_settings.json`. ✅
- Reap timers: in-memory; **rescheduled on boot** by the reconciliation pass (§4.3).

---

## 7. `set_active` storm avoidance
- New `activate`/`pin`/`dismiss` calls are **coarse** (per user navigation / explicit gesture), not per-frame/per-focus. They are **not** wired to `phase.kind` or grid focus.
- Renderer **dedups**: only POST `activate` when the foreground workspace actually changes (mirror the `lastSentActiveRef` pattern from `TerminalPane.tsx`).
- Daemon **recompute is idempotent**: emitting the full set means redundant activates produce identical snapshots (no amplification).
- Grid-pane `decide_set_active` is untouched.

---

## 8. Capability gating / back-compat
- Add `FEATURES['canonical-active'] = '0.39.38'` (server-capabilities.ts). 
- **New client ↔ old daemon:** if the daemon lacks `canonical-active`, the renderer falls back to today's local derivation + (renderer) reap. (Keep the old code path behind the gate for one release, then delete.)
- **Old client ↔ new daemon:** the daemon reaper runs regardless (server-owned); old clients still derive their own bar but the daemon is now the authority on reaping — safe, because the daemon's gates are a superset (it won't reap attached/within-window/pinned/heartbeat sessions). Old clients simply won't get the live mirror.
- All new mutating routes POST-only with `if !is_post { 405 }` guards.

---

## 9. Risks & mitigations
1. **Renderer acts before daemon ACK** (schedules UI off stale Active). → Renderer never reaps now; it only reflects daemon deltas. Optimistic echo reconciles to snapshot.
2. **Multi-client delta ordering.** → Emit the **full set** snapshot, not diffs; last-write-wins, order-independent.
3. **Boot stale-read.** → `GET projects/active` immediately followed by delta subscription; first delta corrects any drift.
4. **Window changed mid-session** (24→12h). → Daemon recomputes + emits; bar updates; any now-aged chat enters the normal 15s grace. Documented as immediate-effect.
5. **Reaper kills a session during a brief disconnect** (client reconnecting). → Covered by **Active membership**, not attach state: a recently-opened workspace is within-window ⇒ Active ⇒ unreapable regardless of the tunnel blip. The 15s grace + fire-time re-check absorb the rest. (This is why the open/attach⇒activate invariant in §4.3.1 is load-bearing.)
6. **Daemon task leak / double-arm.** → Single reconciliation source; timers keyed by projectId in a map; re-arm replaces.

---

## 10. Test plan
- **Daemon unit:** Active-set compute (window boundary, pin, heartbeat); reaper gate truth table (`!active && !heartbeat && subs==0`); grace arm/cancel/fire; idempotent recompute.
- **Daemon integration** (spawn-backed, like `reaper_close_guard_integration.rs`): two simulated subscribers; activate on "client A" → snapshot shows it; reaper does NOT close while attached; detach + age-out → grace → close; `dismiss` → grace → close; cancel-on-reactivate.
- **Renderer:** `useActiveStore` applies snapshot + deltas; host-switch re-subscribes; activate gesture POSTs once (dedup); no local reap timers remain.
- **E2E (manual, post-release — the part that can't run pre-ship):** remote client opens a dormant workspace → host shows it Active → session survives well past 15s; second client sees it appear in its Active bar.

---

## 11. Phasing (single 0.39.38 release)
- **P0 — Daemon Active set + routes:** compute fn, `GET projects/active`, `POST projects/activate|pin|dismiss`. (k2so-core compute + db_routes/dispatcher.)
- **P1 — `ActiveChanged` broadcast:** add variant, emit on recompute, app-level renderer subscriber + `useActiveStore`.
- **P2 — Daemon reaper:** the grace-reap task (gates incl. subscriberCount), boot reconciliation, `--resume` persistence; **delete** renderer sweeps/timers.
- **P3 — Renderer consume + gate:** Active bar from store; gestures → routes; capability gate + old-path fallback; remove local derivation.
- **P4 — Tests + cleanup:** the §10 suite; delete dead renderer code behind the gate; update WHATS_NEW (#22 line now describes the canonical fix).

Each phase is a worktree subagent, file-disjoint where possible (daemon P0/P1/P2 in k2so-core+k2so-daemon; renderer P1-sub/P3 in stores+components), cherry-picked onto main — same pattern that landed the rest of 0.39.38.

---

## 12. What changes for the 0.39.38 commits already on main
- **Keep:** real `subscriberCount` (`b07f4bd`) — retained as **correct data** (UI/telemetry), **not** a reap input (Active-only decision).
- **Keep:** `v2/close` close-guard — retained as **non-load-bearing** defense against stray/non-reaper closes. The reaper bypasses it with `force: true` (it is Active-authoritative), so the guard never blocks an Active-driven reap.
- **Remove/replace (P2/P3):** the renderer `sweepAgedOutWorkspaceChatsFromDaemon` subscriberCount gate + the renderer reap ownership (`ea4e39b`'s tabs.ts reap pieces) + the local Active derivation. The reap + Active both move to the daemon.
- **WHATS_NEW:** keep the user-facing #22 line ("remote chat sessions no longer die"); the mechanism behind it is now canonical Active.

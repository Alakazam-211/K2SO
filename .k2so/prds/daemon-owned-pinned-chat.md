# PRD: Daemon-Owned Pinned-Chat Session Lifecycle

**Status:** DRAFT for review — 2026-06-07 (Rosson pulled into 0.39.38)
**Task:** #683 · part of the #673 daemon-canonical backlog
**Supersedes/absorbs (renderer):** the #681 cold-boot reconcile, #679 dropdown/cold-boot revive orchestration, and #682 self-retrigger guard + circuit breaker — these become unnecessary once the daemon owns spawn (the loop class disappears by construction).
**Builds on:** canonical-Active (#672, `.k2so/prds/daemon-canonical-active.md`), session-events broadcast, `resolve_resume_chat_args`.

---

## 1. Problem

The pinned **Chat** tab orchestrates its own session lifecycle in the renderer: a 4-step `resolve()` `useEffect` in `AgentChatPane.tsx` (~354-574) decides whether to resume/spawn/reattach, builds `claude` args, spawns via `TerminalPane`, then **stamps the resulting session id back into the layout** — which changes a `useEffect` dependency (`restoredSessionId`) and re-runs the resolver. That self-feeding loop, combined with a child that exits immediately (e.g. `--session-id <dup>` → "already in use" → exit 1), produced an unbounded spawn→exit→respawn loop (#682). We patched it with a self-retrigger guard + circuit breaker, but those are band-aids: **the root smell is that the renderer owns session lifecycle at all.** It's loop-prone, races across windows, and is meaningless on a headless daemon.

This is the same lesson as canonical-Active and the #673 audit: **the daemon owns canonical state and the loops that act on it; the client renders the daemon's truth and sends gestures.**

## 2. Goal / Non-Goals

### Goal
The **daemon owns the pinned-chat session** — identity (which Claude session), find-or-spawn, resume/fresh decision, refresh, and dropdown-switch respawn. The renderer **asks the daemon to ensure the session, then attaches the grid-WS and renders.** No renderer-side spawn orchestration, no self-retrigger, no circuit breaker needed.

### Non-Goals
- Multi-subscriber grid-WS (two clients streaming the *same* chat simultaneously) — keep single-subscriber for 0.39.38; it's a clean follow-up.
- Changing heartbeat/agent spawn (daemon-initiated, autonomous — already correct).
- Changing the Claude args contract (`resolve_resume_chat_args` is already correct, incl. `--session-id` for never-chatted).

## 3. Current ownership (verified — file:line)

**Renderer (orchestrates today):** `AgentChatPane.tsx` `resolve()` Steps 0–3 (~354-574): Step 0 cold-boot reconcile (#681), Step 1/1b reattach, Step 2 `resumeChatArgs`, Step 3 fresh; `stampSessionId` (~381) feeds the loop; `handleRefresh` (~211) kill+remount; `switchToSession` (~319) dropdown; `chat-spawn-breaker.ts` (#682 band-aid); `TerminalPane` opens the grid-WS + reports `child_exit`.

**Daemon (already owns):** `resolve_resume_chat_args` (`k2so-core/workspace/resume_chat.rs:69-134`) — SQLite-canonical session + resume/fresh decision + `resumedExisting`. `handle_v2_spawn` (`v2_spawn.rs`) — **idempotent find-or-spawn** (reused=true). `v2_session_map` register/unregister + lookup. `workspace_sessions` table (canonical `session_id`, `active_terminal_id`, `last_activity_at`). `session_events.rs` broadcast (`SessionAdded`/`SessionRemoved`/`ActiveChanged`). `set-chat-session` route (dropdown persistence).

**The gap:** there is no single idempotent daemon entrypoint that *both* resolves the canonical session *and* spawns/attaches it and broadcasts the result. The renderer stitches `resume-chat-args` + `v2/spawn` + `stamp` together — that stitching is the lifecycle ownership we're moving.

## 4. Target design

```
mount:    renderer ──ensure(project)──▶ daemon: resolve canonical session → find-or-spawn (idempotent)
                                                 → stamp active_terminal_id, last_activity_at
                     ◀── {sessionId, claudeSessionId, resumedExisting, cols, rows, reused} ──
          renderer opens grid-WS to sessionId, renders. (NO args, NO resume decision in renderer.)

refresh:  renderer ──ensure(project, forceRespawn:true)──▶ daemon kills + respawns → broadcast → renderer re-attaches
switch:   renderer ──set-chat-session(project, newId)──▶ then ensure(forceRespawn) → daemon respawns on the new id
exit:     daemon observes child exit → unregister → broadcast SessionRemoved (renderer shows idle, no auto-respawn)
relaunch: renderer ──ensure──▶ daemon reads workspace_sessions (canonical) → resume real / fresh otherwise
```

### 4.1 Daemon entrypoint (the core addition)
`POST /cli/workspace/ensure-pinned-chat` — idempotent "ensure workspace W's pinned chat is alive."
- **Request:** `{ project: <path>, forceRespawn?: bool }`
- **Behavior:** call `resolve_resume_chat_args(project)` → internally `v2/spawn` with those args under the canonical key (`agent_name = projectId`); if a live session exists and `!forceRespawn` → return it (`reused: true`); if `forceRespawn` → unregister + spawn fresh. Stamp `active_terminal_id` + bump `last_activity_at`. Emit `SessionAdded` (or Removed+Added on respawn).
- **Response:** `{ sessionId, claudeSessionId, resumedExisting, command, args, cols, rows, reused }`
- Owner/connect-user auth; POST-only guard; host-aware via daemonCli.
- (Refresh + switch both ride `forceRespawn:true`; no separate refresh route needed — **decision D1 below**.)

### 4.2 Broadcast
Reuse `SessionEvent` over `/cli/sessions/events` (already consumed): renderer listens for `SessionAdded`/`SessionRemoved` keyed to its workspace → opens/closes the grid-WS. No new event type required.

### 4.3 Renderer's reduced role
On mount: `ensure-pinned-chat` → open `TerminalPane` at the returned `sessionId`. On unmount: close grid-WS (PTY survives on daemon). Refresh button → `ensure(forceRespawn)`. Dropdown switch → `set-chat-session` then `ensure(forceRespawn)`. React to `SessionAdded/Removed` to re-attach.
**Deleted:** `resolve()` Steps 0–3, `stampSessionId` (agent items), `switchToSession`/`handleRefresh` orchestration internals, the `restoredSessionId`-driven re-resolve, `refreshNonce`-respawn, and `chat-spawn-breaker.ts` (**decision D2**).

### 4.4 Headless correctness
The daemon never auto-spawns a pinned chat (it's user-initiated) — it spawns only when a client calls `ensure`. Contrast heartbeats (daemon-scheduled). Correct for a no-client Linux daemon: no orphan chats.

### 4.5 Preserves the behaviors we just shipped
- **SQLite-canonical session** (#679): `ensure` resolves from `workspace_sessions.session_id`; the layout `restoredSessionId` becomes a non-authoritative hint (or is dropped entirely — **decision D3**).
- **No phantom --resume** (#681): unchanged — `resolve_resume_chat_args` already returns `--session-id` for never-chatted (`resumedExisting:false`).
- **Dropdown switch + reload + relaunch-revive**: all preserved, now daemon-driven.

## 5. `k2so msg` interaction
`k2so msg <workspace> "live chat"` injects into the workspace's live pinned-chat PTY. Daemon ownership makes this *more* robust (the daemon owns the PTY + can `ensure` it). The parallel `k2so msg` review (in flight) will confirm current behavior and whether `ensure` should be the inject path's find-or-spawn. **Fold its findings into Phase 1.**

## 6. Migration / back-compat
- **Capability gate** `FEATURES['daemon-pinned-chat'] = '0.39.38'`. New client + old daemon → fall back to today's renderer path (kept behind the gate for one release). New daemon + old client → old client keeps calling `resume-chat-args` + `v2/spawn` (still works).
- `workspace_sessions.session_id` is authoritative; the renderer layout hint is only a hint → no history loss across the upgrade.
- The `--session-id already in use` race is gone: the daemon allocates + registers atomically inside `ensure`.

## 7. Risks
1. **Session identity drift during gate flip** — mitigated: daemon always consults `workspace_sessions` first; layout hint never wins.
2. **Concurrent `ensure` from two windows** — find-or-spawn is idempotent (one PTY); grid-WS stays single-subscriber for 0.39.38 (second window rejected, as today).
3. **Reworking freshly-shipped code** (#679/#681/#682) — net simplification, but must keep their *behaviors* (tests carry over). Land behind the gate; bake before deleting the old path.
4. **Scope vs release** — this is a real refactor (~3–5 days). It's the last thing gating 0.39.38.

## 8. Test plan
- **Daemon:** `ensure` idempotency (cold/relaunch/forceRespawn); resume-real vs fresh; emits the right events; `--session-id` atomic (no dup-in-use).
- **Renderer:** mount→ensure→attach; refresh→forceRespawn→re-attach; dropdown switch; SessionRemoved→idle (no auto-respawn); capability-gate fallback.
- **E2E (manual):** brand-new workspace first chat (fresh, no loop), dropdown switch, refresh, relaunch-revive, `k2so msg` into a live chat.

## 9. Phasing (single 0.39.38 inclusion, gated)
- **P1 — Daemon `ensure-pinned-chat`** (k2so-core reuse + workspace_routes + dispatcher + events) + unit/integration tests. Fold in `k2so msg` findings.
- **P2 — Renderer cutover behind the capability gate:** `ensure`-on-mount + attach; delete resolve orchestration; refresh/switch → daemon RPCs; keep old path under the gate.
- **P3 — Remove the band-aids** once baked: delete `chat-spawn-breaker.ts` + self-retrigger guard + stamp-back (per D2).
- **P4 — Tests + cleanup + WHATS_NEW.**
Worktree subagents, file-disjoint (daemon P1 vs renderer P2), cherry-picked — same pattern as the rest of 0.39.38.

## 10. Open decisions (for review)
- **D1:** One endpoint (`ensure` + `forceRespawn`) vs separate `ensure` + `refresh`. *Recommend: one.*
- **D2:** Delete the circuit breaker now (loop impossible once daemon-owned) vs keep it dormant one release as defense. *Recommend: keep it through 0.39.38, delete in 0.39.39 after bake.*
- **D3:** Drop the renderer layout `restoredSessionId` hint entirely (daemon is sole source) vs keep it as an offline fallback. *Recommend: keep as hint-only for offline resilience, daemon authoritative when reachable.*
- **D4:** Is single-subscriber acceptable for 0.39.38 (multi-client same-chat deferred)? *Recommend: yes, defer multi-subscriber.*
- **D5:** Given the ~3–5 day scope, confirm this gates 0.39.38 (vs shipping 0.39.38 now with the #682 band-aids and doing this as 0.39.39). *Your call.*

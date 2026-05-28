# K2SO 0.39.8 — WS resilience: set_active dedup (#3) + reconnect on drop (#5)

Closes two distinct long-running-session WebSocket bugs filed by
external users with deep diagnostic profiles:

- **#3 — Renderer `set_active` hot-loop overruns grid-WS broadcast**:
  `subscriber lagged 3409 events, sending fresh snapshot` in
  the daemon log. Renderer emitted `set_active` in a tight loop
  driven by `phase.kind` churn + an unguarded focus-store
  subscriber; daemon's broadcast channel couldn't drain.
- **#5 — WS connections don't reconnect after mid-flight drop**:
  `TerminalPane.tsx`'s grid-WS `onclose` was a no-op, leaving the
  terminal permanently silent after any TCP reset / WebKit
  Networking quirk / App Nap event. `session-events.ts` had a
  parallel hole where `onerror` was swallowed on the assumption
  that `onclose` would follow (which WebKit Networking under
  throttling can skip).

Both reports verified against the actual code — diagnoses were
exact. Fix shapes match the reporters' suggestions.

## Context the user provided that locked in the design

The `set_active` mechanism is **a feature**, not noise: it's the
multi-viewer handshake that tells the daemon WHICH connected
client is the live viewer of a shared PTY session — desktop +
mobile sharing one PTY, or split panes within K2SO. The daemon
uses it to size the grid and route focus events correctly.

So the fix must **preserve the feature, not gut it**. Especially
in the single-viewer case (the dominant desktop scenario), the
WS should be **silent**: one initial claim when the WS opens,
then nothing until window focus genuinely changes.

The send-level dedup (below) is what makes single-viewer silence
robust regardless of how often upstream re-renders the effect.

## Changes

### `src/renderer/terminal-v2/TerminalPane.tsx`

**Issue #3 — `set_active` thrash:**

1. **Send-level dedup**: new `lastSentActiveRef` short-circuits
   `sendSetActive(active)` when `lastSentActiveRef.current ===
   active`. Closes #3 even if upstream (`isFocused`, `phase.kind`)
   ever flaps again.
2. **Drop `phase.kind` from the set_active effect's dep array**:
   the effect body doesn't read `phase.kind` (it reads
   `wsRef.current` + the focus store), so the dep was load-
   bearing for nothing and amplified the thrash by re-running
   the effect on every phase transition (mount → connecting →
   ready → exited → error, …). New deps: `[]`.
3. **Symmetric cleanup**: on effect unmount, emit
   `sendSetActive(false)` IF we previously claimed (`lastSentActiveRef.current === true`). Daemon's
   `active_subscriber` tracking now stays consistent with the
   pane's lifecycle.
4. **Re-prime on each WS (re)connect**: in `boot()` right after
   `wsRef.current = ws`, reset `lastSentActiveRef.current = null`
   and emit the current `isFocused` state as a fresh initial
   claim. Necessary because the daemon-side subscriber on the new
   WS is fresh and has no notion that we were previously active.

**Issue #5 — WS reconnect on mid-flight drop:**

5. **Real `ws.onclose` reconnect path**: schedule a re-boot with
   exponential backoff (500 ms → 5 s capped). Bumps
   `reconnectAttempt` state which is now in the boot effect's
   dep array, tearing the effect down + re-running it (fresh
   `/cli/sessions/v2/spawn` — idempotent on `agent_name`,
   returns the same `sessionId` for a still-alive session —
   then fresh WS handshake).
6. **Phase signaling**: on `onclose`, set `phase → 'connecting'`
   so the UI shows recovery in progress instead of a stuck
   `ready` with a dead WS underneath.
7. **Skip reconnect on real child exit**: if `phase.kind ===
   'exited'`, the child process actually exited and the user
   closed the terminal — don't try to reconnect.
8. **Coalesce reconnect timers**: if a timer is already pending,
   don't double-schedule. Cleanup clears any pending timer on
   effect teardown.

### `src/renderer/stores/session-events.ts`

**Issue #5 — `onerror` no-op:**

9. **New `triggerReconnect()` helper** — idempotent (early-out
   if `reconnectTimer !== null` is already pending), safe to
   call from any handler. Called from BOTH `ws.onerror` AND
   `ws.onclose` so the WebKit-throttled "onerror-without-onclose"
   path no longer leaves the subscriber silently dead.

## Tested

- **Renderer typecheck** baseline drifted 47 → 49; both new
  errors are pre-existing patterns hit on new lines
  (`Property 'env' does not exist on type 'ImportMeta'` from
  the tsconfig + `Property 'sessionId' does not exist on type
  'never'` from a pre-existing `spawn` type-narrowing bug).
  **No new categories of errors.** Zero errors in
  `session-events.ts`.
- **Renderer vitest**: 4 files / 42 tests pass, identical to
  pre-change baseline (11 pre-existing unhandled errors in
  `tabs.test.ts` are unchanged + unrelated to this work).
- **k2so-daemon**: 193 tests pass, 0 failed (no Rust changes
  in this release; sanity check only).
- **k2so-core**: 661 tests pass, 0 failed (unchanged; sanity).
- **Live curl smoke** from 0.39.7 still verifies HTTP keep-alive
  works (no behavior change there).

## Runtime verification reviewers should perform

Reporter's acceptance criteria, all of which the architectural
fix directly addresses:

- **#3**: `grep -c 'sessions_grid_ws.*lagged' ~/.k2so/daemon.stderr.log`
  stays at 0 across a multi-hour session, and
  `stage=active_(claim|release)` events scale with **actual
  user-visible focus transitions** (single-viewer scenario:
  near-zero traffic).
- **#5**: An intentionally-killed `sessions/grid` or
  `sessions/events` WebSocket from a test harness results in the
  renderer re-establishing the connection within ~5 s; a multi-
  hour session never lands in the "1 ESTABLISHED, terminals
  frozen" state. To force-induce for testing: from a Rust
  integration test, open a WS to `/cli/sessions/grid?session=…&token=…`,
  hold briefly, then `shutdown(2)` the TCP socket without
  sending a WS Close frame. The daemon will log `Connection
  reset without closing handshake`; verify the renderer
  reconnects.

## Out of scope (filed for 0.40.x)

- The reporter's **App Nap opt-out** suggestion for #5 — even
  with reconnect working, App Nap can still cause the drops in
  the first place. Worth doing as a follow-up; out of scope here.
- The reporter's optional **daemon grid-broadcast capacity bump**
  for #3 — defensive backstop now that the renderer-side fix is
  in. Re-evaluate after seeing #3's fix in the wild.

## Upgrade notes

- Any 0.39.x → 0.39.8: clean update. No migrations.
- Users on 0.38.x → 0.39.8: full 0.39.0 + 0.39.1 migration
  sequence fires on first boot, gated behind the 0.39.5
  `/boot-status` readiness handshake.

## Credit

Two external users filed Issues #3 and #5 with complete diagnostic
profiles, file/line references, suggested fixes, and acceptance
criteria. Both diagnoses were exact — code matched the reports
verbatim. Methodology to emulate for future renderer-perf bugs.

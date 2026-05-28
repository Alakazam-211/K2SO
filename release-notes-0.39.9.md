# K2SO 0.39.9 — Hotfix: closure-stale `phase` in `ws.onclose` resurrected exited terminals

Same-day hotfix for a regression introduced in 0.39.8 (Issue #5 fix).

## What 0.39.8 broke

0.39.8's new WS-reconnect path in `TerminalPane.tsx`'s `ws.onclose`
included an early-exit guard so that a real `child_exit` wouldn't
trigger an unwanted reconnect:

```tsx
ws.onclose = (ev) => {
  if (cancelled) return
  if (phase.kind === 'exited') return   // ← closure-captured `phase`
  // … schedule reconnect …
}
```

**The bug**: `phase` is captured by closure when the boot effect runs.
The `child_exit` ws-message handler updates phase via `setPhase`,
which queues a state update but does NOT mutate the closure's
captured binding. When the daemon closes the WS in the same JS task
after sending `child_exit` (its normal teardown order),
`ws.onclose` fires with the **stale** `phase` value (`'connecting'`
or `'ready'`), the guard evaluates false, and the renderer
**schedules a reconnect for a terminal that just exited**.

The reconnect bumps `reconnectAttempt`, the boot effect re-runs,
`/cli/sessions/v2/spawn` is called again. Since `spawn` is
idempotent on `agent_name` AND the previous session's PTY is gone
(child exited), the daemon **creates a NEW session** with the same
agent_name. The renderer attaches to it. User sees a brand-new
fresh shell pop up in place of the terminal that had just finished
running — visually confusing and wrong.

Pre-0.39.8 this couldn't happen because `ws.onclose` was a no-op
(the bug 0.39.8 fixed). So 0.39.9 is a true regression hotfix.

## The fix

Two surgical changes in `src/renderer/terminal-v2/TerminalPane.tsx`:

1. **New `phaseRef`** mirrors `phase` via a tiny `useEffect`.
   `ws.onclose` now reads `phaseRef.current.kind === 'exited'`
   instead of the closure-captured `phase.kind`.
2. **Synchronous `phaseRef.current = next` update in the
   `child_exit` ws-message handler**, *before* calling `setPhase`.
   Necessary because the React state update + useEffect-driven
   ref sync both run AFTER the current JS task — and the daemon
   typically closes the WS in the SAME task as the `child_exit`
   message. Writing the ref synchronously here means
   `ws.onclose` (firing later in the same task) sees the new
   value.

The combination guarantees: after a `child_exit`, no reconnect
gets scheduled regardless of how quickly the daemon's WS teardown
follows.

## Why I shipped the regression in the first place

Honest debrief: I caught this on a self-initiated re-read of the
diff while 0.39.8 was already mid-release. The release script
was at the final `gh release create` upload step — cancelling
would have orphaned partial assets. Better to let 0.39.8 ship
and immediately roll a hotfix than risk a broken release.

The lesson: closure-captured state in callbacks bound inside
effects needs a ref-based escape hatch when the callback can fire
after state changes that the effect didn't re-run on. This
applies generally to any `ws.onclose`, `setTimeout` callback, or
event listener inside `useEffect`.

I also ran `cargo clippy -p k2so-daemon` after 0.39.8 shipped
(should have been part of the 0.39.7 + 0.39.8 baseline checks);
zero new warnings on the 0.39.7 keep-alive + dispatcher refactor.
Adding this to the pre-release checklist going forward.

## Tested

- Renderer typecheck: 49 errors total (unchanged from 0.39.8;
  patch added zero new errors).
- Vitest: 42/42 pass, 11 pre-existing unhandled errors in
  `tabs.test.ts` (unchanged baseline).
- `bun run build`: succeeds.
- `cargo clippy -p k2so-daemon --bin k2so-daemon`: zero new
  warnings on the 0.39.7 changes.
- k2so-daemon tests: 193 pass (no Rust changes in this hotfix).
- k2so-core tests: 661 pass (no Rust changes in this hotfix).

## Manual reproduction (pre-fix) / verification (post-fix)

Reproducing the regression on 0.39.8: open a terminal, run a
short-lived command that exits (e.g. `ls && exit`). On 0.39.8,
the closed terminal may pop back open as a fresh shell. On
0.39.9, it stays closed.

## Upgrade notes

- Anyone on 0.39.8: clean update.
- Anyone on 0.39.x older: full migration sequence still gated by
  0.39.5's `/boot-status` readiness handshake.

## What didn't change

- The intended 0.39.8 behavior — reconnect on real mid-flight WS
  drops — works the same. Only the false-positive "reconnect
  after legitimate child_exit" path got fixed.

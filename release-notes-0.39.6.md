# K2SO 0.39.6 — Active-agents polling: terminal-stall storm fixed

Patch release. Closes a renderer-side request-storm that stalled every
open terminal session simultaneously (~15 s freeze, then automatic
recovery) on boxes with many open agent terminals.

## What was happening

The renderer's "Active agents" store (`src/renderer/stores/active-agents.ts`)
polled every running terminal every ~2.5 s with one
`GET /cli/terminal/foreground-cmd` request **per terminal**, via
`Promise.all`. On a workstation with many open agent terminals that
meant a periodic fan-out of N HTTP requests through the WebView's
network stack — enough to spike renderer CPU to 80–128 % and stall
every active terminal session until the storm cleared.

The same `pollOnce` also called `set({ agents: newAgents })`
**unconditionally** with a fresh `Map` every cycle, forcing every
subscriber (sidebar Active section, IconRail, …) to re-render even
when nothing about the agents had changed. The constant re-render
churn compounded the network storm.

## Diagnosis

Submitted by an external contributor who profiled this on a live
0.39.5 install:

- **Live CPU during a stall:** renderer (WebView) 80–128 % CPU, daemon
  0.0 % (idle), the daemon's terminal poll-loop counter frozen — all
  terminals stalled together, recovery sudden. → renderer-side.
- **`sample` of the renderer during the spike:** on-CPU work dominated
  by WebKit *networking* IPC — `IPC::Decoder::decode<ResourceRequest>`,
  `ArgumentCoder<ResourceRequest>::decode`, `WTF::URLParser::parse`,
  and a repeating `RESOURCELOADER_WILLSENDREQUESTINTERNAL →
  RESOURCELOAD_FINISHED` cycle. → HTTP request flood from the
  WebView.
- **`sample` of the daemon:** every thread parked (`__psynch_cvwait`,
  `kevent`, `__accept`). → confirmed idle, downstream victim of
  backpressure.
- **`/cli/terminal/list-running`** confirmed to return `[]` on a box
  running v2/alacritty agent panes — so the old per-terminal
  `foreground-cmd` calls were all misses anyway (pure waste).

## The fix

Two surgical changes in `src/renderer/stores/active-agents.ts`,
+73 / -26 LOC:

1. **N → 1 HTTP requests per poll**: replace the per-terminal
   `Promise.all(foreground-cmd)` with a single
   `terminalListRunning()` call and look each terminal's foreground
   command up in the result. The daemon-side handler
   `handle_list_running` calls the **same** `get_foreground_command`
   per terminal as `handle_foreground_cmd`, just under a single lock
   acquisition — so this is behaviour-preserving from the renderer's
   point of view AND wins a daemon-side lock-contention reduction.

2. **No spurious re-renders**: add `agentMapsEqual(a, b)` and only call
   `set({ agents })` when the polled set has actually changed
   (structural-equality over `terminalId → {command, status,
   hookStatus, tabId, tabTitle, groupIndex}`). Mirrors the existing
   `paneStatuses` guard. Three-branch `set()` now correctly avoids
   calling `set()` when neither map changed.

Daemon failure-mode change (improvement): the old per-terminal
`try/catch` swallowed individual failures and continued with partial
state. The new code's `try { running = await terminalListRunning() }`
skips the whole poll cycle on daemon transients — avoids partial
state instead of thrashing with per-terminal retries.

## Verification

- Renderer typecheck: pre-existing baseline (errors only in
  `TerminalPane.tsx`); **none in `active-agents.ts`**, before or
  after.
- Vitest: 4 files / 42 tests passing, identical to pre-merge baseline
  (the 11 unhandled errors in `tabs.test.ts` are pre-existing and
  unrelated).
- Daemon handler equivalence verified by reading
  `terminal_lifecycle_routes.rs` — both `handle_foreground_cmd` and
  `handle_list_running` route to `manager.get_foreground_command(id)`.

## Runtime check reviewers should perform

- With the WebView Network inspector open, the per-2.5 s request
  fan-out drops to a single `list-running` call.
- The sidebar Active section / IconRail still light up correctly for
  legacy-terminal agents.
- The intermittent terminal-stall storm no longer reproduces under
  many open terminals.

## Credit

Submitted as PR #1 by an external contributor who shipped the
diagnosis along with the fix — profiling methodology was exactly the
kind we should be running for future renderer perf issues
(`sample` the renderer process during a stall; cross-check daemon idle
to confirm direction of causality).

## Out-of-scope follow-ups (surfaced by the same investigation)

Worth filing separately:

- Daemon `[perf] terminal_poll_tick` debug logs firing ~3×/sec →
  `daemon.stderr.log` grows unbounded (6.7 MB observed). Should be
  gated behind a debug flag.
- Phantom `nobody` pending-live signals re-queued every boot, never
  drained.
- If grid-WS resubscribe churn persists after this, that's a separate
  renderer-effect-deps issue to chase with the Network inspector.

## Upgrade notes

- Users on any 0.39.x → 0.39.6: clean update. No migrations.
- Users on 0.38.x → 0.39.6: full 0.39.0 + 0.39.1 migration sequence
  fires on first boot, gated behind the 0.39.5 `/boot-status`
  readiness handshake so the renderer correctly waits for the new
  daemon.

## What else shipped in this release

Nothing else. See `release-notes-0.39.0.md` through
`release-notes-0.39.5.md` for prior content.

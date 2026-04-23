# Kessel Resize Architecture — Context-Survival Notes

**Purpose:** mid-session notes captured before a `/compact` so the
next session can resume with full context on what we're fighting
with, what we've committed, and what the agreed-on fix is.

## Architecture summary (confirmed)

Kessel panes in 0.34.3+ are **local mini-alacritty Terms running
inside the Tauri process**, one per Kessel tab. Each Term subscribes
directly to the daemon's **byte stream** at
`/cli/sessions/bytes?session=<uuid>&from=<offset>` and feeds the
raw PTY bytes through vte::Processor → alacritty_terminal::Term.

**Key point:** Kessel panes do NOT consume LineMux Frame events.
LineMux is the daemon's parallel parser that produces semantic
Frame events for thin clients (mobile companion, web) that can't
afford to host a local terminal emulator. Kessel uses the byte
stream path exclusively.

Layers:
1. **Daemon PTY** — kernel resource, sized via ioctl. What Claude
   sees.
2. **Daemon byte stream** (`/cli/sessions/bytes`) — ordered bytes
   from the PTY + in-band APC markers (currently just
   `k2so:grow_boundary`).
3. **Tauri alacritty Term** (per Kessel pane) — sized via
   `term.resize()`. Reflows its own buffer at its own cols/rows.
4. **Frontend React component** (`SessionStreamViewTerm`) — reads
   snapshot/delta events from Tauri, renders DOM.

Reflow happens at **layer 3**, locally per pane, using alacritty's
native `term.resize()`.

## Recent commits (in order, newest at top)

| SHA | Subject | Status |
|---|---|---|
| cbb8a30f | wipe live grid on resize to prevent stacked-paint corruption | **⚠ CAUSES black-on-workspace-return; REVERT** |
| 5c0fc2a6 | don't close daemon session on component unmount | ✅ load-bearing; keep |
| a239ff8a | paint on first mount — pass initial visibility to attach | ✅ |
| 3397ca7a | Phase A — non-blocking spawn (grow-settle off hot path) | ✅ |
| 8524f57b | docs(prd): kessel-instant-everywhere.md | ✅ |
| 465c1a6e | Phase 9 — idempotent spawn, reattach on remount | ✅ load-bearing |
| 37e64d45 | Phase 8 — perf instrumentation | ✅ (remove before release) |
| fa8d0ac6 | Phase 7 — delta snapshots + hidden-pane pause | ✅ |
| cc48e798 | Phase 6 — SGR color + attrs in Term snapshots | ✅ |
| 699aaec1 | Kessel renderer IS the Canvas Plan upgrade (no 3rd option) | ✅ |
| 26acf994 | Phase 5 — SessionStreamViewTerm | ✅ |
| 99f6f1aa | Phases 3+4 — APC marker injection + Tauri-side Term | ✅ |
| 168103aa | Phase 2 — raw byte stream + archive + WS endpoint | ✅ |
| 02879118 | Phase 1 — sealGrowPhase | ✅ |
| 4aa561a7 | docs(prd): canvas-plan.md | ✅ |
| 651f1e49 | daemon-side grow-then-shrink with grow_boundary marker | ✅ |

0.34.2 shipped + tagged. Everything since is post-0.34.2 work.

## The regression trail

We're chasing paint-race bugs introduced while trying to solve
screen-size reflow. Sequence:

1. After Phase 9 + no-unmount-close: workspace-switch retention
   works (Claude sessions survive workspace navigation). ✅
2. BUT: on mid-session window resize, user reported "stacked paint"
   corruption — a second Claude UI appearing below the first,
   plus cell-level letter interleaving in scrollback content.
   Cause: alacritty's `term.resize()` pulls rows from scrollback
   back into the live grid when growing rows. Content from an
   earlier (narrower) paint survives in those pulled rows. Claude's
   SIGWINCH-repaint at the new width lands on top, producing
   stacked banners + interleaved letters.
3. I added a `CSI 2J + CSI H` wipe in `PaneState::resize` (commit
   cbb8a30f) to clear the live grid after resize, making Claude's
   repaint land on clean cells.
4. This fixed the stacked paint BUT introduced a new bug: on
   workspace return, a new Tauri Term is created fresh and starts
   replaying bytes from offset 0. The ResizeObserver fires with
   container dims != Term dims (because Term was just created at
   defaults). My wipe fires → live grid goes blank → Claude is
   idle, no repaint arrives → user stares at black terminal.

So: wipe = black on workspace return. No wipe = stacked paint on
drag resize. We need a different approach.

## Agreed-on proper fix (Task #352)

Three steps, in order:

### Step 1: Revert the wipe hack (immediate unfuck)
Revert the wipe in `PaneState::resize` from commit cbb8a30f.
Restores the "stacked paint on drag-resize" bug (cosmetic) but
fixes the black-on-workspace-return regression (blocking).

### Step 2: Measure-before-attach (kill the initial-resize storm)
In `SessionStreamViewTerm`, measure the container dims BEFORE
`kessel_term_attach` fires. Use `useLayoutEffect` after
cellMetrics is ready to compute `{cols, rows}` from the container
bounding box. Pass those through `AttachArgs` so the Tauri Term
is created at the real user-window size from the start.
ResizeObserver's `lastCols/lastRows` initialize to the measured
dims so its first fire is a no-op match. Result: workspace-return
doesn't trigger any resize call — Term is already the right size.

### Step 3: APC `k2so:resize` for live user resizes (kill race category)
- Frontend ResizeObserver stops calling `kessel_term_resize`
  directly. Only calls `kessel_resize` (daemon).
- Daemon's `handle_sessions_resize`: before calling
  `session.resize()` (which SIGWINCHes the PTY), inject an APC
  `\x1b_k2so:resize:<json>\x07` via `entry.publish_bytes()`. JSON
  payload: `{cols, rows}`.
- Tauri's APC filter (`ApcFilter` in `kessel_term.rs`) handles
  `kind="resize"`: calls `term.resize(cols, rows)`. Same mechanism
  as the existing `grow_boundary` APC.
- Claude's post-SIGWINCH bytes (which typically include ClearScreen
  + repaint) arrive in the byte stream AFTER the APC, so they land
  in the newly-sized Term.
- Single code path. Zero racing resizes. No competing paints.

## Why APC-driven resize is the right shape

Currently three paths resize the Tauri Term:
- `kessel_term_resize` (from ResizeObserver)
- APC `grow_boundary` (from daemon during spawn)
- Any future resize source

These race because there's no serialization between them relative
to Claude's byte output. The byte stream IS a serialized event
log — putting resize on it guarantees ordering: APC arrives, Term
resizes, Claude's subsequent bytes paint into new dimensions. If
Claude's bytes were already in flight when resize happens, they
land in the old-size Term (bounded corruption); new bytes post-APC
are clean.

This mirrors the pattern we already established for
`grow_boundary` — just a second APC kind.

## Heartbeat invariants (must preserve through the fix)

- `session_map::register` stays synchronous inside
  `spawn_agent_session`.
- `session.write()` already locks its writer mutex; concurrent
  writes (inject + pending-live drain) serialize per-call. Signal
  bytes never interleave mid-signal.
- `handle_sessions_resize` injecting APC via
  `entry.publish_bytes()` is safe — it's the same path
  `spawn_session_stream_and_grow`'s background task uses for
  `k2so:grow_boundary`.
- No heartbeat-driven session code path calls `handle_sessions_resize`
  — resizes are client-initiated only. Headless agent sessions
  continue to ignore the resize architecture entirely.

## Key files for the fix

| File | What changes |
|---|---|
| `src-tauri/src/commands/kessel_term.rs` | PaneState::resize: revert the wipe. ApcFilter / apply_apc_event: handle kind="resize" by calling term.resize (Step 3). |
| `src/renderer/kessel/SessionStreamViewTerm.tsx` | useLayoutEffect to measure before attach (Step 2). ResizeObserver: remove kessel_term_resize invoke (Step 3), keep kessel_resize. |
| `crates/k2so-daemon/src/terminal_routes.rs` | handle_sessions_resize: lookup SessionEntry via registry, inject `k2so:resize` APC bytes before session.resize (Step 3). |

## Tests to update

- No new test files expected for Step 1-2 (revert + measure).
- Step 3 warrants a test in `crates/k2so-core/tests/session_stream_grow_shrink.rs` style: spawn a session, call `/cli/sessions/resize` via the daemon's test harness, assert APC `k2so:resize` appears in the byte ring at the expected offset with correct payload.

## What NOT to do

- Don't add more wipe hacks. The root cause is resize racing; wipes are whack-a-mole.
- Don't resize the daemon's own alacritty Term from the frontend side (`kessel_term_resize` is Tauri-local; daemon's Term is separate and driven by session.resize alone).
- Don't remove the `kessel_term_resize` Tauri command yet — keep for benchmarks/diagnostics even after ResizeObserver stops calling it.

## Where we are in the bigger plan (.k2so/prds/kessel-instant-everywhere.md)

Phase A (non-blocking spawn) + Phase 9 idempotent reattach + no-unmount-close are the committed headline wins. Task #352 (this resize fix) is a dependency of polish; blocks the Milestone-1 "feels instant everywhere" target.

After #352 lands, proceed to:
- Phase F: kessel_term_attach yields early (~1 day)
- Phase B: client snapshot cache (~3 days)
- Phase C: cross-launch persistence (~4 days)
- Phase D: daemon warm-up (~3 days)
- Phase E: lazy per-tab spawn (~3 days)
- Phases G/H/I: rendering polish (~10 days total)
- Phase K: Claude stream-json (separate track)

Milestones in PRD `kessel-instant-everywhere.md`.

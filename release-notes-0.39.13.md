# K2SO 0.39.13 — Stream only the visible terminal (Issue #8 "why")

Renderer-only. No daemon/protocol changes, no migrations. Completes the
Issue #8 work: 0.39.12 stopped the `set_active` claim *storm* (the
symptom); this removes the *why* — the renderer holding live grid
streams for terminals the user isn't looking at.

## The root cause (the "why")
Every mounted `TerminalPane` opened a per-session grid WebSocket to the
daemon and stayed subscribed — including hidden (`display:none`)
background tabs and off-screen heartbeat-spawn panes. A long-running app
with many persisted workspace sessions therefore held a live stream for
each (one report: ~11 streams / 22+ daemon sessions for a single visible
tab). That redundant streaming + reconnect churn is what overran the
per-session grid broadcast channel and produced the terminal stalls.

The daemon owns the PTY authoritatively and it survives regardless, so
the thin client has no reason to stream a session no one is viewing.

## The fix — spawn ⊥ stream, stream only what's visible
- **Spawn is decoupled from streaming.** Mounting a pane still issues the
  idempotent spawn POST (keeps/attaches the daemon PTY — the session
  stays warm), but opening the grid-WS is now gated on visibility.
- **A pane holds a grid-WS only while visible.** A dedicated grid-WS
  lifecycle effect (keyed on a `spawnGeneration` counter + `isTabVisible`)
  is the sole opener/closer. It *reconciles* (open if visible+spawned+not-
  exited, close if hidden) and deliberately does **not** close in its
  cleanup — so an unrelated re-render never tears the socket down. On
  hide it closes the grid-WS (PTY survives — never `/cli/sessions/v2/close`);
  on show it opens a fresh one and the daemon's on-subscribe snapshot
  brings the pane fully current with zero loss.
- **`BackgroundTerminalSpawner` is spawn-only** (rendered under
  `TabVisibilityContext value={false}`) — heartbeat spawns create the PTY
  without ever opening a grid stream.
- **Session-events teardown leak fixed** (`tabs.ts`): switching to a warm
  background workspace no longer leaves the previous workspace's
  session-events WS open; exactly one workspace is subscribed at a time.
- Composes with 0.39.12: a hidden pane has no socket, so it inherently
  can't claim `set_active`; the daemon-side idempotent claim handling and
  the 256→4096 broadcast headroom remain as backstops.

### Why this supersedes the first attempt
An initial version gated visibility by adding `isTabVisible` to the
spawn effect's dep array, which made the whole spawn+WS lifecycle re-run
on visibility-touching re-renders — reconnecting the visible pane's
stream on roughly every active-agents poll (~3s). The shipped version
decouples spawn from the grid-WS lifecycle so a steadily-visible pane
holds one stream that does not churn on the poll cycle.

## Validated live (real environment, 22-27 daemon sessions)
- **Pile-up gone:** with 27 PTY sessions alive, exactly **1 grid-WS held**
  (the visible pane). The held stream **follows the workspace you switch
  to** — net-held stays at the visible count, never climbs (no leak).
- **`lagged`: 0** throughout (no broadcast overruns — the freeze signal).
- **Background sessions keep running** while unviewed (PTYs untouched).
- **Switching tabs/workspaces** shows live content immediately; hide→show
  delivers a fresh snapshot, no blank/stale panes.
- Renderer vitest **63/63**; typecheck **43 errors** (better than the 49
  baseline — net −6 from a typed `sessionId` local).

## Known follow-ups (NOT in this release — tracked, non-blocking)
- A small residual reconnect (~1 per ~12s) on the visible pane's grid-WS
  in busy real environments — benign (invisible fresh-snapshot, no lag,
  no freeze), a >15× reduction from the superseded attempt. Confirmed
  **not** the 0.39.11 webview watchdog (no reload, no breadcrumb) — a
  separate grid-WS-layer reconnect to chase down.
- `/cli/agents/running`'s `subscriberCount` field is **broken** (reports 0
  even for a live grid-WS subscriber) — observability only, no user
  impact; made live validation harder.

## Upgrade notes
- Any 0.39.x → 0.39.13: clean update, no migrations.

## What else shipped in this release
Nothing else. See `release-notes-0.39.0.md` through
`release-notes-0.39.12.md` for prior content.

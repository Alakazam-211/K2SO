# K2SO 0.39.12 — Terminal `set_active` storm fix (#8) + chat-history workspace binding (#7)

Two user-reported bugs. Both fixes are renderer-side with a daemon
defense-in-depth backstop for #8. No protocol changes, no migrations.

## 1. Issue #8 — multi-session `set_active` storm → terminal stalls

### The bug
Terminals intermittently stalled (froze for seconds, then "suddenly
recovered"), worsening with uptime and with the number of persisted
workspace sessions. Reproduced immediately (~3 min) on a fresh launch
with many persisted sessions, not just after a long-running leak.

### Root cause
The renderer's `set_active` "live-viewer" handshake — which tells the
daemon WHICH client owns a session's grid sizing / resize authority —
was keyed on **window focus** (`useWindowFocusStore.isFocused`), a
signal identical for every pane in the window. So on each window-focus
event, **every mounted grid-WS subscriber claimed `active:true` at
once** — including hidden (`display:none`) background-tab panes and the
many background workspace sessions the renderer subscribes on launch.
With a dozen-plus subscribers (one report: 11 sessions live, 86
persisted) cycling claims (each WS reconnect re-runs `boot()` →
resets the dedup → re-emits), the daemon's per-session active pointer
flip-flopped continuously. That churn — plus the resize ping-pong it
triggers when differently-sized panes alternate as "active," forcing
full-screen TUI repaints — overran the per-session grid broadcast
channel → `RecvError::Lagged` → fresh-snapshot flush → more traffic.
The flush *is* the visible "stall then recover."

This is the same broadcast-overflow family as #3 (0.39.8). 0.39.8's
per-pane send dedup (`lastSentActiveRef`) structurally couldn't help:
multiple panes each send legitimately-different values from the shared
window-focus event, so the per-pane guard never fires across panes.

### The fix
- **Renderer — claim only the visible+focused pane.** `set_active(true)`
  now requires `visible && pane-focused && window-focused`
  (`computeDesiredActive`, exhaustively unit-tested). The pane already
  had a pane-level focus signal (shadow-input focus) and a visibility
  signal (`TabVisibilityContext`); the claim now uses both plus window
  focus. **Because DOM focus is singular — only one element holds focus
  at a time — at most ONE pane can ever claim `active`**, regardless of
  how many panes are mounted or reconnecting. That single property
  collapses the multi-session storm, including the reproduce-on-launch
  case. Hidden/blurred panes proactively release. The WS-(re)connect
  re-prime uses the same full predicate, so a backgrounded reconnecting
  pane can't re-claim. The `lastSentActiveRef` dedup is preserved so the
  recompute-on-change path can't thrash (no #3 regression). The
  focus-gain resize re-emit is now gated on actually being the active
  viewer, killing the ping-pong amplifier.
- **Daemon — idempotent claim handling (defense-in-depth).** A new pure
  `decide_set_active(current, subscriber_id, active)` classifies each
  frame as `Claim` / `Release` / `NoOp`; redundant claims (already hold
  it) and redundant releases (never held it / someone else holds it) are
  `NoOp` — no atomic store, no log — so even a misbehaving client can't
  reintroduce the churn. Single-winner election (most-recent-claim-wins,
  CAS-on-release, release-on-disconnect) is unchanged.
- **Daemon — broadcast headroom.** Per-session grid event channel
  capacity raised 256 → 4096 so a single full-screen redraw burst can't
  tip a momentarily-slow subscriber into the lag/flush cycle. Bounded
  (each slot is a small event; ceiling a few hundred KB).

### Known follow-up (intentionally NOT in 0.39.12)
The renderer still *mounts and cycles* grid-WS + PTY-attached
subscribers for many non-visible background workspace sessions on launch
(the ~20 sockets / subscribe→reconnect churn). With the storm fix this
is no longer a *stall* — no background pane claims `active` — but it
remains a **resource/scaling** concern (sockets, daemon load,
per-resubscribe snapshot traffic). Tracked as a separate follow-up:
background workspace *status* (sidebar dots) should come from a
lightweight channel rather than full per-session grid subscribers; only
visible/focused panes should hold a grid-WS. Split out per the
reporter's own recommendation.

## 2. Issue #7 — chat-history panel shows the wrong project

### The bug
Opening the chat-history panel inside workspace A could show workspace
B's chats — specifically whichever workspace was *globally active*
(usually the one with running agents/heartbeats, e.g. K2SO on this
machine).

### Root cause
`ChatHistory.tsx` took no props and resolved its project from the
global `activeProjectId` / `activeWorkspaceId` store pointers, which
track the globally-active workspace and can diverge from the workspace
the panel is actually mounted inside. The daemon SQL + on-disk data are
fully project-scoped and correct (verified by the reporter) — the panel
was simply asking for the wrong project.

### The fix
`ChatHistory` now takes a `projectPath` prop, supplied by its mount
sites (`LeftPanelContent` / `RightPanelContent` already hold the host
workspace's `rootPath`, the same value they pass to `FileTree`). A pure
`resolveChatHistoryHost()` resolves the host project + workspace from
that path (worktree-path match → main-workspace match → host-path-only
fallback), mirroring the established `AgentChatPane` prop pattern. Falls
back to the legacy global-pointer behavior only when no host path is
supplied, so the common single-window case is unchanged. The
globally-scoped `chat/pinned` call is left as-is (harmless: a pin key
can only match a row already in the now-correctly-project-scoped list;
documented in a code comment).

## Tested
- **k2so-core: 665/0.** **k2so-daemon: all suites 0 failed** (incl. 6 new
  `sessions_grid_ws::decide_set_active` idempotence tests).
- **Renderer vitest: 57/57** (incl. 9 new `activeViewer` truth-table
  tests + 6 new `resolveHost` tests).
- **Renderer typecheck: 49 errors — unchanged pre-existing baseline,
  zero new.**

## Upgrade notes
- Any 0.39.x → 0.39.12: clean update, no migrations.
- Fixes are client + daemon code only; the daemon protocol is unchanged
  (`set_active` semantics preserved, just made idempotent server-side).

## What else shipped in this release
Nothing else. See `release-notes-0.39.0.md` through
`release-notes-0.39.11.md` for prior content.

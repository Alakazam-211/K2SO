# K2SO 0.35.6 — Activity detection in v2 panes

Quick UX parity fix. v2 (Alacritty) panes weren't driving the
sidebar's "Active" indicator or the braille spinner in tabs when
an agent was actually working — both worked fine in legacy
panes. After this update, both renderers feed the same activity
store, so the sidebar lights up the same way regardless of which
renderer the user picked.

## What was missing

The legacy renderer (`AlacrittyTerminalView.tsx`) wires two
signals into `useActiveAgentsStore`:

- **`recordOutput(terminalId)`** every time a `terminal:grid`
  event arrives — the heartbeat-style "this pane just produced
  bytes" signal.
- **`recordTitleActivity(terminalId, isWorking)`** when the
  bottom of the viewport contains a known LLM-CLI status line
  ("esc to interrupt", "thinking…", "waiting for…", etc.) per
  `detectWorkingSignal`.

`TerminalPane.tsx` (v2) shipped without either call. v2 receives
the same grid data over its WS — `TermGridSnapshot` /
`TermGridDelta` payloads — but the JSX renderer was wired only to
`setSnapshot`, not to the activity store. So when the user opened
a Cmd+T tab in v2, ran `claude`, and watched it think, the
sidebar's "Active" panel and the tab's braille spinner stayed
quiet.

## Fix

`src/renderer/terminal-v2/TerminalPane.tsx` now mirrors the
legacy wiring:

- On every snapshot/delta, `recordOutput(terminalId)` fires.
- The merged grid is converted to a `Map<row, {text}>` and fed
  to `detectWorkingSignal` — gated on `displayOffset === 0`, so
  scrolled-up panes don't accidentally pin the indicator on.
- A 500 ms idle watcher clears `recordTitleActivity` when no
  working signal has been seen for 1 s, matching the legacy
  grace window so the pane doesn't flicker between active and
  idle on single-frame status-line gaps.

The detector module (`agent-signals.ts`) and the active-agents
store (`active-agents.ts`) are unchanged — same patterns, same
keying on `terminalId`. Legacy and v2 panes are now
indistinguishable from the sidebar's perspective.

## What's not covered

This wires the **viewport scan** — the most reliable signal per
the agent-signals module's docs. Legacy also listens for
title-prefix glyph hints (`✱✲✳✴…` from Claude's title bar) via
`terminal:title:<id>` events. v2's WS protocol doesn't surface
title events today; adding that is a separate follow-up that
needs a daemon-side passthrough on the broadcast channel.
Without it, v2's idle transition can take up to ~1.5 s longer
than legacy's; the active state itself is correct.

## Verification

Hand-check after install: open a Cmd+T tab on the v2 renderer,
run `claude` and ask it something. Sidebar's "Active" section
should populate; the tab should show the braille spinner during
the response. When Claude finishes (status line goes away), both
should clear within ~1.5 s.

# K2SO 0.35.6 — Alacritty (v2) UX parity: activity, cursor, Active Bar

After 0.35.4 (A9 daemon-headless plumbing) and 0.35.5 (auto-update
spawn-retry), v2 was functionally complete but visually behind
legacy in three places that touched workflow on every keystroke:

1. **No braille spinners on tabs / no entries in the sidebar's
   Active section** when an LLM was working in a v2 pane.
2. **Stray cursor block parked at the bottom-left** of TUI panes
   (Cursor Agent, Claude Code) — alacritty's real cursor sat
   below the rendered UI while the TUI drew its own visual
   cursor inside an input box higher up.
3. **Active Bar didn't surface workspaces on visit** — they only
   appeared after navigating away.

This release closes all three.

## What changed

### Title + Bell forwarded over the v2 WS protocol

`crates/k2so-daemon/src/sessions_grid_ws.rs` adds two new
outbound message types alongside `Snapshot` / `Delta` /
`ChildExit`:

- **`{event:"title", payload:{title}}`** — every alacritty title
  change (`OSC 0/1/2`, plus `ResetTitle` as empty). Renderer
  uses it the same way legacy uses `terminal:title:<id>` Tauri
  events: braille-spinner glyph in the title prefix → working,
  ✱-family glyph → idle. Drives the activity store directly.
- **`{event:"bell", payload:null}`** — terminal bell (`\a`).
  Same signal iTerm uses for "agent waiting" notifications.
  Renderer treats it as a definitive idle transition.

`TerminalPane.tsx` subscribes to both, calls
`useActiveAgentsStore.recordTitleActivity` on transitions, and
strips the leading marker glyph from the cleaned title before
calling `setTabTitle`. Net effect: tabs and sidebar light up
their braille spinners on the same signals legacy already
honored, with no daemon-side scanning of viewport text.

Viewport-text fallback (`detectWorkingSignal`) is still wired in
`TerminalPane.tsx` for tools that don't issue OSC title cycles.
That path is also where the multi-LLM signal additions land:

- `cursor-agent`: "planning next moves" + "taking longer than
  expected"
- `pi` (pi-mono): "working..." / "thinking..."
- `tenere`: "🤖: waiting"
- `llm-tui-rs`: "loading..."

The full list is in `src/renderer/lib/agent-signals.ts` —
each entry's intended tool is annotated.

### Active-agents store fix for v2 panes

`pollOnce` was deleting `outputTimestamps` for every paneId not
in `newAgents` (the legacy `terminal_get_foreground_command`
result, which doesn't see v2 sessions). That nuked the
`OUTPUT_TRUST_GRACE_MS` signal that keeps a hook-driven 'working'
status alive through the cleanup branch — meaning every poll
cycle wiped the v2 spinner state.

Fix: skip the timestamp delete for any paneId already in
`paneStatuses`. Hook-driven legacy panes are unaffected (they're
already in `newAgents` via the `KNOWN_AGENT_COMMANDS` check).

`recordTitleActivity` now ALSO populates `paneProjectMap` on the
first 'working' transition (mirroring what `handleLifecycleEvent`
does on a hook 'start' event). Without this, the sidebar's
`getProjectStatus` had no way to attribute a working paneId to a
workspace, so the Active Bar's project-level spinner stayed
dark even when the tab spinner was lit. As a side effect the
project's `lastInteractionAt` gets bumped on activity → 24h
Active Bar tenure.

### Active Bar always shows the active workspace

`useActiveBarItems` previously gated rule 3 ("active project")
on `hasActiveAgents || hasHookActivity`. v2 panes whose detection
hadn't lit up yet would never enter the bar — and once you
navigated away, you'd lose any "I was just here" trail.

New rule: any active workspace is always in the Active Bar.
`setActiveWorkspace` now calls `touchInteraction(projectId)`,
which sets `lastInteractionAt` → the workspace stays in Active
for **24 hours** after your last visit. Right-click → Dismiss
clears it.

### Cursor visibility honors DECTCEM

The daemon's grid serializer hardcoded `CursorSnapshot.visible =
true`. After A8 made every Cmd+T tab v2 by default, TUIs that
issue `\e[?25l` (Cursor Agent's "Plan, search, build anything"
input, Claude Code's `›` prompt, vim's normal mode) had a
phantom cursor block parked wherever alacritty's real cursor
ended up — usually the bottom-left of the rendered area.

Fix: `crates/k2so-core/src/terminal/grid_snapshot.rs` reads
`term.mode().contains(TermMode::SHOW_CURSOR)` for both the full
snapshot path and the delta path. `TerminalPane.tsx` honors the
flag and matches legacy's gate exactly:

```
showCursor = cursor.visible && displayOffset === 0
```

### Inverse-cell rendering uses terminal defaults

`runStyle` was producing no styles when `inverse=true` AND
`fg=null` AND `bg=null` — a common combination for TUI-drawn
cursors. The cell rendered as plain text instead of an inverted
block. Updated to fall back to the terminal's configured
`defaultFg` / `defaultBg`, so `inverse: true` actually swaps in
the expected colors.

### Hollow cursor on unfocused TUI panes

When a v2 pane that's running a TUI loses focus, the cursor
overlay scans the visible grid for the inverse cell, then
overlays a div that:

- Fills with the terminal's default bg (covers the TUI's white
  block)
- Re-renders the cell character in default fg color (so it
  looks like normal text, not the inverted-into-the-block form)
- Adds a 1px caret-color border around the cell

Net effect: the cursor flips between solid bright block
(focused) and hollow outline with normal-colored character
(unfocused) — the same focus-state UX v2's regular shells got
from day one, now extended to TUIs.

The overlay extends 1px above the row to absorb the line-box
bleed that was making the top edge appear thicker than the
other sides on retina displays. `border` instead of
`box-shadow inset` for both scenarios; the latter snaps
unevenly at fractional pixel ratios.

## What's NOT in this release

- Bell signal isn't wired in Kessel (legacy session-stream)
  yet — the protocol carries it for v2 only. A follow-up can
  add `Frame::Bell` to the legacy frame stream.
- Watchdog idle escalation against v2 sessions still skips
  panes that aren't in `k2so_core::session::registry`
  (registry-backed activity tracking for v2 is a separate
  follow-up — see A9 phase 2 docs).

## Upgrade

Standard auto-update path — install + relaunch. The
`spawn-retry-during-daemon-restart` fix from 0.35.5 covers the
~3-5s window where the daemon swaps over.

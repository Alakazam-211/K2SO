// Issue #8 — active-viewer claim predicate.
//
// The daemon's `set_active` handshake tells it WHICH connected client
// is the live viewer of a session so it can size the grid and route
// resize/focus events. Pre-#8 the renderer keyed the claim on WINDOW
// focus alone: every mounted pane (including hidden background-tab
// panes kept alive with `display:none`) re-claimed `active:true` on
// each window-focus event. A long-lived session accumulated many such
// panes, so a single focus event produced a burst of simultaneous
// claims for sessions the user wasn't even looking at — churning the
// daemon's small grid broadcast channel into `lagged`/snapshot-flush
// cycles (the "stall then recover" symptom).
//
// The fix: a pane is the active viewer only when it is the visible,
// pane-focused pane in a focused window. All three must hold.
//
// Kept as a standalone pure function (no React, no DOM) so the truth
// table is exhaustively unit-testable and the TerminalPane effect has
// a single source of truth it can call from refs.
export interface ActiveViewerInputs {
  /** This pane's enclosing tab + pane-item are visible (not display:none). */
  visible: boolean
  /** This pane holds shadow-input focus (only one pane does at a time). */
  paneFocused: boolean
  /** The OS window is focused. */
  windowFocused: boolean
}

/**
 * The desired `set_active` value for this pane: true iff it is the
 * visible, focused pane in a focused window.
 */
export function computeDesiredActive(inputs: ActiveViewerInputs): boolean {
  return inputs.visible && inputs.paneFocused && inputs.windowFocused
}

// Issue #8 (0.39.13) — grid-WS hold predicate.
//
// The "why" behind the active-viewer flood: even with the
// `computeDesiredActive` gate (which keeps a hidden pane from CLAIMING
// active), every mounted `TerminalPane` still OPENED a grid-WS and
// stayed subscribed to the session's grid broadcast — including hidden
// (`display:none`) background tabs and off-screen heartbeat-spawn
// panes. A long-lived window therefore piled up N grid-WS subscribers
// on the daemon's small per-session broadcast channel, and any burst of
// output overran it into `lagged`/snapshot-flush cycles.
//
// The PTY lives in the daemon and survives independently; the renderer
// only needs to STREAM a session's live grid while the user is actually
// looking at that pane. So: a pane holds a grid-WS ONLY while visible.
// Spawning the PTY (the idempotent HTTP POST) stays decoupled from
// streaming it (the grid-WS) — a hidden pane keeps its daemon PTY warm
// without subscribing to its grid.
//
// We also stop holding (and stop reconnecting) once the child has
// exited — there's nothing left to stream, and the existing reconnect
// path already skips a real `child_exit`. Folding that into the same
// pure predicate keeps the lifecycle effect's decision in one
// exhaustively-testable place.
export interface GridWsInputs {
  /** This pane's enclosing tab + pane-item are visible (not display:none). */
  visible: boolean
  /** The session's child process has exited — nothing left to stream. */
  exited: boolean
}

/**
 * Whether this pane should currently hold an open grid-WS streaming the
 * session's live grid. True iff the pane is visible AND its child hasn't
 * exited. A hidden pane (background tab, off-screen heartbeat spawn)
 * holds no grid-WS — its daemon PTY survives untouched.
 */
export function shouldHoldGridWs(inputs: GridWsInputs): boolean {
  return inputs.visible && !inputs.exited
}

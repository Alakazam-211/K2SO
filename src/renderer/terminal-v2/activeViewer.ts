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

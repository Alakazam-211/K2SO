# 0.38.1 — Resize-clear gated on real dimension changes

Hotfix for a regression introduced by 0.38.0 commits 8 + 9.

## The bug

The daemon's `DaemonPtySession::resize` cleared the visible alacritty
grid unconditionally on every call. That was the right move for true
dimension changes (the intended fix — eliminates TUI redraw chrome
duplication). But it broke **same-size resize calls** — phantom
events from menu interactions, ResizeObserver false-positives, focus
transitions that re-emit `lastResizeRef` even when nothing actually
changed.

The kernel doesn't `SIGWINCH` a child for a same-size resize, so the
TUI never redraws. We cleared the grid → user sees a black screen
until the next keystroke or output line triggers Claude to paint
again. Especially visible during Claude's interactive
human-in-the-loop prompts where the user is staring at the screen
deciding what to type.

## Fix

`resize()` now reads the term's current dimensions before doing
anything, and only clears + reflows if at least one of (cols, rows)
actually differs. Same-size calls are a true no-op.

```
let dims_changed = (term.columns() as u16) != cols
    || (term.screen_lines() as u16) != rows;
if dims_changed {
    term.goto(0, 0);
    term.clear_screen(ClearMode::Below);
    term.resize(...);
}
```

`pty_notifier.on_resize` still fires above the gate — the kernel
ignores same-size `WindowSize` updates internally and the call is
cheap, so it's safe to leave.

## What we kept

The Commit 8/9 win for **real** resizes is preserved: when you
genuinely shrink/grow the window, the visible grid still clears
cleanly via `ClearMode::Below` so Claude's in-place redraw doesn't
leave stale chrome behind. Scrollback above the viewport remains
untouched.

## Verified

End-to-end test: open menus + interactive prompts + focus switches
between windows of identical size — grid stays painted with current
content, no more transient black flashes.

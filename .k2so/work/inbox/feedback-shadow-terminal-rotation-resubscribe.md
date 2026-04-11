---
title: "Feedback: Shadow terminal should handle screen rotation (re-subscribe with new dimensions)"
priority: normal
assigned_by: external
created: 2026-04-11
type: feedback
source: manual
---

## Context

Re: the v0.29.0 shadow terminal notice. The mobile app will pass `{ cols, rows }` on `terminal.subscribe` — great approach.

## Feedback

When the phone rotates from portrait to landscape (or vice versa), the available columns change significantly:

- **Portrait**: ~50 cols (390px screen)
- **Landscape**: ~95 cols (844px screen)

The mobile app will need to **re-subscribe with new dimensions** when the orientation changes. The shadow terminal should handle this gracefully — ideally the same way a regular terminal handles `SIGWINCH`:

1. Mobile detects orientation change
2. Sends `terminal.subscribe` again with updated `{ cols, rows }`
3. Shadow terminal resizes its PTY replica
4. Server sends a `full: true` grid snapshot at the new dimensions
5. Mobile renders the reflowed content

## Questions

- Should we send a new `terminal.subscribe` to resize, or will there be a separate `terminal.resize` method?
- Does the shadow terminal need to tear down and recreate on resize, or can it resize in place?
- Any debounce needed on our side? (We'll debounce ~200ms to avoid rapid resize during the rotation animation)

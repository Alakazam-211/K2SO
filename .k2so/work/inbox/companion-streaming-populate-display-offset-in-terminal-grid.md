---
title: Companion streaming: populate display_offset in terminal:grid events
priority: high
assigned_by: user
created: 2026-04-13
type: feature
source: manual
---

Companion terminal streaming: need display_offset or incremental updates

The companion app receives WS terminal:grid events for real-time terminal streaming.
The current format makes it impossible to build a continuous scrollable chat thread.

THE PROBLEM:

Each WS terminal:grid event sends the full reflowed terminal as 355 rows numbered 0-354,
with display_offset always 0. This is a fixed-size snapshot of the terminal screen.

When new content streams in (e.g., Claude responding), the terminal scrolls internally.
Old content falls off the top of the 355-row window, new content appears at the bottom.
But from the companion app's perspective, we just get 355 rows starting at 0 every time.

We have no way to know:
- Which rows are new vs which scrolled up from the previous frame
- How many lines have scrolled off the top since the last frame
- The absolute position of row 0 in the full terminal history

This means we cannot append new lines to a growing buffer. Any approach we try
(offset math, text diffing, clearing and replacing) either loses content in the
middle of the thread or creates a jittery polling experience.

WHAT WE NEED (pick one):

1. display_offset populated correctly
   - Tell us how many lines have scrolled off the top of the terminal
   - WS row 0 at display_offset 480 = absolute row 480 in the full history
   - Simplest server-side change, solves the problem completely
   - We can then append: absolute_row = display_offset + ws_row

2. Incremental updates
   - Only send new/changed lines, not the full 355 rows every frame
   - Include an absolute row number for each line
   - More efficient bandwidth-wise too

3. Scrollback-aware WS streaming
   - Like HTTP readTerminal?scrollback=true but streamed over WS
   - Send the full history (up to 500 lines) on each update
   - Most bandwidth-heavy but simplest for the client

Option 1 is strongly preferred. It is the smallest change on the server side
and completely solves the client-side rendering problem.

CURRENT WORKAROUND:

We tried using HTTP polling (readTerminal with scrollback=true) triggered by WS events.
This works but introduces 200-400ms latency per update, making the streaming feel
jittery and chunked instead of smooth line-by-line flow.

REFERENCE:

- WS event: terminal:grid with payload { terminalId, grid: GridUpdate }
- GridUpdate: { cols, rows, cursor_col, cursor_row, cursor_visible, cursor_shape, lines: CompactLine[], full, display_offset? }
- display_offset is present in the type but always 0 in practice
- The shadow terminal reflow (terminal.subscribe with cols/rows) sends the full reflowed grid

---
title: WS terminal.read with scrollback for real-time companion streaming
priority: high
assigned_by: user
created: 2026-04-13
type: feature
source: manual
---

WS terminal.read with scrollback support for real-time streaming

CONTEXT:

The companion app needs to display a continuously growing terminal thread.
HTTP readTerminal with scrollback=true gives us the full history (499 lines)
but takes ~700ms per request through ngrok. This makes streaming jittery.

WS terminal:grid events are real-time but only send a 20-row visible window
from the shadow terminal. The display_offset is relative to the shadow
terminal's small reflow buffer (values 2-17, fluctuates), not an absolute
position in the full terminal history. So we cannot use it to build a
scrollable buffer.

WHAT WE NEED:

Option A (preferred): WS method terminal.read that supports scrollback
- Same as HTTP GET /companion/terminal/read?scrollback=true but over WS
- Returns { lines: string[] } with up to 500 lines of full history
- We would call this on each terminal:grid event (debounced ~100ms)
- Since the WS connection is already open, no ngrok round-trip overhead
- Expected latency: <50ms vs ~700ms for HTTP through ngrok

Option B: WS push event with incremental lines
- When new lines are added to the terminal, push just the new lines
- Event: terminal:lines with { terminalId, newLines: string[] }
- Client appends to its buffer, no polling needed
- Most efficient but requires more server-side tracking

Option C: Increase shadow terminal buffer size
- Instead of 20 rows, reflow the full terminal history (500+ rows)
- Make display_offset represent the absolute position
- Client can use terminal:grid directly for everything
- Most bandwidth-heavy but simplest client-side

Any of these would give us smooth streaming with complete history.
Current workaround: HTTP polling at 150ms debounce (works but jittery).

CURRENT DATA FLOW:

1. On session open: HTTP readTerminal?scrollback=true (499 lines, ~700ms)
2. During streaming: WS terminal:grid (20 rows, real-time but tiny window)
3. We need: full history updates at WS speed

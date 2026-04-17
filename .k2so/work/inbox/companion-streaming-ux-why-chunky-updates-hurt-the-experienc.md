---
title: Companion streaming UX: why chunky updates hurt the experience
priority: high
assigned_by: user
created: 2026-04-13
type: task
source: manual
---

Follow-up: Why smooth streaming matters for companion UX

CURRENT USER EXPERIENCE:

When a user sends a message to Claude from their phone and watches the response,
they see text appear in ~450ms chunks. Each chunk brings in multiple lines at once,
then pauses, then another chunk. It feels like watching a slow loading webpage,
not a real-time conversation.

Compare this to the desktop K2SO experience where Claude's response streams in
character-by-character, word-by-word. The user can read along at the speed Claude
thinks. This is the expected experience for any AI chat interface (ChatGPT, Claude.ai,
etc all stream smoothly).

WHY IT'S CHUNKY:

The companion app receives terminal:grid events in real-time (every ~50ms during
streaming). These events are fast but only contain a 20-row window of the terminal.
To get the full conversation thread, we must make a separate terminal.read request
after each grid event. That request-response round trip through ngrok adds ~350ms.

Timeline of one update cycle:
  0ms   - terminal:grid arrives (real-time push, fast)
  100ms - debounce timer fires, we send terminal.read request
  450ms - terminal.read response arrives with 500 lines
  450ms - we render the update

The user sees nothing for 450ms, then a burst of new content. During active streaming,
Claude might output 5-10 new lines in that 450ms gap, so each visual update jumps
ahead noticeably.

WHAT SMOOTH STREAMING LOOKS LIKE:

  0ms   - server pushes terminal:scrollback event with 500 lines
  0ms   - we render immediately
  50ms  - next push arrives with 501 lines (1 new line added)
  50ms  - we render the new line

The user sees each new line appear within 50ms of Claude producing it.
This matches the desktop experience and every other AI chat interface.

WHAT WE NEED:

Either:
1. A new terminal:scrollback push event with full history (fires alongside terminal:grid)
2. Or extend terminal.subscribe to accept a scrollback: true param that makes
   terminal:output include the full scrollback buffer instead of just the visible window

Option 2 is probably simpler since terminal:output already fires at the right frequency.

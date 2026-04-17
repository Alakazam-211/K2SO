---
title: WS push event with full scrollback for smooth companion streaming
priority: high
assigned_by: user
created: 2026-04-13
type: feature
source: manual
---

Request: WS push event with full scrollback for smooth companion streaming

PROBLEM:

The companion app needs smooth real-time streaming of terminal content.

Current options and why they fail:
1. terminal:grid (push) — real-time but only 20-row window, no scrollback
2. terminal:output (push) — real-time but only 17-19 lines, same tiny window
3. terminal.read with scrollback (request-response) — full 500 lines but
   ~350ms round-trip per request, making streaming chunky

We need the best of both: real-time push delivery WITH full scrollback content.

REQUEST:

Add a terminal:scrollback push event (or extend terminal:output) that sends
the full scrollback buffer whenever content changes. Similar to what HTTP
readTerminal?scrollback=true returns, but pushed automatically like terminal:grid.

Suggested format:
{
  "event": "terminal:scrollback",
  "data": {
    "terminalId": "...",
    "lines": ["line1", "line2", ... ],  // full 500-line scrollback
    "totalLines": 500
  }
}

This would fire at the same frequency as terminal:grid events (every frame
during streaming). The companion app replaces its buffer with the full content
on each event — no request-response round-trip, no offset math, no polling.

ALTERNATIVE:

Extend the existing terminal:output event to include scrollback when
the subscriber requests it:

terminal.subscribe params: { terminalId, cols, rows, scrollback: true }

When scrollback is true, terminal:output events include the full history
instead of just the visible window.

BANDWIDTH NOTE:

500 lines of plain text is roughly 20-30KB. At ~10 events/sec during active
streaming, that is 200-300KB/sec — acceptable over WiFi/LTE through ngrok.
Idle terminals would send events rarely.

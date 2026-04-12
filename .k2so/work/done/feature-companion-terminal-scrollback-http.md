---
title: "Feature: HTTP endpoint for terminal scrollback history (ring buffer)"
priority: high
assigned_by: external
created: 2026-04-12
type: feature
source: manual
---

## Problem

The mobile companion app uses HTTP fallback for terminal data because iOS WKWebView blocks native WebSocket connections to external hosts. The current `GET /companion/terminal/read` endpoint only returns the visible screen buffer (~63 lines). There's no scrollback history available via HTTP.

The WebSocket `terminal.subscribe` has access to the 2MB ring buffer and replays history on subscribe, but that path is unavailable on iOS.

## What We Need

A new HTTP endpoint (or enhancement to `terminal.read`) that returns scrollback history from the ring buffer:

### Option A: New endpoint

`GET /companion/terminal/history?project={path}&id={terminalId}&lines=1000`

Returns up to N lines from the ring buffer, not just the visible screen.

### Option B: Enhance existing endpoint

Add a `scrollback=true` parameter to `terminal.read`:

`GET /companion/terminal/read?project={path}&id={terminalId}&lines=1000&scrollback=true`

When `scrollback=true`, read from the ring buffer instead of just the visible grid.

### Response format

Same as current `terminal.read`:
```json
{
  "ok": true,
  "data": {
    "lines": ["line 1", "line 2", ...]
  }
}
```

## Why

The mobile app currently can only see 63 lines of terminal output — whatever is on screen at that moment. Users can't scroll up to read earlier output from the session. The ring buffer already stores this history server-side (2MB) — we just need an HTTP path to access it.

## Context

iOS WKWebView blocks native WebSocket to external hosts (like ngrok). The Tauri HTTP plugin bypasses this by making requests through Rust, but there's no equivalent for WebSocket. All mobile terminal data currently goes through HTTP polling every 2 seconds.

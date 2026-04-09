---
title: "Feature: WebSocket-first companion protocol"
priority: normal
assigned_by: user
created: 2026-04-10
type: feature
source: manual
---

## Description

Migrate the companion API from HTTP REST to a WebSocket-first protocol. The mobile app opens a single persistent WebSocket connection and all API calls become JSON messages with request/response correlation by ID.

## Why

- **Tunnel health**: Persistent connection keeps ngrok tunnel alive naturally — no health check polling needed
- **Battery life**: One connection vs repeated HTTP requests from mobile
- **Real-time**: Terminal output, agent lifecycle, status changes push instantly
- **Less overhead**: One TCP connection vs new HTTP connection per request through ngrok

## Architecture

### Message Protocol

Request:
```json
{ "id": "abc123", "method": "projects.list", "params": {} }
```

Response:
```json
{ "id": "abc123", "ok": true, "data": [...] }
```

Server push (no request ID):
```json
{ "type": "terminal:output", "terminalId": "...", "lines": [...] }
{ "type": "agent:lifecycle", "data": {...} }
```

### Methods (mirror existing HTTP endpoints)

- `auth.login` — returns session token
- `projects.list` — all workspaces
- `projects.summary` — workspaces with counts
- `sessions.list` — active sessions across workspaces
- `agents.list` — agents in a workspace
- `agents.running` — running terminals
- `agents.wake` — launch agent session
- `terminal.read` — read terminal buffer
- `terminal.write` — send input to PTY
- `status` — workspace mode
- `reviews.list` — review queue
- `review.approve` / `review.reject` / `review.feedback`

### What Exists Already

- WebSocket upgrade handler in `companion/websocket.rs`
- Auth validation for WebSocket (Bearer token in query params)
- Broadcast infrastructure (events → all connected clients)
- Subscribe/unsubscribe for terminal output

### What Needs Building

- Request/response message router (dispatch `method` to handlers)
- Message ID correlation
- Error responses via WebSocket
- Mobile app: replace HTTP calls with WebSocket messages
- Reconnection logic on mobile (exponential backoff)
- Keep HTTP endpoints as fallback for debugging (curl)

## Prerequisites

- Ship 0.28.x with HTTP on paid ngrok (stable)
- Mobile app functional with current HTTP API

## Notes

- Keep HTTP endpoints for curl/debugging — don't remove them
- Auth can stay as HTTP POST for initial login, then switch to WebSocket with the token
- Consider heartbeat/ping frames to detect dead connections on both sides

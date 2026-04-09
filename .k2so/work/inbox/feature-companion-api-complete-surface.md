---
title: "Feature: Complete the Companion API surface for the mobile app"
priority: high
assigned_by: external
created: 2026-04-10
type: feature
source: manual
---

## The Big Picture: What the K2SO Companion App Does

The K2SO Companion is a **native mobile app** (Tauri v2 Mobile — Rust + React) that lets users monitor and interact with their K2SO agent workspaces from their phone. It connects to K2SO through the ngrok tunnel exposed by the Mobile Companion feature (v0.28.4).

**The repo:** https://github.com/Alakazam-211/K2SO-companion

### The app has 4 screens:

**1. Login**
User enters their ngrok URL, username, and password. The app authenticates via `POST /companion/auth` and receives a session token. All subsequent requests use Bearer token auth.

**2. Workspaces (main screen)**
After login, the user sees all their K2SO workspaces — the same projects that appear in K2SO's sidebar icon rail. They tap a workspace to see its active agent sessions. From here they can also launch new agent sessions within a workspace ("+ New Session" button calls `/companion/agents/wake`).

**3. Chat Session (the killer feature)**
When the user taps a running agent session, they enter a chat view. The app reads the terminal buffer (`GET /companion/terminal/read`) and displays agent output as chat bubbles. The user types messages that get sent to the PTY (`POST /companion/terminal/write`). This lets users interact with running LLM agents from their phone — answer permission prompts, steer the agent, give feedback — without being at their laptop.

The chat view also detects `localhost:PORT` URLs in terminal output and rewrites them to go through ngrok (e.g., `https://your-tunnel.ngrok-free.dev/_preview/3000/`), so users can tap to preview what agents are building.

**4. Settings**
Shows connection status (server URL, username, WebSocket state), app version, and a Disconnect button that clears the session.

### Navigation:
- **Hamburger menu (top left):** Opens a drawer listing all workspaces with status indicators and running agent counts — same concept as K2SO's sidebar icon rail but adapted for mobile.
- **⌘J button (top right):** Opens a bottom sheet showing ALL active agent sessions across ALL workspaces. Tap any session to jump directly into its chat. This is the mobile equivalent of K2SO's ⌘J quick-switch.
- **Tab bar (bottom):** Workspaces | Settings

### How it connects:
- All HTTP requests go through Tauri's HTTP plugin (Rust-side networking, bypasses WKWebView restrictions)
- WebSocket connection for real-time push events (agent lifecycle, terminal output)
- Every request includes `ngrok-skip-browser-warning: true` header for free-tier compatibility
- 10-second timeout on all requests

---

## What's Working Today

The companion API (v0.28.4) has 13 endpoints + auth + WebSocket, all verified working:

| Endpoint | Method | Purpose |
|---|---|---|
| `/companion/auth` | POST | Login (Basic Auth → session token) |
| `/companion/ws` | GET | WebSocket for real-time events |
| `/companion/agents` | GET | List agents in a workspace |
| `/companion/agents/running` | GET | Running terminal sessions in a workspace |
| `/companion/agents/work` | GET | Agent work items |
| `/companion/agents/wake` | POST | Launch an agent session |
| `/companion/reviews` | GET | Review queue |
| `/companion/review/approve` | POST | Approve review |
| `/companion/review/reject` | POST | Reject review |
| `/companion/review/feedback` | POST | Request changes |
| `/companion/terminal/read` | GET | Read terminal buffer |
| `/companion/terminal/write` | POST | Send input to PTY |
| `/companion/status` | GET | Workspace mode |

**The problem:** All of these require `?project=/path/to/workspace` — but the mobile app has no way to discover which workspaces exist. There's no endpoint that lists projects.

---

## What's Missing: New Endpoints Needed

### Priority 1: Project Discovery (blocker — app is stuck without this)

**`GET /companion/projects`** — List all registered workspaces

The mobile app needs this to populate the workspace drawer and main screen. Without it, the user authenticates but sees "No workspaces selected."

This does NOT require a `project` query parameter — it's global.

**Implementation:** Query the `projects` table in `~/.k2so/k2so.db` directly. Same pattern as `/cli/feed` and `/cli/connections` which both open the DB.

**Response format:**
```json
{
  "ok": true,
  "data": [
    {
      "id": "uuid",
      "name": "K2SO",
      "path": "/Users/z3thon/DevProjects/Alakazam Labs/K2SO",
      "color": "#22d3ee",
      "iconUrl": null,
      "agentMode": "agent",
      "pinned": true
    }
  ]
}
```

Fields needed: `id`, `name`, `path`, `color`, `iconUrl`, `agentMode`, `pinned`. The mobile app uses `color` for the workspace icon badge and `name` for the drawer labels.

### Priority 2: Cross-workspace session listing (needed for ⌘J)

**`GET /companion/sessions`** — All active agent sessions across ALL workspaces

Powers the ⌘J session switcher. The mobile app groups sessions by workspace and shows them in a bottom sheet for quick-jumping.

Also does NOT require a `project` query parameter.

**Implementation:** Iterate all projects in the DB, for each one check for running CLI LLM sessions using the same logic as `/cli/agents/running`.

**Response format:**
```json
{
  "ok": true,
  "data": [
    {
      "workspaceName": "K2SO",
      "workspaceId": "uuid",
      "workspaceColor": "#22d3ee",
      "agentName": "rust-eng",
      "terminalId": "term-001",
      "startedAt": "2026-04-10T00:00:00Z"
    }
  ]
}
```

### Priority 3: Workspace summary with counts (nice-to-have, avoids N+1)

**`GET /companion/projects/summary`** — Projects with running agent + review counts

Saves the mobile app from calling `/companion/agents/running` + `/companion/reviews` for every workspace individually.

**Response format:**
```json
{
  "ok": true,
  "data": [
    {
      "id": "uuid",
      "name": "K2SO",
      "path": "/path/to/K2SO",
      "color": "#22d3ee",
      "agentsRunning": 2,
      "reviewsPending": 1,
      "agentMode": "agent"
    }
  ]
}
```

---

## Proxy Changes Needed

Add to `companion/proxy.rs` in `map_route()`:
```rust
("GET",  "/companion/projects")         => Some("/cli/companion/projects"),
("GET",  "/companion/projects/summary") => Some("/cli/companion/projects-summary"),
("GET",  "/companion/sessions")         => Some("/cli/companion/sessions"),
```

These three new internal routes should be added to `agent_hooks.rs`. They are **global** — they should NOT require or use the `project` query parameter. The proxy should pass them through without adding a project param.

The proxy's `handle_request` function currently extracts `project` from query params for all non-auth routes. These three routes need to be exempted from that requirement, or the proxy should pass an empty project string and the internal handler should ignore it.

---

## Future Endpoints (not needed yet, but worth knowing about)

These are features the companion app has designed but hidden for v1. They'll be needed eventually:

- **`GET /companion/agents/work`** — Already exists, will be used when we add a Dashboard view
- **Reviews with preview links** — The app will rewrite `localhost:PORT` URLs to go through ngrok. May need a `/_preview/{port}/*` proxy route in the companion to forward traffic to localhost ports on the laptop.
- **WebSocket terminal streaming** — The companion module already polls terminal output and broadcasts diffs. The mobile app subscribes via `{ type: "subscribe", terminalId: "..." }` messages on the WebSocket.

---

## How to Test

1. Start the companion: toggle on in K2SO Settings → Mobile Companion
2. The mobile app is at https://github.com/Alakazam-211/K2SO-companion
3. Run `npx tauri ios dev "iPhone 16 Pro"` in the companion repo
4. Login with the ngrok URL + credentials
5. After implementing `/companion/projects`, the workspaces should populate
6. Tap a workspace → see its running sessions
7. Tap a session → chat with the agent

---

## Acceptance Criteria

- [ ] `GET /companion/projects` returns all registered workspaces (no `project` param required)
- [ ] `GET /companion/sessions` returns all active sessions across all workspaces
- [ ] `GET /companion/projects/summary` returns workspaces with running/review counts
- [ ] All three endpoints require Bearer token auth
- [ ] Proxy routes added to `companion/proxy.rs`
- [ ] Mobile app populates workspace drawer and session switcher

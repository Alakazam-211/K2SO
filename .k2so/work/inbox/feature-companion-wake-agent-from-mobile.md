---
title: "Feature: Wake Agent from Mobile Companion"
priority: normal
assigned_by: external
created: 2026-04-08
type: feature
source: manual
---

## Description

The K2SO Companion mobile app needs the ability to **wake up idle agents** from the phone. When a user sees an agent on their Dashboard that has work in its inbox but no running terminal session, they should be able to tap "Wake Agent" and have K2SO launch that agent in a new terminal.

This requires the companion proxy (from the `feature-mobile-companion-server-side-support` ticket) to expose a new endpoint.

## New Companion Endpoint

| Companion Endpoint | Maps to Internal Route | Method | Purpose |
|---|---|---|---|
| `/companion/agents/wake` | `/cli/agents/launch` | POST | Launch an idle agent — starts a terminal session and picks work from its inbox |

**Request body:**
```json
{ "agent": "rust-eng" }
```

**Proxied to K2SO internal:**
```
GET /cli/agents/launch?agent=rust-eng&token={TOKEN}&project={PATH}
```

**Response (from K2SO):**
```json
{
  "ok": true,
  "data": { "success": true, "note": "Agent session will be launched by K2SO" }
}
```

After K2SO launches the agent, it emits `cli:agent-launch` and `sync:projects` events via `app_handle.emit()`. The companion proxy should also broadcast these as WebSocket events (`agent:lifecycle` type with event `start`) so the mobile app's Dashboard updates in real-time.

## How It Works End-to-End

1. Mobile Dashboard shows agent "rust-eng" as idle (gray dot) with "2 inbox" badge
2. User taps the **Wake Agent** button on the card
3. Mobile sends `POST /companion/agents/wake` with `{ "agent": "rust-eng" }`
4. Companion proxy validates auth, translates to `GET /cli/agents/launch?agent=rust-eng`
5. K2SO's `k2so_agents_build_launch()` creates a terminal session and emits `cli:agent-launch`
6. Companion proxy broadcasts `agent:lifecycle` over WebSocket
7. Mobile receives the WS event, refreshes agent list
8. Agent card transitions to green dot + "Running in terminal — tap to chat"
9. User taps the card → enters chat view → can now interact with the agent

## Security Note

This is a controlled launch — it uses K2SO's existing `/cli/agents/launch` which respects workspace mode (Build/Managed Service/Maintenance/Locked). If the workspace is in Locked mode, the launch will fail and the companion should return the error to the mobile app.

## Dependency

This is part of the companion proxy module from:
`.k2so/work/inbox/feature-mobile-companion-server-side-support.md`

The wake endpoint should be added to the companion proxy's route table alongside the other 12 endpoints.

## Mobile App Side (already implemented)

The companion mobile app already has:
- `POST /companion/agents/wake` API call in `src/api/client.ts`
- `wakeAgent()` action in the agents Zustand store (`src/stores/agents.ts`)
- "Wake Agent" button on idle agent cards with inbox work (`src/components/AgentCard.tsx`)
- Yellow "Waking up..." transition state while waiting for the agent to start
- Auto-refresh after 2 seconds + WebSocket-driven refresh on `agent:lifecycle`

Source: `/Users/z3thon/DevProjects/Alakazam Labs/K2SO-companion/`

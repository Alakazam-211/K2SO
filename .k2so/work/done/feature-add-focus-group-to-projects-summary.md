---
title: "Feature: Add focusGroup, pinned, tabOrder to /companion/projects/summary"
priority: high
assigned_by: external
created: 2026-04-10
type: feature
source: manual
---

## Description

The `/companion/projects/summary` endpoint (10KB) is the mobile app's primary data source because `/companion/projects` (254KB) is too slow — it includes base64-encoded icon images for all 51 workspaces and times out on mobile.

The summary endpoint currently returns: `id`, `name`, `path`, `color`, `agentMode`, `agentsRunning`, `reviewsPending`.

The mobile app needs these additional fields to display the workspace drawer correctly:

- **`focusGroup`** — `{ id, name, color }` or `null` — needed for focus group tabs/filtering
- **`pinned`** — `boolean` — needed for Zone 1 (pinned workspaces at top)
- **`tabOrder`** — `number` — needed for correct sort order

Without these, the mobile app shows all workspaces in a flat list with no focus group filtering, which is 51 items on the user's setup.

## Current Response

```json
{
  "id": "uuid",
  "name": "K2SO",
  "path": "/path/to/K2SO",
  "color": "#3b82f6",
  "agentMode": "manager",
  "agentsRunning": 1,
  "reviewsPending": 0
}
```

## Desired Response

```json
{
  "id": "uuid",
  "name": "K2SO",
  "path": "/path/to/K2SO",
  "color": "#3b82f6",
  "agentMode": "manager",
  "agentsRunning": 1,
  "reviewsPending": 0,
  "pinned": false,
  "tabOrder": 0,
  "focusGroup": {
    "id": "uuid",
    "name": "Alakazam Labs",
    "color": null
  }
}
```

## Implementation

The summary endpoint already queries the `projects` table. Just add the LEFT JOIN on `focus_groups` (same join already used in `/companion/projects`) and include `pinned`, `tab_order`, and `focus_group_id` in the SELECT.

## Why

The mobile app is live on a real iPhone and working. Workspaces load, agents show with correct badges, pinned workspaces appear at top. But the 51-workspace flat list needs focus group filtering to be usable.

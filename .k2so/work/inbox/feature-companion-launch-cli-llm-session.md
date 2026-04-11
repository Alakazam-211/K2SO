---
title: "Feature: Launch CLI LLM tool sessions from mobile companion"
priority: normal
assigned_by: external
created: 2026-04-10
type: feature
source: manual
---

## Description

The K2 mobile app's "+ New Session" button currently shows K2SO workspace agents (rust-eng, frontend-eng, etc.). It should instead let users pick a **CLI LLM tool** (Claude, Codex, Cursor, etc.) to launch in a new terminal tab within the selected workspace — the same way the desktop app lets you open a new terminal and pick which tool to run.

## New Endpoints Needed

### 1. `GET /companion/presets` — List available CLI LLM tool presets

Returns the agent presets from K2SO's `agent_presets` database table.

**Response:**
```json
{
  "ok": true,
  "data": [
    { "id": "uuid", "name": "Claude", "command": "claude", "icon": "claude" },
    { "id": "uuid", "name": "Codex", "command": "codex", "icon": "codex" },
    { "id": "uuid", "name": "Cursor Agent", "command": "cursor-agent", "icon": "cursor" },
    { "id": "uuid", "name": "Gemini CLI", "command": "gemini", "icon": "gemini" }
  ]
}
```

This is global (no project param needed).

### 2. `POST /companion/terminal/spawn` — Launch a new terminal with a CLI LLM tool

**Request:**
```json
{ "project": "/path/to/workspace", "command": "claude", "title": "Claude" }
```

Maps to the existing `/cli/terminal/spawn` internal endpoint which emits `cli:terminal-spawn` for K2SO to open a new tab.

**Response:**
```json
{ "ok": true, "data": { "success": true } }
```

## Mobile App Flow

1. User taps "+ New Session" on the Workspaces page
2. App shows a list of CLI LLM presets (Claude, Codex, Cursor, etc.)
3. User taps one
4. App calls `POST /companion/terminal/spawn` with the command
5. K2SO opens a new terminal tab in that workspace running the selected tool
6. The terminal appears in the mobile app's session list after a short refresh

## Proxy Changes

Add to `companion/proxy.rs`:
```rust
("GET",  "/companion/presets")         => Some("/cli/companion/presets"),
("POST", "/companion/terminal/spawn")  => Some("/cli/terminal/spawn"),
```

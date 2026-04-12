---
title: Companion API: background terminal spawn endpoint
priority: high
assigned_by: user
created: 2026-04-12
type: feature
source: manual
---

Companion API: Add background terminal spawn endpoint

The companion app needs to create new CLI LLM sessions in specific workspaces
WITHOUT switching the desktop user's active workspace.

Current behavior:
- POST /companion/terminal/spawn routes to /cli/terminal/spawn
- This emits cli:terminal-spawn to the frontend
- Frontend creates the terminal in the current active workspace tab
- This switches the user's view on the desktop, which is disruptive

Requested behavior:
- New endpoint: POST /companion/terminal/spawn-background (or add a background flag)
- Calls terminal_create() directly in Rust with the specified cwd
- Uses a deterministic terminal ID (e.g., companion-{timestamp} or companion-{uuid})
- Does NOT emit cli:terminal-spawn to the frontend
- Does NOT switch the desktop user's active workspace
- The PTY runs in the background, accessible via terminal.read/write/subscribe

This follows the same pattern as the agent delegation system (active-agents.ts)
which creates background PTYs with deterministic IDs via terminal_create()
without disrupting the user's current workspace.

The companion app would use this to let mobile users launch new sessions
in any workspace without affecting what's happening on the desktop.

Reference files:
- src-tauri/src/commands/terminal.rs (terminal_create)
- src-tauri/src/agent_hooks.rs lines 634-650 (current terminal.spawn)
- src/renderer/stores/active-agents.ts lines 478-557 (background PTY pattern)
- src-tauri/src/companion/proxy.rs (companion routing)

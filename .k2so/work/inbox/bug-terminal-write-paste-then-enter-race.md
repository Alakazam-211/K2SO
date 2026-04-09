---
title: "Bug: terminal_write paste doesn't auto-submit — Enter key lost in paste stream"
priority: high
assigned_by: user
created: 2026-04-10
type: bug
source: manual
---

## Description

When sending a multi-line message to a terminal via `terminal_write` (used by `k2so msg --wake`, PTY injection, and direct `k2so terminal write`), the text is pasted but the trailing `\r` (Enter) doesn't trigger submission. The user has to manually come back and hit Enter.

## Root Cause (Suspected)

CLI LLM tools (Claude Code, Codex, etc.) use special input modes where Enter needs to be a separate keystroke event, not embedded at the end of a paste stream. When `terminal_write` sends `"message text\r"` as one atomic write, the terminal processes it as a paste operation and the `\r` gets swallowed or ignored.

## Proposed Fix

Split the write into two steps with a brief delay:
1. Write the message text (without `\r`)
2. Wait ~100-200ms for the paste to complete
3. Write `\r` as a separate keystroke

This affects:
- `k2so msg --wake` (PTY injection in agent_hooks.rs)
- `k2so terminal write` (CLI command)
- Heartbeat wake triage message injection
- Launch button checkin injection (WorkspacePanel.tsx)

## Impact

Agents can't reliably send messages to each other's running sessions. The message arrives but never submits, requiring manual intervention. This blocks the full agent-to-agent communication flow.

## Files to Check

- `src-tauri/src/commands/terminal.rs` — `terminal_write` function
- `src-tauri/src/agent_hooks.rs` — wake chain PTY injection
- `src/renderer/components/WorkspacePanel/WorkspacePanel.tsx` — launch button checkin injection
- `src-tauri/src/terminal/` — PTY write implementation

---
title: "Feature: Structured chat protocol for mobile companion (inspired by pi-Mono RPC)"
priority: high
assigned_by: external
created: 2026-04-10
type: feature
source: manual
---

## Problem

The mobile companion currently receives raw terminal grid data (120 columns × N rows with ANSI color spans) and tries to render it on a phone screen. This doesn't work well because:

1. The grid is formatted for the desktop terminal width (120 cols) — text wraps awkwardly on a 390px phone screen
2. Tool execution bars, progress spinners, and cursor positioning are meaningless on mobile
3. The mobile app is a **conversation companion**, not a terminal emulator — it should show the conversation, not the terminal rendering

## Inspiration: pi-Mono's RPC Protocol

The pi-Mono project (`/Users/z3thon/DevProjects/Alakazam Labs/pi-Mono/`) solves this perfectly. Their agent has three output modes:

1. **Interactive (TUI)** — differential terminal rendering for desktop
2. **Web UI** — responsive HTML components
3. **RPC (JSON-L)** — structured conversation data for headless/mobile clients

The RPC mode sends **typed message objects**, not terminal grids:

```json
{"role": "user", "content": "Fix the login bug", "timestamp": 1712700000}
{"role": "assistant", "content": [{"type": "text", "text": "I'll look at the auth module..."}], "model": "claude-opus-4-6"}
{"role": "toolResult", "toolName": "Edit", "content": [{"type": "text", "text": "Updated auth.ts"}], "isError": false}
{"role": "bashExecution", "command": "npm test", "output": "All tests passed", "exitCode": 0}
```

The mobile client receives structured conversation turns and renders them responsively — wrapping text to the phone screen naturally, without fighting a fixed-width terminal grid.

## What K2SO Should Expose

### New endpoint: `GET /companion/terminal/chat` (or WS method `terminal.chat`)

Returns the conversation history for a terminal session as structured messages instead of raw grid data.

**Params:** `{ project, terminalId, since?: timestamp }`

**Response:**
```json
{
  "ok": true,
  "data": {
    "messages": [
      {
        "role": "user",
        "content": "Fix the login timeout issue",
        "timestamp": 1712700000
      },
      {
        "role": "assistant",
        "content": "I'll investigate the auth module. Let me read the relevant files...",
        "timestamp": 1712700005
      },
      {
        "role": "tool",
        "toolName": "Read",
        "filePath": "src/auth.ts",
        "timestamp": 1712700006
      },
      {
        "role": "tool",
        "toolName": "Edit",
        "filePath": "src/auth.ts",
        "summary": "Added 30-second timeout to login fetch",
        "timestamp": 1712700010
      },
      {
        "role": "assistant",
        "content": "I've added a 30-second timeout to the login request. The issue was...",
        "timestamp": 1712700012
      },
      {
        "role": "bash",
        "command": "npm test",
        "output": "PASS src/auth.test.ts\n  ✓ login times out after 30s",
        "exitCode": 0,
        "timestamp": 1712700015
      }
    ]
  }
}
```

### How to extract structured messages from the PTY

K2SO already knows what CLI LLM tool is running in each terminal (Claude Code, Codex, etc.). The approach:

**Option A: Parse the terminal output**
- Claude Code's output follows predictable patterns: user prompts (`> `), assistant responses, tool calls (with specific formatting), bash output blocks
- A parser could convert the rendered terminal text into structured messages
- This is fragile but works without changing the LLM tools

**Option B: Intercept the MCP/tool protocol**
- Claude Code communicates via MCP channels — K2SO already has `k2so-events` channel integration
- The channel events include structured tool calls and responses
- Pipe these as companion chat messages

**Option C: Use the LLM tool's own structured output**
- Claude Code has `--output-format json` or similar structured output modes
- Capture that alongside the PTY output
- Send structured data to companion, raw PTY to desktop

**Recommended: Option A for now** (parse terminal output into messages), with Option B as the long-term solution (intercept MCP events for perfect structured data).

## What the Mobile App Will Do With This

Instead of rendering a `TerminalView` component that mimics a terminal grid, the mobile app will render a `ChatView` with:

- **User messages** — right-aligned bubbles (or top-aligned with user icon)
- **Assistant responses** — left-aligned, markdown-rendered, wrapping to screen width
- **Tool calls** — collapsible cards showing file name + summary
- **Bash output** — monospace blocks with exit code badge
- **Thinking blocks** — collapsible, dimmed

This is the natural mobile UX for interacting with an AI agent — a chat interface, not a terminal mirror.

## Fallback

The existing `terminal.read` endpoint (raw text) continues to work as a fallback. The mobile app can show raw terminal output if the structured chat endpoint isn't available for a given terminal.

## Reference

- pi-Mono RPC protocol: `/Users/z3thon/DevProjects/Alakazam Labs/pi-Mono/packages/coding-agent/src/modes/rpc/rpc-types.ts`
- pi-Mono message types: `/Users/z3thon/DevProjects/Alakazam Labs/pi-Mono/packages/ai/src/types.ts`
- pi-Mono custom messages: `/Users/z3thon/DevProjects/Alakazam Labs/pi-Mono/packages/coding-agent/src/core/messages.ts`
- pi-Mono web UI (responsive): `/Users/z3thon/DevProjects/Alakazam Labs/pi-Mono/packages/web-ui/src/ChatPanel.ts`

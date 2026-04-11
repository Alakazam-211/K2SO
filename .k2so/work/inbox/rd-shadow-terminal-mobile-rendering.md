---
title: "R&D: Shadow terminal for mobile-native rendering"
priority: high
assigned_by: user
created: 2026-04-10
updated: 2026-04-11
type: research
source: manual
---

## Concept

Create a second in-memory terminal emulator at mobile screen dimensions that receives the same raw PTY byte stream as the desktop terminal. The mobile companion gets grid data rendered at phone dimensions instead of the desktop's 120-column grid.

## Industry Context

**Nobody has solved simultaneous multi-size terminal viewing.** We researched every major tool:
- tmux/screen/zellij — shared view, smallest client wins
- ttyd/GoTTY/tty-share — single PTY size, one client controls resize
- sshx/tmate/upterm — shared terminal, no per-client sizing
- Mosh — server-side rendering, sequential adaptation (not simultaneous)
- xterm.js — client-side only, delegates resize to backend PTY

K2SO has an opportunity to be first to solve this properly.

---

## Deep Dive Findings

### WezTerm — Reflow Algorithm (The Key Technique)

**Repo:** github.com/wez/wezterm (MIT) — cloned at /tmp/wezterm-research

**Core files:** `term/src/screen.rs`, `wezterm-surface/src/line/line.rs`

**How reflow works (two-phase):**

1. **Join phase** — iterate all physical rows. If a row's last cell has the `wrapped` flag (bit 11 of `CellAttributes`), append the next row onto it to reconstruct the logical line. This unwraps soft-wrapped lines.

2. **Re-wrap phase** — for each logical line, call `line.wrap(new_cols)` to split it into physical rows at the new width, setting the `wrapped` flag on each split point.

**Key function:** `Screen::rewrap_lines()` (lines 99-189 of screen.rs)
- Drains all lines, joins wrapped continuations via `prior.append_line(line)`
- Re-wraps at new width via `line.wrap(physical_cols)`
- Tracks cursor position through the reflow
- Handles alternate screen separately (truncate/pad, no reflow — for fullscreen apps like vim)

**The wrapped flag:** Stored on the last cell of each physical row — `cell.attrs().wrapped()`. This is how WezTerm knows "this row continues on the next row" vs "the program sent a newline."

**Damage tracking:** Per-line `seqno` (sequence number). Each modification increments the seqno. Renderers call `line.changed_since(last_seqno)` to skip unchanged lines. During reflow, all affected lines are marked dirty.

**Applicability:** The algorithm is portable but the data structures are not. WezTerm uses a 1D `VecDeque<Line>` where wrapped lines are continuations. Alacritty uses a 2D fixed grid. We'd need an **adapter layer** that reads Alacritty's grid + WRAPLINE flags and performs WezTerm-style reflow into a separate output buffer at mobile dimensions.

**Alacritty's WRAPLINE flag:** Alacritty DOES track soft wraps — each row has flags including `WRAPLINE`. This gives us the information needed for phase 1 (joining logical lines). Phase 2 (re-wrapping) is straightforward string splitting.

### Mosh — Stateful Differential Sync (The Protocol Technique)

**Repo:** github.com/mobile-shell/mosh (GPL-3.0) — cloned at /tmp/mosh-research

**Core files:** `src/terminal/terminaldisplay.cc`, `src/statesync/completeterminal.cc`

**Architecture:** Server runs a full VT100 terminal emulator (`Framebuffer` — 2D grid of `Cell` objects). Instead of sending raw PTY bytes to clients, it computes diffs between consecutive framebuffer states and sends ANSI escape codes representing the changes.

**Smart diff engine (`Display::new_frame()`):**
- Detects full scrolls (row N of new matches row N+k of old → scroll command instead of redraw)
- Row-by-row cell comparison, skips identical cells
- Groups blank cells for efficient `erase-to-end-of-line`
- Groups runs of cells with same renditions (colors/bold)
- Cursor movement optimization (CR/LF for down+left, backspace for backward)

**Result:** Typical typing produces 20-200 byte diffs instead of full screen redraws.

**Prediction/speculation:** Client predicts what keystrokes will produce (cursor moves, character insertion). Overlays predicted state on top of confirmed state. Server's echo-ack mechanism confirms predictions. This masks latency for common operations.

**Resize handling:** When client resizes, server re-renders entire screen at new dimensions. PTY is also resized via ioctl. This is sequential adaptation (not simultaneous) — one client, one size.

**Applicability:** We're already doing a version of this with CompactLine + hash-based change detection. Mosh's approach is more sophisticated — cell-level diffing, scroll detection, cursor optimization. Worth adopting for efficiency, but doesn't solve the multi-size problem directly. The prediction system could reduce perceived latency for mobile input.

### sshx — Rust WebSocket Relay (The Architecture Confirmation)

**Repo:** github.com/ekzhang/sshx (MIT) — cloned at /tmp/sshx-research

**Core files:** `crates/sshx-server/src/session.rs`, `crates/sshx/src/runner.rs`

**Architecture:** Confirms the industry-standard pattern:
- Single PTY per shell, single size
- Raw bytes relayed via gRPC (backend ↔ server) + WebSocket (server ↔ browsers)
- xterm.js renders on client — server does NO terminal interpretation
- All clients share the same PTY size; any client can resize (broadcast to all)

**Rolling buffer:** 2MB per shell (`SHELL_STORED_BYTES`). New clients catch up by subscribing from a chunk offset. Exactly what we proposed for our ring buffer.

**Key insight:** sshx's "infinite canvas" is purely client-side CSS transforms (pan/zoom). It doesn't affect the terminal rendering at all. Each terminal on the canvas still has a single PTY size shared by all clients.

**Applicability:** Validates our ring buffer approach (2MB rolling history, subscribe from offset). CBOR binary encoding is more efficient than JSON for WebSocket — could adopt. The gRPC + WebSocket hybrid is interesting but overkill for our ngrok tunnel setup.

---

## Recommended Approach: Shadow Terminal with WezTerm-Style Reflow

Based on all research, the best approach combines techniques from all three:

### Architecture

```
PTY master (120×38)
    ↓
K2SO reads bytes → Ring Buffer (2MB, always recording)
    ↓
    ├── bytes → Alacritty Term (120×38) → desktop CompactLine → desktop UI
    │
    └── bytes → Alacritty Term (50×20) → reflow layer → mobile CompactLine → companion WS
                 (shadow, created on subscribe)     ↑
                                          WezTerm-style logical
                                          line reconstruction
                                          + re-wrap at mobile width
```

### Phase 1: Ring Buffer (Foundation)
- 2MB ring buffer per terminal recording raw PTY bytes (sshx validated this size)
- Always recording, minimal overhead
- Enables catch-up when mobile subscribes mid-session

### Phase 2: Shadow Terminal
- Second `alacritty_terminal::Term` at mobile dimensions
- Created when mobile sends `terminal.subscribe` with `{ terminalId, cols, rows }`
- Replay ring buffer into shadow term for catch-up
- Tee live PTY bytes into both terms going forward
- Drop on disconnect

### Phase 3: Reflow Layer (Quality Upgrade)
- Read Alacritty's `WRAPLINE` flags to identify soft-wrapped rows
- Join soft-wrapped rows into logical lines (WezTerm phase 1)
- Re-wrap logical lines at mobile width (WezTerm phase 2)
- This produces clean text reflow — conversational content wraps naturally
- Cursor-positioned UI elements still garbled, but text content is clean

### Phase 4: Mosh-Style Diffing (Optimization)
- Instead of sending full CompactLine snapshots, compute diffs
- Row-by-row comparison, skip unchanged rows
- Scroll detection (shifted rows → scroll command instead of redraw)
- Reduces bandwidth 10-50x for typical agent output

---

## New WS Method: `terminal.resize`

Added based on companion team feedback — phone rotation changes available columns (portrait ~50 cols, landscape ~95 cols). Instead of re-subscribing (which tears down the shadow term), a `terminal.resize` method resizes the shadow term in place and sends a `full: true` snapshot at the new dimensions.

```json
{ "id": "...", "method": "terminal.resize", "params": { "terminalId": "...", "cols": 95, "rows": 30 } }
```

The shadow term's `Term::resize()` is called, the reflow layer re-wraps content at the new width, and the next grid event is a full snapshot. No blank screen, no catch-up delay. Alacritty's `resize()` is ~1ms — mobile client should debounce at ~200ms to avoid unnecessary work during rotation animation.

## What This Solves

| Problem | Solution |
|---|---|
| 120-col text on 50-col phone | Shadow term wraps at phone width |
| Blank screen on mid-session connect | Ring buffer replay catches up |
| Bandwidth to mobile | Mosh-style diffing reduces payload |
| Input latency on mobile | Mosh-style prediction (future) |
| Text wrapping input lines/status bars | Reflow layer distinguishes soft/hard wraps |

## What This Doesn't Solve (Accepted Limitations)

- **Cursor-positioned UI elements** (permission prompts, status bars, progress bars) — formatted for 120 cols, will still be garbled at 50 cols
- **Programs that query terminal size** — they'll get 120 cols from the PTY, not 50
- **Fullscreen TUI apps** (vim, htop) — alternate screen doesn't reflow, by design

These are acceptable because the mobile companion is a **monitoring + messaging** tool, not a full terminal replacement. 80%+ of agent output is conversational text that reflows cleanly.

---

## Implementation Strategy

- **Branch-based development** — this is a rework, not a hotfix. Build in a feature branch, test thoroughly, merge when stable
- **Current implementation works** — CompactLine streaming is functional. This R&D improves quality, it doesn't unblock anything
- **Phase 1+2 first** — ring buffer + shadow term gives immediate improvement
- **Phase 3 later** — reflow is the polish that makes it production-quality
- **Phase 4 optional** — Mosh-style diffing is optimization, not required for correctness

## Files to Modify

| File | Change |
|---|---|
| `src-tauri/src/terminal/alacritty_backend.rs` | Ring buffer per terminal, shadow Term creation, byte tee, dual snapshot |
| `src-tauri/src/terminal/reflow.rs` | **NEW** — WezTerm-style reflow: read WRAPLINE flags, join logical lines, re-wrap |
| `src-tauri/src/companion/mod.rs` | Terminal polling uses shadow grid for mobile clients |
| `src-tauri/src/companion/websocket.rs` | Accept mobile dimensions in `terminal.subscribe` |
| `src-tauri/src/companion/types.rs` | Shadow term storage per subscribed mobile client |

## Reference Repos

| Repo | What to Study | License | Cloned At |
|---|---|---|---|
| WezTerm | `Screen::rewrap_lines()`, `Line::wrap()`, WRAPLINE flag, seqno damage tracking | MIT | /tmp/wezterm-research |
| Mosh | `Display::new_frame()` diff engine, prediction overlay, echo-ack | GPL-3.0 | /tmp/mosh-research |
| sshx | Rolling buffer, WebSocket relay, CBOR encoding, session transfer | MIT | /tmp/sshx-research |

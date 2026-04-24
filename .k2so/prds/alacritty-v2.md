# Alacritty_v2 — Daemon-hosted Terminal Renderer

**Status:** Planned. Not yet started.
**Captured:** 2026-04-24
**Branch:** forthcoming (currently on `kessel/reset-from-phase-7` at Phase 6 baseline).

## Why this exists

Today's K2SO ships one renderer (Alacritty_v1) that runs entirely in the
Tauri process — its PTY, child, and `alacritty_terminal::Term` all live
in-app. This has two consequences:

1. **Sessions die when Tauri quits.** The PTY master is owned by Tauri;
   app close → master drops → SIGHUP → child dies.
2. **Heartbeats cannot target these sessions.** The daemon has no handle
   to them; its scheduler cannot inject signals, cannot audit, cannot
   wake.

Alacritty_v2 moves the PTY + Term into the daemon. Tauri becomes a thin
viewer. The terminal survives Tauri quit, heartbeats target it
naturally, and the user experience is byte-identical to v1 on screen.

## The renderer landscape (reminder)

| Renderer | PTY owner | Heartbeats? | Multi-subscriber? | Status |
|---|---|---|---|---|
| **Alacritty_v1** | Tauri | No | No | Today's default. Transitional. Retire when v2 proves stable. |
| **Alacritty_v2** | Daemon | Yes | **No — single subscriber by design** | This PRD. |
| **Kessel (future)** | Daemon | Yes | Yes (via JSON frame streams only) | Separate PRD. T1-capable harnesses only. |

Alacritty_v1 and v2 are functionally interchangeable from the user's
viewpoint. Kessel is a different product tier for tools that expose
stream-json / NDJSON output.

## Design principles

1. **UX parity with v1 is non-negotiable.** Scroll, reflow on resize,
   scrollback, copy/paste, cursor UX, keystroke echo — all indistinguishable
   from v1 from the user side.
2. **Daemon is the source of truth.** The `alacritty_terminal::Term` on
   the daemon holds authoritative grid + scrollback state. Tauri renders
   what it's told.
3. **Minimum surface area.** No LineMux, no byte ring, no broadcast, no
   APC coordination, no grow-shrink choreography. None of those solve a
   problem that applies to single-subscriber TUI rendering.
4. **Scroll is local-first.** Client maintains its own viewport offset
   over a local mirror of the daemon's grid+scrollback. No WS round-trip
   on scroll. Matches v1's instant-feel exactly.
5. **Reuse over reinvention.** Lift patterns from Zed (proven code:
   `crates/terminal/src/terminal.rs`) and our own Phase 6 snapshot/delta
   serializers. Write new code only where it's strictly required.

## Architecture

```
[Tauri UI]                                 ← React DOM renderer
    ↑  WebSocket (JSON snapshots + deltas; JSON commands)
[NEW: sessions_grid_ws.rs]                 ← serializes Term state,
                                             forwards client commands
    ↑  reads from + writes to
[NEW: terminal/daemon_pty.rs]              ← owns PTY + child + Term,
                                             uses alacritty's EventLoop::spawn()
    │
    ├── [alacritty_terminal::Term]         ← authoritative grid + scrollback
    │       ↑  fed bytes by alacritty's built-in EventLoop
    └── [portable-pty master]              ← kernel PTY
            ↕
          [portable-pty slave]             ← attached to child's std{in,out,err}
            ↕
          [child process]                  ← claude / bash / vim / etc.
```

Comparison with v1: the stack is identical **above** `alacritty_terminal::Term`
— same grid structure, same scrollback, same resize behavior, same Rust
crates. The only difference is where `Term` lives (daemon vs Tauri) and
how Tauri receives its contents (WS messages vs direct memory).

## Scope

### In scope — what v2 ships

- PTY + child + `alacritty_terminal::Term` on the daemon.
- WS endpoint serving `TermGridSnapshot` on attach, `TermGridDelta`
  stream on damage (~4ms coalescing, Zed's pattern).
- Reverse channel for client → daemon: keystrokes (`input`), resize
  (`resize`).
- `/cli/sessions/spawn` gains find-or-spawn semantics on `agent_name`
  so remount reattaches the same session.
- Tauri thin client: WS subscriber, local mirror of snapshot + scrollback,
  DOM renderer, local viewport offset for scroll, no local Term, no ANSI
  processing.
- Session lifecycle: unmount does NOT close daemon session; deliberate
  tab close (`stores/tabs.ts::removeTab`) calls `/cli/sessions/close`.
- Heartbeat compatibility: works out of the box because daemon owns the
  session. No v2-specific wiring needed.

### Out of scope — explicitly not v2's problem

- Multi-subscriber support. Second client attempting to subscribe gets
  rejected-with-busy (initial policy; revisit later if needed).
- JSON-stream adapters (Claude, Codex, Gemini, etc.). Those are Kessel's
  domain.
- LineMux / Frame stream / `SemanticEvent` emission from v2's path.
  LineMux stays alive in the daemon for other consumers (heartbeat
  activity tracking, future T1 prep) but does not sit on v2's byte path.
- Grow-then-shrink. Daemon Term accumulates scrollback naturally as
  content scrolls. No coordination needed.
- APC-coordinated resize. Resize is a plain request/response.
- Per-subscriber reflow. Architecturally impossible at the byte layer;
  lives at T1 in Kessel.

## Phase plan

| Phase | Work | Effort |
|---|---|---|
| **A1** | Daemon-side terminal module (`terminal/daemon_pty.rs`). Wraps PTY + `alacritty_terminal::EventLoop::spawn()` + custom `EventListener` that emits damage events. Mirrors Zed's `TerminalBuilder` at `crates/terminal/src/terminal.rs:340`. | 1-2 days |
| **A2** | Move `CellRun`, `TermGridSnapshot`, `TermGridDelta`, `snapshot_term()`, `build_delta()` from `src-tauri/.../kessel_term.rs` into `crates/k2so-core/src/terminal/grid_snapshot.rs`. Strip Tauri deps. | 0.5 day |
| **A3** | New daemon WS endpoint `crates/k2so-daemon/src/sessions_grid_ws.rs`. On connect: snapshot. On damage (4ms coalesce): delta. Accepts input + resize from client. | 1-2 days |
| **A4** | Modify `/cli/sessions/spawn` to be idempotent on `agent_name`: look up `session_map`; return existing session's `{sessionId, cols, rows}` instead of spawning a duplicate. | 0.5 day |
| **A5** | Tauri `TerminalPane.tsx` (replaces `KesselTerminal.tsx` + `SessionStreamViewTerm.tsx`). WS subscribe, local mirror state, DOM render, resize + keystroke forwarding. Drops Tauri-side `alacritty_terminal` / `vte` dependencies. | 1-2 days |
| **A6** | Lifecycle wiring in `stores/tabs.ts`: `kessel_close` moves from unmount path to `removeTab`. Unmount preserves daemon session. | 0.5 day |
| **A7** | Cutover + parity validation. Run v2 behind a per-tab flag during the bakeoff. Once stable, delete v1 + the Kessel-T0 machinery it outgrew (byte ring, broadcast, grow-shrink, APC filter on this path). | 1 day + bake time |
| **Total** | | **~6-8 days focused work** |

## UX parity checklist (for A7 validation)

- Open a new bash tab → prompt visible immediately, no flicker.
- Type a command, enter → output appears, cursor tracks correctly.
- Output overflows viewport → scroll up reveals history, instantly, no
  WS latency perceptible.
- Drag-resize window → TUI reflows, no stacked paints, no black flash.
- Launch vim → alt-screen enters, grid switches. Exit → restores.
- Open Claude → TUI loads, interact normally, scrollback navigable.
- Close K2SO, reopen → tab reattaches to same daemon session, shows
  current state.
- Deliberate Cmd+W on tab → child process dies, session cleaned up.
- Heartbeat fires while Tauri is closed → agent receives signal.
- Heartbeat fires while Tauri is open → agent receives signal, user sees
  output flow in real-time.

Any regression against any of these blocks the cutover.

## What we're borrowing from Zed

Read-and-adapt targets (verified by research, 2026-04-24):

| Pattern | Zed source |
|---|---|
| Headless Term construction | `crates/terminal/src/terminal.rs:346` (`new_display_only()`) |
| Custom `EventListener` impl | `crates/terminal/src/terminal.rs:185-191` (`ZedListener`) |
| Using alacritty's `EventLoop::spawn()` | `crates/terminal/src/terminal.rs:602` |
| Grid snapshot struct shape | `crates/terminal/src/terminal.rs:789-801` (`TerminalContent`) |
| Event coalescing cadence (~4ms) | `crates/terminal/src/terminal.rs:691-750` |

These are patterns, not literal code copies — adapt into our daemon
context (no GPUI, no Zed Entity model).

## What Zed does NOT have (so we build ourselves)

- Grid-state-over-WS protocol. Zed's remote terminals are raw PTY
  bytes tunneled over SSH; no Zed client ever receives a serialized
  grid. A3 and A2 are our original work.
- Multi-subscriber broadcast. Not needed for v2 (single subscriber by
  design). When Kessel needs it later, we already have primitives in
  `session::registry`.

## Open decisions deferred

1. **Second-subscriber behavior.** Initial policy: "busy, try again."
   Revisit if a real use case emerges (e.g., read-only peek from
   mobile). Not blocking for v2 ship.
2. **Scrollback cap.** Ship with Phase 6's existing `SCROLLBACK_CAP = 5000`.
   Revisit if users hit the limit in practice.
3. **Which tools route to v2 vs Kessel in the final UX.** Post-v2 product
   decision. For now, v2 handles anything the user points at it; Kessel
   handles T1-capable harnesses when Kessel ships.
4. **Heartbeat visibility UX.** When a heartbeat fires in a closed tab
   and produces output, should the tab auto-pop-open? Badge update? TBD;
   orthogonal to v2's architecture.

## What happens to Kessel-T0 (current path)

After A7 cutover: delete. Specifically:
- `crates/k2so-daemon/src/sessions_bytes_ws.rs` — byte-stream WS, unused.
- `crates/k2so-core/src/terminal/grow_settle.rs` — grow-shrink driver.
- Grow-shrink branches in `session_stream_pty.rs` (the file itself
  stays if Kessel needs a LineMux-bearing session type, but the
  grow-shrink / APC branches go).
- APC filter in `src-tauri/.../kessel_term.rs` — moves out with the
  rest of the Tauri-side Term code.
- The `use_session_stream` feature flag (if it's still gating things
  by then).

See `.k2so/notes/renderer-roadmap-post-t0.md` for the full archive of
T0-era polish phases that also roll off the board.

## Sign-off

- Single-subscriber v2, heartbeat-native, UX-identical to v1, minimum
  surface, Zed-validated patterns, no multi-device pretense.
- When v2 is stable, v1 retires.
- Kessel begins as a separate, fresh effort for multi-device JSON-stream
  rendering — its PRD is next.
